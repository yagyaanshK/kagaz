//! A small local HTTP server that exposes each Tapo camera's live view as
//! `http://127.0.0.1:<port>/tapo/<device-id>.ts`: a plain MPEG-TS stream
//! that go2rtc (or any player) can read. Every client gets its own relay
//! session; the relay fans the camera's live stream out, so several
//! viewers of one camera cost the camera nothing extra.

use super::cloud::{Camera, Session};
use super::relay::{request_relay, stream_preview};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

pub struct TsServer {
    pub port: u16,
    session: Arc<Mutex<Session>>,
    cameras: Arc<Mutex<Vec<Camera>>>,
}

impl TsServer {
    /// Bind on 127.0.0.1 (a free port when `port` is 0) and serve in the background.
    pub fn start(
        session: Session,
        cameras: Vec<Camera>,
        port: u16,
    ) -> std::io::Result<Arc<TsServer>> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let server = Arc::new(TsServer {
            port,
            session: Arc::new(Mutex::new(session)),
            cameras: Arc::new(Mutex::new(cameras)),
        });
        let s = server.clone();
        std::thread::Builder::new()
            .name("kagaz-ts-server".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let s = s.clone();
                    std::thread::spawn(move || s.handle(stream));
                }
            })?;
        Ok(server)
    }

    /// The HD stream URL for a camera.
    pub fn url_for(&self, device_id: &str) -> String {
        format!("http://127.0.0.1:{}/tapo/{device_id}.ts", self.port)
    }

    /// The low-resolution (VGA) stream URL, for small tiles.
    pub fn vga_url_for(&self, device_id: &str) -> String {
        format!("http://127.0.0.1:{}/tapo/{device_id}.ts?res=vga", self.port)
    }

    pub fn cameras(&self) -> Vec<Camera> {
        self.cameras.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn set_cameras(&self, cameras: Vec<Camera>) {
        if let Ok(mut c) = self.cameras.lock() {
            *c = cameras;
        }
    }

    fn handle(&self, mut stream: TcpStream) {
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        });
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        // Drain the headers.
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) if line.trim().is_empty() => break,
                Ok(_) => {}
                Err(_) => return,
            }
        }
        let path = request_line.split_whitespace().nth(1).unwrap_or("/");
        let (path_only, query) = path.split_once('?').unwrap_or((path, ""));
        let resolution = if query
            .split('&')
            .any(|kv| kv.eq_ignore_ascii_case("res=vga"))
        {
            "VGA"
        } else {
            "HD"
        };
        let device_id = path_only
            .strip_prefix("/tapo/")
            .and_then(|p| p.strip_suffix(".ts"))
            .map(str::to_string);
        let Some(device_id) = device_id else {
            if path == "/" || path == "/index.json" {
                let cams = self.cameras();
                let body = serde_json::to_string(&cams).unwrap_or_else(|_| "[]".into());
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            return;
        };
        let cam = self
            .cameras()
            .into_iter()
            .find(|c| c.device_id == device_id);
        let Some(cam) = cam else {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        };
        let (session, terminal) = match self.session.lock() {
            Ok(s) => (s.clone(), s.terminal_uuid.clone()),
            Err(_) => return,
        };
        let track = format!(
            "preview-{}-{}",
            cam.device_id,
            uuid::Uuid::new_v4().simple()
        );
        let relay = match request_relay(
            &session,
            &cam.device_id,
            &cam.app_server,
            &track,
            resolution,
        ) {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{msg}",
                    msg.len()
                );
                return;
            }
        };
        if stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")
            .is_err()
        {
            return;
        }
        let _ = stream_preview(&relay, &terminal, &track, resolution, &mut |chunk| {
            stream.write_all(chunk).is_ok()
        });
    }
}
