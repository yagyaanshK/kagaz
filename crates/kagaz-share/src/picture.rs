//! The screen as pictures: where they come from, the latest one kept for
//! whoever sends next, and H.264 through the graphics pipeline.

use ironrdp_egfx::pdu::{Avc420Region, CapabilitiesAdvertisePdu, CapabilitySet};
use ironrdp_egfx::server::{GraphicsPipelineHandler, GraphicsPipelineServer};
use ironrdp_server::{EgfxServerMessage, GfxDvcBridge, GfxServerFactory, GfxServerHandle, ServerEvent, ServerEventSender};
use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, QpRange, RateControlMode, UsageType};
use openh264::formats::YUVBuffer;
use openh264::OpenH264API;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

use crate::say;

/// One picture of the screen: blue, green, red and a fourth byte that is
/// not used, row after row with no gaps.
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub bgra: Vec<u8>,
}

/// Where pictures come from: this computer's screen, or the test card.
pub trait Source: Send {
    /// What it is, for the log.
    fn name(&self) -> String;
    /// The next picture, waiting at most `wait` for one; `None` when
    /// nothing changed.
    fn next(&mut self, wait: Duration) -> anyhow::Result<Option<Frame>>;
}

/// The latest picture, numbered, and what is shared about the session.
#[derive(Clone, Default)]
pub struct Latest {
    pub frame: Arc<Mutex<Option<(u64, Arc<Frame>)>>>,
    /// Viewers connected now: pictures are taken only while one is.
    pub viewers: Arc<AtomicUsize>,
    /// The screen's size, as the latest picture has it.
    pub size: Arc<Mutex<(u16, u16)>>,
    /// The graphics pipeline is in use: no plain bitmaps then.
    pub pipeline: Arc<AtomicBool>,
}

impl Latest {
    pub fn get(&self) -> Option<(u64, Arc<Frame>)> {
        self.frame.lock().unwrap().clone()
    }
}

/// Takes pictures while a viewer is connected, at most `rate` a second.
/// A source that fails is opened again after a pause (the screen goes
/// away for a moment when Windows changes its mode or shows its secure
/// desktop).
pub fn take(mut open: impl FnMut() -> anyhow::Result<Box<dyn Source>> + Send + 'static, latest: Latest, rate: u32) {
    std::thread::spawn(move || {
        let each = Duration::from_micros(1_000_000 / rate.max(1) as u64);
        let mut source: Option<Box<dyn Source>> = None;
        let mut number = 0u64;
        let mut said = String::new();
        // A failure is said once, not every second it lasts.
        let mut failed = String::new();
        loop {
            if latest.viewers.load(Ordering::Acquire) == 0 {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            let Some(from) = source.as_mut() else {
                match open() {
                    Ok(s) => {
                        let name = s.name();
                        if name != said {
                            say(&format!("pictures from {name}"));
                            said = name;
                        }
                        source = Some(s);
                    }
                    Err(e) => {
                        let line = format!("the screen could not be read: {e:#}");
                        if line != failed {
                            say(&line);
                            failed = line;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                continue;
            };
            let began = Instant::now();
            match from.next(each) {
                Ok(Some(frame)) => {
                    failed.clear();
                    number += 1;
                    *latest.size.lock().unwrap() = (frame.width, frame.height);
                    *latest.frame.lock().unwrap() = Some((number, Arc::new(frame)));
                }
                Ok(None) => {}
                Err(e) => {
                    let line = format!("the screen was lost for a moment: {e:#}");
                    if line != failed {
                        say(&line);
                        failed = line;
                    }
                    source = None;
                    std::thread::sleep(Duration::from_millis(300));
                    continue;
                }
            }
            std::thread::sleep(each.saturating_sub(began.elapsed()));
        }
    });
}

/// A picture's colours as H.264 carries them over Remote Desktop: full
/// range, by the HD (BT.709) weights, colour at half the size each way.
/// A picture of odd size gets its last row or column once more.
pub fn planes(frame: &Frame) -> YUVBuffer {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (ew, eh) = (w + (w & 1), h + (h & 1));
    let px = &frame.bgra;
    let at = |x: usize, y: usize| -> (i32, i32, i32) {
        let i = (y.min(h - 1) * w + x.min(w - 1)) * 4;
        (px[i + 2] as i32, px[i + 1] as i32, px[i] as i32)
    };
    let mut out = vec![0u8; ew * eh * 3 / 2];
    let (luma, chroma) = out.split_at_mut(ew * eh);
    let (cb, cr) = chroma.split_at_mut(ew * eh / 4);
    // Weights times 256: 0.2126, 0.7152, 0.0722; 1 / 1.8556 and 1 / 1.5748.
    let y_of = |(r, g, b): (i32, i32, i32)| (54 * r + 183 * g + 19 * b + 128) >> 8;
    for y in 0..eh / 2 {
        for x in 0..ew / 2 {
            let (mut sr, mut sg, mut sb) = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (cx, cy) = (x * 2 + dx, y * 2 + dy);
                let c = at(cx, cy);
                luma[cy * ew + cx] = y_of(c).clamp(0, 255) as u8;
                sr += c.0;
                sg += c.1;
                sb += c.2;
            }
            let (r, g, b) = ((sr + 2) >> 2, (sg + 2) >> 2, (sb + 2) >> 2);
            let l = y_of((r, g, b));
            cb[y * (ew / 2) + x] = (((b - l) * 138 + 128) / 256 + 128).clamp(0, 255) as u8;
            cr[y * (ew / 2) + x] = (((r - l) * 163 + 128) / 256 + 128).clamp(0, 255) as u8;
        }
    }
    YUVBuffer::from_vec(out, ew, eh)
}

/// The graphics pipeline: the way to it once a viewer has opened it, and
/// the way to the viewer.
#[derive(Clone, Default)]
pub struct Pipeline {
    handle: Arc<Mutex<Option<GfxServerHandle>>>,
    to_viewer: Arc<Mutex<Option<UnboundedSender<ServerEvent>>>>,
}

struct PipelineEvents;

impl GraphicsPipelineHandler for PipelineEvents {
    fn capabilities_advertise(&mut self, _pdu: &CapabilitiesAdvertisePdu) {}
    fn on_ready(&mut self, negotiated: &CapabilitySet) {
        let name = format!("{negotiated:?}");
        say(&format!("graphics pipeline ready ({})", name.split([' ', '{']).next().unwrap_or("")));
    }
    fn on_frame_ack(&mut self, _frame_id: u32, _queue_depth: u32, _total_frames_decoded: u32) {}
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

/// The encoder at a steady quality: sharp text, and nothing skipped (a
/// skipped picture would leave the viewer showing an old screen until the
/// next change). A still screen costs next to nothing after its first
/// picture.
fn encoder(rate: u32) -> anyhow::Result<Encoder> {
    let config = EncoderConfig::new()
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Off)
        .qp(QpRange::new(14, 24))
        .bitrate(BitRate::from_bps(40_000_000))
        .max_frame_rate(FrameRate::from_hz(rate as f32))
        .adaptive_quantization(false)
        .background_detection(false)
        .skip_frames(false);
    Ok(Encoder::with_api_config(OpenH264API::from_source(), config)?)
}

/// Sends the latest picture as H.264 whenever there is a newer one and
/// the viewer has kept up, once it has opened the graphics pipeline.
/// Nothing is encoded that is not sent, so the viewer never misses a
/// picture the next one builds on.
pub fn pump(latest: Latest, pipeline: Pipeline, rate: u32) {
    std::thread::spawn(move || {
        let began = Instant::now();
        let mut seen: Option<GfxServerHandle> = None;
        let mut surface: Option<(u16, (u16, u16))> = None;
        let mut coder: Option<Encoder> = None;
        let mut sent = 0u64;
        let mut changed: Option<Instant> = None;
        let mut refused = false;
        // For the log: pictures, bytes and time spent encoding.
        let (mut count, mut bytes, mut spent, mut since) = (0u32, 0usize, Duration::ZERO, Instant::now());
        loop {
            std::thread::sleep(Duration::from_millis(4));
            let Some(handle) = pipeline.handle.lock().unwrap().clone() else { continue };
            if !seen.as_ref().is_some_and(|s| Arc::ptr_eq(s, &handle)) {
                seen = Some(handle.clone());
                (surface, coder, sent, changed, refused) = (None, None, 0, None, false);
                latest.pipeline.store(false, Ordering::Release);
            }
            let mut gfx = handle.lock().unwrap();
            if !gfx.is_ready() {
                continue;
            }
            if !gfx.supports_avc420() {
                if !refused {
                    say("the viewer takes no H.264: plain pictures instead");
                    refused = true;
                }
                continue;
            }
            latest.pipeline.store(true, Ordering::Release);
            let Some((number, frame)) = latest.get() else { continue };
            let size = (frame.width, frame.height);
            if surface.map(|(_, of)| of) != Some(size) {
                if surface.is_some() {
                    // The viewer starts the screen over after a change of
                    // size; the pipeline follows a moment later.
                    let at = *changed.get_or_insert_with(Instant::now);
                    if at.elapsed() < Duration::from_millis(400) {
                        continue;
                    }
                    gfx.resize(size.0, size.1);
                } else {
                    gfx.set_output_dimensions(size.0, size.1);
                }
                changed = None;
                let Some(id) = gfx.create_surface(size.0, size.1) else { continue };
                gfx.map_surface_to_output(id, 0, 0);
                surface = Some((id, size));
                coder = None;
                sent = 0;
            }
            let Some((id, _)) = surface else { continue };
            if number != sent && !gfx.should_backpressure() {
                let start = Instant::now();
                let made = match coder.as_mut() {
                    Some(c) => c,
                    None => match encoder(rate) {
                        Ok(c) => coder.insert(c),
                        Err(e) => {
                            say(&format!("the encoder did not start: {e:#}"));
                            std::thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                    },
                };
                let yuv = planes(&frame);
                let picture = match made.encode(&yuv) {
                    Ok(bits) => bits.to_vec(),
                    Err(e) => {
                        say(&format!("a picture could not be encoded: {e}"));
                        coder = None;
                        continue;
                    }
                };
                sent = number;
                if !picture.is_empty() {
                    let region = Avc420Region::full_frame(size.0, size.1, 22);
                    if gfx.send_avc420_frame(id, &picture, &[region], began.elapsed().as_millis() as u32).is_none() {
                        // Refused after all: the next picture starts afresh.
                        coder = None;
                    } else {
                        count += 1;
                        bytes += picture.len();
                        spent += start.elapsed();
                    }
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
                    Err(e) => say(&format!("the pipeline's messages could not be packed: {e}")),
                }
            }
            if since.elapsed() >= Duration::from_secs(5) {
                if count > 0 {
                    let secs = since.elapsed().as_secs_f64();
                    say(&format!(
                        "pictures: {:.1} a second, {} kbit/s, {:.1} ms to encode each, {}x{}",
                        count as f64 / secs,
                        (bytes as f64 * 8.0 / secs / 1000.0) as u64,
                        spent.as_secs_f64() * 1000.0 / count as f64,
                        size.0,
                        size.1
                    ));
                }
                (count, bytes, spent, since) = (0, 0, Duration::ZERO, Instant::now());
            }
        }
    });
}

/// The test card, moving `rate` ticks a second: no one's screen.
pub struct Card {
    pub width: u16,
    pub height: u16,
    pub tick: u32,
    pub last: Option<u32>,
    pub began: Instant,
    pub rate: u32,
}

impl Source for Card {
    fn name(&self) -> String {
        format!("the test card, {}x{}", self.width, self.height)
    }
    fn next(&mut self, wait: Duration) -> anyhow::Result<Option<Frame>> {
        let tick = if self.rate == 0 { 0 } else { (self.began.elapsed().as_millis() as u64 * self.rate as u64 / 1000) as u32 };
        if self.last == Some(tick) {
            std::thread::sleep(wait);
            return Ok(None);
        }
        self.last = Some(tick);
        self.tick = tick;
        let mut rgba = crate::pattern::card(self.width, self.height, tick);
        for p in rgba.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        Ok(Some(Frame { width: self.width, height: self.height, bgra: rgba }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_stays_grey_and_red_is_red() {
        let mut bgra = Vec::new();
        for i in 0..16 {
            // Left half mid-grey, right half pure red.
            let _ = i;
            bgra.extend_from_slice(&[128, 128, 128, 0, 128, 128, 128, 0, 0, 0, 255, 0, 0, 0, 255, 0]);
        }
        let frame = Frame { width: 4, height: 4, bgra };
        let yuv = planes(&frame);
        use openh264::formats::YUVSource;
        let (y, u, v) = (yuv.y(), yuv.u(), yuv.v());
        assert_eq!(y[0], 128);
        assert_eq!((u[0], v[0]), (128, 128));
        // Red: Y about 54, V well above the middle, U below.
        assert!((50..=58).contains(&y[2]), "{}", y[2]);
        assert!(v[1] > 230 && u[1] < 120, "{} {}", u[1], v[1]);
    }

    #[test]
    fn odd_sizes_are_made_even() {
        let frame = Frame { width: 3, height: 5, bgra: vec![200; 3 * 5 * 4] };
        let yuv = planes(&frame);
        use openh264::formats::YUVSource;
        assert_eq!(yuv.dimensions(), (4, 6));
    }
}
