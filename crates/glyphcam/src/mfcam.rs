//! Windows 11 "system camera": the Media Foundation virtual camera that
//! Discord, Teams and the Windows Camera app can see (they ignore
//! DirectShow-only cameras).
//!
//! Setup needs one elevated step because the Frame Server service only
//! loads camera sources registered in HKLM from a location it can read:
//! `glyphcam --install-system-camera` copies `glyphcam-mfcam.dll` to
//! `%ProgramFiles%\glyphcam\camera\p<proto>-<build>\` and registers it.
//! Creating the camera itself (`MFCreateVirtualCamera`, system lifetime,
//! current user) needs no admin rights and is done by the app at startup.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HMODULE, WAIT_OBJECT_0};
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFStartup, MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_System,
    MFVirtualCameraType_SoftwareCameraSource, MFSTARTUP_FULL, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

pub const CAMERA_NAME: &str = "glyphcam";
const DLL_NAME: &str = "glyphcam-mfcam.dll";

type CreateFn = unsafe extern "system" fn(
    i32,
    i32,
    i32,
    PCWSTR,
    PCWSTR,
    *const windows::core::GUID,
    u32,
    *mut *mut core::ffi::c_void,
) -> windows::core::HRESULT;

/// `MFCreateVirtualCamera`, resolved at runtime so glyphcam still starts on
/// Windows 10 (where the export doesn't exist).
fn create_fn() -> Option<CreateFn> {
    unsafe {
        let lib: HMODULE = LoadLibraryW(&HSTRING::from("mfsensorgroup.dll")).ok()?;
        let p = GetProcAddress(lib, windows::core::s!("MFCreateVirtualCamera"))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            CreateFn,
        >(p))
    }
}

/// Windows 11 (build 22000+) with virtual camera support.
pub fn supported() -> bool {
    create_fn().is_some()
}

fn proto_tag() -> String {
    format!("p{}-", glyphcam_shm::VERSION)
}

/// The camera DLL shipped with this build.
pub fn bundled_dll() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [
        dir.join("vcam")
            .join(crate::vcam::install::build_tag())
            .join(DLL_NAME),
        dir.join("vcam").join(DLL_NAME),
        dir.join(DLL_NAME),
        dir.join("glyphcam_mfcam.dll"), // cargo dev builds
    ]
    .into_iter()
    .find(|p| p.exists())
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub supported: bool,
    pub registered: Option<PathBuf>,
}

impl Status {
    pub fn installed(&self) -> bool {
        self.registered.as_ref().is_some_and(|p| p.exists())
    }
    /// Installed with a DLL speaking the current shared-memory protocol.
    pub fn up_to_date(&self) -> bool {
        self.installed()
            && self
                .registered
                .as_ref()
                .is_some_and(|p| p.to_string_lossy().contains(&proto_tag()))
    }
}

pub fn status() -> Status {
    Status {
        supported: supported(),
        registered: glyphcam_shm::win::registered_mf_dll(),
    }
}

fn program_files() -> PathBuf {
    std::env::var_os("ProgramW6432")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
}

fn camera_root() -> PathBuf {
    program_files().join("glyphcam").join("camera")
}

/// Elevated part of the setup (`--install-system-camera`).
pub fn install_admin() -> Result<PathBuf> {
    let src = bundled_dll().ok_or_else(|| anyhow!("{DLL_NAME} not found next to glyphcam"))?;
    let dir = camera_root().join(format!(
        "{}{}",
        proto_tag(),
        crate::vcam::install::build_tag()
    ));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let dst = dir.join(DLL_NAME);
    if !dst.exists() {
        std::fs::copy(&src, &dst).with_context(|| format!("copying to {}", dst.display()))?;
    }
    // Frame Server runs as LOCAL SERVICE, its monitor as SYSTEM.
    let _ = std::process::Command::new("icacls")
        .arg(&dir)
        .args([
            "/grant",
            "*S-1-5-19:(OI)(CI)RX",
            "*S-1-5-18:(OI)(CI)RX",
            "*S-1-5-32-545:(OI)(CI)RX",
            "/T",
            "/Q",
        ])
        .status();
    glyphcam_shm::win::register_mf_source(&dst).context("registering the camera source")?;
    // Remove older copies that aren't in use.
    if let Ok(rd) = std::fs::read_dir(camera_root()) {
        for e in rd.flatten() {
            if e.path() != dir {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(dst)
}

/// Elevated part of removal.
pub fn uninstall_admin() {
    glyphcam_shm::win::unregister_mf_source();
    let _ = std::fs::remove_dir_all(program_files().join("glyphcam"));
}

/// Run this executable elevated with `arg` (UAC prompt) and wait for it.
pub fn run_elevated(arg: &str) -> Result<()> {
    let exe = std::env::current_exe()?;
    let file = HSTRING::from(exe.as_os_str());
    let verb = HSTRING::from("runas");
    let params = HSTRING::from(arg);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info)
            .map_err(|e| anyhow!("administrator permission was not granted ({e})"))?;
        if info.hProcess.is_invalid() {
            return Err(anyhow!("could not start the setup helper"));
        }
        let w = WaitForSingleObject(info.hProcess, 120_000);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if w != WAIT_OBJECT_0 {
            return Err(anyhow!("setup helper timed out"));
        }
        if code != 0 {
            return Err(anyhow!(
                "setup helper failed (code {code}); see the glyphcam log"
            ));
        }
    }
    Ok(())
}

fn with_camera<R>(f: impl FnOnce(&IMFVirtualCamera) -> windows::core::Result<R>) -> Result<R> {
    let create = create_fn()
        .ok_or_else(|| anyhow!("Windows 11 virtual cameras are not available on this system"))?;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let name = HSTRING::from(CAMERA_NAME);
        let id = HSTRING::from(glyphcam_shm::MF_SOURCE_CLSID);
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        create(
            MFVirtualCameraType_SoftwareCameraSource.0,
            MFVirtualCameraLifetime_System.0,
            MFVirtualCameraAccess_CurrentUser.0,
            PCWSTR(name.as_ptr()),
            PCWSTR(id.as_ptr()),
            std::ptr::null(),
            0,
            &mut raw,
        )
        .ok()
        .context("MFCreateVirtualCamera")?;
        let cam = IMFVirtualCamera::from_raw(raw);
        Ok(f(&cam)?)
    }
}

/// Create/enable the camera for this user (idempotent, no admin needed).
pub fn ensure_camera() -> Result<()> {
    with_camera(|c| unsafe { c.Start(None) })
}

/// Remove the camera for this user (no admin needed).
pub fn remove_camera() -> Result<()> {
    with_camera(|c| unsafe { c.Remove() })
}

/// Full setup from the (non-elevated) app: admin step, then the camera.
pub fn setup() -> Result<()> {
    if !supported() {
        return Err(anyhow!("needs Windows 11"));
    }
    if !status().up_to_date() {
        run_elevated("--install-system-camera")?;
    }
    ensure_camera()
}

/// Full removal (used by the uninstaller): camera, then the admin cleanup.
pub fn teardown() {
    let _ = remove_camera();
    if glyphcam_shm::win::registered_mf_dll().is_some() {
        let _ = run_elevated("--uninstall-system-camera-admin");
    }
}

#[allow(dead_code)]
fn _p(_: &Path) {}
