//! Publishes an animated test pattern to the virtual camera for N seconds.
fn main() {
    #[cfg(windows)]
    {
        let secs: u64 = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let m = glyphcam_shm::win::SharedMapping::open_or_create().expect("mapping");
        let r = m.region();
        let (w, h) = (1280u32, 720u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let start = std::time::Instant::now();
        let mut n = 0u32;
        while start.elapsed().as_secs() < secs {
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    buf[i] = ((x + n * 4) % 256) as u8; // B
                    buf[i + 1] = ((y + n * 2) % 256) as u8; // G
                    buf[i + 2] = if x < w / 2 { 255 } else { 0 }; // R: left half red
                    buf[i + 3] = 255;
                }
            }
            r.write_frame(w, h, &buf);
            n += 1;
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
        println!("wrote {n} frames, reader idle {} ms", r.reader_idle_ms());
    }
}
