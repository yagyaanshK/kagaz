//! Ring sounds. Kagaz makes its own calm tones (synthesised here, so there
//! is nothing to license), and the person can add their own audio files.
//! A tone is chosen per kind of ring (a door station, a family member) and,
//! optionally, per device. The page plays the tone while the window is
//! open; on a phone the ring notification plays the same file.
//!
//! Settings: `<config>/ringtones.json`. Own files: `<config>/ringtones/`.
//! Made tones: `<cache>/ringtones/<id>.wav`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f32::consts::PI;
use std::path::PathBuf;

const RATE: u32 = 22_050;
const MAX_FILE: usize = 15 * 1024 * 1024;

/// The tones Kagaz makes: (id, name).
pub const BUILT_IN: &[(&str, &str)] = &[
    ("soft-chime", "Soft chime"),
    ("morning", "Morning marimba"),
    ("wind-bells", "Wind bells"),
    ("harp", "Harp"),
    ("lullaby", "Lullaby"),
    ("classic", "Classic ding-dong"),
];
pub const DEFAULT: &str = "soft-chime";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Custom {
    pub id: String,
    pub name: String,
    /// The file's name inside `<config>/ringtones/`.
    pub file: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Per kind of ring: "doorbell" (door stations, monitors, guard
    /// phones), "family" (people on the home's account), "local" (a door
    /// station ringing this device directly on the network).
    pub events: BTreeMap<String, String>,
    /// Per device, by its name in lower case; overrides the kind.
    pub devices: BTreeMap<String, String>,
    /// 0 to 1.
    pub volume: f32,
    pub custom: Vec<Custom>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            events: ["doorbell", "family", "local"]
                .into_iter()
                .map(|k| (k.to_string(), DEFAULT.to_string()))
                .collect(),
            devices: BTreeMap::new(),
            volume: 0.8,
            custom: Vec::new(),
        }
    }
}

fn settings_path() -> PathBuf {
    kagaz_core::paths::config_dir().join("ringtones.json")
}

fn own_dir() -> PathBuf {
    kagaz_core::paths::config_dir().join("ringtones")
}

pub fn load() -> Settings {
    std::fs::read(settings_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(s: &Settings) -> Result<(), String> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn exists(s: &Settings, id: &str) -> bool {
    BUILT_IN.iter().any(|(b, _)| *b == id) || s.custom.iter().any(|c| c.id == id)
}

/// The tone for a ring from `peer` (a device's name) of `kind`.
pub fn tone_for(s: &Settings, peer: &str, kind: &str) -> String {
    let device = s.devices.get(&peer.trim().to_lowercase());
    let event = s.events.get(kind);
    device
        .or(event)
        .filter(|id| exists(s, id))
        .cloned()
        .unwrap_or_else(|| DEFAULT.to_string())
}

/// The kind of ring for a call device's kind ("door", "indoor", "apps"…).
pub fn kind_of(device_kind: &str) -> &'static str {
    match device_kind {
        "apps" | "family" => "family",
        _ => "doorbell",
    }
}

/// A playable file for a tone: an own file as it is, a made tone written
/// to the cache once.
pub fn file_for(id: &str) -> Result<PathBuf, String> {
    let s = load();
    if let Some(c) = s.custom.iter().find(|c| c.id == id) {
        return Ok(own_dir().join(&c.file));
    }
    if !BUILT_IN.iter().any(|(b, _)| *b == id) {
        return file_for(DEFAULT);
    }
    let dir = kagaz_core::paths::cache_dir().join("ringtones");
    let path = dir.join(format!("{id}-v1.wav"));
    if !path.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, wav(&make(id))).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

// ---------- making tones ----------

/// A bell-like note: a few inharmonic partials with their own decays.
fn bell(out: &mut [f32], at: f32, freq: f32, gain: f32, length: f32) {
    let partials = [(1.0, 1.0, 1.0), (2.0, 0.45, 1.6), (2.76, 0.25, 2.4), (5.4, 0.08, 4.0)];
    note(out, at, length, |t| {
        partials
            .iter()
            .map(|(ratio, amp, fall)| {
                amp * (2.0 * PI * freq * ratio * t).sin() * (-t * 2.2 * fall / length).exp()
            })
            .sum::<f32>()
            * gain
    });
}

/// A marimba-like note: a fundamental and a quick fourth partial.
fn mallet(out: &mut [f32], at: f32, freq: f32, gain: f32) {
    note(out, at, 1.2, |t| {
        ((2.0 * PI * freq * t).sin() * (-t * 4.0).exp()
            + 0.3 * (2.0 * PI * freq * 4.0 * t).sin() * (-t * 14.0).exp())
            * gain
    });
}

/// A soft sine with a slow start, as a hummed note.
fn hum(out: &mut [f32], at: f32, freq: f32, gain: f32, length: f32) {
    note(out, at, length, |t| {
        let attack = (t / 0.25).min(1.0);
        let release = ((length - t) / 0.4).clamp(0.0, 1.0);
        ((2.0 * PI * freq * t).sin() + 0.15 * (2.0 * PI * freq * 2.0 * t).sin())
            * attack
            * release
            * gain
    });
}

/// A plucked string (Karplus-Strong), deterministic.
fn pluck(out: &mut [f32], at: f32, freq: f32, gain: f32) {
    let period = (RATE as f32 / freq).round().max(2.0) as usize;
    let mut seed: u32 = (freq as u32).wrapping_mul(2_654_435_761);
    let mut line: Vec<f32> = (0..period)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect();
    let start = (at * RATE as f32) as usize;
    let length = (2.5 * RATE as f32) as usize;
    for i in 0..length {
        let Some(slot) = out.get_mut(start + i) else { break };
        let j = i % period;
        let next = (j + 1) % period;
        let value = line[j];
        line[j] = 0.996 * 0.5 * (line[j] + line[next]);
        *slot += value * gain;
    }
}

fn note(out: &mut [f32], at: f32, length: f32, f: impl Fn(f32) -> f32) {
    let start = (at * RATE as f32) as usize;
    let count = (length * RATE as f32) as usize;
    for i in 0..count {
        let Some(slot) = out.get_mut(start + i) else { break };
        *slot += f(i as f32 / RATE as f32);
    }
}

/// The samples of a made tone: a few seconds that loop well.
fn make(id: &str) -> Vec<f32> {
    let seconds = match id {
        "lullaby" => 6.0,
        "wind-bells" => 5.0,
        _ => 4.0,
    };
    let mut out = vec![0.0f32; (seconds * RATE as f32) as usize];
    match id {
        "morning" => {
            for (i, f) in [523.25, 659.25, 783.99, 1046.5, 783.99, 659.25].iter().enumerate() {
                mallet(&mut out, i as f32 * 0.22, *f, 0.5);
            }
        }
        "wind-bells" => {
            // A pentatonic scatter, the same every time.
            let notes = [1174.7, 1318.5, 1568.0, 1760.0, 2093.0, 1568.0, 1318.5, 1760.0];
            let times = [0.0, 0.35, 0.6, 1.05, 1.4, 1.9, 2.25, 2.7];
            for (f, t) in notes.iter().zip(times) {
                bell(&mut out, t, *f, 0.22, 2.2);
            }
        }
        "harp" => {
            for (i, f) in [392.0, 493.88, 587.33, 783.99, 987.77].iter().enumerate() {
                pluck(&mut out, i as f32 * 0.16, *f, 0.45);
            }
        }
        "lullaby" => {
            for (f, t, l) in [
                (659.25, 0.0, 0.9),
                (587.33, 0.9, 0.9),
                (523.25, 1.8, 1.4),
                (587.33, 3.3, 0.9),
                (523.25, 4.2, 1.4),
            ] {
                hum(&mut out, t, f, 0.35, l);
            }
        }
        "classic" => {
            bell(&mut out, 0.0, 783.99, 0.45, 1.2);
            bell(&mut out, 0.5, 622.25, 0.45, 2.0);
        }
        _ => {
            // Soft chime: two warm bells, a fifth apart.
            bell(&mut out, 0.0, 659.25, 0.4, 2.4);
            bell(&mut out, 0.45, 440.0, 0.4, 3.0);
        }
    }
    // Keep it calm: peak at about -6 dB, and fade out the last moment so
    // the loop does not click.
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let fade = (0.05 * RATE as f32) as usize;
    let n = out.len();
    for (i, v) in out.iter_mut().enumerate() {
        *v *= 0.5 / peak;
        if i + fade > n {
            *v *= (n - i) as f32 / fade as f32;
        }
    }
    out
}

/// 16-bit mono WAV.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    b
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct Tone {
    id: String,
    name: String,
    own: bool,
}

#[derive(Serialize)]
pub struct Listing {
    tones: Vec<Tone>,
    settings: Settings,
}

fn listing() -> Listing {
    let settings = load();
    let mut tones: Vec<Tone> = BUILT_IN
        .iter()
        .map(|(id, name)| Tone {
            id: id.to_string(),
            name: name.to_string(),
            own: false,
        })
        .collect();
    tones.extend(settings.custom.iter().map(|c| Tone {
        id: c.id.clone(),
        name: c.name.clone(),
        own: true,
    }));
    Listing { tones, settings }
}

#[tauri::command]
pub fn ringtones_list() -> Listing {
    listing()
}

/// Choose tones: `events` and `devices` as in [`Settings`], and the volume.
#[tauri::command]
pub fn ringtones_set(
    events: BTreeMap<String, String>,
    devices: BTreeMap<String, String>,
    volume: f32,
) -> Result<Listing, String> {
    let mut s = load();
    s.events = events
        .into_iter()
        .filter(|(_, id)| exists(&s, id))
        .collect();
    s.devices = devices
        .into_iter()
        .filter(|(_, id)| exists(&s, id))
        .map(|(k, v)| (k.trim().to_lowercase(), v))
        .collect();
    s.volume = volume.clamp(0.0, 1.0);
    save(&s)?;
    Ok(listing())
}

/// The sound of a tone, as its file's bytes.
#[tauri::command]
pub async fn ringtone_audio(id: String) -> Result<tauri::ipc::Response, String> {
    super::blocking(move || {
        let path = file_for(&id)?;
        std::fs::read(path)
            .map(tauri::ipc::Response::new)
            .map_err(|e| format!("could not read the tone: {e}"))
    })
    .await
}

/// Keep an audio file of the person's own as a tone. `data` is base64.
#[tauri::command]
pub async fn ringtone_add(name: String, file_name: String, data: String) -> Result<Listing, String> {
    use base64::Engine;
    super::blocking(move || {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|_| "the file could not be read".to_string())?;
        if bytes.is_empty() || bytes.len() > MAX_FILE {
            return Err("Choose an audio file under 15 MB.".into());
        }
        let ext = std::path::Path::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .filter(|e| matches!(e.as_str(), "mp3" | "m4a" | "aac" | "ogg" | "oga" | "opus" | "wav" | "flac" | "webm"))
            .ok_or("Kagaz plays MP3, M4A, AAC, OGG, Opus, WAV and FLAC files.")?;
        let id = format!("own-{}", uuid::Uuid::new_v4().simple());
        let file = format!("{id}.{ext}");
        std::fs::create_dir_all(own_dir()).map_err(|e| e.to_string())?;
        std::fs::write(own_dir().join(&file), &bytes).map_err(|e| e.to_string())?;
        let mut s = load();
        let name = name.trim();
        s.custom.push(Custom {
            id,
            name: if name.is_empty() { "My tone".into() } else { name.chars().take(60).collect() },
            file,
        });
        save(&s)?;
        Ok(listing())
    })
    .await
}

#[tauri::command]
pub fn ringtone_remove(id: String) -> Result<Listing, String> {
    let mut s = load();
    if let Some(i) = s.custom.iter().position(|c| c.id == id) {
        let c = s.custom.remove(i);
        let _ = std::fs::remove_file(own_dir().join(c.file));
        s.events.retain(|_, v| *v != id);
        s.devices.retain(|_, v| *v != id);
        save(&s)?;
    }
    Ok(listing())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_tones_are_calm_and_loop_cleanly() {
        for (id, _) in BUILT_IN {
            let s = make(id);
            assert!(s.len() > RATE as usize * 3, "{id} too short");
            let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((0.45..=0.51).contains(&peak), "{id} peak {peak}");
            assert!(s.last().unwrap().abs() < 0.01, "{id} ends with a click");
        }
        let w = wav(&make("classic"));
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + make("classic").len() * 2);
    }

    #[test]
    fn device_tone_overrides_the_kind() {
        let mut s = Settings::default();
        s.events.insert("family".into(), "harp".into());
        s.devices.insert("front gate".into(), "morning".into());
        assert_eq!(tone_for(&s, "Front Gate ", "doorbell"), "morning");
        assert_eq!(tone_for(&s, "Lobby", "doorbell"), DEFAULT);
        assert_eq!(tone_for(&s, "Asha", "family"), "harp");
        s.devices.insert("lobby".into(), "gone".into());
        assert_eq!(tone_for(&s, "Lobby", "doorbell"), DEFAULT);
    }
}
