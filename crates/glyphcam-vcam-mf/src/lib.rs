//! glyphcam Windows 11 virtual camera: a Media Foundation custom media
//! source COM DLL used through `MFCreateVirtualCamera`. Unlike the
//! DirectShow filter it is visible to Media Foundation apps (Discord,
//! Windows Camera, Teams) as well as DirectShow apps and browsers.
//!
//! It must be registered machine-wide (HKLM) and live where the Frame
//! Server service (LOCAL SERVICE) can read it; glyphcam does both through a
//! one-time elevated step (`glyphcam --install-system-camera`).
#![cfg(windows)]
#![allow(non_snake_case)]

mod attrs;
mod feed;
mod source;

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use windows::core::{implement, IUnknown, Interface, Ref, BOOL, GUID, HRESULT};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, HINSTANCE, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

#[no_mangle]
extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _r: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(hinst.0, Ordering::SeqCst);
    }
    true.into()
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if ppv.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let act = source::Activator::create()?;
        unsafe { act.query(riid, ppv).ok() }
    }

    fn LockServer(&self, _l: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

#[no_mangle]
unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() || rclsid.is_null() || riid.is_null() {
        return E_POINTER;
    }
    *ppv = std::ptr::null_mut();
    if *rclsid != glyphcam_shm::win::mf_source_clsid() {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let f: IClassFactory = Factory.into();
    f.query(riid, ppv)
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

fn module_path() -> std::path::PathBuf {
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(Some(HMODULE(MODULE.load(Ordering::SeqCst))), &mut buf) }
        as usize;
    std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}

/// `regsvr32` (as administrator) registers in HKLM.
#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    match glyphcam_shm::win::register_mf_source(&module_path()) {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    glyphcam_shm::win::unregister_mf_source();
    S_OK
}

/// Create the activator directly (tests and diagnostics, no COM registration).
pub fn create_activator(
) -> windows::core::Result<windows::Win32::Media::MediaFoundation::IMFActivate> {
    source::Activator::create()
}
