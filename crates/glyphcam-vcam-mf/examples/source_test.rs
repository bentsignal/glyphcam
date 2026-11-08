//! Drives the media source in-process with an IMFSourceReader (no admin,
//! no Frame Server) and saves a frame: verifies media types, stream start,
//! sample delivery and pacing.
#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::*;
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let act = glyphcam_mfcam::create_activator()?;
        let src: IMFMediaSource = act.ActivateObject()?;
        let pd = src.CreatePresentationDescriptor()?;
        let mut sel = windows::core::BOOL(0);
        let mut sd: Option<IMFStreamDescriptor> = None;
        pd.GetStreamDescriptorByIndex(0, &mut sel, &mut sd)?;
        let h = sd.unwrap().GetMediaTypeHandler()?;
        let n = h.GetMediaTypeCount()?;
        println!("{n} media types");
        let reader = MFCreateSourceReaderFromMediaSource(&src, None).map_err(|e| {
            eprintln!("create reader: {e}");
            e
        })?;
        for want_rgb in [false, true] {
            if want_rgb {
                let t = MFCreateMediaType()?;
                t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
                t.SetUINT64(&MF_MT_FRAME_SIZE, (640u64 << 32) | 360)?;
                reader.SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    &t,
                )?;
            }
            let t0 = std::time::Instant::now();
            let mut frames = 0;
            let mut last: Option<IMFSample> = None;
            while frames < 30 {
                let mut flags = 0u32;
                let mut s: Option<IMFSample> = None;
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        None,
                        Some(&mut s),
                    )
                    .map_err(|e| {
                        eprintln!("read sample #{frames}: {e} flags={flags:#x}");
                        e
                    })?;
                if s.is_some() {
                    frames += 1;
                    last = s;
                }
            }
            let mt = reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)?;
            let size = mt.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (w, hh) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
            let s = last.unwrap();
            let buf = s.ConvertToContiguousBuffer()?;
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buf.Lock(&mut p, None, Some(&mut len))?;
            let data = std::slice::from_raw_parts(p, len as usize).to_vec();
            buf.Unlock()?;
            println!(
                "{} {}x{}: {} frames in {:.2}s ({:.1} fps), {} bytes",
                if want_rgb { "RGB32" } else { "NV12" },
                w,
                hh,
                frames,
                t0.elapsed().as_secs_f32(),
                frames as f32 / t0.elapsed().as_secs_f32(),
                len
            );
            // save a preview (luma for NV12, RGB for RGB32)
            let out = std::env::temp_dir().join(if want_rgb {
                "glyphcam_mf_rgb.pgm"
            } else {
                "glyphcam_mf_nv12.pgm"
            });
            let mut img = format!("P5 {w} {hh} 255\n").into_bytes();
            if want_rgb {
                for px in data.chunks_exact(4) {
                    img.push(((px[0] as u32 + px[1] as u32 + px[2] as u32) / 3) as u8);
                }
            } else {
                img.extend_from_slice(&data[..w * hh]);
            }
            std::fs::write(&out, img).unwrap();
            println!("  saved {}", out.display());
        }
        src.Shutdown()?;
    }
    Ok(())
}
#[cfg(not(windows))]
fn main() {}
