//! Shared-memory frame protocol between the glyphcam app (single writer) and
//! the virtual-camera filter DLL loaded inside consumer apps such as Discord,
//! Zoom, OBS or a browser (any number of readers).
//!
//! Layout: a 4 KiB header followed by `BUFFERS` BGRA frame slots of
//! `MAX_W * MAX_H * 4` bytes. Each slot is guarded by a sequence lock
//! (odd = being written). The writer round-robins through the slots, so a
//! reader copying the latest slot never races the writer in practice; the
//! seqlock catches the rare case where it would.
//!
//! The layout uses only fixed-size types so 32-bit and 64-bit processes
//! agree on it. Bump [`VERSION`] (and the mapping name) on any change.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const MAGIC: u32 = u32::from_le_bytes(*b"GLYC");
pub const VERSION: u32 = 1;
pub const MAX_W: u32 = 1920;
pub const MAX_H: u32 = 1080;
pub const BUFFERS: usize = 3;
pub const HEADER_SIZE: usize = 4096;
pub const SLOT_SIZE: usize = (MAX_W * MAX_H * 4) as usize;
pub const TOTAL_SIZE: usize = HEADER_SIZE + SLOT_SIZE * BUFFERS;

/// Per-session name; consumer apps run in the same session as glyphcam.
pub const MAPPING_NAME: &str = "Local\\glyphcam-vcam-v1";

/// Display name of the virtual camera.
pub const CAMERA_NAME: &str = "glyphcam";
/// CLSID of the DirectShow source filter.
pub const FILTER_CLSID: &str = "{03BC56EC-AA15-4444-9927-0F394623923E}";

/// What the writer is doing, shown on the camera's placeholder frame.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterState {
    /// App not running (or heartbeat stale).
    Offline = 0,
    /// App running, camera is warming up because a reader appeared.
    Starting = 1,
    /// Live frames.
    Live = 2,
    /// User switched the virtual camera off in glyphcam.
    Paused = 3,
    /// The physical camera failed to open.
    NoCamera = 4,
}

impl WriterState {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Starting,
            2 => Self::Live,
            3 => Self::Paused,
            4 => Self::NoCamera,
            _ => Self::Offline,
        }
    }
}

#[repr(C)]
pub struct SlotMeta {
    pub seq: AtomicU64,
    pub width: AtomicU32,
    pub height: AtomicU32,
    pub frame_no: AtomicU64,
}

#[repr(C)]
pub struct Header {
    pub magic: AtomicU32,
    pub version: AtomicU32,
    pub writer_state: AtomicU32,
    pub latest: AtomicU32,
    /// Unix milliseconds, refreshed by the app at least every 250 ms.
    pub writer_heartbeat: AtomicU64,
    /// Unix milliseconds, refreshed by every streaming reader each frame.
    pub reader_heartbeat: AtomicU64,
    /// Most recent resolution a reader negotiated (hint for the writer).
    pub reader_width: AtomicU32,
    pub reader_height: AtomicU32,
    pub frame_counter: AtomicU64,
    pub slots: [SlotMeta; BUFFERS],
}

const _: () = assert!(std::mem::size_of::<Header>() <= HEADER_SIZE);

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A mapped view of the shared region. Platform code provides the memory.
pub struct Region {
    base: *mut u8,
}

unsafe impl Send for Region {}
unsafe impl Sync for Region {}

impl Region {
    /// # Safety
    /// `base` must point to at least [`TOTAL_SIZE`] writable bytes that stay
    /// mapped for the lifetime of the `Region`.
    pub unsafe fn from_raw(base: *mut u8) -> Self {
        Region { base }
    }

    pub fn header(&self) -> &Header {
        unsafe { &*(self.base as *const Header) }
    }

    fn slot_ptr(&self, i: usize) -> *mut u8 {
        unsafe { self.base.add(HEADER_SIZE + i * SLOT_SIZE) }
    }

    /// Initialize the header if nobody has yet (idempotent).
    pub fn init(&self) {
        let h = self.header();
        if h.magic.load(Ordering::Acquire) != MAGIC {
            h.version.store(VERSION, Ordering::Relaxed);
            h.magic.store(MAGIC, Ordering::Release);
        }
    }

    pub fn is_valid(&self) -> bool {
        let h = self.header();
        h.magic.load(Ordering::Acquire) == MAGIC && h.version.load(Ordering::Relaxed) == VERSION
    }

    // ---------------- writer side ----------------

    pub fn set_state(&self, s: WriterState) {
        let h = self.header();
        h.writer_state.store(s as u32, Ordering::Release);
        h.writer_heartbeat.store(now_ms(), Ordering::Release);
    }

    /// Publish a top-down BGRA frame (`stride == width * 4`).
    pub fn write_frame(&self, width: u32, height: u32, bgra: &[u8]) {
        let width = width.min(MAX_W);
        let height = height.min(MAX_H);
        let len = (width * height * 4) as usize;
        if bgra.len() < len {
            return;
        }
        let h = self.header();
        let next = (h.latest.load(Ordering::Relaxed) as usize + 1) % BUFFERS;
        let meta = &h.slots[next];
        let seq = meta.seq.load(Ordering::Relaxed);
        meta.seq.store(seq | 1, Ordering::Release);
        std::sync::atomic::fence(Ordering::Release);
        unsafe { std::ptr::copy_nonoverlapping(bgra.as_ptr(), self.slot_ptr(next), len) };
        meta.width.store(width, Ordering::Relaxed);
        meta.height.store(height, Ordering::Relaxed);
        let n = h.frame_counter.fetch_add(1, Ordering::Relaxed) + 1;
        meta.frame_no.store(n, Ordering::Relaxed);
        meta.seq.store((seq | 1) + 1, Ordering::Release);
        h.latest.store(next as u32, Ordering::Release);
        h.writer_state
            .store(WriterState::Live as u32, Ordering::Release);
        h.writer_heartbeat.store(now_ms(), Ordering::Release);
    }

    /// Milliseconds since a reader last pulled a frame.
    pub fn reader_idle_ms(&self) -> u64 {
        now_ms().saturating_sub(self.header().reader_heartbeat.load(Ordering::Acquire))
    }

    // ---------------- reader side ----------------

    /// Current writer state, treating a stale heartbeat as offline.
    pub fn writer_state(&self) -> WriterState {
        let h = self.header();
        let age = now_ms().saturating_sub(h.writer_heartbeat.load(Ordering::Acquire));
        if age > 2000 {
            WriterState::Offline
        } else {
            WriterState::from_u32(h.writer_state.load(Ordering::Acquire))
        }
    }

    pub fn touch_reader(&self, width: u32, height: u32) {
        let h = self.header();
        h.reader_heartbeat.store(now_ms(), Ordering::Release);
        h.reader_width.store(width, Ordering::Relaxed);
        h.reader_height.store(height, Ordering::Relaxed);
    }

    /// Call `f(width, height, bgra)` with the latest frame if it is newer
    /// than `last_frame_no`. Returns the frame number read.
    pub fn read_latest<R>(
        &self,
        last_frame_no: u64,
        f: impl FnOnce(u32, u32, &[u8]) -> R,
    ) -> Option<(u64, R)> {
        let h = self.header();
        let idx = h.latest.load(Ordering::Acquire) as usize % BUFFERS;
        let meta = &h.slots[idx];
        let s1 = meta.seq.load(Ordering::Acquire);
        if s1 & 1 == 1 {
            return None;
        }
        let n = meta.frame_no.load(Ordering::Relaxed);
        if n == last_frame_no || n == 0 {
            return None;
        }
        let w = meta.width.load(Ordering::Relaxed).min(MAX_W);
        let hgt = meta.height.load(Ordering::Relaxed).min(MAX_H);
        let data =
            unsafe { std::slice::from_raw_parts(self.slot_ptr(idx), (w * hgt * 4) as usize) };
        let r = f(w, hgt, data);
        std::sync::atomic::fence(Ordering::Acquire);
        let s2 = meta.seq.load(Ordering::Acquire);
        if s1 != s2 {
            return None; // torn; caller keeps its previous frame
        }
        Some((n, r))
    }
}

#[cfg(windows)]
pub mod win;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_in_memory() {
        let mut mem = vec![0u8; TOTAL_SIZE + 8];
        let base = unsafe { mem.as_mut_ptr().add(mem.as_ptr().align_offset(8)) };
        let r = unsafe { Region::from_raw(base) };
        r.init();
        assert!(r.is_valid());
        assert_eq!(r.writer_state(), WriterState::Offline);
        let frame: Vec<u8> = (0..(4 * 4 * 4)).map(|i| i as u8).collect();
        r.write_frame(4, 4, &frame);
        assert_eq!(r.writer_state(), WriterState::Live);
        let (n, sum) = r
            .read_latest(0, |w, h, d| {
                (w, h, d.iter().map(|&v| v as u32).sum::<u32>())
            })
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(sum, (4, 4, frame.iter().map(|&v| v as u32).sum()));
        assert!(r.read_latest(n, |_, _, _| ()).is_none());
    }
}
