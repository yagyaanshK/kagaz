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
    /// Loopback addresses this server answers on (127.0.0.1 first). A
    /// webview allows only a few connections per host, so each camera tile
    /// can be given its own address.
    pub hosts: Vec<String>,
    session: Arc<Mutex<Session>>,
    cameras: Arc<Mutex<Vec<Camera>>>,
    /// The video engine's API port, once it runs, for the `/engine/` proxy.
    engine_port: Arc<Mutex<Option<u16>>>,
}

/// How many extra loopback addresses to try (127.0.0.2 ...).
const ALIASES: u8 = 24;

impl TsServer {
    /// Bind on 127.0.0.1 (a free port when `port` is 0) and, where the
    /// system allows, on 127.0.0.2 onwards too; serve in the background.
    pub fn start(
        session: Session,
        cameras: Vec<Camera>,
        port: u16,
    ) -> std::io::Result<Arc<TsServer>> {
        let first = TcpListener::bind(("127.0.0.1", port))?;
        let port = first.local_addr()?.port();
        let mut listeners = vec![first];
        let mut hosts = vec!["127.0.0.1".to_string()];
        for n in 2..(2 + ALIASES) {
            let host = format!("127.0.0.{n}");
            if let Ok(l) = TcpListener::bind((host.as_str(), port)) {
                listeners.push(l);
                hosts.push(host);
            } else {
                break;
            }
        }
        let server = Arc::new(TsServer {
            port,
            hosts,
            session: Arc::new(Mutex::new(session)),
            cameras: Arc::new(Mutex::new(cameras)),
            engine_port: Arc::new(Mutex::new(None)),
        });
        for listener in listeners {
            let s = server.clone();
            std::thread::Builder::new()
                .name("kagaz-ts-server".into())
                .spawn(move || {
                    for stream in listener.incoming().flatten() {
                        let s = s.clone();
                        std::thread::spawn(move || s.handle(stream));
                    }
                })?;
        }
        Ok(server)
    }

    /// Tell the server where the video engine listens, enabling `/engine/<stream>.ts`.
    pub fn set_engine_port(&self, port: u16) {
        if let Ok(mut p) = self.engine_port.lock() {
            *p = Some(port);
        }
    }

    /// The i-th camera's own host (wraps around when there are more cameras than addresses).
    pub fn host_for(&self, index: usize) -> &str {
        if self.hosts.len() > 1 {
            &self.hosts[1 + index % (self.hosts.len() - 1)]
        } else {
            &self.hosts[0]
        }
    }

    /// The engine's MPEG-TS output for `stream`, proxied on the i-th camera's own host.
    pub fn tile_url(&self, index: usize, stream: &str) -> String {
        format!(
            "http://{}:{}/engine/{stream}.ts",
            self.host_for(index),
            self.port
        )
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

    /// Pipe the engine's `/api/stream.ts?src=<name>` to this client.
    fn proxy_engine(&self, name: &str, mut client: TcpStream) {
        let Some(port) = self.engine_port.lock().ok().and_then(|p| *p) else {
            let _ = client.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        };
        let Ok(mut upstream) = TcpStream::connect(("127.0.0.1", port)) else {
            let _ = client.write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        };
        let safe: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if upstream
            .write_all(format!("GET /api/stream.ts?src={safe} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
            .is_err()
        {
            return;
        }
        // Forward the engine's response verbatim (its transfer encoding is
        // what tells the player this is a live stream), then the body.
        let mut upstream = upstream;
        let _ = std::io::copy(&mut upstream, &mut client);
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
        if let Some(name) = path_only
            .strip_prefix("/engine/")
            .and_then(|p| p.strip_suffix(".ts"))
        {
            let name = name.to_string();
            self.proxy_engine(&name, stream);
            return;
        }
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
