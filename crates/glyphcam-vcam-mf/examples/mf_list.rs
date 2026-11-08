//! Lists video capture devices the way Media Foundation apps (Discord,
//! Windows Camera, Teams) see them; optionally grabs one frame.
//!   cargo run -p glyphcam-vcam-mf --example mf_list -- [name-substring-to-grab]
#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use windows::core::*;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::*;
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n = 0u32;
        MFEnumDeviceSources(&attrs, &mut list, &mut n)?;
        let grab = std::env::args().nth(1).map(|s| s.to_lowercase());
        println!("{n} Media Foundation video devices:");
        for i in 0..n as usize {
            let Some(act) = (*list.add(i)).take() else {
                continue;
            };
            let mut name = PWSTR::null();
            let mut len = 0;
            act.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len)?;
            let name_s = name.to_string().unwrap_or_default();
            println!("  - {name_s}");
            if let Some(g) = &grab {
                if name_s.to_lowercase().contains(g) {
                    let src: IMFMediaSource = act.ActivateObject()?;
                    let reader = MFCreateSourceReaderFromMediaSource(&src, None)?;
                    let mut got = false;
                    for _ in 0..90 {
                        let mut flags = 0u32;
                        let mut sample: Option<IMFSample> = None;
                        reader.ReadSample(
                            MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                            0,
                            None,
                            Some(&mut flags),
                            None,
                            Some(&mut sample),
                        )?;
                        if let Some(s) = sample {
                            let mt = reader.GetCurrentMediaType(
                                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                            )?;
                            let size = mt.GetUINT64(&MF_MT_FRAME_SIZE)?;
                            let sub = mt.GetGUID(&MF_MT_SUBTYPE)?;
                            println!(
                                "    frame: {}x{} subtype {:?} bytes {}",
                                size >> 32,
                                size & 0xffff_ffff,
                                sub,
                                s.GetTotalLength()?
                            );
                            if sub == MFVideoFormat_NV12 {
                                let (w, h) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
                                let b = s.ConvertToContiguousBuffer()?;
                                let mut p: *mut u8 = std::ptr::null_mut();
                                b.Lock(&mut p, None, None)?;
                                let mut img = format!(
                                    "P5 {w} {h} 255
"
                                )
                                .into_bytes();
                                img.extend_from_slice(std::slice::from_raw_parts(p, w * h));
                                b.Unlock()?;
                                let out = std::env::temp_dir().join("glyphcam_mf_grab.pgm");
                                std::fs::write(&out, img).unwrap();
                                println!("    saved {}", out.display());
                            }
                            got = true;
                            break;
                        }
                    }
                    if !got {
                        println!("    no frame received");
                    }
                    let _ = src.Shutdown();
                }
            }
        }
        CoTaskMemFree(Some(list as *const _));
    }
    Ok(())
}
#[cfg(not(windows))]
fn main() {}
