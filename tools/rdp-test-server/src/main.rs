//! A Remote Desktop server that shows Kagaz's test card: no screen of
//! anyone's is involved. The tests of kagaz-core's `remote::rdp` start it
//! and talk to it over its standard input and output; it can also be run
//! by hand to try the viewer.
//!
//!   kagaz-rdp-test-server [--port N] [--size 1280x720] [--fit] [--h264] [--no-files] [--user asha] [--password sesame]
//!
//! With `--fit` its screen takes the size the viewer asks for, at the
//! start and whenever the viewer's size changes, as a real computer's
//! does. With `--no-files` it takes no files over its clipboard. With
//! `--h264` the card goes as H.264 through the newer picture's channel
//! (the graphics pipeline) and never as plain bitmaps; it says `heard
//! PipelineReady(<what the two agreed on>)`, `heard Frame(<tick>,
//! <bytes>)` for each picture sent and `heard FrameAck` for each the
//! viewer says it has drawn. With `rate <n>` on its input the card moves
//! by itself, n ticks a second (0 stops it).
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
use ironrdp_egfx::pdu::{Avc420Region, CapabilitiesAdvertisePdu, CapabilitySet};
use ironrdp_egfx::server::{GraphicsPipelineHandler, GraphicsPipelineServer};
use ironrdp_server::tokio_rustls::{rustls, TlsAcceptor};
use ironrdp_server::{
    BitmapUpdate, CliprdrServerFactory, Credentials, DesktopSize, DisplayUpdate, EgfxServerMessage, GfxDvcBridge,
    GfxServerFactory, GfxServerHandle, KeyboardEvent, MouseEvent, PixelFormat, RdpServer, RdpServerDisplay,
    RdpServerDisplayUpdates, RdpServerInputHandler, ServerEvent, ServerEventSender,
};
use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, QpRange, RateControlMode, UsageType};
use openh264::formats::YUVBuffer;
use openh264::OpenH264API;
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
    /// The card goes as H.264 through the graphics pipeline, not as
    /// plain bitmaps.
    h264: bool,
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
            if !self.shared.h264 && self.sent != Some((tick, size)) {
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

/// The newer picture's channel: the way to it once a viewer has opened
/// it, and the way to the viewer.
#[derive(Clone, Default)]
struct Pipeline {
    handle: Arc<Mutex<Option<GfxServerHandle>>>,
    to_viewer: Arc<Mutex<Option<UnboundedSender<ServerEvent>>>>,
}

/// What the channel tells of: what the two agreed on, and each picture
/// the viewer says it has drawn.
struct PipelineEvents;

impl GraphicsPipelineHandler for PipelineEvents {
    fn capabilities_advertise(&mut self, _pdu: &CapabilitiesAdvertisePdu) {}

    fn on_ready(&mut self, negotiated: &CapabilitySet) {
        let name = format!("{negotiated:?}");
        say(&format!("heard PipelineReady({})", name.split([' ', '{']).next().unwrap_or("")));
    }

    fn on_frame_ack(&mut self, _frame_id: u32, _queue_depth: u32, _total_frames_decoded: u32) {
        say("heard FrameAck");
    }
}

impl ServerEventSender for Pipeline {
    fn set_sender(&mut self, sender: UnboundedSender<ServerEvent>) {
        *self.to_viewer.lock().unwrap() = Some(sender);
    }
}

impl GfxServerFactory for Pipeline {
    fn build_gfx_handler(&self) -> Box<dyn GraphicsPipelineHandler> {
        Box::new(PipelineEvents)
    }

    fn build_server_with_handle(&self) -> Option<(GfxDvcBridge, GfxServerHandle)> {
        let server: GfxServerHandle = Arc::new(Mutex::new(GraphicsPipelineServer::new(Box::new(PipelineEvents))));
        *self.handle.lock().unwrap() = Some(server.clone());
        Some((GfxDvcBridge::new(server.clone()), server))
    }
}

/// The card's colours as H.264 carries them over Remote Desktop: full
/// range, by the HD (BT.709) weights, colour at half the size each way.
/// A picture of odd size gets its last row or column once more.
fn card_planes(width: u16, height: u16, tick: u32) -> (YUVBuffer, usize, usize) {
    let rgba = pattern::card(width, height, tick);
    let (w, h) = (width as usize, height as usize);
    let (ew, eh) = (w + (w & 1), h + (h & 1));
    let at = |x: usize, y: usize| {
        let i = (y.min(h - 1) * w + x.min(w - 1)) * 4;
        (rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32)
    };
    let mut planes = vec![0u8; ew * eh * 3 / 2];
    let (luma, chroma) = planes.split_at_mut(ew * eh);
    let (cb, cr) = chroma.split_at_mut(ew * eh / 4);
    for y in 0..eh {
        for x in 0..ew {
            let (r, g, b) = at(x, y);
            luma[y * ew + x] = (0.2126 * r + 0.7152 * g + 0.0722 * b).round().clamp(0.0, 255.0) as u8;
        }
    }
    for y in 0..eh / 2 {
        for x in 0..ew / 2 {
            let (mut u, mut v) = (0.0, 0.0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (r, g, b) = at(x * 2 + dx, y * 2 + dy);
                let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                u += (b - luma) / 1.8556 + 128.0;
                v += (r - luma) / 1.5748 + 128.0;
            }
            cb[y * (ew / 2) + x] = (u / 4.0).round().clamp(0.0, 255.0) as u8;
            cr[y * (ew / 2) + x] = (v / 4.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    (YUVBuffer::from_vec(planes, ew, eh), ew, eh)
}

/// Sends the card as H.264 whenever its tick or the screen's size
/// changes, once a viewer has opened the newer picture's channel.
fn pump(shared: Shared, pipeline: Pipeline) {
    let began = std::time::Instant::now();
    let mut handle_seen: Option<GfxServerHandle> = None;
    let mut surface: Option<(u16, (u16, u16))> = None;
    let mut encoder: Option<Encoder> = None;
    let mut sent: Option<(u32, (u16, u16))> = None;
    let mut size_changed: Option<std::time::Instant> = None;
    let mut refused = false;
    loop {
        std::thread::sleep(Duration::from_millis(5));
        let Some(handle) = pipeline.handle.lock().unwrap().clone() else { continue };
        // Another viewer, another channel: start over.
        if !handle_seen.as_ref().is_some_and(|seen| Arc::ptr_eq(seen, &handle)) {
            handle_seen = Some(handle.clone());
            (surface, encoder, sent, size_changed, refused) = (None, None, None, None, false);
        }
        let mut gfx = handle.lock().unwrap();
        if !gfx.is_ready() {
            continue;
        }
        if !gfx.supports_avc420() {
            if !refused {
                say("heard NoH264");
                refused = true;
            }
            continue;
        }
        let size = *shared.size.lock().unwrap();
        let tick = shared.tick.load(Ordering::Acquire);
        if surface.map(|(_, of)| of) != Some(size) {
            // After a change of size the viewer and the computer start
            // the screen over; the pipeline follows a moment later, as a
            // real computer's does.
            if surface.is_some() {
                let since = *size_changed.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() < Duration::from_millis(400) {
                    continue;
                }
                gfx.resize(size.0, size.1);
            } else {
                gfx.set_output_dimensions(size.0, size.1);
            }
            size_changed = None;
            let Some(id) = gfx.create_surface(size.0, size.1) else { continue };
            gfx.map_surface_to_output(id, 0, 0);
            surface = Some((id, size));
            encoder = None;
            sent = None;
        }
        let Some((id, _)) = surface else { continue };
        if sent != Some((tick, size)) {
            let (planes, _, _) = card_planes(size.0, size.1, tick);
            let made = encoder.get_or_insert_with(|| {
                let config = EncoderConfig::new()
                    .usage_type(UsageType::ScreenContentRealTime)
                    .rate_control_mode(RateControlMode::Off)
                    .qp(QpRange::new(8, 18))
                    .bitrate(BitRate::from_bps(40_000_000))
                    .max_frame_rate(FrameRate::from_hz(60.0))
                    .skip_frames(false);
                Encoder::with_api_config(OpenH264API::from_source(), config).expect("the encoder starts")
            });
            let picture = match made.encode(&planes) {
                Ok(bits) => bits.to_vec(),
                Err(e) => {
                    say(&format!("ended the encoder failed: {e}"));
                    continue;
                }
            };
            if picture.is_empty() {
                // Nothing changed for the encoder: nothing to send.
                sent = Some((tick, size));
                continue;
            }
            let region = Avc420Region::full_frame(size.0, size.1, 18);
            // None while the viewer is behind with its acknowledgements:
            // tried again on the next round.
            if gfx.send_avc420_frame(id, &picture, &[region], began.elapsed().as_millis() as u32).is_some() {
                sent = Some((tick, size));
                say(&format!("heard Frame({tick}, {})", picture.len()));
            }
        }
        if gfx.has_pending_output() {
            let Some(channel) = gfx.channel_id() else { continue };
            let messages = gfx.drain_output();
            drop(gfx);
            match ironrdp_dvc::encode_dvc_messages(channel, messages, ironrdp_svc::ChannelFlags::empty()) {
                Ok(messages) => {
                    if let Some(sender) = pipeline.to_viewer.lock().unwrap().as_ref() {
                        let _ = sender.send(ServerEvent::Egfx(EgfxServerMessage::SendMessages { messages }));
                    }
                }
                Err(e) => say(&format!("ended the pipeline's messages could not be packed: {e}")),
            }
        }
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
        h264: args.iter().any(|a| a == "--h264"),
    };
    let pipeline = Pipeline::default();
    if shared.h264 {
        let (shared, pipeline) = (shared.clone(), pipeline.clone());
        std::thread::spawn(move || pump(shared, pipeline));
    }
    // The card moving by itself, so many ticks a second.
    let rate = Arc::new(AtomicU32::new(0));
    {
        let (shared, rate) = (shared.clone(), rate.clone());
        std::thread::spawn(move || loop {
            match rate.load(Ordering::Acquire) {
                0 => std::thread::sleep(Duration::from_millis(50)),
                hz => {
                    std::thread::sleep(Duration::from_micros(1_000_000 / hz as u64));
                    shared.tick.fetch_add(1, Ordering::AcqRel);
                }
            }
        });
    }

    let clipboard = Clipboard {
        no_files: args.iter().any(|a| a == "--no-files"),
        ..Clipboard::default()
    };
    // What the tests (or a person) ask for; the end of the input ends it.
    {
        let shared = shared.clone();
        let clipboard = clipboard.clone();
        let rate = rate.clone();
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
                    ["rate", hz] => rate.store(hz.parse().unwrap_or(0), Ordering::Release),
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
    let h264 = shared.h264;
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
            .with_gfx_factory(if h264 { Some(Box::new(pipeline)) } else { None })
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
