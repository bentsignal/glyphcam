//! Virtual camera output. On Windows this feeds the DirectShow filter
//! through shared memory; on macOS there is no system virtual camera yet
//! (needs a signed CoreMediaIO Camera Extension), so this is a no-op and the
//! MJPEG stream server is the supported route.

use glyphcam_core::Frame;
pub use glyphcam_shm::WriterState;

pub struct VcamOutput {
    /// Per-session mapping read by the DirectShow filter.
    #[cfg(windows)]
    map: Option<glyphcam_shm::win::SharedMapping>,
    /// Machine-wide mapping read by the Windows 11 camera source. It only
    /// exists once the Frame Server has loaded the source, so we poll for it.
    #[cfg(windows)]
    global: std::cell::RefCell<(
        Option<glyphcam_shm::win::SharedMapping>,
        Option<std::time::Instant>,
    )>,
}

impl VcamOutput {
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            let map = match glyphcam_shm::win::SharedMapping::open_or_create() {
                Ok(m) => Some(m),
                Err(e) => {
                    log::error!("virtual camera shared memory unavailable: {e}");
                    None
                }
            };
            VcamOutput {
                map,
                global: std::cell::RefCell::new((None, None)),
            }
        }
        #[cfg(not(windows))]
        VcamOutput {}
    }

    /// Run `f` on every mapping that exists.
    #[cfg(windows)]
    fn each(&self, mut f: impl FnMut(&glyphcam_shm::Region)) {
        if let Some(m) = &self.map {
            f(m.region());
        }
        let mut g = self.global.borrow_mut();
        if g.0.is_none()
            && g.1
                .is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(250))
        {
            g.1 = Some(std::time::Instant::now());
            if let Ok(m) = glyphcam_shm::win::SharedMapping::open_global() {
                log::info!("Windows 11 camera connected");
                g.0 = Some(m);
            }
        }
        if let Some(m) = &g.0 {
            f(m.region());
        }
    }

    pub fn set_state(&self, _s: WriterState) {
        #[cfg(windows)]
        self.each(|r| r.set_state(_s));
    }

    pub fn write(&self, _f: &Frame) {
        #[cfg(windows)]
        self.each(|r| r.write_frame(_f.width, _f.height, &_f.data));
    }

    /// An app has one of the cameras open and is pulling frames right now.
    pub fn consumer_active(&self) -> bool {
        #[cfg(windows)]
        {
            let mut active = false;
            self.each(|r| active |= r.reader_idle_ms() < 1500);
            return active;
        }
        #[allow(unreachable_code)]
        false
    }
}

impl Drop for VcamOutput {
    fn drop(&mut self) {
        self.set_state(WriterState::Offline);
    }
}

/// Registration of the DirectShow filter DLLs (per user, no admin).
#[cfg(windows)]
pub mod install {
    use std::path::{Path, PathBuf};

    pub fn build_tag() -> String {
        format!(
            "{}-{}",
            env!("CARGO_PKG_VERSION"),
            env!("GLYPHCAM_BUILD_SHA")
        )
    }

    fn exe_dir() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default()
    }

    /// Locate the filter DLL for this build (`x86 = true` for the 32-bit one).
    pub fn dll_path(x86: bool) -> Option<PathBuf> {
        let name = if x86 {
            "glyphcam-vcam-x86.dll"
        } else {
            "glyphcam-vcam-x64.dll"
        };
        let dir = exe_dir();
        let candidates = [
            dir.join("vcam").join(build_tag()).join(name),
            dir.join("vcam").join(name),
            dir.join(name),
            // cargo dev builds
            dir.join(if x86 {
                "glyphcam_vcam_x86.dll"
            } else {
                "glyphcam_vcam.dll"
            }),
        ];
        candidates.into_iter().find(|p| p.exists())
    }

    #[derive(Clone, Debug, Default)]
    pub struct Status {
        pub dll64: Option<PathBuf>,
        pub registered64: Option<PathBuf>,
        pub registered32: Option<PathBuf>,
    }

    impl Status {
        pub fn ok(&self) -> bool {
            matches!((&self.dll64, &self.registered64), (Some(a), Some(b)) if a == b)
        }
    }

    pub fn status() -> Status {
        Status {
            dll64: dll_path(false),
            registered64: glyphcam_shm::win::registered_dll(false),
            registered32: glyphcam_shm::win::registered_dll(true),
        }
    }

    /// Register this build's DLLs if the registry points elsewhere.
    pub fn ensure_registered() -> anyhow::Result<()> {
        let st = status();
        let dll64 = st
            .dll64
            .clone()
            .ok_or_else(|| anyhow::anyhow!("virtual camera DLL not found next to glyphcam"))?;
        if st.registered64.as_ref() != Some(&dll64) {
            glyphcam_shm::win::register_filter(&dll64, false)?;
            log::info!("registered virtual camera: {}", dll64.display());
        }
        if let Some(dll32) = dll_path(true) {
            if st.registered32.as_ref() != Some(&dll32) {
                glyphcam_shm::win::register_filter(&dll32, true)?;
            }
        }
        Ok(())
    }

    pub fn unregister() {
        glyphcam_shm::win::unregister_filter(false);
        glyphcam_shm::win::unregister_filter(true);
    }
}
