//! Windows specifics: the named file mapping and per-user registration of
//! the DirectShow virtual camera filter.

use crate::{Region, CAMERA_NAME, FILTER_CLSID, MAPPING_NAME, TOTAL_SIZE};
use std::path::{Path, PathBuf};
use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    REG_BINARY, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, RRF_RT_REG_SZ,
};

pub struct SharedMapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    region: Region,
}

unsafe impl Send for SharedMapping {}
unsafe impl Sync for SharedMapping {}

impl SharedMapping {
    /// Create the mapping, or open it if another process already did.
    pub fn open_or_create() -> windows::core::Result<Self> {
        unsafe {
            // Everyone + AppContainers may read/write; low-integrity
            // processes (sandboxed browser helpers) are allowed to write too.
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let sddl = HSTRING::from("D:(A;;GA;;;WD)(A;;GA;;;AC)S:(ML;;NW;;;LW)");
            let have_sd = ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .is_ok();
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            let name = HSTRING::from(MAPPING_NAME);
            let size = TOTAL_SIZE as u64;
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                if have_sd { Some(&sa) } else { None },
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                &name,
            );
            if have_sd {
                let _ = LocalFree(Some(HLOCAL(sd.0)));
            }
            let handle = handle?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, TOTAL_SIZE);
            if view.Value.is_null() {
                let err = windows::core::Error::from_thread();
                let _ = CloseHandle(handle);
                return Err(err);
            }
            let region = Region::from_raw(view.Value as *mut u8);
            region.init();
            Ok(SharedMapping {
                handle,
                view,
                region,
            })
        }
    }

    pub fn region(&self) -> &Region {
        &self.region
    }
}

impl Drop for SharedMapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub const CLSID_VIDEO_INPUT_DEVICE_CATEGORY: GUID =
    GUID::from_u128(0x860BB310_5D01_11d0_BD3B_00A0C911CE86);
pub const MEDIATYPE_VIDEO: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00AA00389B71);
pub const PIN_CATEGORY_CAPTURE: GUID = GUID::from_u128(0xfb6c4281_0353_11d1_905f_0000c0cc16ba);
const MERIT_DO_NOT_USE: u32 = 0x0020_0000;
const REG_PINFLAG_B_OUTPUT: u32 = 0x8;

pub fn filter_clsid() -> GUID {
    GUID::from_u128(0x03BC56EC_AA15_4444_9927_0F394623923E)
}

fn guid_bytes(g: &GUID) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&g.data1.to_le_bytes());
    b[4..6].copy_from_slice(&g.data2.to_le_bytes());
    b[6..8].copy_from_slice(&g.data3.to_le_bytes());
    b[8..16].copy_from_slice(&g.data4);
    b
}

/// Serialized REGFILTER2 (version 2) blob describing one video output pin,
/// in the format IFilterMapper2 writes to the `FilterData` value.
pub fn filter_data() -> Vec<u8> {
    let mut main = Vec::new();
    let dw = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    // REG_RF
    dw(&mut main, 2);
    dw(&mut main, MERIT_DO_NOT_USE);
    dw(&mut main, 1); // pins
    dw(&mut main, 0);
    // Offsets point past the fixed part, into the GUID store.
    // fixed size = REG_RF(16) + REG_RFP(24) + category offset(4) + REG_TYPE(16)
    let fixed = 16 + 24 + 4 + 16;
    let store = [PIN_CATEGORY_CAPTURE, MEDIATYPE_VIDEO, GUID::zeroed()];
    // REG_RFP
    main.extend_from_slice(b"0pi3");
    dw(&mut main, REG_PINFLAG_B_OUTPUT);
    dw(&mut main, 1); // instances
    dw(&mut main, 1); // media types
    dw(&mut main, 0); // mediums
    dw(&mut main, 1); // has category
    dw(&mut main, fixed as u32); // category offset
                                 // REG_TYPE
    main.extend_from_slice(b"0ty3");
    dw(&mut main, 0);
    dw(&mut main, (fixed + 16) as u32); // major
    dw(&mut main, (fixed + 32) as u32); // minor (MEDIASUBTYPE_NULL)
    assert_eq!(main.len(), fixed);
    for g in store {
        main.extend_from_slice(&guid_bytes(&g));
    }
    main
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn create_key(path: &str, view: REG_SAM_FLAGS) -> windows::core::Result<Key> {
    let mut k = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS | view,
            None,
            &mut k,
            None,
        )
        .ok()?;
    }
    Ok(Key(k))
}

fn set_sz(k: &Key, name: Option<&str>, value: &str) -> windows::core::Result<()> {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    let name_h = name.map(HSTRING::from);
    unsafe {
        RegSetValueExW(
            k.0,
            name_h
                .as_ref()
                .map(|h| PCWSTR(h.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            None,
            REG_SZ,
            Some(bytes),
        )
        .ok()
    }
}

fn set_bin(k: &Key, name: &str, value: &[u8]) -> windows::core::Result<()> {
    unsafe { RegSetValueExW(k.0, &HSTRING::from(name), None, REG_BINARY, Some(value)).ok() }
}

fn view_flag(wow32: bool) -> REG_SAM_FLAGS {
    if wow32 {
        KEY_WOW64_32KEY
    } else {
        KEY_WOW64_64KEY
    }
}

/// Register the filter DLL for the current user (no admin rights needed).
/// `wow32` selects the 32-bit registry view (for 32-bit host apps).
pub fn register_filter(dll: &Path, wow32: bool) -> windows::core::Result<()> {
    let view = view_flag(wow32);
    let clsid = FILTER_CLSID;
    let base = format!("Software\\Classes\\CLSID\\{clsid}");
    let k = create_key(&base, view)?;
    set_sz(&k, None, CAMERA_NAME)?;
    let k = create_key(&format!("{base}\\InprocServer32"), view)?;
    set_sz(&k, None, &dll.to_string_lossy())?;
    set_sz(&k, Some("ThreadingModel"), "Both")?;
    let cat = format!("{:?}", CLSID_VIDEO_INPUT_DEVICE_CATEGORY);
    let k = create_key(
        &format!("Software\\Classes\\CLSID\\{{{cat}}}\\Instance\\{clsid}"),
        view,
    )?;
    set_sz(&k, Some("CLSID"), clsid)?;
    set_sz(&k, Some("FriendlyName"), CAMERA_NAME)?;
    set_bin(&k, "FilterData", &filter_data())?;
    Ok(())
}

pub fn unregister_filter(wow32: bool) {
    let view = view_flag(wow32);
    let cat = format!("{:?}", CLSID_VIDEO_INPUT_DEVICE_CATEGORY);
    for path in [
        format!("Software\\Classes\\CLSID\\{{{cat}}}\\Instance\\{FILTER_CLSID}"),
        format!("Software\\Classes\\CLSID\\{FILTER_CLSID}"),
    ] {
        unsafe {
            let mut k = HKEY::default();
            let parent_path = path.rsplit_once('\\').unwrap().0.to_string();
            let leaf = path.rsplit_once('\\').unwrap().1.to_string();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(parent_path),
                None,
                KEY_ALL_ACCESS | view,
                &mut k,
            )
            .is_ok()
            {
                let _ = RegDeleteTreeW(k, &HSTRING::from(leaf));
                let _ = RegCloseKey(k);
            }
        }
    }
}

/// DLL path currently registered for the current user, if any.
pub fn registered_dll(wow32: bool) -> Option<PathBuf> {
    let view = view_flag(wow32);
    let path = HSTRING::from(format!(
        "Software\\Classes\\CLSID\\{FILTER_CLSID}\\InprocServer32"
    ));
    unsafe {
        let mut k = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, &path, None, KEY_READ | view, &mut k)
            .ok()
            .ok()?;
        let key = Key(k);
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        RegGetValueW(
            key.0,
            PCWSTR::null(),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
        .ok()
        .ok()?;
        let n = (len as usize / 2).saturating_sub(1);
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}
