//! Tries to open the machine-wide (Windows 11 camera) mapping and reports why it fails.
fn main() {
    #[cfg(windows)]
    for i in 0..20 {
        match glyphcam_shm::win::SharedMapping::open_global() {
            Ok(m) => {
                let r = m.region();
                println!(
                    "open ok after {i} tries; valid={} reader_idle_ms={} writer={:?}",
                    r.is_valid(),
                    r.reader_idle_ms(),
                    r.writer_state()
                );
                return;
            }
            Err(e) => {
                if i % 5 == 0 {
                    println!("open failed: {e} ({:?})", e.code());
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
    }
}
