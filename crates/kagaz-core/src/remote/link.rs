//! Kagaz on both ends: pairing a device with a computer that shares itself
//! (kagaz-share, docs/remote-connection.md step 2), and how the paired
//! device signs in after. Both programs build this same file.
//!
//! Pairing goes over the computer's own Remote Desktop port: a pairing
//! starts with `MAGIC`, which no Remote Desktop connection does. The code
//! shown on the computer goes into SPAKE2, so the two arrive at one key
//! only if they have the same code, and nobody listening learns the code
//! or can try codes after the fact. Each side proves it has the key; both
//! show three words made from it, and the people compare them. The
//! device's secret is made from the key on both sides and never sent: the
//! computer keeps only its hash.
//!
//! A paired device signs in to the computer's Remote Desktop with its id
//! there as the user and `password(secret, pin)` as the password, inside
//! TLS with the certificate pinned at pairing.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::io::{Read, Write};

/// What a pairing starts with.
pub const MAGIC: &[u8; 4] = b"KGZP";
/// This way of pairing; a computer refuses another.
pub const VERSION: u64 = 1;
/// How long a code is good for, in seconds (once).
pub const CODE_LASTS: u64 = 5 * 60;
/// The largest message either side sends.
const LARGEST: usize = 64 * 1024;

/// One message: its length, then JSON.
pub fn write_frame(to: &mut impl Write, value: &serde_json::Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    to.write_all(&(bytes.len() as u32).to_be_bytes())?;
    to.write_all(&bytes)?;
    to.flush()
}

pub fn read_frame(from: &mut impl Read) -> std::io::Result<serde_json::Value> {
    let mut len = [0u8; 4];
    from.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > LARGEST {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "a message too large"));
    }
    let mut bytes = vec![0u8; len];
    from.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

/// The code as both sides use it: its digits only.
pub fn digits(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// A half of the key exchange: what to keep, and what to send.
pub struct Half {
    state: Spake2<Ed25519Group>,
    pub message: Vec<u8>,
}

pub fn start(code: &str) -> Half {
    let (state, message) =
        Spake2::<Ed25519Group>::start_symmetric(&Password::new(digits(code).as_bytes()), &Identity::new(b"kagaz pairing 1"));
    Half { state, message }
}

/// The key both sides hold if their codes were the same (and a
/// different one otherwise: the proofs tell).
pub fn finish(half: Half, theirs: &[u8]) -> Option<Vec<u8>> {
    half.state.finish(theirs).ok()
}

/// Proof of the key over what was said, under a label for its purpose.
pub fn proof(key: &[u8], label: &str, said: &[&[u8]]) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("any key length");
    mac.update(label.as_bytes());
    for part in said {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part);
    }
    hex(&mac.finalize().into_bytes())
}

/// Do they match, without telling by how long it took?
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The three words both people compare.
pub fn words(key: &[u8]) -> String {
    let p = unhex(&proof(key, "words", &[])).unwrap_or_default();
    [p[0], p[1], p[2]].iter().map(|b| WORDS[*b as usize]).collect::<Vec<_>>().join(" · ")
}

/// The device's secret, made by both from the key.
pub fn device_secret(key: &[u8]) -> String {
    proof(key, "device secret", &[])
}

/// What the computer keeps of a secret or a PIN: its hash.
pub fn hash(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// The password a paired device signs in with; the PIN may be empty
/// (the person at the computer is asked then).
pub fn password(secret: &str, pin: &str) -> String {
    format!("{secret}:{}", digits(pin))
}

pub fn split_password(password: &str) -> (&str, &str) {
    password.split_once(':').unwrap_or((password, ""))
}

const WORDS: [&str; 256] = [
    "acorn", "amber", "anchor", "apple", "april", "arrow", "aspen", "atlas", "badge", "baker", "bamboo", "banjo", "basil",
    "beach", "berry", "bison", "blade", "bloom", "board", "bonus", "brave", "bread", "brick", "brook", "brush", "cabin",
    "cactus", "camel", "candle", "canoe", "canyon", "cargo", "carrot", "castle", "cedar", "chalk", "cherry", "chess",
    "chili", "cider", "cinema", "circle", "citrus", "clay", "cliff", "clock", "cloud", "clover", "cobalt", "cocoa",
    "comet", "coral", "cotton", "crane", "crayon", "cricket", "crown", "cube", "daisy", "dance", "delta", "denim",
    "desert", "dinner", "dolphin", "dragon", "dream", "drum", "eagle", "earth", "echo", "elbow", "elder", "ember",
    "emerald", "engine", "falcon", "feather", "fern", "fiddle", "field", "fig", "flame", "flute", "forest", "fossil",
    "fox", "frost", "garden", "garlic", "gecko", "giant", "ginger", "glacier", "globe", "gold", "grape", "gravel",
    "harbor", "harvest", "hazel", "heron", "honey", "horizon", "husky", "igloo", "indigo", "iris", "island", "ivory",
    "jacket", "jade", "jasmine", "jelly", "jungle", "kayak", "kettle", "kiwi", "koala", "ladder", "lagoon", "lantern",
    "lemon", "lily", "lime", "linen", "lizard", "llama", "lotus", "magnet", "mango", "maple", "marble", "meadow",
    "melon", "meteor", "mint", "mirror", "monsoon", "moss", "mountain", "mustard", "nectar", "needle", "nickel",
    "noodle", "nutmeg", "oasis", "ocean", "olive", "onion", "opal", "orange", "orbit", "orchid", "otter", "owl",
    "paddle", "panda", "paper", "parrot", "peach", "pebble", "pepper", "piano", "pillow", "pine", "planet", "plum",
    "pocket", "pollen", "pony", "poppy", "prism", "pumpkin", "puzzle", "quartz", "quill", "rabbit", "radish", "rain",
    "raven", "reef", "ribbon", "river", "robin", "rocket", "rose", "ruby", "saddle", "saffron", "sail", "salmon",
    "sand", "sapphire", "saturn", "scarf", "season", "seven", "shadow", "shell", "silver", "sky", "sloth", "snow",
    "sparrow", "spice", "spring", "spruce", "squash", "star", "stone", "storm", "sugar", "summer", "sunset", "swan",
    "tablet", "tango", "teapot", "thunder", "tiger", "timber", "tomato", "topaz", "tortoise", "tulip", "tundra",
    "turnip", "umbrella", "valley", "velvet", "violet", "volcano", "walnut", "walrus", "water", "whale", "wheat",
    "willow", "window", "winter", "wolf", "yarn", "yogurt", "zebra", "zinc", "acrobat", "bagel", "biscuit", "bubble",
    "button", "cello", "dune", "fable", "glow",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_code_makes_the_same_key_and_words() {
        let (a, b) = (start("482 917"), start("482917"));
        let (ma, mb) = (a.message.clone(), b.message.clone());
        let (ka, kb) = (finish(a, &mb).unwrap(), finish(b, &ma).unwrap());
        assert_eq!(ka, kb);
        assert_eq!(words(&ka), words(&kb));
        assert_eq!(device_secret(&ka), device_secret(&kb));
        assert_eq!(words(&ka).split(" · ").count(), 3);
    }

    #[test]
    fn another_code_makes_another_key() {
        let (a, b) = (start("482917"), start("482918"));
        let (ma, mb) = (a.message.clone(), b.message.clone());
        let (ka, kb) = (finish(a, &mb).unwrap(), finish(b, &ma).unwrap());
        assert_ne!(ka, kb);
        assert!(!same(&proof(&ka, "viewer", &[b"x"]), &proof(&kb, "viewer", &[b"x"])));
    }

    #[test]
    fn the_words_are_all_different() {
        let mut seen = std::collections::HashSet::new();
        for w in WORDS {
            assert!(seen.insert(w), "{w} twice");
        }
    }

    #[test]
    fn a_password_comes_apart_again() {
        let p = password("abc", "48 29 17");
        assert_eq!(split_password(&p), ("abc", "482917"));
        assert_eq!(split_password(&password("abc", "")), ("abc", ""));
        assert_eq!(unhex(&hex(&[0, 255, 16])).unwrap(), vec![0, 255, 16]);
    }
}
