//! Tiny localhost MJPEG server so any app can pull glyphcam's output:
//! OBS "Browser Source" (then OBS's own Virtual Camera), a browser tab, VLC…
//! This is the cross-platform fallback, and the macOS route until a signed
//! Camera Extension exists.

use glyphcam_core::Frame;
use parking_lot::{Condvar, Mutex};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct Inner {
    jpeg: Mutex<(u64, Option<Arc<Vec<u8>>>)>,
    cv: Condvar,
    clients: AtomicUsize,
    stop: AtomicBool,
}

pub struct StreamServer {
    inner: Arc<Inner>,
    pub port: u16,
    /// Listening on all interfaces (reachable from the LAN).
    pub lan: bool,
}

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>glyphcam</title>
<style>html,body{margin:0;height:100%;background:#000;overflow:hidden}
img{width:100vw;height:100vh;object-fit:contain;display:block}</style></head>
<body><img src="/stream.mjpg" alt=""></body></html>"#;

impl StreamServer {
    pub fn start(port: u16, lan: bool) -> std::io::Result<Self> {
        let host = if lan { "0.0.0.0" } else { "127.0.0.1" };
        let listener = TcpListener::bind((host, port))?;
        listener.set_nonblocking(true)?;
        let inner = Arc::new(Inner {
            jpeg: Mutex::new((0, None)),
            cv: Condvar::new(),
            clients: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let inn = inner.clone();
        std::thread::Builder::new()
            .name("glyphcam-http".into())
            .spawn(move || {
                while !inn.stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((sock, _)) => {
                            let inn = inn.clone();
                            let _ = std::thread::Builder::new()
                                .name("glyphcam-http-client".into())
                                .spawn(move || handle(sock, inn));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(100))
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(100)),
                    }
                }
            })?;
        Ok(StreamServer { inner, port, lan })
    }

    pub fn clients(&self) -> usize {
        self.inner.clients.load(Ordering::Relaxed)
    }

    /// Encode and publish a frame (only when someone is watching).
    pub fn publish(&self, f: &Frame) {
        if self.clients() == 0 {
            return;
        }
        let mut out = Vec::with_capacity(256 * 1024);
        let enc = jpeg_encoder::Encoder::new(&mut out, 85);
        if enc
            .encode(
                &f.data,
                f.width as u16,
                f.height as u16,
                jpeg_encoder::ColorType::Bgra,
            )
            .is_ok()
        {
            let mut j = self.inner.jpeg.lock();
            j.0 += 1;
            j.1 = Some(Arc::new(out));
            drop(j);
            self.inner.cv.notify_all();
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// URLs other devices can use (empty unless LAN access is on).
    pub fn lan_urls(&self) -> Vec<String> {
        if !self.lan {
            return Vec::new();
        }
        lan_addresses()
            .into_iter()
            .map(|(ip, label)| format!("http://{ip}:{}/  ({label})", self.port))
            .collect()
    }
}

impl Drop for StreamServer {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        self.inner.cv.notify_all();
    }
}

struct ClientGuard<'a>(&'a AtomicUsize);
impl Drop for ClientGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn handle(mut sock: TcpStream, inner: Arc<Inner>) {
    let _ = sock.set_nonblocking(false);
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = sock.set_nodelay(true);
    let mut reader = BufReader::new(match sock.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    // drain headers
    loop {
        let mut h = String::new();
        match reader.read_line(&mut h) {
            Ok(0) | Err(_) => break,
            Ok(_) if h == "\r\n" || h == "\n" => break,
            _ => {}
        }
    }
    let path = path.split('?').next().unwrap_or("/");
    match path {
        "/" | "/index.html" => {
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n{}",
                PAGE.len(),
                PAGE
            );
        }
        "/stream.mjpg" | "/stream" => {
            inner.clients.fetch_add(1, Ordering::Relaxed);
            let _g = ClientGuard(&inner.clients);
            if write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=glyphcamframe\r\nCache-Control: no-cache\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n"
            )
            .is_err()
            {
                return;
            }
            let mut last = 0u64;
            while !inner.stop.load(Ordering::Relaxed) {
                let jpeg = {
                    let mut j = inner.jpeg.lock();
                    if j.0 == last {
                        inner.cv.wait_for(&mut j, Duration::from_millis(1000));
                    }
                    if j.0 == last {
                        continue;
                    }
                    last = j.0;
                    j.1.clone()
                };
                let Some(jpeg) = jpeg else { continue };
                let head = format!(
                    "--glyphcamframe\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                    jpeg.len()
                );
                if sock.write_all(head.as_bytes()).is_err()
                    || sock.write_all(&jpeg).is_err()
                    || sock.write_all(b"\r\n").is_err()
                {
                    return;
                }
            }
        }
        "/frame.jpg" => {
            inner.clients.fetch_add(1, Ordering::Relaxed);
            let jpeg = {
                let _g = ClientGuard(&inner.clients);
                let mut j = inner.jpeg.lock();
                let start = j.0;
                if j.1.is_none() || j.0 == start {
                    inner.cv.wait_for(&mut j, Duration::from_millis(1500));
                }
                j.1.clone()
            };
            match jpeg {
                Some(j) => {
                    let _ = write!(
                        sock,
                        "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
                        j.len()
                    );
                    let _ = sock.write_all(&j);
                }
                None => {
                    let _ = write!(sock, "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            }
        }
        _ => {
            let _ = write!(
                sock,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    }
}

/// This machine's IPv4 addresses that other devices could reach, best first
/// (the default-route address, then other LAN / Tailscale ones), with a label.
/// Virtual adapters (WSL, Hyper-V, Docker, VMs) are skipped.
pub fn lan_addresses() -> Vec<(std::net::Ipv4Addr, &'static str)> {
    use std::net::{IpAddr, Ipv4Addr, UdpSocket};
    // Connecting a UDP socket sends nothing; it just picks the outgoing interface.
    let primary = UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|s| s.connect(("192.0.2.1", 9)).map(|_| s))
        .and_then(|s| s.local_addr())
        .ok()
        .and_then(|a| match a.ip() {
            IpAddr::V4(v) if !v.is_unspecified() && !v.is_loopback() => Some(v),
            _ => None,
        });
    let tailscale = |v: &Ipv4Addr| v.octets()[0] == 100 && (64..128).contains(&v.octets()[1]);
    let usable = |v: &Ipv4Addr| {
        !v.is_loopback()
            && !v.is_link_local()
            && !v.is_unspecified()
            && (v.is_private() || tailscale(v))
    };
    let virtual_if = |name: &str| {
        let n = name.to_ascii_lowercase();
        [
            "vethernet",
            "wsl",
            "hyper-v",
            "docker",
            "virtualbox",
            "vmware",
            "vmnet",
            "vboxnet",
            "bridge",
            "utun",
        ]
        .iter()
        .any(|k| n.contains(k))
    };
    let mut out: Vec<(Ipv4Addr, &'static str)> = Vec::new();
    let ifs = if_addrs::get_if_addrs().unwrap_or_default();
    let label = |v: &Ipv4Addr, name: &str| -> &'static str {
        if tailscale(v) || name.to_ascii_lowercase().contains("tailscale") {
            "Tailscale"
        } else {
            "local network"
        }
    };
    if let Some(p) = primary {
        let name = ifs
            .iter()
            .find(|i| i.ip() == IpAddr::V4(p))
            .map(|i| i.name.clone())
            .unwrap_or_default();
        out.push((p, label(&p, &name)));
    }
    for i in &ifs {
        if let IpAddr::V4(v) = i.ip() {
            let ts = tailscale(&v) || i.name.to_ascii_lowercase().contains("tailscale");
            if usable(&v) && (ts || !virtual_if(&i.name)) && !out.iter().any(|(a, _)| *a == v) {
                out.push((v, label(&v, &i.name)));
            }
        }
    }
    out
}
