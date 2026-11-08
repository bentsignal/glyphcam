//! The DirectShow source filter: one output ("Capture") pin that pushes
//! frames read from glyphcam's shared memory.

use crate::convert::convert;
use crate::media::{self, Fmt};
use crate::placeholder;
use glyphcam_shm::win::{filter_clsid, SharedMapping};
use glyphcam_shm::WriterState;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{implement, Interface, Ref, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, E_UNEXPECTED, S_FALSE, S_OK,
};
use windows::Win32::Media::DirectShow::*;
use windows::Win32::Media::IReferenceClock;
use windows::Win32::Media::KernelStreaming::{IKsPropertySet, IKsPropertySet_Impl};
use windows_core::IUnknownImpl;
const KSPROPERTY_SUPPORT_GET: u32 = 1;
const PIN_CATEGORY_CAPTURE: GUID = GUID::from_u128(0xfb6c4281_0353_11d1_905f_0000c0cc16ba);
use windows::Win32::Media::MediaFoundation::{
    AMPROPSETID_Pin, CLSID_MemoryAllocator, AM_MEDIA_TYPE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoUninitialize, IPersist_Impl,
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

const PIN_NAME: &str = "Capture";
const E_PROP_ID_UNSUPPORTED: HRESULT = HRESULT(0x80070490u32 as i32);
const E_PROP_SET_UNSUPPORTED: HRESULT = HRESULT(0x80070492u32 as i32);
const VFW_E_NO_CLOCK: HRESULT = HRESULT(0x80040213u32 as i32);
const VFW_E_NOT_FOUND: HRESULT = HRESULT(0x80040216u32 as i32);
const VFW_E_INVALIDMEDIATYPE: HRESULT = HRESULT(0x80040200u32 as i32);

/// Wrapper to move COM pointers into the streaming thread. DirectShow
/// filters and allocators are free-threaded.
struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

struct Streamer {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

struct Inner {
    state: FILTER_STATE,
    /// Non-owning (DirectShow convention: filters don't AddRef the graph).
    graph: *mut c_void,
    name: Vec<u16>,
    clock: Option<IReferenceClock>,
    connected: Option<IPin>,
    mem_input: Option<IMemInputPin>,
    allocator: Option<IMemAllocator>,
    fmt: Option<Fmt>,
    preferred: Option<Fmt>,
    start_time: i64,
    /// Non-owning pointer back to the filter, cleared when it dies.
    filter: *mut c_void,
    streamer: Option<Streamer>,
}

unsafe impl Send for Inner {}

pub struct Shared {
    inner: Mutex<Inner>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stop_streaming(&self) {
        let s = self.lock().streamer.take();
        if let Some(s) = s {
            s.stop.store(true, Ordering::SeqCst);
            // Unblock a GetBuffer wait by decommitting.
            if let Some(a) = self.lock().allocator.clone() {
                unsafe {
                    let _ = a.Decommit();
                }
            }
            let _ = s.handle.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

#[implement(IBaseFilter, IAMFilterMiscFlags)]
pub struct Filter {
    sh: Arc<Shared>,
    pin: IPin,
}

impl Filter {
    pub fn create() -> IBaseFilter {
        let sh = Arc::new(Shared {
            inner: Mutex::new(Inner {
                state: State_Stopped,
                graph: std::ptr::null_mut(),
                name: Vec::new(),
                clock: None,
                connected: None,
                mem_input: None,
                allocator: None,
                fmt: None,
                preferred: None,
                start_time: 0,
                filter: std::ptr::null_mut(),
                streamer: None,
            }),
        });
        let pin: IPin = Pin { sh: sh.clone() }.into();
        let filter: IBaseFilter = Filter {
            sh: sh.clone(),
            pin,
        }
        .into();
        sh.lock().filter = filter.as_raw();
        unsafe { patch_get_state(&filter) };
        filter
    }
}

impl Drop for Filter {
    fn drop(&mut self) {
        self.sh.stop_streaming();
        let mut i = self.sh.lock();
        i.filter = std::ptr::null_mut();
        i.connected = None;
        i.mem_input = None;
        if let Some(a) = i.allocator.take() {
            unsafe {
                let _ = a.Decommit();
            }
        }
        i.clock = None;
    }
}

// GetState must return VFW_S_CANT_CUE while paused (we're a live source),
// which the generated Result<FILTER_STATE> shim can't express. We patch the
// object's vtable pointer to a copy whose GetState wraps the original.
type GetStateFn = unsafe extern "system" fn(*mut c_void, u32, *mut FILTER_STATE) -> HRESULT;
static ORIG_GET_STATE: OnceLock<usize> = OnceLock::new();
static PATCHED_VTBL: OnceLock<usize> = OnceLock::new();

unsafe extern "system" fn get_state_live(
    this: *mut c_void,
    ms: u32,
    state: *mut FILTER_STATE,
) -> HRESULT {
    let orig: GetStateFn = std::mem::transmute(*ORIG_GET_STATE.get().unwrap());
    let hr = orig(this, ms, state);
    if hr == S_OK && !state.is_null() && *state == State_Paused {
        return VFW_S_CANT_CUE;
    }
    hr
}

unsafe fn patch_get_state(filter: &IBaseFilter) {
    let slot = filter.as_raw() as *mut *const IBaseFilter_Vtbl;
    let patched = *PATCHED_VTBL.get_or_init(|| {
        let mut v: IBaseFilter_Vtbl = std::ptr::read(*slot);
        let _ = ORIG_GET_STATE.set(v.base__.GetState as usize);
        v.base__.GetState = get_state_live;
        Box::leak(Box::new(v)) as *const IBaseFilter_Vtbl as usize
    });
    *slot = patched as *const IBaseFilter_Vtbl;
}

impl IPersist_Impl for Filter_Impl {
    fn GetClassID(&self) -> windows::core::Result<GUID> {
        Ok(filter_clsid())
    }
}

impl IMediaFilter_Impl for Filter_Impl {
    fn Stop(&self) -> windows::core::Result<()> {
        self.sh.stop_streaming();
        let mut i = self.sh.lock();
        if let Some(a) = &i.allocator {
            unsafe {
                let _ = a.Decommit();
            }
        }
        i.state = State_Stopped;
        Ok(())
    }

    fn Pause(&self) -> windows::core::Result<()> {
        let state = self.sh.lock().state;
        if state == State_Running {
            self.sh.stop_streaming();
        }
        let mut i = self.sh.lock();
        if let Some(a) = &i.allocator {
            unsafe {
                let _ = a.Commit();
            }
        }
        i.state = State_Paused;
        Ok(())
    }

    fn Run(&self, tstart: i64) -> windows::core::Result<()> {
        if self.sh.lock().state == State_Stopped {
            self.Pause()?;
        }
        let mut i = self.sh.lock();
        i.start_time = tstart;
        i.state = State_Running;
        if i.streamer.is_none() {
            if let (Some(mip), Some(alloc), Some(fmt)) =
                (i.mem_input.clone(), i.allocator.clone(), i.fmt)
            {
                unsafe {
                    let _ = alloc.Commit();
                }
                let stop = Arc::new(AtomicBool::new(false));
                let st = stop.clone();
                let sh = self.sh.clone();
                let mip = SendBox(mip);
                let alloc = SendBox(alloc);
                let handle = std::thread::Builder::new()
                    .name("glyphcam-vcam".into())
                    .spawn(move || {
                        let (mip, alloc) = (mip, alloc);
                        stream_loop(sh, st, mip.0, alloc.0, fmt)
                    })
                    .map_err(|_| windows::core::Error::from(E_FAIL))?;
                i.streamer = Some(Streamer { stop, handle });
            }
        }
        Ok(())
    }

    fn GetState(&self, _ms: u32) -> windows::core::Result<FILTER_STATE> {
        Ok(self.sh.lock().state)
    }

    fn SetSyncSource(&self, pclock: Ref<IReferenceClock>) -> windows::core::Result<()> {
        self.sh.lock().clock = pclock.cloned();
        Ok(())
    }

    fn GetSyncSource(&self) -> windows::core::Result<IReferenceClock> {
        self.sh
            .lock()
            .clock
            .clone()
            .ok_or_else(|| VFW_E_NO_CLOCK.into())
    }
}

impl IBaseFilter_Impl for Filter_Impl {
    fn EnumPins(&self) -> windows::core::Result<IEnumPins> {
        Ok(PinEnum {
            pins: vec![self.pin.clone()],
            pos: Mutex::new(0),
        }
        .into())
    }

    fn FindPin(&self, id: &PCWSTR) -> windows::core::Result<IPin> {
        let s = unsafe { id.to_string() }.unwrap_or_default();
        if s == PIN_NAME || s == "0" || s == "1" {
            Ok(self.pin.clone())
        } else {
            Err(VFW_E_NOT_FOUND.into())
        }
    }

    fn QueryFilterInfo(&self, pinfo: *mut FILTER_INFO) -> windows::core::Result<()> {
        if pinfo.is_null() {
            return Err(E_POINTER.into());
        }
        let i = self.sh.lock();
        let mut info = FILTER_INFO::default();
        let n = i.name.len().min(127);
        info.achName[..n].copy_from_slice(&i.name[..n]);
        let graph = unsafe { IFilterGraph::from_raw_borrowed(&i.graph).cloned() };
        info.pGraph = std::mem::ManuallyDrop::new(graph);
        unsafe { pinfo.write(info) };
        Ok(())
    }

    fn JoinFilterGraph(
        &self,
        pgraph: Ref<IFilterGraph>,
        pname: &PCWSTR,
    ) -> windows::core::Result<()> {
        let mut i = self.sh.lock();
        i.graph = pgraph
            .as_ref()
            .map(|g| g.as_raw())
            .unwrap_or(std::ptr::null_mut());
        i.name = if pname.is_null() {
            Vec::new()
        } else {
            unsafe { pname.as_wide().to_vec() }
        };
        if i.graph.is_null() {
            i.clock = None;
        }
        Ok(())
    }

    fn QueryVendorInfo(&self) -> windows::core::Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }
}

impl IAMFilterMiscFlags_Impl for Filter_Impl {
    fn GetMiscFlags(&self) -> u32 {
        AM_FILTER_MISC_FLAGS_IS_SOURCE.0 as u32
    }
}

// ---------------------------------------------------------------------------
// Pin
// ---------------------------------------------------------------------------

#[implement(IPin, IAMStreamConfig, IKsPropertySet)]
pub struct Pin {
    sh: Arc<Shared>,
}

unsafe fn co_wstr(s: &str) -> PWSTR {
    let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let p = CoTaskMemAlloc(w.len() * 2) as *mut u16;
    if !p.is_null() {
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
    }
    PWSTR(p)
}

impl Pin_Impl {
    fn candidates(&self, pmt: *const AM_MEDIA_TYPE) -> Vec<Fmt> {
        unsafe {
            if let Some(f) = media::parse_mt(pmt) {
                return vec![f];
            }
            if let Some(p) = self.sh.lock().preferred {
                return if media::matches_partial(pmt, &p) {
                    vec![p]
                } else {
                    vec![]
                };
            }
            media::all_formats()
                .into_iter()
                .filter(|f| media::matches_partial(pmt, f))
                .collect()
        }
    }

    fn decide_allocator(
        &self,
        receiver: &IPin,
        f: &Fmt,
    ) -> windows::core::Result<(IMemInputPin, IMemAllocator)> {
        unsafe {
            let mip: IMemInputPin = receiver.cast()?;
            let req = mip.GetAllocatorRequirements().unwrap_or_default();
            let want = ALLOCATOR_PROPERTIES {
                cBuffers: req.cBuffers.max(3),
                cbBuffer: f.image_size() as i32,
                cbAlign: req.cbAlign.max(1),
                cbPrefix: req.cbPrefix.max(0),
            };
            let try_alloc = |a: &IMemAllocator| -> windows::core::Result<()> {
                let actual = a.SetProperties(&want)?;
                if actual.cbBuffer < want.cbBuffer {
                    return Err(E_FAIL.into());
                }
                mip.NotifyAllocator(a, false)
            };
            if let Ok(a) = mip.GetAllocator() {
                if try_alloc(&a).is_ok() {
                    return Ok((mip, a));
                }
            }
            let a: IMemAllocator =
                CoCreateInstance(&CLSID_MemoryAllocator, None, CLSCTX_INPROC_SERVER)?;
            try_alloc(&a)?;
            Ok((mip, a))
        }
    }
}

impl IPin_Impl for Pin_Impl {
    fn Connect(
        &self,
        preceivepin: Ref<IPin>,
        pmt: *const AM_MEDIA_TYPE,
    ) -> windows::core::Result<()> {
        let receiver = preceivepin.ok()?.clone();
        {
            let i = self.sh.lock();
            if i.connected.is_some() {
                return Err(VFW_E_ALREADY_CONNECTED.into());
            }
            if i.state != State_Stopped {
                return Err(VFW_E_NOT_STOPPED.into());
            }
        }
        let me: IPin = self.to_interface();
        for f in self.candidates(pmt) {
            let mut mt = AM_MEDIA_TYPE::default();
            unsafe { media::fill_mt(&mut mt, &f) };
            {
                let mut i = self.sh.lock();
                i.connected = Some(receiver.clone());
                i.fmt = Some(f);
            }
            let ok = unsafe { receiver.ReceiveConnection(&me, &mt) }.is_ok();
            unsafe { media::free_mt_contents(&mut mt) };
            if ok {
                match self.decide_allocator(&receiver, &f) {
                    Ok((mip, alloc)) => {
                        let mut i = self.sh.lock();
                        i.mem_input = Some(mip);
                        i.allocator = Some(alloc);
                        return Ok(());
                    }
                    Err(_) => unsafe {
                        let _ = receiver.Disconnect();
                    },
                }
            }
            let mut i = self.sh.lock();
            i.connected = None;
            i.fmt = None;
        }
        Err(VFW_E_NO_ACCEPTABLE_TYPES.into())
    }

    fn ReceiveConnection(
        &self,
        _c: Ref<IPin>,
        _pmt: *const AM_MEDIA_TYPE,
    ) -> windows::core::Result<()> {
        Err(E_UNEXPECTED.into()) // output pins don't receive connections
    }

    fn Disconnect(&self) -> windows::core::Result<()> {
        let mut i = self.sh.lock();
        if i.state != State_Stopped {
            return Err(VFW_E_NOT_STOPPED.into());
        }
        if i.connected.is_none() {
            return Err(S_FALSE.into());
        }
        if let Some(a) = i.allocator.take() {
            unsafe {
                let _ = a.Decommit();
            }
        }
        i.connected = None;
        i.mem_input = None;
        i.fmt = None;
        Ok(())
    }

    fn ConnectedTo(&self) -> windows::core::Result<IPin> {
        self.sh
            .lock()
            .connected
            .clone()
            .ok_or_else(|| VFW_E_NOT_CONNECTED.into())
    }

    fn ConnectionMediaType(&self, pmt: *mut AM_MEDIA_TYPE) -> windows::core::Result<()> {
        if pmt.is_null() {
            return Err(E_POINTER.into());
        }
        let i = self.sh.lock();
        match (i.connected.as_ref(), i.fmt) {
            (Some(_), Some(f)) => {
                unsafe { media::fill_mt(pmt, &f) };
                Ok(())
            }
            _ => {
                unsafe { pmt.write(AM_MEDIA_TYPE::default()) };
                Err(VFW_E_NOT_CONNECTED.into())
            }
        }
    }

    fn QueryPinInfo(&self, pinfo: *mut PIN_INFO) -> windows::core::Result<()> {
        if pinfo.is_null() {
            return Err(E_POINTER.into());
        }
        let i = self.sh.lock();
        let filter = unsafe { IBaseFilter::from_raw_borrowed(&i.filter).cloned() };
        let mut info = PIN_INFO {
            pFilter: std::mem::ManuallyDrop::new(filter),
            dir: PINDIR_OUTPUT,
            achName: [0; 128],
        };
        for (d, s) in info.achName.iter_mut().zip(PIN_NAME.encode_utf16()) {
            *d = s;
        }
        unsafe { pinfo.write(info) };
        Ok(())
    }

    fn QueryDirection(&self) -> windows::core::Result<PIN_DIRECTION> {
        Ok(PINDIR_OUTPUT)
    }

    fn QueryId(&self) -> windows::core::Result<PWSTR> {
        Ok(unsafe { co_wstr(PIN_NAME) })
    }

    fn QueryAccept(&self, pmt: *const AM_MEDIA_TYPE) -> HRESULT {
        if unsafe { media::parse_mt(pmt) }.is_some() {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn EnumMediaTypes(&self) -> windows::core::Result<IEnumMediaTypes> {
        let list = match self.sh.lock().preferred {
            Some(p) => vec![p],
            None => media::all_formats(),
        };
        Ok(MediaTypeEnum {
            list,
            pos: Mutex::new(0),
        }
        .into())
    }

    fn QueryInternalConnections(
        &self,
        _a: windows::core::OutRef<IPin>,
        _n: *mut u32,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn EndOfStream(&self) -> windows::core::Result<()> {
        Err(E_UNEXPECTED.into())
    }

    fn BeginFlush(&self) -> windows::core::Result<()> {
        Err(E_UNEXPECTED.into())
    }

    fn EndFlush(&self) -> windows::core::Result<()> {
        Err(E_UNEXPECTED.into())
    }

    fn NewSegment(&self, _a: i64, _b: i64, _r: f64) -> windows::core::Result<()> {
        Ok(())
    }
}

impl IAMStreamConfig_Impl for Pin_Impl {
    fn SetFormat(&self, pmt: *const AM_MEDIA_TYPE) -> windows::core::Result<()> {
        if pmt.is_null() {
            self.sh.lock().preferred = None;
            return Ok(());
        }
        let f = unsafe { media::parse_mt(pmt) }
            .ok_or_else(|| windows::core::Error::from(VFW_E_INVALIDMEDIATYPE))?;
        let (connected, current, graph, state) = {
            let mut i = self.sh.lock();
            i.preferred = Some(f);
            (i.connected.clone(), i.fmt, i.graph, i.state)
        };
        if let Some(peer) = connected {
            if current == Some(f) {
                return Ok(());
            }
            if state != State_Stopped {
                return Err(VFW_E_NOT_STOPPED.into());
            }
            let mut mt = AM_MEDIA_TYPE::default();
            unsafe { media::fill_mt(&mut mt, &f) };
            let accepted = unsafe { peer.QueryAccept(&mt) } == S_OK;
            unsafe { media::free_mt_contents(&mut mt) };
            if !accepted {
                return Err(VFW_E_INVALIDMEDIATYPE.into());
            }
            if let Some(g) = unsafe { IFilterGraph::from_raw_borrowed(&graph) } {
                let me: IPin = self.to_interface();
                unsafe { g.Reconnect(&me)? };
            }
        }
        Ok(())
    }

    fn GetFormat(&self) -> windows::core::Result<*mut AM_MEDIA_TYPE> {
        let i = self.sh.lock();
        let f = i
            .fmt
            .or(i.preferred)
            .unwrap_or_else(|| media::all_formats()[0]);
        Ok(unsafe { media::alloc_mt(&f) })
    }

    fn GetNumberOfCapabilities(
        &self,
        picount: *mut i32,
        pisize: *mut i32,
    ) -> windows::core::Result<()> {
        if picount.is_null() || pisize.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *picount = media::all_formats().len() as i32;
            *pisize = std::mem::size_of::<VIDEO_STREAM_CONFIG_CAPS>() as i32;
        }
        Ok(())
    }

    fn GetStreamCaps(
        &self,
        iindex: i32,
        ppmt: *mut *mut AM_MEDIA_TYPE,
        pscc: *mut u8,
    ) -> windows::core::Result<()> {
        let all = media::all_formats();
        let f = all
            .get(iindex as usize)
            .ok_or_else(|| windows::core::Error::from(S_FALSE))?;
        if ppmt.is_null() || pscc.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *ppmt = media::alloc_mt(f);
            (pscc as *mut VIDEO_STREAM_CONFIG_CAPS).write_unaligned(media::caps(f));
        }
        Ok(())
    }
}

impl IKsPropertySet_Impl for Pin_Impl {
    fn Set(
        &self,
        _g: *const GUID,
        _id: u32,
        _i: *const c_void,
        _ci: u32,
        _d: *const c_void,
        _cd: u32,
    ) -> windows::core::Result<()> {
        Err(E_PROP_SET_UNSUPPORTED.into())
    }

    fn Get(
        &self,
        guidpropset: *const GUID,
        dwpropid: u32,
        _i: *const c_void,
        _ci: u32,
        ppropdata: *mut c_void,
        cbpropdata: u32,
        pcbreturned: *mut u32,
    ) -> windows::core::Result<()> {
        unsafe {
            if guidpropset.is_null() || *guidpropset != AMPROPSETID_Pin {
                return Err(E_PROP_SET_UNSUPPORTED.into());
            }
            if dwpropid != AMPROPERTY_PIN_CATEGORY.0 as u32 {
                return Err(E_PROP_ID_UNSUPPORTED.into());
            }
            if !pcbreturned.is_null() {
                *pcbreturned = std::mem::size_of::<GUID>() as u32;
            }
            if ppropdata.is_null() {
                return Ok(());
            }
            if (cbpropdata as usize) < std::mem::size_of::<GUID>() {
                return Err(E_UNEXPECTED.into());
            }
            (ppropdata as *mut GUID).write_unaligned(PIN_CATEGORY_CAPTURE);
        }
        Ok(())
    }

    fn QuerySupported(
        &self,
        guidpropset: *const GUID,
        dwpropid: u32,
    ) -> windows::core::Result<u32> {
        unsafe {
            if guidpropset.is_null() || *guidpropset != AMPROPSETID_Pin {
                return Err(E_PROP_SET_UNSUPPORTED.into());
            }
        }
        if dwpropid != AMPROPERTY_PIN_CATEGORY.0 as u32 {
            return Err(E_PROP_ID_UNSUPPORTED.into());
        }
        Ok(KSPROPERTY_SUPPORT_GET)
    }
}

// ---------------------------------------------------------------------------
// Enumerators
// ---------------------------------------------------------------------------

#[implement(IEnumPins)]
struct PinEnum {
    pins: Vec<IPin>,
    pos: Mutex<usize>,
}

impl IEnumPins_Impl for PinEnum_Impl {
    fn Next(&self, cpins: u32, pppins: *mut Option<IPin>, pcfetched: *mut u32) -> HRESULT {
        if pppins.is_null() {
            return E_POINTER;
        }
        let mut pos = self.pos.lock().unwrap();
        let mut n = 0u32;
        while n < cpins && *pos < self.pins.len() {
            unsafe { pppins.add(n as usize).write(Some(self.pins[*pos].clone())) };
            *pos += 1;
            n += 1;
        }
        if !pcfetched.is_null() {
            unsafe { *pcfetched = n };
        }
        if n == cpins {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, c: u32) -> windows::core::Result<()> {
        let mut pos = self.pos.lock().unwrap();
        *pos += c as usize;
        if *pos <= self.pins.len() {
            Ok(())
        } else {
            *pos = self.pins.len();
            Err(S_FALSE.into())
        }
    }

    fn Reset(&self) -> windows::core::Result<()> {
        *self.pos.lock().unwrap() = 0;
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumPins> {
        Ok(PinEnum {
            pins: self.pins.clone(),
            pos: Mutex::new(*self.pos.lock().unwrap()),
        }
        .into())
    }
}

#[implement(IEnumMediaTypes)]
struct MediaTypeEnum {
    list: Vec<Fmt>,
    pos: Mutex<usize>,
}

impl IEnumMediaTypes_Impl for MediaTypeEnum_Impl {
    fn Next(&self, c: u32, pp: *mut *mut AM_MEDIA_TYPE, pcfetched: *mut u32) -> HRESULT {
        if pp.is_null() {
            return E_POINTER;
        }
        let mut pos = self.pos.lock().unwrap();
        let mut n = 0u32;
        while n < c && *pos < self.list.len() {
            unsafe { *pp.add(n as usize) = media::alloc_mt(&self.list[*pos]) };
            *pos += 1;
            n += 1;
        }
        if !pcfetched.is_null() {
            unsafe { *pcfetched = n };
        }
        if n == c {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, c: u32) -> windows::core::Result<()> {
        let mut pos = self.pos.lock().unwrap();
        *pos += c as usize;
        if *pos <= self.list.len() {
            Ok(())
        } else {
            *pos = self.list.len();
            Err(S_FALSE.into())
        }
    }

    fn Reset(&self) -> windows::core::Result<()> {
        *self.pos.lock().unwrap() = 0;
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumMediaTypes> {
        Ok(MediaTypeEnum {
            list: self.list.clone(),
            pos: Mutex::new(*self.pos.lock().unwrap()),
        }
        .into())
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

fn stream_loop(
    sh: Arc<Shared>,
    stop: Arc<AtomicBool>,
    mip: IMemInputPin,
    alloc: IMemAllocator,
    f: Fmt,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let shm = SharedMapping::open_or_create().ok();
    let size = f.image_size();
    let mut out = vec![0u8; size];
    let mut shown: Option<WriterState> = None;
    let mut last_frame_no = 0u64;
    let interval = Duration::from_nanos(f.interval as u64 * 100);
    let mut next = Instant::now();
    let mut frame_n: i64 = 0;

    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > interval * 4 {
            next = now; // fell behind (e.g. machine asleep); resync
        }
        next += interval;
        if stop.load(Ordering::SeqCst) {
            break;
        }

        // Refresh the converted frame.
        let state = match &shm {
            Some(m) => {
                let r = m.region();
                r.touch_reader(f.w as u32, f.h as u32);
                r.writer_state()
            }
            None => WriterState::Offline,
        };
        if state == WriterState::Live {
            if let Some(m) = &shm {
                if let Some((n, ())) = m.region().read_latest(last_frame_no, |w, h, data| {
                    convert(data, w as usize, h as usize, &f, &mut out)
                }) {
                    last_frame_no = n;
                    shown = Some(WriterState::Live);
                }
            }
        } else if shown != Some(state) {
            let ph = placeholder::render(state, f.w as usize, f.h as usize);
            convert(&ph, f.w as usize, f.h as usize, &f, &mut out);
            shown = Some(state);
            last_frame_no = 0;
        }

        let mut sample: Option<IMediaSample> = None;
        if unsafe { alloc.GetBuffer(&mut sample, None, None, 0) }.is_err() {
            break; // decommitted
        }
        let Some(sample) = sample else { break };
        unsafe {
            let Ok(ptr) = sample.GetPointer() else {
                continue;
            };
            let cap = sample.GetSize().max(0) as usize;
            let len = size.min(cap);
            std::ptr::copy_nonoverlapping(out.as_ptr(), ptr, len);
            let _ = sample.SetActualDataLength(len as i32);
            let (clock, start_time) = {
                let i = sh.lock();
                (i.clock.clone(), i.start_time)
            };
            let mut t0 = frame_n * f.interval;
            if let Some(c) = clock {
                if let Ok(t) = c.GetTime() {
                    t0 = (t - start_time).max(0);
                }
            }
            let t1 = t0 + f.interval;
            let _ = sample.SetTime(Some(&t0), Some(&t1));
            let _ = sample.SetSyncPoint(true);
            let _ = sample.SetDiscontinuity(frame_n == 0);
            let _ = mip.Receive(&sample);
        }
        frame_n += 1;
    }
    drop(mip);
    drop(alloc);
    unsafe { CoUninitialize() };
}

#[allow(dead_code)]
fn _assert_hresults() -> [HRESULT; 2] {
    [E_INVALIDARG, S_OK]
}
