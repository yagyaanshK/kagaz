//! Remote Desktop (RDP): a Windows computer's own remote access, also
//! GNOME's. The protocol's parts come from IronRDP; the session is
//! Kagaz's, so that the certificate the computer shows is checked against
//! the one remembered before a password is sent, and so that the picture
//! lands in the shared [`Screen`] as changed areas.
//!
//! One thread runs a session. It waits on the connection and on a waker
//! that [`InputSender`] pokes, so a key or a click leaves at once.

mod clip;
mod sound;

use super::keys;
use super::reach;
use super::screen::{Event, Pointer, Rect, Screen, Sending};
use super::session::{
    buttons_for, Info, Input, InputSender, Numbers, OpenError, Session, SignIn, Trust, View, BACK, FORWARD, LEFT,
    MIDDLE, RIGHT,
};
use super::{Settings, Speaks};
use ironrdp_blocking::Framed;
use ironrdp_cliprdr::pdu::{ClipboardFormat, ClipboardFormatId, FileContentsResponse, FormatDataResponse};
use ironrdp_cliprdr::CliprdrClient;
use ironrdp_connector::connection_activation::{ConnectionActivationFactory, ConnectionActivationState};
use ironrdp_connector::sspi::generator::NetworkRequest;
use ironrdp_connector::sspi::network_client::NetworkClient;
use ironrdp_connector::{
    BitmapConfig, ClientConnector, Config, ConnectionOutcome, ConnectorErrorKind, Credentials,
    DesktopSize, Sequence as _, ServerName,
};
use ironrdp_core::{IntoOwned as _, WriteBuf};
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_dvc::DrdynvcClient;
use ironrdp_egfx::client::{BitmapUpdate, GraphicsPipelineClient, GraphicsPipelineHandler};
use ironrdp_egfx::decode::{H264Decoder, OpenH264Decoder};
use ironrdp_egfx::pdu::{CapabilitySet, Codec1Type};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::gcc::{ConnectionType, KeyboardType};
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags, SynchronizeFlags};
use ironrdp_pdu::input::mouse::{MousePdu, PointerFlags};
use ironrdp_pdu::input::mouse_x::{MouseXPdu, PointerXFlags};
use ironrdp_pdu::rdp::capability_sets::{client_codecs_capabilities, MajorPlatformType, RailSupportLevel};
use ironrdp_pdu::rdp::client_info::{CompressionType, PerformanceFlags, TimezoneInfo};
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use ironrdp_rdpdr::{NoopRdpdrBackend, Rdpdr};
use ironrdp_rdpsnd::client::{Rdpsnd, RdpsndDvcState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// The screen asked for when the viewer's size is not known, or when
/// the person wants the picture scaled instead of drawn to fit.
const USUAL_SIZE: (u16, u16) = (1280, 800);
const OWN_SIZE: (u16, u16) = (1920, 1080);

/// A size a Remote Desktop screen can have: within its limits, its width
/// even.
fn fitting(width: u16, height: u16) -> (u16, u16) {
    ((width.clamp(200, 8192)) & !1, height.clamp(200, 8192))
}

/// A scale Remote Desktop takes: 100 to 500 percent.
fn fitting_scale(scale: u32) -> u32 {
    scale.clamp(100, 500)
}
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a read waits before the session looks at its other work.
const READ_SLICE: Duration = Duration::from_millis(4);

/// The connection, counting what passes.
struct Counted {
    stream: TcpStream,
    numbers: Arc<Numbers>,
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.stream.read(buf)?;
        self.numbers.add_received(n);
        Ok(n)
    }
}

impl Write for Counted {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.stream.write(buf)?;
        self.numbers.add_sent(n);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

type Tls = rustls::StreamOwned<rustls::ClientConnection, Counted>;

/// Signing in to a computer at home needs no directory server; a sign-in
/// that asks for one (a company domain) is told that Kagaz has none.
struct NoDirectory;

impl NetworkClient for NoDirectory {
    fn send(&self, _request: &NetworkRequest) -> ironrdp_connector::sspi::Result<Vec<u8>> {
        Err(ironrdp_connector::sspi::Error::new(
            ironrdp_connector::sspi::ErrorKind::NoAuthenticatingAuthority,
            "Kagaz signs in to the computer itself, not through a domain's servers",
        ))
    }
}

fn config(
    user: &str,
    password: &str,
    size: (u16, u16),
    scale: u32,
    newer_picture: bool,
    sound: bool,
) -> Result<Config, OpenError> {
    // "DOMAIN\user" names a domain; a plain name signs in to the computer.
    let (domain, username) = match user.split_once('\\') {
        Some((domain, name)) if !domain.is_empty() => (Some(domain.to_string()), name.to_string()),
        _ => (None, user.to_string()),
    };
    Ok(Config {
        desktop_size: DesktopSize {
            width: size.0,
            height: size.1,
        },
        monitor_layout: None,
        // Text and controls as large as on this screen: without it a fine
        // screen would get everything at half the size.
        desktop_scale_factor: scale,
        desktop_physical_size: None,
        desktop_orientation: None,
        device_scale_factor: Some(100),
        monitor_extended_layout: None,
        relative_mouse_input: false,
        enable_tls: true,
        enable_credssp: true,
        enable_standard_rdp_security: false,
        credentials: Credentials::UsernamePassword {
            username,
            password: password.to_string(),
        },
        domain,
        client_build: 0,
        client_name: "Kagaz".into(),
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        // The device's own layout decides what the keys mean.
        keyboard_layout: 0,
        connection_type: ConnectionType::Autodetect,
        ime_file_name: String::new(),
        bitmap: Some(BitmapConfig {
            lossy_compression: true,
            color_depth: 32,
            codecs: client_codecs_capabilities(&[]).map_err(OpenError::Other)?,
        }),
        dig_product_id: String::new(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".into(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        remote_application_mode: false,
        rail_support_level: RailSupportLevel::empty(),
        platform: if cfg!(windows) {
            MajorPlatformType::WINDOWS
        } else if cfg!(target_os = "macos") {
            MajorPlatformType::MACINTOSH
        } else {
            MajorPlatformType::UNIX
        },
        hardware_id: None,
        request_data: None,
        autologon: false,
        // Its sound plays here, not on the computer, where there is the
        // channel to take it.
        enable_audio_playback: sound,
        enable_audio_capture: false,
        performance_flags: PerformanceFlags::default(),
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        compression_type: Some(CompressionType::K64),
        // The pointer's shape comes here and is drawn by the viewer, so
        // that it moves at once.
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
        // The newer picture, where its channel is there to take it.
        support_dyn_vc_gfx_protocol: newer_picture,
    })
}

/// Watches the newer picture's channel (the graphics pipeline) to tell
/// how the picture travels: which version the two agreed on, and how
/// many areas came by each way of packing them.
struct Watching {
    numbers: Arc<Numbers>,
    agreed: String,
    /// Areas by codec, in the order first seen.
    areas: Vec<(&'static str, u64)>,
    frames: u64,
}

impl Watching {
    fn tell(&self) {
        let mut via = format!("newer picture {}", self.agreed);
        for (codec, count) in &self.areas {
            via.push_str(&format!(", {codec} {count}"));
        }
        via.push_str(&format!(", {} frames", self.frames));
        self.numbers.set_via(via);
    }
}

impl GraphicsPipelineHandler for Watching {
    fn on_capabilities_confirmed(&mut self, caps: &CapabilitySet) {
        // "V10_7 { flags: … }" as "10.7".
        let name = format!("{caps:?}");
        let version = name.split([' ', '{']).next().unwrap_or("").trim_start_matches('V').replace('_', ".");
        self.agreed = version;
        self.tell();
    }

    fn on_bitmap_updated(&mut self, update: &BitmapUpdate) {
        let codec = match update.codec_id {
            Codec1Type::Avc420 => "H.264",
            Codec1Type::Avc444 | Codec1Type::Avc444v2 => "H.264 full colour",
            Codec1Type::ClearCodec => "text",
            Codec1Type::RemoteFx => "RemoteFX",
            Codec1Type::Planar => "planar",
            Codec1Type::Alpha => "alpha",
            // What the progressive codec hands over once it is decoded.
            Codec1Type::Uncompressed => "plain or progressive",
        };
        match self.areas.iter_mut().find(|(name, _)| *name == codec) {
            Some((_, count)) => *count += 1,
            None => self.areas.push((codec, 1)),
        }
    }

    fn on_frame_complete(&mut self, _frame_id: u32) {
        self.frames += 1;
        self.tell();
    }
}

/// Why the computer turned the connection down, in words a person can
/// act on.
fn refusal(e: ironrdp_connector::ConnectorError) -> OpenError {
    match e.kind() {
        // Network-level sign-in failed: the user name or the password.
        ConnectorErrorKind::Credssp(_) | ConnectorErrorKind::AccessDenied => OpenError::WrongPassword,
        ConnectorErrorKind::Negotiation(failure) => OpenError::Refused(failure.to_string()),
        ConnectorErrorKind::Reason(why) => OpenError::Refused(why.clone()),
        _ => {
            // The first cause that says something, without the library's
            // own names for its steps.
            let mut why = e.to_string();
            let mut source = std::error::Error::source(&e);
            while let Some(s) = source {
                why = s.to_string();
                source = s.source();
            }
            OpenError::Other(why)
        }
    }
}

/// One changed area of the picture, kept within it.
fn area_of(image: &DecodedImage, r: &InclusiveRectangle) -> Rect {
    let width = image.width();
    let height = image.height();
    let right = r.right.min(width.saturating_sub(1));
    let bottom = r.bottom.min(height.saturating_sub(1));
    if width == 0 || height == 0 || r.left > right || r.top > bottom {
        return Rect::default();
    }
    Rect::new(r.left, r.top, right - r.left + 1, bottom - r.top + 1)
}

/// A way to rouse the session's thread while it waits on the connection.
#[cfg(unix)]
mod waker {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::time::Duration;

    pub struct Waiter {
        woken: UnixStream,
    }

    pub fn pair() -> std::io::Result<(Waiter, Arc<dyn Fn() + Send + Sync>)> {
        let (woken, poke) = UnixStream::pair()?;
        woken.set_nonblocking(true)?;
        poke.set_nonblocking(true)?;
        let wake = move || {
            // A full pipe already means "wake up".
            let _ = (&poke).write(&[1]);
        };
        Ok((Waiter { woken }, Arc::new(wake)))
    }

    impl Waiter {
        /// Wait until the connection has something, the waker was poked,
        /// or the time ran out.
        pub fn wait(&mut self, connection: &TcpStream, timeout: Duration) {
            let mut fds = [
                libc::pollfd {
                    fd: connection.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.woken.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: `fds` is two valid entries that outlive the call.
            unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout.as_millis().min(i32::MAX as u128) as i32) };
            let mut drain = [0u8; 64];
            while matches!(self.woken.read(&mut drain), Ok(n) if n > 0) {}
        }
    }
}

/// Where there is no such waiting, the short read is the wait.
#[cfg(not(unix))]
mod waker {
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::time::Duration;

    pub struct Waiter;

    pub fn pair() -> std::io::Result<(Waiter, Arc<dyn Fn() + Send + Sync>)> {
        Ok((Waiter, Arc::new(|| {})))
    }

    impl Waiter {
        pub fn wait(&mut self, _connection: &TcpStream, _timeout: Duration) {}
    }
}

/// The delay the connection itself has measured, there and back.
#[cfg(target_os = "linux")]
fn measured_delay(connection: &TcpStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    // SAFETY: `info` is a plain struct the kernel fills up to `len` bytes.
    unsafe {
        let mut info: libc::tcp_info = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::tcp_info>() as libc::socklen_t;
        let ok = libc::getsockopt(
            connection.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            (&mut info as *mut libc::tcp_info).cast(),
            &mut len,
        );
        if ok != 0 || info.tcpi_rtt == 0 {
            return None;
        }
        // The lowest of the last minutes where the system keeps it: the
        // running average swells when the other side holds its
        // acknowledgements back (it does while it has nothing to send),
        // which says nothing of the path.
        let lowest = match info.tcpi_min_rtt {
            0 | u32::MAX => info.tcpi_rtt,
            lowest => lowest.min(info.tcpi_rtt),
        };
        Some(lowest.div_ceil(1000))
    }
}

#[cfg(not(target_os = "linux"))]
fn measured_delay(_connection: &TcpStream) -> Option<u32> {
    None
}

/// Everything one session's thread holds.
struct Running {
    framed: Framed<Tls>,
    /// The same connection, to wait on and to close.
    connection: TcpStream,
    stage: ActiveStage,
    image: DecodedImage,
    /// Makes the opening exchange again when the computer asks for it.
    activation: ConnectionActivationFactory,
    screen: Arc<Screen>,
    numbers: Arc<Numbers>,
    swapped: bool,
    /// Draw the screen to fit the viewer (not when the person asked for
    /// its own size), at this scale.
    fits: bool,
    scale: u32,
    /// The size the viewer has now, until the computer has been asked.
    wanted: Option<(u16, u16)>,
    buttons: u8,
    /// Something was drawn since the last count.
    drew: bool,
    /// What the clipboard channel asked for and the session has yet to do.
    clip: Arc<clip::Shared>,
    /// What is here for the computer's clipboard.
    here: Here,
    /// The computer has been told of it.
    here_said: bool,
    /// The text last seen, from either side: it is not sent back.
    clip_last: Option<String>,
    /// The same for the picture last seen (see [`mark`]).
    clip_last_picture: Option<u64>,
    /// The text of the computer's latest copy, for a picture that comes
    /// with it.
    their_text: Option<String>,
    /// Offers the computer asked to keep, by the number it gave each.
    kept: HashMap<u32, Arc<clip::Offer>>,
    /// When the viewer was last told how a file is getting on.
    told: Option<Instant>,
    /// The person has turned the computer's sound off here for now.
    quiet: Arc<AtomicBool>,
    /// Keys that are down, to let go when the session closes.
    held: HashMap<String, FastPathInputEvent>,
}

/// What is here for the computer's clipboard: handed over when it asks.
enum Here {
    Nothing,
    Text(String),
    /// A picture (PNG), with the text that was copied along with it.
    Picture { png: Arc<Vec<u8>>, text: Option<String> },
    Files(Arc<clip::Offer>),
}

impl Here {
    /// What the computer's clipboard is told there is to have.
    fn formats(&self) -> Vec<ClipboardFormat> {
        let text = ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT);
        match self {
            Here::Text(_) => vec![text],
            Here::Picture { text: with, .. } => {
                let mut formats = clip::picture_formats().to_vec();
                if with.is_some() {
                    formats.push(text);
                }
                formats
            }
            // Files are said in a way of their own, once the channel is
            // ready for them.
            Here::Nothing | Here::Files(_) => Vec::new(),
        }
    }

    /// What is here in the format the computer asks for.
    fn give(&self, format: ClipboardFormatId) -> Option<FormatDataResponse<'static>> {
        let text = |text: &str| FormatDataResponse::new_unicode_string(&clip::to_wire(text)).into_owned();
        match self {
            Here::Text(here) if format == ClipboardFormatId::CF_UNICODETEXT => Some(text(here)),
            Here::Picture { text: Some(with), .. } if format == ClipboardFormatId::CF_UNICODETEXT => Some(text(with)),
            Here::Picture { png, .. } => clip::picture_as(png, format).map(FormatDataResponse::new_data),
            _ => None,
        }
    }
}

/// A picture told apart from another without keeping both.
fn mark(png: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    png.hash(&mut hasher);
    hasher.finish()
}

/// What the loop does next.
enum Next {
    Go,
    End(String),
}

impl Running {
    fn write(&mut self, frame: &[u8]) -> Result<(), String> {
        self.framed.write_all(frame).map_err(|_| "the connection to the device broke".to_string())
    }

    /// What the session's parts ask for after handling something.
    fn outputs(&mut self, outputs: Vec<ActiveStageOutput>) -> Result<Next, String> {
        for out in outputs {
            match out {
                ActiveStageOutput::ResponseFrame(frame) => self.write(&frame)?,
                ActiveStageOutput::GraphicsUpdate(area) => {
                    // The newer picture says its screen's size by itself.
                    let size = (self.image.width(), self.image.height());
                    if self.screen.size() != size {
                        self.screen.resize(size.0, size.1);
                    }
                    let rect = area_of(&self.image, &area);
                    if !rect.is_empty() {
                        self.screen.put_from(rect, self.image.data(), self.image.stride());
                        self.drew = true;
                    }
                }
                ActiveStageOutput::PointerDefault => self.screen.post(Event::PointerDefault),
                ActiveStageOutput::PointerHidden => self.screen.post(Event::Pointer(Pointer {
                    width: 0,
                    height: 0,
                    hot_x: 0,
                    hot_y: 0,
                    rgba: Vec::new(),
                })),
                ActiveStageOutput::PointerBitmap(p) => self.screen.post(Event::Pointer(Pointer {
                    width: p.width,
                    height: p.height,
                    hot_x: p.hotspot_x,
                    hot_y: p.hotspot_y,
                    rgba: p.bitmap_data.clone(),
                })),
                ActiveStageOutput::Terminate(reason) => return Ok(Next::End(ended_words(&reason.description()))),
                ActiveStageOutput::DeactivateAll => self.reactivate()?,
                _ => {}
            }
        }
        Ok(Next::Go)
    }

    /// The computer starts its screen over (after a new size, a sign-in):
    /// the opening exchange once more, then a picture of the new size.
    fn reactivate(&mut self) -> Result<(), String> {
        let broke = |_| "the connection to the device broke".to_string();
        let mut sequence = self.activation.create();
        let mut buf = WriteBuf::new();
        self.connection.set_read_timeout(Some(Duration::from_secs(10))).map_err(broke)?;
        let done = loop {
            buf.clear();
            let written = match sequence.next_pdu_hint() {
                Some(hint) => {
                    let pdu = self.framed.read_by_hint(hint).map_err(broke)?;
                    sequence.step(&pdu, self.framed.last_read_at(), &mut buf)
                }
                None => sequence.step_no_input(&mut buf),
            }
            .map_err(|e| format!("the device started its screen over and that failed ({e})"))?;
            if let Some(len) = written.size() {
                let frame = buf[..len].to_vec();
                self.write(&frame)?;
            }
            if let ConnectionActivationState::Finalized {
                desktop_size,
                share_id,
                enable_server_pointer,
                pointer_software_rendering,
                ..
            } = sequence.connection_activation_state()
            {
                break (desktop_size, share_id, enable_server_pointer, pointer_software_rendering);
            }
        };
        self.connection.set_read_timeout(Some(READ_SLICE)).map_err(broke)?;
        let (size, share_id, server_pointer, software_pointer) = done;
        let chunk = self.stage.static_channel_chunk_size();
        self.stage.reactivate(
            sequence.io_channel_id(),
            sequence.user_channel_id(),
            share_id,
            server_pointer,
            software_pointer,
            chunk,
        );
        self.image = DecodedImage::new(PixelFormat::RgbA32, size.width, size.height);
        self.screen.resize(size.width, size.height);
        Ok(())
    }

    /// Do what the clipboard channel asked for since the last look.
    fn clipboard(&mut self) -> Result<(), String> {
        let carried = |e: &dyn std::fmt::Display| format!("the clipboard could not be carried ({e})");
        loop {
            let Some(asked) = self.clip.next() else {
                return Ok(());
            };
            let Some(channel) = self.stage.get_svc_processor_mut::<CliprdrClient>() else {
                continue;
            };
            let messages = match asked {
                // Asked at the start, when text and pictures can be said;
                // files offered that early are said once the channel is
                // ready.
                clip::Asked::List => {
                    let formats = self.here.formats();
                    self.here_said = !formats.is_empty();
                    channel.initiate_copy(&formats).map_err(|e| carried(&e))?
                }
                clip::Asked::Mine => {
                    if self.here_said || !channel.is_ready() {
                        continue;
                    }
                    self.here_said = true;
                    match &self.here {
                        Here::Nothing => continue,
                        Here::Text(_) | Here::Picture { .. } => {
                            channel.initiate_copy(&self.here.formats()).map_err(|e| carried(&e))?
                        }
                        Here::Files(offer) => {
                            let offered = if self.clip.files.load(Ordering::Acquire) {
                                channel
                                    .initiate_file_copy(offer.descriptors())
                                    .map_err(|e| format!("the files could not be offered ({e})"))
                            } else {
                                Err("this device does not take files over its clipboard".to_string())
                            };
                            match offered {
                                Ok(messages) => {
                                    self.screen.post(Event::Sending(Sending::Offered {
                                        names: offer.names(),
                                        bytes: offer.bytes(),
                                    }));
                                    messages
                                }
                                Err(why) => {
                                    self.here = Here::Nothing;
                                    self.screen.post(Event::Sending(Sending::Failed { why }));
                                    continue;
                                }
                            }
                        }
                    }
                }
                clip::Asked::Fetch => channel.initiate_paste(ClipboardFormatId::CF_UNICODETEXT).map_err(|e| carried(&e))?,
                clip::Asked::Give(format) => channel
                    .submit_format_data(self.here.give(format).unwrap_or_else(FormatDataResponse::new_error))
                    .map_err(|e| carried(&e))?,
                clip::Asked::FetchPicture(format) => channel.initiate_paste(format).map_err(|e| carried(&e))?,
                clip::Asked::Picture(png) => {
                    let seen = mark(&png);
                    if self.clip_last_picture != Some(seen) {
                        self.clip_last_picture = Some(seen);
                        // The computer's now: nothing to hand back to it.
                        self.withdraw();
                        let text = self.their_text.clone();
                        self.screen.post(Event::ClipboardPicture { png, text });
                    }
                    continue;
                }
                clip::Asked::Text(text) => {
                    self.their_text = Some(text.clone());
                    if self.clip_last.as_deref() != Some(&text) {
                        self.clip_last = Some(text.clone());
                        // What is here now is the computer's: no longer
                        // something to hand back to it.
                        self.withdraw();
                        self.screen.post(Event::Clipboard(text));
                    }
                    continue;
                }
                clip::Asked::Theirs => {
                    // A new copy on the computer: its text, if it has
                    // any, comes next.
                    self.their_text = None;
                    if matches!(self.here, Here::Files(_)) {
                        self.withdraw();
                    }
                    continue;
                }
                clip::Asked::Lock(number) => {
                    if let (Here::Files(offer), true) = (&self.here, self.kept.len() < ironrdp_cliprdr::MAX_LOCKED_FILE_LISTS) {
                        self.kept.insert(number, offer.clone());
                    }
                    continue;
                }
                clip::Asked::Unlock(number) => {
                    self.kept.remove(&number);
                    continue;
                }
                clip::Asked::Contents(request) => {
                    // From the offer it asked to keep, else what is offered now.
                    let offer = request.data_id.and_then(|number| self.kept.get(&number)).or(match &self.here {
                        Here::Files(offer) => Some(offer),
                        _ => None,
                    });
                    let (response, news) = match offer {
                        Some(offer) => offer.answer(&request),
                        None => (FileContentsResponse::new_error(request.stream_id), None),
                    };
                    match news {
                        // Not each piece: a few times a second is enough to watch.
                        Some(Sending::Going { .. })
                            if self.told.is_some_and(|at| at.elapsed() < Duration::from_millis(200)) => {}
                        Some(news) => {
                            self.told = Some(Instant::now());
                            self.screen.post(Event::Sending(news));
                        }
                        None => {}
                    }
                    channel.submit_file_contents(response).map_err(|e| carried(&e))?
                }
            };
            let frame = self.stage.process_svc_processor_messages(messages).map_err(|e| carried(&e))?;
            self.write(&frame)?;
        }
    }

    /// What was here for the computer is no longer on its clipboard.
    fn withdraw(&mut self) {
        if matches!(std::mem::replace(&mut self.here, Here::Nothing), Here::Files(_)) {
            self.screen.post(Event::Sending(Sending::Withdrawn));
        }
        self.here_said = false;
    }

    /// Text was copied here: tell the computer there is text to have.
    /// The text last seen is not said again, unless the person asked and
    /// something else is on offer now.
    fn copied_here(&mut self, text: &str, asked: bool) {
        let offered_now = matches!(&self.here, Here::Text(t) if t == text);
        if self.clip_last.as_deref() == Some(text) && (offered_now || !asked) {
            return;
        }
        self.withdraw();
        self.clip_last = Some(text.to_string());
        self.here = Here::Text(text.to_string());
        self.clip.ask(clip::Asked::Mine);
    }

    /// A picture was copied here: tell the computer there is one to have.
    /// The one last seen, from either side, is not said again.
    fn copied_picture(&mut self, png: &[u8], text: &Option<String>) {
        let seen = mark(png);
        if self.clip_last_picture == Some(seen) {
            return;
        }
        self.withdraw();
        self.clip_last_picture = Some(seen);
        if let Some(text) = text {
            self.clip_last = Some(text.clone());
        }
        self.here = Here::Picture {
            png: Arc::new(png.to_vec()),
            text: text.clone(),
        };
        self.clip.ask(clip::Asked::Mine);
    }

    /// Files to send: tell the computer they are there to be pasted.
    fn offer(&mut self, paths: &[std::path::PathBuf]) {
        match clip::Offer::of(paths) {
            Ok(offer) => {
                self.withdraw();
                self.here = Here::Files(Arc::new(offer));
                self.clip.ask(clip::Asked::Mine);
            }
            Err(why) => self.screen.post(Event::Sending(Sending::Failed { why })),
        }
    }

    /// The clipboard channel's housekeeping: offers the computer asked to
    /// keep and then left alone are let go.
    fn tidy_clipboard(&mut self) -> Result<(), String> {
        let Some(channel) = self.stage.get_svc_processor_mut::<CliprdrClient>() else {
            return Ok(());
        };
        let Ok(messages) = channel.drive_timeouts() else {
            return Ok(());
        };
        let frame = self
            .stage
            .process_svc_processor_messages(messages)
            .map_err(|e| format!("the clipboard could not be carried ({e})"))?;
        if !frame.is_empty() {
            self.write(&frame)?;
        }
        Ok(())
    }

    /// What the person did, as the events Remote Desktop has for it.
    fn events(&mut self, input: &Input, out: &mut Vec<FastPathInputEvent>) {
        let mouse = |flags: PointerFlags, x: u16, y: u16, wheel: i16| {
            FastPathInputEvent::MouseEvent(MousePdu {
                flags,
                number_of_wheel_rotation_units: wheel,
                x_position: x,
                y_position: y,
            })
        };
        match input {
            Input::Pointer { x, y, buttons } => {
                let now = buttons_for(*buttons, self.swapped);
                out.push(mouse(PointerFlags::MOVE, *x, *y, 0));
                for (bit, flag) in [
                    (LEFT, PointerFlags::LEFT_BUTTON),
                    (MIDDLE, PointerFlags::MIDDLE_BUTTON_OR_WHEEL),
                    (RIGHT, PointerFlags::RIGHT_BUTTON),
                ] {
                    if (now ^ self.buttons) & bit != 0 {
                        let down = if now & bit != 0 { PointerFlags::DOWN } else { PointerFlags::empty() };
                        out.push(mouse(flag | down, *x, *y, 0));
                    }
                }
                // The side buttons have an event of their own.
                for (bit, flag) in [(BACK, PointerXFlags::BUTTON1), (FORWARD, PointerXFlags::BUTTON2)] {
                    if (now ^ self.buttons) & bit != 0 {
                        let down = if now & bit != 0 { PointerXFlags::DOWN } else { PointerXFlags::empty() };
                        out.push(FastPathInputEvent::MouseEventEx(MouseXPdu {
                            flags: flag | down,
                            x_position: *x,
                            y_position: *y,
                        }));
                    }
                }
                self.buttons = now;
            }
            Input::Locks { caps, num, scroll } => {
                let mut flags = SynchronizeFlags::empty();
                flags.set(SynchronizeFlags::CAPS_LOCK, *caps);
                flags.set(SynchronizeFlags::NUM_LOCK, *num);
                flags.set(SynchronizeFlags::SCROLL_LOCK, *scroll);
                out.push(FastPathInputEvent::SyncEvent(flags));
            }
            Input::Wheel { x, y, up, right } => {
                // A notch is 120 units; down and left are "negative".
                for (notches, axis) in [(*up, PointerFlags::VERTICAL_WHEEL), (*right, PointerFlags::HORIZONTAL_WHEEL)] {
                    for _ in 0..notches.unsigned_abs().min(20) {
                        let (flags, units) = if notches > 0 {
                            (axis, 120)
                        } else {
                            (axis | PointerFlags::WHEEL_NEGATIVE, -120)
                        };
                        out.push(mouse(flags, *x, *y, units));
                    }
                }
            }
            Input::Key { key, code, down } => {
                let release = if *down { KeyboardFlags::empty() } else { KeyboardFlags::RELEASE };
                let id = if code.is_empty() { key } else { code };
                // The key itself where the page names it, else the letter
                // it stands for (a phone's keyboard, a composed letter).
                let key_event = |flags: KeyboardFlags| match keys::scancode(code) {
                    Some((number, extended)) => {
                        let extended = if extended { KeyboardFlags::EXTENDED } else { KeyboardFlags::empty() };
                        Some(FastPathInputEvent::KeyboardEvent(flags | extended, number))
                    }
                    None => {
                        let mut chars = key.chars();
                        match (chars.next(), chars.next()) {
                            (Some(c), None) if c.len_utf16() == 1 => {
                                Some(FastPathInputEvent::UnicodeKeyboardEvent(flags, c as u16))
                            }
                            _ => None,
                        }
                    }
                };
                let event = key_event(release).zip(key_event(KeyboardFlags::RELEASE));
                if let Some((event, let_go)) = event {
                    if *down {
                        self.held.insert(id.clone(), let_go);
                    } else {
                        self.held.remove(id);
                    }
                    out.push(event);
                }
            }
            Input::SwappedButtons(now) => self.swapped = *now,
            Input::Sound(on) => self.quiet.store(!*on, Ordering::Release),
            Input::Resize { width, height } => {
                if self.fits {
                    self.wanted = Some(fitting(*width, *height));
                }
            }
            Input::Clipboard(text) => self.copied_here(text, false),
            Input::ClipboardAsked(text) => self.copied_here(text, true),
            Input::ClipboardPicture { png, text } => self.copied_picture(png, text),
            Input::Files(paths) => self.offer(paths),
            // The computer suits its picture to the link by itself.
            Input::Picture(_) => {}
            Input::Close => {}
        }
    }
}

/// The computer's reason for ending, as a sentence's end.
fn ended_words(reason: &str) -> String {
    let reason = reason.trim().trim_end_matches('.');
    if reason.is_empty() {
        "the device closed the session".into()
    } else {
        format!("the device closed the session ({})", reason.to_lowercase())
    }
}

fn run(mut s: Running, rx: mpsc::Receiver<Input>, mut waiter: waker::Waiter) -> String {
    let mut measured = Instant::now();
    loop {
        // What the person did since the last look.
        let mut events = Vec::new();
        let mut close = false;
        loop {
            match rx.try_recv() {
                Ok(Input::Close) | Err(mpsc::TryRecvError::Disconnected) => {
                    close = true;
                    break;
                }
                Ok(input) => s.events(&input, &mut events),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if close {
            // Leave no key held down on the computer, and say goodbye.
            events.extend(std::mem::take(&mut s.held).into_values());
            if let Ok(outputs) = s.stage.process_fastpath_input(&mut s.image, &events) {
                let _ = s.outputs(outputs);
            }
            if let Ok(outputs) = s.stage.graceful_shutdown() {
                let _ = s.outputs(outputs);
            }
            let _ = s.connection.shutdown(Shutdown::Both);
            return "closed".into();
        }
        // What was copied or offered here goes to the clipboard channel
        // before anything more is read from the computer, so that the
        // channel and the session agree on what is here.
        if let Err(why) = s.clipboard() {
            return why;
        }
        if !events.is_empty() {
            let handled = s
                .stage
                .process_fastpath_input(&mut s.image, &events)
                .map_err(|e| format!("something the session could not send ({e})"))
                .and_then(|outputs| s.outputs(outputs));
            match handled {
                Ok(Next::Go) => {}
                Ok(Next::End(why)) | Err(why) => return why,
            }
        }
        // What the computer sent: all that is there, but not for so long
        // that a key waits.
        let began = Instant::now();
        let mut quiet = false;
        while began.elapsed() < Duration::from_millis(12) {
            let (action, payload) = match s.framed.read_pdu() {
                Ok(frame) => frame,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    quiet = true;
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return "the device closed the connection".into(),
                Err(_) => return "the connection to the device broke".into(),
            };
            let working = Instant::now();
            let handled = s
                .stage
                .process(&mut s.image, action, &payload, s.framed.last_read_at())
                .map_err(|e| format!("the device sent something Kagaz could not read ({e})"))
                .and_then(|outputs| s.outputs(outputs));
            s.numbers.add_work(working.elapsed());
            match handled {
                Ok(Next::Go) => {}
                Ok(Next::End(why)) | Err(why) => return why,
            }
        }
        if let Err(why) = s.clipboard() {
            return why;
        }
        // The viewer has another size: ask for the screen at that size,
        // once the computer has said it takes such asking. It answers by
        // starting its screen over.
        if let Some((width, height)) = s.wanted {
            if (width, height) == (s.image.width(), s.image.height()) {
                s.wanted = None;
            } else if let Some(frame) = s.stage.encode_resize(width as u32, height as u32, Some(s.scale), None) {
                s.wanted = None;
                if let Err(why) = frame.map_err(|e| format!("the new size could not be asked for ({e})")).and_then(|f| s.write(&f)) {
                    return why;
                }
            }
        }
        s.screen.commit();
        if std::mem::take(&mut s.drew) {
            s.numbers.add_update();
        }
        if measured.elapsed() > Duration::from_secs(1) {
            measured = Instant::now();
            if let Some(delay) = measured_delay(&s.connection) {
                s.numbers.set_delay(delay);
            }
            if let Err(why) = s.tidy_clipboard() {
                return why;
            }
        }
        if quiet {
            waiter.wait(&s.connection, Duration::from_secs(1));
        }
    }
}

/// Open a session with the Remote Desktop server at `addr`.
pub fn open(
    addr: SocketAddr,
    sign_in: &SignIn,
    settings: &Settings,
    trust: &Trust,
    view: Option<View>,
    timeout: Duration,
) -> Result<Session, OpenError> {
    let fits = !settings.own_size;
    let (size, scale) = match view {
        Some(v) if fits => (fitting(v.width, v.height), fitting_scale(v.scale)),
        Some(v) => (OWN_SIZE, fitting_scale(v.scale)),
        None => (if fits { USUAL_SIZE } else { OWN_SIZE }, 100),
    };
    let (Some(user), Some(password)) = (sign_in.user.as_deref().filter(|u| !u.trim().is_empty()), &sign_in.password)
    else {
        return Err(if sign_in.user.as_deref().is_some_and(|u| !u.trim().is_empty()) {
            OpenError::NeedsPassword
        } else {
            OpenError::NeedsUser
        });
    };
    let numbers = Arc::new(Numbers::default());
    let began = Instant::now();
    let stream = TcpStream::connect_timeout(&addr, timeout)?;
    // How long the computer took to answer the knock: the link's delay.
    let answered = began.elapsed();
    numbers.set_delay(answered.as_millis() as u32);
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let connection = stream.try_clone()?;
    let local = stream.local_addr()?;
    let counted = Counted {
        stream,
        numbers: numbers.clone(),
    };

    // Up to the point where both sides switch to TLS.
    let mut framed = Framed::new(counted);
    // With the channels for the clipboard and for asking for a new size.
    let (clipboard, clip) = clip::Backend::new();
    // And, unless the older picture was asked for, with the channel the
    // newer one comes through: H.264 decoded by OpenH264, the text and
    // picture codecs beside it, each frame acknowledged so that the
    // computer sends as fast as this side keeps up.
    let newer_picture = !settings.older_picture;
    let mut dynamic = DrdynvcClient::new().with_dynamic_channel(DisplayControlClient::new(|_| Ok(Vec::new())));
    if newer_picture {
        let decoder = OpenH264Decoder::new().ok().map(|d| Box::new(d) as Box<dyn H264Decoder>);
        let watching = Watching {
            numbers: numbers.clone(),
            agreed: String::new(),
            areas: Vec::new(),
            frames: 0,
        };
        dynamic = dynamic.with_dynamic_channel(GraphicsPipelineClient::new(Box::new(watching), decoder));
    } else {
        numbers.set_via("older picture".into());
    }
    // The computer's sound, unless it is not to be played here. Windows
    // since 8 sends it through a dynamic channel where the viewer has
    // one, and only otherwise through the older channel of its own: both
    // are there, each with a player that starts when sound comes to it.
    let with_sound = !settings.mute;
    let quiet = Arc::new(AtomicBool::new(false));
    // A link that answered within a fiftieth of a second carries the
    // sound as it is; a slower one is left to the computer's own choice.
    let fast = answered <= Duration::from_millis(20);
    // How long each block of sound was held here before it was said to
    // be played: the computer keeps sound and picture together by it.
    let clock = || -> ironrdp_rdpsnd::client::Clock {
        let start = Instant::now();
        Box::new(move || start.elapsed())
    };
    if with_sound {
        let newer =
            RdpsndDvcState::new(Box::new(sound::Sound::new("newer channel", fast, quiet.clone(), numbers.clone())))
                .with_clock(clock());
        dynamic = dynamic.with_dynamic_channel(newer.channel()).with_dynamic_channel(newer.lossy_channel());
    }
    let mut connector =
        ClientConnector::new(config(user.trim(), password, size, scale, newer_picture, with_sound)?, local)
            .with_static_channel(CliprdrClient::new(Box::new(clipboard)))
            .with_static_channel(dynamic);
    if with_sound {
        // Windows starts its sound only for a viewer that also offers the
        // channel for redirected devices (drives, printers), and that
        // channel in turn only with the sound's: here it carries no
        // device at all.
        connector = connector
            .with_static_channel(
                Rdpsnd::new(Box::new(sound::Sound::new("older channel", fast, quiet.clone(), numbers.clone())))
                    .with_clock(clock()),
            )
            .with_static_channel(Rdpdr::new(Box::new(NoopRdpdrBackend), "Kagaz".into()));
    }
    let should_upgrade = ironrdp_blocking::connect_begin(&mut framed, &mut connector).map_err(refusal)?;

    // TLS: the computer shows its certificate. It is compared with the
    // one remembered before anything of the sign-in is sent.
    let tls = reach::any_certificate_tls().ok_or_else(|| OpenError::Other("TLS could not be set up".into()))?;
    let name = rustls::pki_types::ServerName::IpAddress(addr.ip().into());
    let client = rustls::ClientConnection::new(tls, name).map_err(|e| OpenError::Other(e.to_string()))?;
    let mut stream = rustls::StreamOwned::new(client, framed.into_inner_no_leftover());
    stream.flush()?;
    let der = stream
        .conn
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.as_ref().to_vec())
        .ok_or_else(|| OpenError::Other("it showed no certificate".into()))?;
    let identity = reach::identity_of(&der);
    if let (Some(known), Some(shown), false) = (&trust.known, &identity.certificate, trust.accept_new) {
        if known != shown {
            return Err(OpenError::CertificateChanged);
        }
    }
    // The sign-in is tied to the key in that certificate, so nobody in
    // between can pass it on to the real computer.
    let public_key = {
        use x509_cert::der::Decode as _;
        let cert = x509_cert::Certificate::from_der(&der)
            .map_err(|_| OpenError::Other("its certificate could not be read".into()))?;
        cert.tbs_certificate
            .subject_public_key_info
            .subject_public_key
            .as_bytes()
            .map(<[u8]>::to_vec)
            .ok_or_else(|| OpenError::Other("its certificate has no key Kagaz can read".into()))?
    };
    let upgraded = ironrdp_blocking::mark_as_upgraded(should_upgrade, &mut connector);
    let mut framed = Framed::new(stream);
    let outcome = ironrdp_blocking::connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut NoDirectory,
        ServerName::new(addr.ip().to_string()),
        public_key,
        None,
    )
    .map_err(refusal)?;
    let result = match outcome {
        ConnectionOutcome::Connected(result) => result,
        ConnectionOutcome::Redirected(_) => {
            return Err(OpenError::Unsupported(
                "it sends the session on to another computer, which Kagaz does not follow yet".into(),
            ))
        }
    };

    let size = result.desktop_size;
    let screen = Arc::new(Screen::new());
    screen.resize(size.width, size.height);
    let activation = result.activation_factory;
    let stage = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: result.pointer_software_rendering,
    }
    .build();
    connection.set_read_timeout(Some(READ_SLICE))?;
    let (waiter, wake) = waker::pair()?;
    let (tx, rx) = mpsc::channel();
    let running = Running {
        framed,
        connection,
        stage,
        image: DecodedImage::new(PixelFormat::RgbA32, size.width, size.height),
        activation,
        screen: screen.clone(),
        numbers: numbers.clone(),
        swapped: settings.swapped_buttons,
        fits,
        scale,
        wanted: None,
        buttons: 0,
        drew: false,
        clip,
        here: Here::Nothing,
        here_said: false,
        clip_last: None,
        clip_last_picture: None,
        their_text: None,
        kept: HashMap::new(),
        told: None,
        quiet,
        held: HashMap::new(),
    };
    {
        let screen = screen.clone();
        std::thread::Builder::new()
            .name("rdp".into())
            .spawn(move || {
                let why = run(running, rx, waiter);
                screen.post(Event::Ended(why));
            })
            .map_err(|e| OpenError::Other(e.to_string()))?;
    }
    Ok(Session {
        info: Info {
            speaks: Speaks::Rdp,
            name: identity.name.clone().unwrap_or_default(),
            encrypted: true,
            sound: with_sound,
            files: true,
            monitors: false,
            resizes: fits,
            picture_setting: false,
            identity: Some(identity),
        },
        screen,
        numbers,
        input: InputSender::new(tx, Some(wake)),
    })
}

#[cfg(test)]
mod tests;
