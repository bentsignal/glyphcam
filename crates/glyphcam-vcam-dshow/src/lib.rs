//! glyphcam virtual camera: a DirectShow video source filter DLL.
//!
//! The DLL is loaded *inside* consumer apps (Discord, Zoom, OBS, browsers…)
//! when they open the "glyphcam" camera. It reads frames that the glyphcam
//! app publishes to shared memory and pushes them downstream. When glyphcam
//! isn't running it shows a placeholder card instead.
#![cfg(windows)]
#![allow(non_snake_case)]

mod convert;
mod filter;
mod media;
mod placeholder;

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
extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
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
        let f = filter::Filter::create();
        unsafe { f.query(riid, ppv).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
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
    if *rclsid != glyphcam_shm::win::filter_clsid() {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory.into();
    factory.query(riid, ppv)
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    // Streaming threads may outlive filter objects briefly; stay loaded.
    S_FALSE
}

fn module_path() -> std::path::PathBuf {
    let mut buf = [0u16; 1024];
    let h = HMODULE(MODULE.load(Ordering::SeqCst));
    let n = unsafe { GetModuleFileNameW(Some(h), &mut buf) } as usize;
    std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}

const WOW32: bool = cfg!(target_pointer_width = "32");

/// `regsvr32 glyphcam-vcam.dll` registers for the current user.
#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    match glyphcam_shm::win::register_filter(&module_path(), WOW32) {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    glyphcam_shm::win::unregister_filter(WOW32);
    S_OK
}
