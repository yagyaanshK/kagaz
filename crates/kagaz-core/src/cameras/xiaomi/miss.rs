//! "miss", Xiaomi's camera streaming since 2020, on top of a peer-to-peer
//! transport ([`super::cs2`]): the app signs in with a key pair whose public
//! half Xiaomi's cloud has signed, both sides derive the same key (X25519
//! and HSalsa20, as NaCl's box does), and every command and media piece is
//! ChaCha20-encrypted under it with its own 8-byte nonce in front.

use super::cs2;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use std::time::Duration;

pub const CODEC_H264: u32 = 4;
pub const CODEC_H265: u32 = 5;
pub const CODEC_PCM: u32 = 1024;
pub const CODEC_PCMU: u32 = 1026;
pub const CODEC_PCMA: u32 = 1027;
pub const CODEC_OPUS: u32 = 1032;

const CMD_AUTH: u32 = 0x100;
const CMD_VIDEO_START: u32 = 0x102;
const CMD_VIDEO_STOP: u32 = 0x103;
const CMD_SPEAKER_START: u32 = 0x106;
const CMD_PLAYBACK: u32 = 0x10D;
const CMD_PLAYBACK_ANSWER: u32 = 0x10E;
const CMD_PLAYBACK_SPEED: u32 = 0x10F;
const CMD_ENCODED: u32 = 0x1001;

/// This app's key pair for one connection.
pub struct Keys {
    secret: x25519_dalek::StaticSecret,
    pub public: [u8; 32],
}

impl Keys {
    pub fn new() -> Keys {
        let mut seed = [0u8; 32];
        seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let secret = x25519_dalek::StaticSecret::from(seed);
        let public = x25519_dalek::PublicKey::from(&secret).to_bytes();
        Keys { secret, public }
    }

    pub fn public_hex(&self) -> String {
        hex(&self.public)
    }

    /// The key both sides share: NaCl's `box` precomputation.
    pub fn shared(&self, device_public: &[u8; 32]) -> [u8; 32] {
        let dh = self.secret.diffie_hellman(&x25519_dalek::PublicKey::from(*device_public));
        let out = salsa20::hsalsa::<salsa20::cipher::consts::U10>(dh.as_bytes().into(), &[0u8; 16].into());
        out.into()
    }
}

impl Default for Keys {
    fn default() -> Self {
        Keys::new()
    }
}

/// Encrypt with a fresh nonce, which goes in front.
pub fn encode(plain: &[u8], key: &[u8; 32]) -> Vec<u8> {
    let mut nonce = [0u8; 8];
    nonce.copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..8]);
    let mut out = nonce.to_vec();
    out.extend_from_slice(plain);
    apply(&mut out[8..], &nonce, key);
    out
}

/// Decrypt what [`encode`] made.
pub fn decode(sealed: &[u8], key: &[u8; 32]) -> Option<Vec<u8>> {
    let nonce: [u8; 8] = sealed.get(..8)?.try_into().ok()?;
    let mut out = sealed[8..].to_vec();
    apply(&mut out, &nonce, key);
    Some(out)
}

fn apply(data: &mut [u8], nonce8: &[u8; 8], key: &[u8; 32]) {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(nonce8);
    chacha20::ChaCha20::new(key.into(), &nonce.into()).apply_keystream(data);
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// One media piece from the camera.
#[derive(Debug, Clone)]
pub struct Packet {
    pub codec: u32,
    pub sequence: u32,
    pub flags: u32,
    /// Milliseconds, the camera's clock.
    pub timestamp: u64,
    pub payload: Vec<u8>,
}

/// A signed-in connection to a camera.
pub struct Client {
    conn: cs2::Conn,
    key: [u8; 32],
    model: String,
}

impl Client {
    /// Sign in over `conn` with this connection's keys: `sign` is Xiaomi's
    /// signature over our public key, `device_public` the camera's (hex).
    pub fn login(mut conn: cs2::Conn, keys: &Keys, device_public: &str, sign: &str, model: &str) -> Result<Client, String> {
        let device = unhex32(device_public).ok_or("Xiaomi sent no usable camera key")?;
        let key = keys.shared(&device);
        let auth = format!(
            r#"{{"public_key":"{}","sign":"{sign}","uuid":"","support_encrypt":0}}"#,
            keys.public_hex()
        );
        conn.write_command(CMD_AUTH, auth.as_bytes())?;
        let (_, answer) = conn.read_command(Duration::from_secs(5))?;
        if !String::from_utf8_lossy(&answer).contains(r#""result":"success""#) {
            return Err(format!("the camera refused the sign-in: {}", String::from_utf8_lossy(&answer)));
        }
        Ok(Client {
            conn,
            key,
            model: model.to_string(),
        })
    }

    pub fn protocol(&self) -> &'static str {
        self.conn.protocol()
    }

    fn command(&mut self, cmd: u32, body: &str) -> Result<(), String> {
        let mut plain = cmd.to_be_bytes().to_vec();
        plain.extend_from_slice(body.as_bytes());
        let sealed = encode(&plain, &self.key);
        self.conn.write_command(CMD_ENCODED, &sealed)
    }

    /// Start pictures and sound. `quality`: 0 lets the camera choose, 1 is
    /// its small stream, 2 its large one (3 on a few newer models).
    pub fn start(&mut self, quality: u8, audio: bool) -> Result<(), String> {
        let quality = match quality {
            2 if matches!(self.model.as_str(), "chuangmi.camera.046c04" | "chuangmi.camera.72ac1") => 3,
            q => q,
        };
        self.command(
            CMD_VIDEO_START,
            &format!(r#"{{"videoquality":{quality},"enableaudio":{}}}"#, audio as u8),
        )
    }

    /// Play the SD card from `start` to `end` (unix seconds) at `speed`
    /// (1 = real time), in place of the live view; the pieces then come in
    /// as they do live. Stopping is [`Client::stop`].
    pub fn playback(&mut self, start: i64, end: i64, speed: u32) -> Result<(), String> {
        self.command(
            CMD_PLAYBACK,
            &format!(
                r#"{{"sessionid":{start},"starttime":{start},"endtime":{end},"autoswitchtolive":0,"offset":0,"speed":{speed},"avchannelmerge":1}}"#
            ),
        )
    }

    /// Whether the card holds footage at `at` (unix seconds): the file's
    /// start and length when it does. The camera answers only when it
    /// finds one, so silence for `wait` means none.
    pub fn footage_at(&mut self, at: i64, wait: Duration) -> Result<Option<(i64, i64)>, String> {
        self.conn.drain_media();
        self.playback(at, at + 60, 1)?;
        let until = std::time::Instant::now() + wait;
        let mut found = None;
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match self.read_answer(left) {
                Ok((CMD_PLAYBACK_ANSWER, text)) => {
                    // The camera ends its answers with a zero byte.
                    let text = text.strip_suffix(&[0]).unwrap_or(&text);
                    let v: serde_json::Value = serde_json::from_slice(text).unwrap_or_default();
                    if v.get("status").and_then(|s| s.as_str()) == Some("filefound") {
                        let start = v.get("starttime").and_then(|s| s.as_i64()).unwrap_or(at);
                        let length = v.get("duration").and_then(|s| s.as_i64()).unwrap_or(60);
                        found = Some((start, length));
                    }
                    break;
                }
                Ok(_) => continue,
                Err(e) if e.contains("did not answer") => break,
                Err(e) => return Err(e),
            }
        }
        self.stop()?;
        self.conn.drain_media();
        Ok(found)
    }

    pub fn playback_speed(&mut self, speed: u32) -> Result<(), String> {
        self.command(CMD_PLAYBACK_SPEED, &format!(r#"{{"speed":{speed}}}"#))
    }

    /// Answers from the camera (to playback requests and the like), decrypted.
    pub fn read_answer(&self, wait: Duration) -> Result<(u32, Vec<u8>), String> {
        let (cmd, data) = self.conn.read_command(wait)?;
        if cmd == CMD_ENCODED {
            let plain = decode(&data, &self.key).ok_or("an answer too short to read")?;
            if plain.len() >= 4 {
                return Ok((u32::from_be_bytes(plain[..4].try_into().unwrap()), plain[4..].to_vec()));
            }
        }
        Ok((cmd, data))
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.command(CMD_VIDEO_STOP, "")
    }

    /// Ask for the camera's speaker (talk-back).
    pub fn start_speaker(&mut self) -> Result<(), String> {
        self.command(CMD_SPEAKER_START, "")
    }

    /// The codec the camera's speaker takes, when it is not the usual A-law.
    pub fn speaker_codec(&self) -> u32 {
        match self.model.as_str() {
            "isa.camera.df3" | "isa.camera.hlc6" => CODEC_PCM,
            "chuangmi.camera.72ac1" => CODEC_OPUS,
            _ => CODEC_PCMA,
        }
    }

    pub fn read(&self, wait: Duration) -> Result<Packet, String> {
        let (h, payload) = self.conn.read_packet(wait)?;
        let payload = decode(&payload, &self.key).ok_or("a media piece too short to read")?;
        let u32le = |i: usize| u32::from_le_bytes(h[i..i + 4].try_into().unwrap());
        Ok(Packet {
            codec: u32le(4),
            sequence: u32le(8),
            flags: u32le(12),
            timestamp: u64::from_le_bytes(h[16..24].try_into().unwrap()),
            payload,
        })
    }

    /// Send sound to the camera's speaker.
    pub fn write_audio(&mut self, codec: u32, payload: &[u8]) -> Result<(), String> {
        let sealed = encode(payload, &self.key);
        let mut header = [0u8; 32];
        header[..4].copy_from_slice(&(sealed.len() as u32).to_le_bytes());
        header[4..8].copy_from_slice(&codec.to_le_bytes());
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        header[16..24].copy_from_slice(&ms.to_le_bytes());
        self.conn.write_packet(&header, &sealed)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> [u8; 32] {
        unhex32(s).unwrap()
    }

    /// NaCl's own box test: Alice's secret key with Bob's public key
    /// precomputes to this shared key.
    #[test]
    fn the_shared_key_is_nacls_box_key() {
        let keys = Keys {
            secret: x25519_dalek::StaticSecret::from(h("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")),
            public: [0; 32],
        };
        let bob = h("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
        assert_eq!(hex(&keys.shared(&bob)), "1b27556473e985d462cd51197a9a46c76009549eac6474f206c4ee0844f68389");
    }

    /// RFC 8439's ChaCha20 vector (section 2.4.2), whose nonce starts with
    /// four zero bytes as these nonces do; counter 0 here, so the vector's
    /// counter-1 block is the second one.
    #[test]
    fn chacha20_matches_the_rfc() {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let nonce8 = [0, 0, 0, 0x4a, 0, 0, 0, 0];
        let mut data = vec![0u8; 64 + 16];
        apply(&mut data, &nonce8, &key);
        assert_eq!(hex(&data[64..80]), "224f51f3401bd9e12fde276fb8631ded");
    }

    #[test]
    fn what_is_encoded_decodes() {
        let key = [7u8; 32];
        let sealed = encode(b"\x00\x00\x01\x02{}", &key);
        assert_eq!(sealed.len(), 8 + 6);
        assert_eq!(decode(&sealed, &key).unwrap(), b"\x00\x00\x01\x02{}");
    }
}
