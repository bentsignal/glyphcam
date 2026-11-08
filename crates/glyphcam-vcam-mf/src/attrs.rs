//! `IMFAttributes` delegation: every glyphcam MF object keeps an inner
//! attribute store (`self.attrs`) and forwards all 30 methods to it.

macro_rules! impl_attributes {
    ($t:ty) => {
        impl windows::Win32::Media::MediaFoundation::IMFAttributes_Impl for $t {
            fn GetItem(
                &self,
                k: *const windows::core::GUID,
                v: *mut windows::Win32::System::Com::StructuredStorage::PROPVARIANT,
            ) -> windows::core::Result<()> {
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).GetItem)(
                        windows::core::Interface::as_raw(&self.attrs),
                        k,
                        v,
                    )
                    .ok()
                }
            }
            fn GetItemType(
                &self,
                k: *const windows::core::GUID,
            ) -> windows::core::Result<windows::Win32::Media::MediaFoundation::MF_ATTRIBUTE_TYPE>
            {
                unsafe { self.attrs.GetItemType(k) }
            }
            fn CompareItem(
                &self,
                k: *const windows::core::GUID,
                v: *const windows::Win32::System::Com::StructuredStorage::PROPVARIANT,
            ) -> windows::core::Result<windows::core::BOOL> {
                unsafe { self.attrs.CompareItem(k, v) }
            }
            fn Compare(
                &self,
                theirs: windows::core::Ref<windows::Win32::Media::MediaFoundation::IMFAttributes>,
                m: windows::Win32::Media::MediaFoundation::MF_ATTRIBUTES_MATCH_TYPE,
            ) -> windows::core::Result<windows::core::BOOL> {
                let mut r = windows::core::BOOL(0);
                let p = theirs
                    .as_ref()
                    .map(|t| windows::core::Interface::as_raw(t))
                    .unwrap_or(std::ptr::null_mut());
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).Compare)(
                        windows::core::Interface::as_raw(&self.attrs),
                        p,
                        m,
                        &mut r,
                    )
                    .ok()?
                };
                Ok(r)
            }
            fn GetUINT32(&self, k: *const windows::core::GUID) -> windows::core::Result<u32> {
                unsafe { self.attrs.GetUINT32(k) }
            }
            fn GetUINT64(&self, k: *const windows::core::GUID) -> windows::core::Result<u64> {
                unsafe { self.attrs.GetUINT64(k) }
            }
            fn GetDouble(&self, k: *const windows::core::GUID) -> windows::core::Result<f64> {
                unsafe { self.attrs.GetDouble(k) }
            }
            fn GetGUID(
                &self,
                k: *const windows::core::GUID,
            ) -> windows::core::Result<windows::core::GUID> {
                unsafe { self.attrs.GetGUID(k) }
            }
            fn GetStringLength(&self, k: *const windows::core::GUID) -> windows::core::Result<u32> {
                unsafe { self.attrs.GetStringLength(k) }
            }
            fn GetString(
                &self,
                k: *const windows::core::GUID,
                s: windows::core::PWSTR,
                cch: u32,
                pcch: *mut u32,
            ) -> windows::core::Result<()> {
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).GetString)(
                        windows::core::Interface::as_raw(&self.attrs),
                        k,
                        s,
                        cch,
                        pcch,
                    )
                    .ok()
                }
            }
            fn GetAllocatedString(
                &self,
                k: *const windows::core::GUID,
                pp: *mut windows::core::PWSTR,
                pcch: *mut u32,
            ) -> windows::core::Result<()> {
                unsafe { self.attrs.GetAllocatedString(k, pp, pcch) }
            }
            fn GetBlobSize(&self, k: *const windows::core::GUID) -> windows::core::Result<u32> {
                unsafe { self.attrs.GetBlobSize(k) }
            }
            fn GetBlob(
                &self,
                k: *const windows::core::GUID,
                b: *mut u8,
                cb: u32,
                pcb: *mut u32,
            ) -> windows::core::Result<()> {
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).GetBlob)(
                        windows::core::Interface::as_raw(&self.attrs),
                        k,
                        b,
                        cb,
                        pcb,
                    )
                    .ok()
                }
            }
            fn GetAllocatedBlob(
                &self,
                k: *const windows::core::GUID,
                pp: *mut *mut u8,
                pcb: *mut u32,
            ) -> windows::core::Result<()> {
                unsafe { self.attrs.GetAllocatedBlob(k, pp, pcb) }
            }
            fn GetUnknown(
                &self,
                k: *const windows::core::GUID,
                riid: *const windows::core::GUID,
                ppv: *mut *mut core::ffi::c_void,
            ) -> windows::core::Result<()> {
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).GetUnknown)(
                        windows::core::Interface::as_raw(&self.attrs),
                        k,
                        riid,
                        ppv,
                    )
                    .ok()
                }
            }
            fn SetItem(
                &self,
                k: *const windows::core::GUID,
                v: *const windows::Win32::System::Com::StructuredStorage::PROPVARIANT,
            ) -> windows::core::Result<()> {
                unsafe { self.attrs.SetItem(k, v) }
            }
            fn DeleteItem(&self, k: *const windows::core::GUID) -> windows::core::Result<()> {
                unsafe { self.attrs.DeleteItem(k) }
            }
            fn DeleteAllItems(&self) -> windows::core::Result<()> {
                unsafe { self.attrs.DeleteAllItems() }
            }
            fn SetUINT32(
                &self,
                k: *const windows::core::GUID,
                v: u32,
            ) -> windows::core::Result<()> {
                unsafe { self.attrs.SetUINT32(k, v) }
            }
            fn SetUINT64(
                &self,
                k: *const windows::core::GUID,
                v: u64,
            ) -> windows::core::Result<()> {
                unsafe { self.attrs.SetUINT64(k, v) }
            }
            fn SetDouble(
                &self,
                k: *const windows::core::GUID,
                v: f64,
            ) -> windows::core::Result<()> {
                unsafe { self.attrs.SetDouble(k, v) }
            }
            fn SetGUID(
                &self,
                k: *const windows::core::GUID,
                v: *const windows::core::GUID,
            ) -> windows::core::Result<()> {
                unsafe { self.attrs.SetGUID(k, v) }
            }
            fn SetString(
                &self,
                k: *const windows::core::GUID,
                v: &windows::core::PCWSTR,
            ) -> windows::core::Result<()> {
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).SetString)(
                        windows::core::Interface::as_raw(&self.attrs),
                        k,
                        *v,
                    )
                    .ok()
                }
            }
            fn SetBlob(
                &self,
                k: *const windows::core::GUID,
                b: *const u8,
                cb: u32,
            ) -> windows::core::Result<()> {
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).SetBlob)(
                        windows::core::Interface::as_raw(&self.attrs),
                        k,
                        b,
                        cb,
                    )
                    .ok()
                }
            }
            fn SetUnknown(
                &self,
                k: *const windows::core::GUID,
                u: windows::core::Ref<windows::core::IUnknown>,
            ) -> windows::core::Result<()> {
                let p = u
                    .as_ref()
                    .map(|x| windows::core::Interface::as_raw(x))
                    .unwrap_or(std::ptr::null_mut());
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).SetUnknown)(
                        windows::core::Interface::as_raw(&self.attrs),
                        k,
                        p,
                    )
                    .ok()
                }
            }
            fn LockStore(&self) -> windows::core::Result<()> {
                unsafe { self.attrs.LockStore() }
            }
            fn UnlockStore(&self) -> windows::core::Result<()> {
                unsafe { self.attrs.UnlockStore() }
            }
            fn GetCount(&self) -> windows::core::Result<u32> {
                unsafe { self.attrs.GetCount() }
            }
            fn GetItemByIndex(
                &self,
                i: u32,
                k: *mut windows::core::GUID,
                v: *mut windows::Win32::System::Com::StructuredStorage::PROPVARIANT,
            ) -> windows::core::Result<()> {
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).GetItemByIndex)(
                        windows::core::Interface::as_raw(&self.attrs),
                        i,
                        k,
                        v,
                    )
                    .ok()
                }
            }
            fn CopyAllItems(
                &self,
                dest: windows::core::Ref<windows::Win32::Media::MediaFoundation::IMFAttributes>,
            ) -> windows::core::Result<()> {
                let p = dest
                    .as_ref()
                    .map(|t| windows::core::Interface::as_raw(t))
                    .unwrap_or(std::ptr::null_mut());
                unsafe {
                    (windows::core::Interface::vtable(&self.attrs).CopyAllItems)(
                        windows::core::Interface::as_raw(&self.attrs),
                        p,
                    )
                    .ok()
                }
            }
        }
    };
}

pub(crate) use impl_attributes;
