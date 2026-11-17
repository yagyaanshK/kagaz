//! A Remote Desktop server that shows Kagaz's test card: no screen of
//! anyone's is involved. The tests of kagaz-core's `remote::rdp` start it
//! and talk to it over its standard input and output; it can also be run
//! by hand to try the viewer.
//!
//!   kagaz-rdp-test-server [--port N] [--size 1280x720] [--fit] [--user asha] [--password sesame]
//!
//! With `--fit` its screen takes the size the viewer asks for, at the
//! start and whenever the viewer's size changes, as a real computer's
//! does.
//!
//! It says `listening <address>` and `fingerprint <sha-256 of its
//! certificate>`, then `heard <what a viewer did>` for every key and
//! mouse event. On its input, `tick <n>` moves the card to that tick and
//! `size <width> <height>` changes the screen's size.

// The same card as the VNC test server and the tests draw.
#[path = "../../../crates/kagaz-core/src/remote/pattern.rs"]
#[allow(dead_code)]
mod pattern;

use ironrdp_server::tokio_rustls::{rustls, TlsAcceptor};
use ironrdp_server::{
    BitmapUpdate, Credentials, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent, PixelFormat, RdpServer,
    RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler,
};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Write};
use std::num::{NonZeroU16, NonZeroUsize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

#[derive(Clone)]
struct Shared {
    tick: Arc<AtomicU32>,
    size: Arc<Mutex<(u16, u16)>>,
    /// Take the size the viewer asks for.
    fit: bool,
}

impl RdpServerInputHandler for Shared {
    fn keyboard(&mut self, event: KeyboardEvent) {
        say(&format!("heard {event:?}"));
    }
    fn mouse(&mut self, event: MouseEvent) {
        say(&format!("heard {event:?}"));
    }
}

/// The card, whole, each time the tick or the size changes.
struct Updates {
    shared: Shared,
    sent: Option<(u32, (u16, u16))>,
    size: (u16, u16),
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for Updates {
    async fn next_update(&mut self) -> anyhow::Result<Option<DisplayUpdate>> {
        loop {
            let tick = self.shared.tick.load(Ordering::Acquire);
            let size = *self.shared.size.lock().unwrap();
            if size != self.size {
                self.size = size;
                self.sent = None;
                return Ok(Some(DisplayUpdate::Resize(DesktopSize {
                    width: size.0,
                    height: size.1,
                })));
            }
            if self.sent != Some((tick, size)) {
                self.sent = Some((tick, size));
                let (width, height) = size;
                return Ok(Some(DisplayUpdate::Bitmap(BitmapUpdate {
                    x: 0,
                    y: 0,
                    width: NonZeroU16::new(width).expect("a size is not zero"),
                    height: NonZeroU16::new(height).expect("a size is not zero"),
                    format: PixelFormat::RgbA32,
                    data: pattern::card(width, height, tick).into(),
                    stride: NonZeroUsize::new(width as usize * 4).expect("a size is not zero"),
                })));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for Shared {
    async fn size(&mut self) -> DesktopSize {
        let (width, height) = *self.size.lock().unwrap();
        DesktopSize { width, height }
    }

    async fn request_initial_size(&mut self, viewer: DesktopSize) -> DesktopSize {
        if self.fit {
            *self.size.lock().unwrap() = (viewer.width, viewer.height);
        }
        self.size().await
    }

    fn request_layout(&mut self, layout: ironrdp_displaycontrol::pdu::DisplayControlMonitorLayout) {
        let Some(monitor) = layout.monitors().iter().find(|m| m.is_primary()).or(layout.monitors().first()) else {
            return;
        };
        let (width, height) = monitor.dimensions();
        if self.fit {
            *self.size.lock().unwrap() = (width as u16, height as u16);
        }
    }

    async fn updates(&mut self) -> anyhow::Result<Box<dyn RdpServerDisplayUpdates>> {
        let size = *self.size.lock().unwrap();
        Ok(Box::new(Updates {
            shared: self.clone(),
            sent: None,
            size,
        }))
    }
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = value(&args, "--port").map(|p| p.parse()).transpose()?.unwrap_or(0);
    let size = value(&args, "--size").unwrap_or_else(|| "1280x720".into());
    let (width, height) = size.split_once('x').ok_or_else(|| anyhow::anyhow!("a size is a width and a height, as 1280x720"))?;
    let (width, height): (u16, u16) = (width.parse()?, height.parse()?);
    anyhow::ensure!(width >= 16 && height >= 16, "a size of at least 16x16");
    let username = value(&args, "--user").unwrap_or_else(|| "asha".into());
    let password = value(&args, "--password").unwrap_or_else(|| "sesame".into());

    // A certificate of its own, made for this run.
    let made = rcgen::generate_simple_self_signed(vec!["kagaz-test-computer".to_string()])?;
    let der = made.cert.der().to_vec();
    let key = made.signing_key.serialize_der();
    let public_key = {
        use x509_cert::der::Decode as _;
        let cert = x509_cert::Certificate::from_der(&der)?;
        cert.tbs_certificate
            .subject_public_key_info
            .subject_public_key
            .as_bytes()
            .ok_or_else(|| anyhow::anyhow!("the certificate has no key"))?
            .to_vec()
    };
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(der.clone())],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key.into()),
        )?;
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let shared = Shared {
        tick: Arc::new(AtomicU32::new(0)),
        size: Arc::new(Mutex::new((width, height))),
        fit: args.iter().any(|a| a == "--fit"),
    };

    // What the tests (or a person) ask for; the end of the input ends it.
    {
        let shared = shared.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                let words: Vec<&str> = line.split_whitespace().collect();
                match words.as_slice() {
                    ["tick", n] => {
                        if let Ok(n) = n.parse() {
                            shared.tick.store(n, Ordering::Release);
                        }
                    }
                    ["size", w, h] => {
                        if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                            *shared.size.lock().unwrap() = (w, h);
                        }
                    }
                    _ => {}
                }
            }
            std::process::exit(0);
        });
    }

    let fit = shared.fit;
    let fingerprint: String = Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let mut server = RdpServer::builder()
            .with_addr(addr)
            .with_hybrid(acceptor, public_key)
            .with_input_handler(shared.clone())
            .with_display_handler(shared)
            .with_cliprdr_factory(None)
            .with_sound_factory(None)
            .with_honor_client_desktop_size(fit)
            .build();
        server.set_credentials(Some(Credentials {
            username,
            password,
            domain: None,
        }));
        say(&format!("listening {addr}"));
        say(&format!("fingerprint {fingerprint}"));
        loop {
            let (stream, _) = listener.accept().await?;
            if let Err(e) = server.run_connection(stream).await {
                say(&format!("ended {e:#}"));
            }
        }
    })
}
