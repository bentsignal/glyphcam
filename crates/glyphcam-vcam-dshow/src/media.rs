//! Media types offered by the virtual camera and AM_MEDIA_TYPE helpers.

use std::mem::{size_of, ManuallyDrop};
use windows::core::GUID;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
use windows::Win32::Media::DirectShow::VIDEO_STREAM_CONFIG_CAPS;
use windows::Win32::Media::MediaFoundation::{
    FORMAT_VideoInfo, MEDIATYPE_Video, AM_MEDIA_TYPE, MEDIASUBTYPE_I420, MEDIASUBTYPE_NV12,
    MEDIASUBTYPE_RGB24, MEDIASUBTYPE_RGB32, MEDIASUBTYPE_YUY2, VIDEOINFOHEADER,
};
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pix {
    Yuy2,
    Nv12,
    I420,
    Rgb24,
    Rgb32,
}

pub const PIXES: [Pix; 5] = [Pix::Yuy2, Pix::Nv12, Pix::I420, Pix::Rgb24, Pix::Rgb32];
/// Same list as glyphcam's output resolutions; 1280x720 first (default).
pub const SIZES: [(i32, i32); 4] = [(1280, 720), (1920, 1080), (960, 540), (640, 360)];
pub const DEFAULT_INTERVAL: i64 = 333_333; // 30 fps
pub const MIN_INTERVAL: i64 = 166_666; // 60 fps
pub const MAX_INTERVAL: i64 = 2_000_000; // 5 fps

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fmt {
    pub pix: Pix,
    pub w: i32,
    pub h: i32,
    pub interval: i64,
    /// RGB only: rows stored top-down (negative biHeight).
    pub top_down: bool,
}

impl Fmt {
    pub fn new(pix: Pix, w: i32, h: i32) -> Self {
        Fmt {
            pix,
            w,
            h,
            interval: DEFAULT_INTERVAL,
            top_down: false,
        }
    }

    pub fn subtype(&self) -> GUID {
        match self.pix {
            Pix::Yuy2 => MEDIASUBTYPE_YUY2,
            Pix::Nv12 => MEDIASUBTYPE_NV12,
            Pix::I420 => MEDIASUBTYPE_I420,
            Pix::Rgb24 => MEDIASUBTYPE_RGB24,
            Pix::Rgb32 => MEDIASUBTYPE_RGB32,
        }
    }

    pub fn bits(&self) -> u16 {
        match self.pix {
            Pix::Yuy2 => 16,
            Pix::Nv12 | Pix::I420 => 12,
            Pix::Rgb24 => 24,
            Pix::Rgb32 => 32,
        }
    }

    fn compression(&self) -> u32 {
        let fourcc = |s: &[u8; 4]| u32::from_le_bytes(*s);
        match self.pix {
            Pix::Yuy2 => fourcc(b"YUY2"),
            Pix::Nv12 => fourcc(b"NV12"),
            Pix::I420 => fourcc(b"I420"),
            Pix::Rgb24 | Pix::Rgb32 => BI_RGB.0,
        }
    }

    pub fn rgb_stride(&self) -> usize {
        match self.pix {
            Pix::Rgb24 => ((self.w as usize * 3) + 3) & !3,
            _ => self.w as usize * 4,
        }
    }

    pub fn image_size(&self) -> usize {
        let (w, h) = (self.w as usize, self.h as usize);
        match self.pix {
            Pix::Yuy2 => w * h * 2,
            Pix::Nv12 | Pix::I420 => w * h + 2 * ((w / 2) * (h / 2)),
            Pix::Rgb24 | Pix::Rgb32 => self.rgb_stride() * h,
        }
    }

    fn vih(&self) -> VIDEOINFOHEADER {
        let height = if self.top_down && matches!(self.pix, Pix::Rgb24 | Pix::Rgb32) {
            -self.h
        } else {
            self.h
        };
        VIDEOINFOHEADER {
            rcSource: RECT::default(),
            rcTarget: RECT::default(),
            dwBitRate: (self.image_size() as i64 * 8 * 10_000_000 / self.interval) as u32,
            dwBitErrorRate: 0,
            AvgTimePerFrame: self.interval,
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.w,
                biHeight: height,
                biPlanes: 1,
                biBitCount: self.bits(),
                biCompression: self.compression(),
                biSizeImage: self.image_size() as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
        }
    }
}

pub fn all_formats() -> Vec<Fmt> {
    let mut v = Vec::new();
    for &(w, h) in &SIZES {
        for &p in &PIXES {
            v.push(Fmt::new(p, w, h));
        }
    }
    v
}

/// Fill a caller-owned AM_MEDIA_TYPE (allocating its format block).
pub unsafe fn fill_mt(mt: *mut AM_MEDIA_TYPE, f: &Fmt) {
    let vih = f.vih();
    let pb = CoTaskMemAlloc(size_of::<VIDEOINFOHEADER>()) as *mut VIDEOINFOHEADER;
    pb.write(vih);
    mt.write(AM_MEDIA_TYPE {
        majortype: MEDIATYPE_Video,
        subtype: f.subtype(),
        bFixedSizeSamples: true.into(),
        bTemporalCompression: false.into(),
        lSampleSize: f.image_size() as u32,
        formattype: FORMAT_VideoInfo,
        pUnk: ManuallyDrop::new(None),
        cbFormat: size_of::<VIDEOINFOHEADER>() as u32,
        pbFormat: pb as *mut u8,
    });
}

/// Allocate a whole AM_MEDIA_TYPE with CoTaskMemAlloc (for enumerators,
/// GetFormat, GetStreamCaps).
pub unsafe fn alloc_mt(f: &Fmt) -> *mut AM_MEDIA_TYPE {
    let p = CoTaskMemAlloc(size_of::<AM_MEDIA_TYPE>()) as *mut AM_MEDIA_TYPE;
    if !p.is_null() {
        fill_mt(p, f);
    }
    p
}

/// Free the format block of an AM_MEDIA_TYPE we filled.
pub unsafe fn free_mt_contents(mt: &mut AM_MEDIA_TYPE) {
    if !mt.pbFormat.is_null() {
        CoTaskMemFree(Some(mt.pbFormat as *const _));
        mt.pbFormat = std::ptr::null_mut();
        mt.cbFormat = 0;
    }
    let unk = ManuallyDrop::take(&mut mt.pUnk);
    drop(unk);
}

/// Parse and validate a proposed media type. Partially specified types
/// (null subtype / format) return `None`; see [`matches_partial`].
pub unsafe fn parse_mt(pmt: *const AM_MEDIA_TYPE) -> Option<Fmt> {
    let mt = pmt.as_ref()?;
    if mt.majortype != MEDIATYPE_Video || mt.formattype != FORMAT_VideoInfo {
        return None;
    }
    if mt.cbFormat < size_of::<VIDEOINFOHEADER>() as u32 || mt.pbFormat.is_null() {
        return None;
    }
    let pix = PIXES
        .into_iter()
        .find(|p| Fmt::new(*p, 0, 0).subtype() == mt.subtype)?;
    let vih = &*(mt.pbFormat as *const VIDEOINFOHEADER);
    let w = vih.bmiHeader.biWidth;
    let h_raw = vih.bmiHeader.biHeight;
    let h = h_raw.abs();
    if !SIZES.contains(&(w, h)) {
        return None;
    }
    let top_down = h_raw < 0;
    if top_down && !matches!(pix, Pix::Rgb24 | Pix::Rgb32) {
        // negative height is meaningless for YUV; tolerate it
    }
    let interval = if vih.AvgTimePerFrame == 0 {
        DEFAULT_INTERVAL
    } else {
        vih.AvgTimePerFrame.clamp(MIN_INTERVAL, MAX_INTERVAL)
    };
    if vih.AvgTimePerFrame != 0
        && (vih.AvgTimePerFrame < MIN_INTERVAL - 10 || vih.AvgTimePerFrame > MAX_INTERVAL)
    {
        return None;
    }
    Some(Fmt {
        pix,
        w,
        h,
        interval,
        top_down: top_down && matches!(pix, Pix::Rgb24 | Pix::Rgb32),
    })
}

/// Whether a partially specified media type (e.g. only majortype set) is
/// compatible with `f`.
pub unsafe fn matches_partial(pmt: *const AM_MEDIA_TYPE, f: &Fmt) -> bool {
    let Some(mt) = pmt.as_ref() else { return true };
    let null = GUID::zeroed();
    (mt.majortype == null || mt.majortype == MEDIATYPE_Video)
        && (mt.subtype == null || mt.subtype == f.subtype())
        && (mt.formattype == null || mt.formattype == FORMAT_VideoInfo)
}

pub fn caps(f: &Fmt) -> VIDEO_STREAM_CONFIG_CAPS {
    use windows::Win32::Foundation::SIZE;
    let sz = SIZE { cx: f.w, cy: f.h };
    VIDEO_STREAM_CONFIG_CAPS {
        guid: FORMAT_VideoInfo,
        VideoStandard: 0,
        InputSize: sz,
        MinCroppingSize: sz,
        MaxCroppingSize: sz,
        CropGranularityX: 1,
        CropGranularityY: 1,
        CropAlignX: 1,
        CropAlignY: 1,
        MinOutputSize: sz,
        MaxOutputSize: sz,
        OutputGranularityX: 1,
        OutputGranularityY: 1,
        StretchTapsX: 0,
        StretchTapsY: 0,
        ShrinkTapsX: 0,
        ShrinkTapsY: 0,
        MinFrameInterval: MIN_INTERVAL,
        MaxFrameInterval: MAX_INTERVAL,
        MinBitsPerSecond: (f.image_size() as i64 * 8 * 5) as i32,
        MaxBitsPerSecond: (f.image_size() as i64 * 8 * 60).min(i32::MAX as i64) as i32,
    }
}
