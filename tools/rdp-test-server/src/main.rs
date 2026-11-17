//! A Remote Desktop server that shows Kagaz's test card: no screen of
//! anyone's is involved. The tests of kagaz-core's `remote::rdp` start it
//! and talk to it over its standard input and output; it can also be run
//! by hand to try the viewer.
//!
//!   kagaz-rdp-test-server [--port N] [--size 1280x720] [--fit] [--no-files] [--user asha] [--password sesame]
//!
//! With `--fit` its screen takes the size the viewer asks for, at the
//! start and whenever the viewer's size changes, as a real computer's
//! does. With `--no-files` it takes no files over its clipboard.
//!
//! It says `listening <address>` and `fingerprint <sha-256 of its
//! certificate>`, then `heard <what a viewer did>` for every key and
//! mouse event, and `heard Text("…")` for text a viewer copied (it asks
//! for it at once, as a clipboard history does). On its input, `tick <n>`
//! moves the card to that tick, `size <width> <height>` changes the
//! screen's size, and `copy <text>` copies text "on the computer" (write
//! `\n` for a line's end).
//!
//! Files a viewer offers are said as `heard Offer`; they are taken when
//! `paste` comes on the input (`paste <ms>` waits that long before each
//! piece, as a slow link would). It keeps none of them: it says `heard
//! FileList([("name", size), …])`, then for each `heard File("name",
//! size, sha-256 of what arrived)` or `heard FileFailed("name")`, and
//! `heard PasteDone`.

// The same card as the VNC test server and the tests draw.
#[path = "../../../crates/kagaz-core/src/remote/pattern.rs"]
#[allow(dead_code)]
mod pattern;

use ironrdp_cliprdr::backend::{ClipboardMessage, CliprdrBackend, CliprdrBackendFactory};
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags, FileContentsFlags,
    FileContentsRequest, FileContentsResponse, FileDescriptor, FormatDataRequest, FormatDataResponse, LockDataId,
};
use ironrdp_server::tokio_rustls::{rustls, TlsAcceptor};
use ironrdp_server::{
    BitmapUpdate, CliprdrServerFactory, Credentials, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent,
    PixelFormat, RdpServer, RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler, ServerEvent,
    ServerEventSender,
};
use tokio::sync::mpsc::UnboundedSender;
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

/// Files being taken from the viewer, one after another, a piece at a
/// time.
#[derive(Default)]
struct Taking {
    files: Vec<(String, u64)>,
    /// The one being taken, and the number the viewer's offer is kept
    /// under while it is.
    index: usize,
    kept: Option<u32>,
    /// Its size is known; so much of it is here.
    sized: bool,
    size: u64,
    position: u64,
    sum: Sha256,
    stream: u32,
    /// Wait this long before asking for each piece.
    wait: Duration,
}

impl std::fmt::Debug for Taking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Taking({} of {})", self.index, self.files.len())
    }
}

/// As much as a computer asks for at a time.
const PIECE: u64 = 65536;

/// The computer's clipboard: the text "copied" there, the files a viewer
/// offers, and the way to the viewer once one is connected.
#[derive(Clone, Debug, Default)]
struct Clipboard {
    text: Arc<Mutex<Option<String>>>,
    to_viewer: Arc<Mutex<Option<UnboundedSender<ServerEvent>>>>,
    /// It takes no files.
    no_files: bool,
    /// The viewer offers files, under this format.
    offered: Arc<Mutex<Option<ClipboardFormatId>>>,
    taking: Arc<Mutex<Taking>>,
}

impl Clipboard {
    fn send(&self, message: ClipboardMessage) {
        if let Some(sender) = self.to_viewer.lock().unwrap().as_ref() {
            let _ = sender.send(ServerEvent::Clipboard(message));
        }
    }

    fn formats(&self) -> Vec<ClipboardFormat> {
        match *self.text.lock().unwrap() {
            Some(_) => vec![ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)],
            None => Vec::new(),
        }
    }

    /// Text is copied on the computer: the viewer is told there is text.
    fn copy(&self, text: String) {
        *self.text.lock().unwrap() = Some(text);
        *self.offered.lock().unwrap() = None;
        self.send(ClipboardMessage::SendInitiateCopy(self.formats()));
    }

    /// What the viewer offers is pasted on the computer: its list of
    /// files is asked for, then each file.
    fn paste(&self, wait: Duration) {
        let offered = *self.offered.lock().unwrap();
        match offered {
            Some(format) => {
                self.taking.lock().unwrap().wait = wait;
                self.send(ClipboardMessage::SendInitiatePaste(format));
            }
            None => say("heard NothingToPaste"),
        }
    }

    /// Ask for the next piece of the file being taken, or say all are in.
    fn ask(&self, taking: &mut Taking) {
        let Some((_, size)) = taking.files.get(taking.index) else {
            say("heard PasteDone");
            return;
        };
        std::thread::sleep(taking.wait);
        taking.stream += 1;
        let (flags, position, requested_size) = if taking.sized {
            (FileContentsFlags::RANGE, taking.position, PIECE.min(size - taking.position) as u32)
        } else {
            (FileContentsFlags::SIZE, 0, 8)
        };
        self.send(ClipboardMessage::SendFileContentsRequest(FileContentsRequest {
            stream_id: taking.stream,
            index: taking.index as i32,
            flags,
            position,
            requested_size,
            data_id: taking.kept,
        }));
    }

    /// The file being taken is over, well or not: on to the next.
    fn file_over(&self, taking: &mut Taking, well: bool) {
        let (name, size) = taking.files[taking.index].clone();
        if well {
            let sum: String = std::mem::take(&mut taking.sum).finalize().iter().map(|b| format!("{b:02x}")).collect();
            say(&format!("heard File({name:?}, {size}, {sum})"));
        } else {
            taking.sum = Sha256::new();
            say(&format!("heard FileFailed({name:?})"));
        }
        taking.index += 1;
        taking.sized = false;
        taking.position = 0;
        self.ask(taking);
    }
}

impl ServerEventSender for Clipboard {
    fn set_sender(&mut self, sender: UnboundedSender<ServerEvent>) {
        *self.to_viewer.lock().unwrap() = Some(sender);
    }
}

impl CliprdrBackendFactory for Clipboard {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        Box::new(self.clone())
    }
}

impl CliprdrServerFactory for Clipboard {}

ironrdp_core::impl_as_any!(Clipboard);

impl CliprdrBackend for Clipboard {
    fn temporary_directory(&self) -> &str {
        ""
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        if self.no_files {
            return ClipboardGeneralCapabilityFlags::empty();
        }
        ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS
            | ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED
            | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA
    }

    fn on_ready(&mut self) {}

    fn on_request_format_list(&mut self) {
        self.send(ClipboardMessage::SendInitiateCopy(self.formats()));
    }

    fn on_process_negotiated_capabilities(&mut self, _capabilities: ClipboardGeneralCapabilityFlags) {}

    // The viewer has text: ask for it at once. It has files: they wait
    // until they are pasted.
    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        let files = available_formats
            .iter()
            .find(|f| f.name().is_some_and(|n| n.value() == ClipboardFormatName::FILE_LIST.value()))
            .map(|f| f.id());
        *self.offered.lock().unwrap() = files;
        if files.is_some() {
            say("heard Offer");
        }
        if available_formats.iter().any(|f| f.id() == ClipboardFormatId::CF_UNICODETEXT) {
            self.send(ClipboardMessage::SendInitiatePaste(ClipboardFormatId::CF_UNICODETEXT));
        }
    }

    // The list of what was pasted: take the first file.
    fn on_remote_file_list(&mut self, files: &[FileDescriptor], clip_data_id: Option<u32>) {
        let files: Vec<(String, u64)> = files.iter().map(|f| (f.name.clone(), f.file_size.unwrap_or(0))).collect();
        say(&format!("heard FileList({files:?})"));
        let mut taking = self.taking.lock().unwrap();
        *taking = Taking {
            files,
            kept: clip_data_id,
            stream: taking.stream,
            wait: taking.wait,
            ..Taking::default()
        };
        self.ask(&mut taking);
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        let text = self.text.lock().unwrap().clone();
        self.send(ClipboardMessage::SendFormatData(match text {
            Some(text) if request.format == ClipboardFormatId::CF_UNICODETEXT => {
                FormatDataResponse::new_unicode_string(&text)
            }
            _ => FormatDataResponse::new_error(),
        }));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if let Ok(text) = response.to_unicode_string() {
            say(&format!("heard Text({:?})", text.trim_end_matches('\0')));
        }
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        let mut taking = self.taking.lock().unwrap();
        if taking.index >= taking.files.len() {
            return;
        }
        if response.is_error() {
            return self.file_over(&mut taking, false);
        }
        if !taking.sized {
            let Ok(size) = response.data_as_size() else {
                return self.file_over(&mut taking, false);
            };
            taking.sized = true;
            taking.size = size;
            // What the list said and what the file says must agree.
            if size != taking.files[taking.index].1 {
                return self.file_over(&mut taking, false);
            }
            if size == 0 {
                return self.file_over(&mut taking, true);
            }
            return self.ask(&mut taking);
        }
        let piece = response.data();
        if piece.is_empty() {
            return self.file_over(&mut taking, false);
        }
        taking.sum.update(piece);
        taking.position += piece.len() as u64;
        if taking.position >= taking.size {
            let whole = taking.position == taking.size;
            return self.file_over(&mut taking, whole);
        }
        self.ask(&mut taking);
    }
    fn on_lock(&mut self, _data_id: LockDataId) {}
    fn on_unlock(&mut self, _data_id: LockDataId) {}
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

    let clipboard = Clipboard {
        no_files: args.iter().any(|a| a == "--no-files"),
        ..Clipboard::default()
    };
    // What the tests (or a person) ask for; the end of the input ends it.
    {
        let shared = shared.clone();
        let clipboard = clipboard.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if let Some(text) = line.strip_prefix("copy ") {
                    // As Windows keeps it: lines end in CR LF.
                    clipboard.copy(text.replace("\\n", "\r\n"));
                    continue;
                }
                let words: Vec<&str> = line.split_whitespace().collect();
                match words.as_slice() {
                    ["tick", n] => {
                        if let Ok(n) = n.parse() {
                            shared.tick.store(n, Ordering::Release);
                        }
                    }
                    ["paste"] => clipboard.paste(Duration::ZERO),
                    ["paste", ms] => clipboard.paste(Duration::from_millis(ms.parse().unwrap_or(0))),
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
            .with_cliprdr_factory(Some(Box::new(clipboard)))
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
