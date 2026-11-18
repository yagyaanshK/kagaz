//! Sharing this computer: a Remote Desktop server that shows the screen
//! of the person signed in, who goes on using it, to a Kagaz elsewhere
//! (docs/remote-connection.md, step 2). It runs only while sharing is
//! turned on; until Kagaz starts it, nothing of it runs or listens.
//!
//!   kagaz-share [--port 3390] [--local] [--user NAME] [--pin 123456]
//!               [--watch] [--rate 30] [--capture dxgi|gdi]
//!               [--pattern [--size 1280x720] [--card-rate 5]] [--keep DIR]
//!
//! `--port` is the port it listens on (3390 unless said: Windows' own
//! Remote Desktop keeps 3389). `--local` listens on this computer only.
//! The viewer signs in as `--user` (the signed-in person's name unless
//! said) with `--pin` as the password (six digits made up and shown when
//! none is given). `--watch` shows the screen and takes no mouse or keys.
//! `--pattern` shows the test card instead of the screen, on any system,
//! and says each key and mouse event as `heard …`: for the tests.
//!
//! Its certificate is kept in `--keep` (Kagaz's folder for it unless
//! said), so that the viewer recognises this computer the next time.
//!
//! It says what it does on its output, one line each: `listening
//! <address>`, `fingerprint <sha-256 of its certificate>`, `pictures from
//! …`, `a viewer connected` / `the viewer left`, and every five seconds
//! the pictures' rate, size and time to encode. Never a picture.

// The same card as the test servers and the viewer's tests draw.
#[path = "../../kagaz-core/src/remote/pattern.rs"]
#[allow(dead_code)]
mod pattern;
mod picture;
#[cfg(windows)]
mod win;

use ironrdp_server::tokio_rustls::{rustls, TlsAcceptor};
use ironrdp_server::{
    Credentials, DesktopSize, DisplayUpdate, GfxServerFactory, KeyboardEvent, MouseEvent, PixelFormat,
    RdpServer, RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler,
};
use picture::{Latest, Source};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::num::{NonZeroU16, NonZeroUsize};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Where the mouse and keys go: this computer, nowhere (watching), or
/// the output (the test card).
#[derive(Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Input {
    Here(Latest),
    Nowhere,
    Said,
}

impl RdpServerInputHandler for Input {
    fn keyboard(&mut self, event: KeyboardEvent) {
        match self {
            #[cfg(windows)]
            Input::Here(_) => win::key(event),
            #[cfg(not(windows))]
            Input::Here(_) => {}
            Input::Nowhere => {}
            Input::Said => say(&format!("heard {event:?}")),
        }
    }
    fn mouse(&mut self, event: MouseEvent) {
        match self {
            #[cfg(windows)]
            Input::Here(latest) => {
                let size = *latest.size.lock().unwrap();
                win::mouse(event, size)
            }
            #[cfg(not(windows))]
            Input::Here(_) => {}
            Input::Nowhere => {}
            Input::Said => say(&format!("heard {event:?}")),
        }
    }
}

/// The screen as the session sees it: its size, and plain pictures for a
/// viewer that has not opened the graphics pipeline.
#[derive(Clone)]
struct Display {
    latest: Latest,
}

/// One viewer's stream of updates. The library starts a new one each
/// time the session is set up again (after a change of size too), so it
/// starts from the size the viewer was told, and says only real changes.
struct Updates {
    latest: Latest,
    size: (u16, u16),
    sent: u64,
    began: Instant,
    pointer_told: bool,
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for Updates {
    async fn next_update(&mut self) -> anyhow::Result<Option<DisplayUpdate>> {
        loop {
            if !self.pointer_told {
                // The viewer's own pointer is a plain arrow; this
                // computer's pointer is drawn into the pictures, so that
                // the person at it is seen pointing too.
                self.pointer_told = true;
                return Ok(Some(DisplayUpdate::DefaultPointer));
            }
            let size = *self.latest.size.lock().unwrap();
            if size != self.size && size.0 > 0 {
                self.size = size;
                self.sent = 0;
                return Ok(Some(DisplayUpdate::Resize(DesktopSize { width: size.0, height: size.1 })));
            }
            // Plain pictures only when the viewer has not taken the
            // graphics pipeline after a moment.
            let plain = !self.latest.pipeline.load(Ordering::Acquire) && self.began.elapsed() > Duration::from_secs(3);
            if plain {
                if let Some((number, frame)) = self.latest.get() {
                    if number != self.sent && (frame.width, frame.height) == self.size {
                        self.sent = number;
                        return Ok(Some(DisplayUpdate::Bitmap(ironrdp_server::BitmapUpdate {
                            x: 0,
                            y: 0,
                            width: NonZeroU16::new(frame.width).expect("a size is not zero"),
                            height: NonZeroU16::new(frame.height).expect("a size is not zero"),
                            format: PixelFormat::BgrX32,
                            data: frame.bgra.clone().into(),
                            stride: NonZeroUsize::new(frame.width as usize * 4).expect("a size is not zero"),
                        })));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for Display {
    async fn size(&mut self) -> DesktopSize {
        // The first picture says the size; wait a little for it.
        for _ in 0..50 {
            let (width, height) = *self.latest.size.lock().unwrap();
            if width > 0 {
                return DesktopSize { width, height };
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        DesktopSize { width: 1280, height: 720 }
    }

    async fn request_initial_size(&mut self, _viewer: DesktopSize) -> DesktopSize {
        // This computer's screen keeps its size: the person at it is
        // using it. The viewer fits it to its window.
        self.size().await
    }

    fn request_layout(&mut self, _layout: ironrdp_displaycontrol::pdu::DisplayControlMonitorLayout) {}

    async fn updates(&mut self) -> anyhow::Result<Box<dyn RdpServerDisplayUpdates>> {
        let told = self.size().await;
        Ok(Box::new(Updates {
            latest: self.latest.clone(),
            size: (told.width, told.height),
            sent: 0,
            began: Instant::now(),
            pointer_told: false,
        }))
    }
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Kagaz's folder for what sharing keeps.
fn keep_dir(args: &[String]) -> PathBuf {
    if let Some(dir) = value(args, "--keep") {
        return dir.into();
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("kagaz").join("share")
}

/// The certificate and its key, made the first time and kept after.
fn certificate(dir: &std::path::Path) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let (cert_at, key_at) = (dir.join("certificate.der"), dir.join("key.der"));
    if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_at), std::fs::read(&key_at)) {
        return Ok((cert, key));
    }
    std::fs::create_dir_all(dir)?;
    let made = rcgen::generate_simple_self_signed(vec!["kagaz-share".to_string()])?;
    let (cert, key) = (made.cert.der().to_vec(), made.signing_key.serialize_der());
    std::fs::write(&cert_at, &cert)?;
    std::fs::write(&key_at, &key)?;
    Ok((cert, key))
}

/// Six digits from the clock and the process: a PIN for one run.
fn made_pin() -> String {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
        ^ (std::process::id() as u128) << 64;
    let sum = Sha256::digest(seed.to_le_bytes());
    let n = u32::from_le_bytes([sum[0], sum[1], sum[2], sum[3]]) % 1_000_000;
    format!("{n:06}")
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        say("kagaz-share [--port 3390] [--local] [--user NAME] [--pin 123456] [--watch] [--rate 30] [--capture dxgi|gdi] [--pattern [--size 1280x720] [--card-rate 5]] [--keep DIR]");
        return Ok(());
    }
    #[cfg(windows)]
    win::dpi_aware();
    let port: u16 = value(&args, "--port").map(|p| p.parse()).transpose()?.unwrap_or(3390);
    let local = args.iter().any(|a| a == "--local");
    let rate: u32 = value(&args, "--rate").map(|p| p.parse()).transpose()?.unwrap_or(30).clamp(1, 60);
    let pattern = args.iter().any(|a| a == "--pattern");
    let watch = args.iter().any(|a| a == "--watch");
    let username = value(&args, "--user")
        .or_else(|| std::env::var("USERNAME").ok())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "kagaz".into());
    let given_pin = value(&args, "--pin");
    let pin = given_pin.clone().unwrap_or_else(made_pin);

    let (der, key) = certificate(&keep_dir(&args))?;
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

    let host = if local { std::net::Ipv4Addr::LOCALHOST } else { std::net::Ipv4Addr::UNSPECIFIED };
    let listener = std::net::TcpListener::bind((host, port))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;

    let latest = Latest::default();
    if pattern {
        let size = value(&args, "--size").unwrap_or_else(|| "1280x720".into());
        let (w, h) = size.split_once('x').ok_or_else(|| anyhow::anyhow!("a size is a width and a height, as 1280x720"))?;
        let (width, height): (u16, u16) = (w.parse()?, h.parse()?);
        anyhow::ensure!(width >= 16 && height >= 16, "a size of at least 16x16");
        let card_rate: u32 = value(&args, "--card-rate").map(|p| p.parse()).transpose()?.unwrap_or(5);
        picture::take(
            move || -> anyhow::Result<Box<dyn Source>> {
                Ok(Box::new(picture::Card { width, height, tick: 0, last: None, began: Instant::now(), rate: card_rate }))
            },
            latest.clone(),
            rate,
        );
    } else {
        #[cfg(windows)]
        {
            let how = value(&args, "--capture");
            picture::take(move || win::open(how.as_deref()), latest.clone(), rate);
        }
        #[cfg(not(windows))]
        anyhow::bail!("sharing this computer's screen comes to this system later; --pattern shows the test card");
    }
    let pipeline = picture::Pipeline::default();
    picture::pump(latest.clone(), pipeline.clone(), rate);

    let input = if watch {
        Input::Nowhere
    } else if pattern {
        Input::Said
    } else {
        Input::Here(latest.clone())
    };
    let fingerprint: String = Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let viewers = latest.viewers.clone();
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let mut server = RdpServer::builder()
            .with_addr(addr)
            .with_hybrid(acceptor, public_key)
            .with_input_handler(input)
            .with_display_handler(Display { latest: latest.clone() })
            .with_gfx_factory(Some(Box::new(pipeline) as Box<dyn GfxServerFactory>))
            .with_honor_client_desktop_size(false)
            .build();
        server.set_credentials(Some(Credentials { username: username.clone(), password: pin.clone(), domain: None }));
        say(&format!("listening {addr}"));
        say(&format!("fingerprint {fingerprint}"));
        say(&format!("sign in as {username}"));
        if given_pin.is_none() {
            say(&format!("PIN for this run: {pin}"));
        }
        if watch {
            say("watch only: the mouse and keys stay with this computer");
        }
        loop {
            let (stream, _) = listener.accept().await?;
            say("a viewer connected");
            // Pictures are taken from now until it leaves, so that the
            // screen's size is known when the session is set up.
            viewers.fetch_add(1, Ordering::AcqRel);
            let began = Instant::now();
            match server.run_connection(stream).await {
                Ok(()) => say(&format!("the viewer left after {} s", began.elapsed().as_secs())),
                Err(e) => say(&format!("the viewer left after {} s: {e:#}", began.elapsed().as_secs())),
            }
            viewers.fetch_sub(1, Ordering::AcqRel);
        }
    })
}
