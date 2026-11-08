//! Media Foundation custom media source for the Windows 11 virtual camera
//! (`MFCreateVirtualCamera`). Structure follows Microsoft's guidance and the
//! VCamSample reference: an `IMFActivate` that hands out a live
//! `IMFMediaSourceEx` with one video stream (`IMFMediaStream2`).
//!
//! The Frame Server service loads this DLL and pulls samples; frames come
//! from glyphcam through machine-wide shared memory (see `feed.rs`).

use crate::attrs::impl_attributes;
use crate::feed::{write_nv12, write_rgb32, Feed};
use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{implement, w, ComObject, IUnknown, Interface, Ref, Result, GUID, HRESULT};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER, S_OK};
use windows::Win32::Media::KernelStreaming::{
    IKsControl, IKsControl_Impl, KSCAMERAPROFILE_HighFrameRate, KSCAMERAPROFILE_Legacy,
    KSIDENTIFIER, PINNAME_VIDEO_CAPTURE,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const ERROR_SET_NOT_FOUND: HRESULT = HRESULT(0x8007_0492u32 as i32);
const FRAME_100NS: i64 = 333_333;

/// Formats offered, NV12 first (what most consumers pick).
const SIZES: [(u32, u32); 3] = [(1280, 720), (1920, 1080), (640, 360)];

struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

fn new_attrs() -> Result<IMFAttributes> {
    let mut a: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut a, 8)? };
    a.ok_or_else(|| E_POINTER.into())
}

/// VT_I8 PROPVARIANT holding `v` (event payload for MESourceStarted/Stopped).
fn pv_i64(v: i64) -> PROPVARIANT {
    let mut pv: PROPVARIANT = unsafe { std::mem::zeroed() };
    unsafe {
        let p = &mut pv as *mut PROPVARIANT as *mut u8;
        *(p as *mut u16) = 20; // VT_I8
        *(p.add(8) as *mut i64) = v;
    }
    pv
}

fn video_type(subtype: &GUID, w: u32, h: u32) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        t.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        let nv12 = *subtype == MFVideoFormat_NV12;
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, if nv12 { w } else { w * 4 })?;
        let bytes = if nv12 { w * h * 3 / 2 } else { w * h * 4 };
        t.SetUINT32(&MF_MT_SAMPLE_SIZE, bytes)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, bytes.saturating_mul(8 * 30))?;
        Ok(t)
    }
}

// ---------------------------------------------------------------------------
// Activator
// ---------------------------------------------------------------------------

#[implement(IMFActivate)]
pub struct Activator {
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSource>>,
}

impl Activator {
    pub fn create() -> Result<IMFActivate> {
        let attrs = new_attrs()?;
        unsafe {
            attrs.SetUINT32(&MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES, 1)?;
            attrs.SetGUID(
                &MFT_TRANSFORM_CLSID_Attribute,
                &glyphcam_shm::win::mf_source_clsid(),
            )?;
        }
        let source = MediaSource::create(&attrs)?;
        Ok(Activator {
            attrs,
            source: Mutex::new(Some(source)),
        }
        .into())
    }
}

impl_attributes!(Activator_Impl);

impl IMFActivate_Impl for Activator_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        if ppv.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        let mut g = self.source.lock().unwrap();
        if g.is_none() {
            *g = Some(MediaSource::create(&self.attrs)?);
        }
        unsafe { g.as_ref().unwrap().query(riid, ppv).ok() }
    }

    fn ShutdownObject(&self) -> Result<()> {
        Ok(())
    }

    fn DetachObject(&self) -> Result<()> {
        *self.source.lock().unwrap() = None;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Media source
// ---------------------------------------------------------------------------

struct SrcState {
    queue: Option<IMFMediaEventQueue>,
    pd: Option<IMFPresentationDescriptor>,
}

#[implement(
    IMFMediaSourceEx,
    IMFGetService,
    IKsControl,
    IMFSampleAllocatorControl,
    IMFAttributes
)]
pub struct MediaSource {
    attrs: IMFAttributes,
    st: Mutex<SrcState>,
    stream: ComObject<MediaStream>,
}

impl MediaSource {
    fn create(from: &IMFAttributes) -> Result<IMFMediaSource> {
        let attrs = new_attrs()?;
        unsafe {
            from.CopyAllItems(&attrs)?;
            // Sensor profiles, as the Windows camera stack expects them.
            let coll = MFCreateSensorProfileCollection()?;
            let p =
                MFCreateSensorProfile(&KSCAMERAPROFILE_Legacy, 0, windows::core::PCWSTR::null())?;
            p.AddProfileFilter(0, w!("((RES==;FRT<=30,1;SUT==))"))?;
            coll.AddProfile(&p)?;
            let p = MFCreateSensorProfile(
                &KSCAMERAPROFILE_HighFrameRate,
                0,
                windows::core::PCWSTR::null(),
            )?;
            p.AddProfileFilter(0, w!("((RES==;FRT>=60,1;SUT==))"))?;
            coll.AddProfile(&p)?;
            attrs.SetUnknown(&MF_DEVICEMFT_SENSORPROFILE_COLLECTION, &coll)?;
        }
        let stream = ComObject::new(MediaStream::new(0)?);
        let desc = stream.descriptor()?;
        let pd = unsafe { MFCreatePresentationDescriptor(Some(&[Some(desc)]))? };
        // The video stream is selected by default.
        unsafe { pd.SelectStream(0)? };
        let queue = unsafe { MFCreateEventQueue()? };
        let src = ComObject::new(MediaSource {
            attrs,
            st: Mutex::new(SrcState {
                queue: Some(queue),
                pd: Some(pd),
            }),
            stream: stream.clone(),
        });
        let ex: IMFMediaSourceEx = src.to_interface();
        let iface: IMFMediaSource = ex.cast()?;
        stream.set_source(Some(iface.clone()));
        Ok(iface)
    }

    fn queue(&self) -> Result<IMFMediaEventQueue> {
        self.st
            .lock()
            .unwrap()
            .queue
            .clone()
            .ok_or_else(|| MF_E_SHUTDOWN.into())
    }
}

impl_attributes!(MediaSource_Impl);

impl IMFMediaEventGenerator_Impl for MediaSource_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        let q = self.queue()?;
        unsafe { q.GetEvent(flags.0 as u32) }
    }
    fn BeginGetEvent(&self, cb: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        let q = self.queue()?;
        unsafe { q.BeginGetEvent(cb.ok()?, state.as_ref()) }
    }
    fn EndGetEvent(&self, r: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        let q = self.queue()?;
        unsafe { q.EndGetEvent(r.ok()?) }
    }
    fn QueueEvent(
        &self,
        met: u32,
        ext: *const GUID,
        hr: HRESULT,
        pv: *const PROPVARIANT,
    ) -> Result<()> {
        let q = self.queue()?;
        unsafe { q.QueueEventParamVar(met, ext, hr, pv) }
    }
}

impl IMFMediaSource_Impl for MediaSource_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        let pd = self
            .st
            .lock()
            .unwrap()
            .pd
            .clone()
            .ok_or_else(|| windows::core::Error::from(MF_E_SHUTDOWN))?;
        unsafe { pd.Clone() }
    }

    fn Start(
        &self,
        pdin: Ref<IMFPresentationDescriptor>,
        fmt: *const GUID,
        pos: *const PROPVARIANT,
    ) -> Result<()> {
        dbg_log("source.Start");
        let pdin = pdin.ok()?;
        if pos.is_null() {
            return Err(E_POINTER.into());
        }
        if !fmt.is_null() && unsafe { *fmt } != GUID::zeroed() {
            return Err(E_INVALIDARG.into());
        }
        let (queue, pd) = {
            let st = self.st.lock().unwrap();
            (st.queue.clone(), st.pd.clone())
        };
        let (Some(queue), Some(pd)) = (queue, pd) else {
            return Err(MF_E_SHUTDOWN.into());
        };
        unsafe {
            let count = pdin.GetStreamDescriptorCount()?;
            for i in 0..count {
                let mut selected = windows::core::BOOL(0);
                let mut desc: Option<IMFStreamDescriptor> = None;
                pdin.GetStreamDescriptorByIndex(i, &mut selected, &mut desc)?;
                let Some(desc) = desc else { continue };
                if selected.as_bool() {
                    pd.SelectStream(0)?;
                    let was_running = self.stream.state() != MF_STREAM_STATE_STOPPED;
                    // Start the stream before announcing it, so a consumer
                    // reacting to MENewStream can request samples at once.
                    let ty = desc.GetMediaTypeHandler()?.GetCurrentMediaType()?;
                    self.stream.start(Some(ty))?;
                    let unk: IUnknown = self.stream.to_interface();
                    let ev = if was_running {
                        MEUpdatedStream
                    } else {
                        MENewStream
                    };
                    queue.QueueEventParamUnk(ev.0 as u32, &GUID::zeroed(), S_OK, &unk)?;
                } else {
                    pd.DeselectStream(0)?;
                    self.stream.stop()?;
                }
            }
            let t = pv_i64(MFGetSystemTime());
            queue.QueueEventParamVar(MESourceStarted.0 as u32, &GUID::zeroed(), S_OK, &t)?;
        }
        Ok(())
    }

    fn Stop(&self) -> Result<()> {
        let (queue, pd) = {
            let st = self.st.lock().unwrap();
            (st.queue.clone(), st.pd.clone())
        };
        let (Some(queue), Some(pd)) = (queue, pd) else {
            return Err(MF_E_SHUTDOWN.into());
        };
        self.stream.stop()?;
        unsafe {
            pd.DeselectStream(0)?;
            let t = pv_i64(MFGetSystemTime());
            queue.QueueEventParamVar(MESourceStopped.0 as u32, &GUID::zeroed(), S_OK, &t)?;
        }
        Ok(())
    }

    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        let mut st = self.st.lock().unwrap();
        if let Some(q) = st.queue.take() {
            unsafe {
                let _ = q.Shutdown();
            }
        }
        st.pd = None;
        drop(st);
        self.stream.shutdown();
        Ok(())
    }
}

impl IMFMediaSourceEx_Impl for MediaSource_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.attrs.clone())
    }

    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        if id != 0 {
            return Err(E_INVALIDARG.into());
        }
        Ok(self.stream.attrs.clone())
    }

    fn SetD3DManager(&self, _m: Ref<IUnknown>) -> Result<()> {
        // Deliberately stay on CPU memory samples: simpler and works for
        // every consumer (the frame server converts as needed).
        Ok(())
    }
}

impl IMFGetService_Impl for MediaSource_Impl {
    fn GetService(&self, _s: *const GUID, _r: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        if !ppv.is_null() {
            unsafe { *ppv = std::ptr::null_mut() };
        }
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

impl IMFSampleAllocatorControl_Impl for MediaSource_Impl {
    fn SetDefaultAllocator(&self, id: u32, alloc: Ref<IUnknown>) -> Result<()> {
        if id != 0 {
            return Err(E_INVALIDARG.into());
        }
        let a: IMFVideoSampleAllocatorEx = alloc.ok()?.cast()?;
        self.stream.shared.lock().unwrap().allocator = Some(a);
        Ok(())
    }

    fn GetAllocatorUsage(
        &self,
        id: u32,
        pin: *mut u32,
        usage: *mut MFSampleAllocatorUsage,
    ) -> Result<()> {
        if pin.is_null() || usage.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *pin = id;
            *usage = MFSampleAllocatorUsage_UsesProvidedAllocator;
        }
        Ok(())
    }
}

impl IKsControl_Impl for MediaSource_Impl {
    fn KsProperty(
        &self,
        _p: *const KSIDENTIFIER,
        _l: u32,
        _d: *mut c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.into())
    }
    fn KsMethod(
        &self,
        _p: *const KSIDENTIFIER,
        _l: u32,
        _d: *mut c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.into())
    }
    fn KsEvent(
        &self,
        _p: *const KSIDENTIFIER,
        _l: u32,
        _d: *mut c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.into())
    }
}

// ---------------------------------------------------------------------------
// Media stream
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct OutFmt {
    nv12: bool,
    w: u32,
    h: u32,
}

struct StreamShared {
    state: MF_STREAM_STATE,
    queue: Option<IMFMediaEventQueue>,
    desc: Option<IMFStreamDescriptor>,
    source: Option<IMFMediaSource>,
    allocator: Option<IMFVideoSampleAllocatorEx>,
    fmt: Option<OutFmt>,
    worker: Option<mpsc::Sender<Option<SendBox<Option<IUnknown>>>>>,
}

unsafe impl Send for StreamShared {}

#[implement(IMFMediaStream2, IKsControl, IMFAttributes)]
pub struct MediaStream {
    attrs: IMFAttributes,
    shared: Arc<Mutex<StreamShared>>,
}

impl MediaStream {
    fn new(index: u32) -> Result<Self> {
        let attrs = new_attrs()?;
        unsafe {
            attrs.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            attrs.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, index)?;
            attrs.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            attrs.SetUINT32(
                &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
                MFFrameSourceTypes_Color.0 as u32,
            )?;
        }
        let mut types: Vec<Option<IMFMediaType>> = Vec::new();
        for (w, h) in SIZES {
            types.push(Some(video_type(&MFVideoFormat_NV12, w, h)?));
            types.push(Some(video_type(&MFVideoFormat_RGB32, w, h)?));
        }
        let desc = unsafe { MFCreateStreamDescriptor(index, &types)? };
        unsafe {
            desc.GetMediaTypeHandler()?
                .SetCurrentMediaType(types[0].as_ref().unwrap())?;
        }
        let queue = unsafe { MFCreateEventQueue()? };
        Ok(MediaStream {
            attrs,
            shared: Arc::new(Mutex::new(StreamShared {
                state: MF_STREAM_STATE_STOPPED,
                queue: Some(queue),
                desc: Some(desc),
                source: None,
                allocator: None,
                fmt: None,
                worker: None,
            })),
        })
    }

    fn descriptor(&self) -> Result<IMFStreamDescriptor> {
        self.shared
            .lock()
            .unwrap()
            .desc
            .clone()
            .ok_or_else(|| MF_E_SHUTDOWN.into())
    }

    fn set_source(&self, s: Option<IMFMediaSource>) {
        self.shared.lock().unwrap().source = s;
    }

    fn state(&self) -> MF_STREAM_STATE {
        self.shared.lock().unwrap().state
    }

    fn start(&self, ty: Option<IMFMediaType>) -> Result<()> {
        dbg_log("stream.start");
        let ty = match ty {
            Some(t) => t,
            None => unsafe {
                self.descriptor()?
                    .GetMediaTypeHandler()?
                    .GetCurrentMediaType()?
            },
        };
        let (sub, size) = unsafe {
            (
                ty.GetGUID(&MF_MT_SUBTYPE)?,
                ty.GetUINT64(&MF_MT_FRAME_SIZE)?,
            )
        };
        let fmt = OutFmt {
            nv12: sub == MFVideoFormat_NV12,
            w: (size >> 32) as u32,
            h: size as u32,
        };
        let mut sh = self.shared.lock().unwrap();
        let queue = sh
            .queue
            .clone()
            .ok_or_else(|| windows::core::Error::from(MF_E_SHUTDOWN))?;
        if sh.allocator.is_none() {
            // No allocator provided by the pipeline: make our own.
            let mut p: *mut c_void = std::ptr::null_mut();
            unsafe {
                MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut p)?;
                sh.allocator = Some(IMFVideoSampleAllocatorEx::from_raw(p));
            }
        }
        let alloc = sh.allocator.clone().unwrap();
        unsafe {
            let _ = alloc.UninitializeSampleAllocator();
            alloc.InitializeSampleAllocator(10, &ty)?;
        }
        sh.fmt = Some(fmt);
        sh.state = MF_STREAM_STATE_RUNNING;
        if sh.worker.is_none() {
            let (tx, rx) = mpsc::channel();
            sh.worker = Some(tx);
            let shared = self.shared.clone();
            std::thread::Builder::new()
                .name("glyphcam-mf-stream".into())
                .spawn(move || worker(shared, rx))
                .map_err(|_| windows::core::Error::from(E_POINTER))?;
        }
        drop(sh);
        unsafe {
            queue.QueueEventParamVar(
                MEStreamStarted.0 as u32,
                &GUID::zeroed(),
                S_OK,
                std::ptr::null(),
            )?
        };
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        dbg_log("stream.stop");
        let mut sh = self.shared.lock().unwrap();
        let queue = sh
            .queue
            .clone()
            .ok_or_else(|| windows::core::Error::from(MF_E_SHUTDOWN))?;
        if let Some(tx) = sh.worker.take() {
            let _ = tx.send(None);
        }
        if let Some(a) = &sh.allocator {
            unsafe {
                let _ = a.UninitializeSampleAllocator();
            }
        }
        let was = sh.state;
        sh.state = MF_STREAM_STATE_STOPPED;
        drop(sh);
        if was != MF_STREAM_STATE_STOPPED {
            unsafe {
                queue.QueueEventParamVar(
                    MEStreamStopped.0 as u32,
                    &GUID::zeroed(),
                    S_OK,
                    std::ptr::null(),
                )?
            };
        }
        Ok(())
    }

    fn shutdown(&self) {
        let mut sh = self.shared.lock().unwrap();
        if let Some(tx) = sh.worker.take() {
            let _ = tx.send(None);
        }
        if let Some(q) = sh.queue.take() {
            unsafe {
                let _ = q.Shutdown();
            }
        }
        sh.desc = None;
        sh.source = None;
        sh.allocator = None;
        sh.state = MF_STREAM_STATE_STOPPED;
    }
}

/// Produces one sample per request, paced to 30 fps.
fn worker(shared: Arc<Mutex<StreamShared>>, rx: mpsc::Receiver<Option<SendBox<Option<IUnknown>>>>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut feed = Feed::new();
    let interval = Duration::from_nanos(FRAME_100NS as u64 * 100);
    let mut next = Instant::now();
    while let Ok(Some(token)) = rx.recv() {
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > interval * 3 {
            next = now;
        }
        next += interval;
        let (alloc, queue, fmt, running) = {
            let sh = shared.lock().unwrap();
            (
                sh.allocator.clone(),
                sh.queue.clone(),
                sh.fmt,
                sh.state == MF_STREAM_STATE_RUNNING,
            )
        };
        let (Some(alloc), Some(queue), Some(fmt)) = (alloc, queue, fmt) else {
            break;
        };
        if !running {
            continue;
        }
        if let Ok(sample) = make_sample(&alloc, &mut feed, fmt) {
            unsafe {
                if let Some(t) = token.0.as_ref() {
                    let _ = sample.SetUnknown(&MFSampleExtension_Token, t);
                }
                let _ = queue.QueueEventParamUnk(
                    MEMediaSample.0 as u32,
                    &GUID::zeroed(),
                    S_OK,
                    &sample,
                );
            }
        }
    }
    unsafe { CoUninitialize() };
}

fn make_sample(
    alloc: &IMFVideoSampleAllocatorEx,
    feed: &mut Feed,
    fmt: OutFmt,
) -> Result<IMFSample> {
    let (w, h) = (fmt.w as usize, fmt.h as usize);
    unsafe {
        let sample = alloc.AllocateSample()?;
        sample.SetSampleTime(MFGetSystemTime())?;
        sample.SetSampleDuration(FRAME_100NS)?;
        let buf = sample.GetBufferByIndex(0)?;
        let (src, sw, sh) = feed.current(w, h);
        if let Ok(b2) = buf.cast::<IMF2DBuffer2>() {
            let mut scan0: *mut u8 = std::ptr::null_mut();
            let mut pitch: i32 = 0;
            let mut start: *mut u8 = std::ptr::null_mut();
            let mut len: u32 = 0;
            b2.Lock2DSize(
                MF2DBuffer_LockFlags_Write,
                &mut scan0,
                &mut pitch,
                &mut start,
                &mut len,
            )?;
            if fmt.nv12 {
                write_nv12(scan0, pitch as isize, w, h, src, sw, sh);
            } else {
                write_rgb32(scan0, pitch as isize, w, h, src, sw, sh);
            }
            b2.Unlock2D()?;
        } else {
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut max = 0u32;
            buf.Lock(&mut p, Some(&mut max), None)?;
            let need = if fmt.nv12 { w * h * 3 / 2 } else { w * h * 4 };
            if (max as usize) >= need {
                if fmt.nv12 {
                    write_nv12(p, w as isize, w, h, src, sw, sh);
                } else {
                    write_rgb32(p, (w * 4) as isize, w, h, src, sw, sh);
                }
            }
            buf.Unlock()?;
            buf.SetCurrentLength(need as u32)?;
        }
        Ok(sample)
    }
}

impl_attributes!(MediaStream_Impl);

impl IMFMediaEventGenerator_Impl for MediaStream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        let q = self
            .shared
            .lock()
            .unwrap()
            .queue
            .clone()
            .ok_or_else(|| windows::core::Error::from(MF_E_SHUTDOWN))?;
        unsafe { q.GetEvent(flags.0 as u32) }
    }
    fn BeginGetEvent(&self, cb: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        let q = self
            .shared
            .lock()
            .unwrap()
            .queue
            .clone()
            .ok_or_else(|| windows::core::Error::from(MF_E_SHUTDOWN))?;
        unsafe { q.BeginGetEvent(cb.ok()?, state.as_ref()) }
    }
    fn EndGetEvent(&self, r: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        let q = self
            .shared
            .lock()
            .unwrap()
            .queue
            .clone()
            .ok_or_else(|| windows::core::Error::from(MF_E_SHUTDOWN))?;
        unsafe { q.EndGetEvent(r.ok()?) }
    }
    fn QueueEvent(
        &self,
        met: u32,
        ext: *const GUID,
        hr: HRESULT,
        pv: *const PROPVARIANT,
    ) -> Result<()> {
        let q = self
            .shared
            .lock()
            .unwrap()
            .queue
            .clone()
            .ok_or_else(|| windows::core::Error::from(MF_E_SHUTDOWN))?;
        unsafe { q.QueueEventParamVar(met, ext, hr, pv) }
    }
}

impl IMFMediaStream_Impl for MediaStream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        self.shared
            .lock()
            .unwrap()
            .source
            .clone()
            .ok_or_else(|| MF_E_SHUTDOWN.into())
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        self.descriptor()
    }

    fn RequestSample(&self, token: Ref<IUnknown>) -> Result<()> {
        dbg_log("RequestSample");
        let sh = self.shared.lock().unwrap();
        if sh.queue.is_none() {
            return Err(MF_E_SHUTDOWN.into());
        }
        match &sh.worker {
            Some(tx) => {
                let _ = tx.send(Some(SendBox(token.cloned())));
                Ok(())
            }
            None => Err(MF_E_INVALIDREQUEST.into()),
        }
    }
}

impl IMFMediaStream2_Impl for MediaStream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        dbg_log(&format!("SetStreamState {:?}", value));
        let cur = self.state();
        if cur == value {
            return Ok(());
        }
        match value {
            MF_STREAM_STATE_PAUSED => {
                if cur != MF_STREAM_STATE_RUNNING {
                    return Err(MF_E_INVALID_STATE_TRANSITION.into());
                }
                self.shared.lock().unwrap().state = value;
                Ok(())
            }
            MF_STREAM_STATE_RUNNING => self.start(None),
            MF_STREAM_STATE_STOPPED => self.stop(),
            _ => Err(MF_E_INVALID_STATE_TRANSITION.into()),
        }
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        Ok(self.state())
    }
}

impl IKsControl_Impl for MediaStream_Impl {
    fn KsProperty(
        &self,
        _p: *const KSIDENTIFIER,
        _l: u32,
        _d: *mut c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.into())
    }
    fn KsMethod(
        &self,
        _p: *const KSIDENTIFIER,
        _l: u32,
        _d: *mut c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.into())
    }
    fn KsEvent(
        &self,
        _p: *const KSIDENTIFIER,
        _l: u32,
        _d: *mut c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.into())
    }
}

pub(crate) fn dbg_log(msg: &str) {
    if std::env::var_os("GLYPHCAM_MF_DEBUG").is_some() {
        eprintln!("[mfcam] {msg}");
    }
}
