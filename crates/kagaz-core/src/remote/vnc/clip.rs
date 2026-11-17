//! Clipboard text over VNC. The old way carries Latin-1 only; the
//! extended clipboard (which a server and a viewer both have to offer)
//! carries any text, announced first and sent when asked for.
//!
//! An extended message is a set of flags and then data: which formats it
//! is about (Kagaz does text only) and what it does.

use flate2::read::{ZlibDecoder, ZlibEncoder};
use flate2::Compression;
use std::io::{self, Read};

pub const TEXT: u32 = 1;
const FORMATS: u32 = 0xffff;
pub const CAPS: u32 = 1 << 24;
pub const REQUEST: u32 = 1 << 25;
pub const PEEK: u32 = 1 << 26;
pub const NOTIFY: u32 = 1 << 27;
pub const PROVIDE: u32 = 1 << 28;

/// Text up to this size may be sent without being asked for.
pub const TEXT_MAX: u32 = 1 << 20;
/// No clipboard is larger than this, packed or not.
const MOST: u64 = 1 << 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// What the other side can do, and how much text it takes unasked.
    Caps { flags: u32, text_max: u32 },
    /// Send me these formats.
    Request(u32),
    /// Tell me what you have.
    Peek,
    /// I have these formats (none: the clipboard was emptied).
    Notify(u32),
    /// Here it is; none when the message held no text.
    Provide(Option<String>),
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

fn u32_at(data: &[u8], at: usize) -> io::Result<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| bad("a clipboard message cut short"))
}

pub fn parse(payload: &[u8]) -> io::Result<Message> {
    let flags = u32_at(payload, 0)?;
    if flags & CAPS != 0 {
        // One size for each format named, lowest first: text is first.
        let text_max = if flags & TEXT != 0 { u32_at(payload, 4)? } else { 0 };
        return Ok(Message::Caps { flags, text_max });
    }
    if flags & REQUEST != 0 {
        return Ok(Message::Request(flags & FORMATS));
    }
    if flags & PEEK != 0 {
        return Ok(Message::Peek);
    }
    if flags & NOTIFY != 0 {
        return Ok(Message::Notify(flags & FORMATS));
    }
    if flags & PROVIDE != 0 {
        if flags & TEXT == 0 {
            return Ok(Message::Provide(None));
        }
        let mut data = Vec::new();
        ZlibDecoder::new(&payload[4..])
            .take(MOST)
            .read_to_end(&mut data)
            .map_err(|_| bad("clipboard text that does not unpack"))?;
        let len = u32_at(&data, 0)? as usize;
        let text = data.get(4..4 + len).ok_or_else(|| bad("clipboard text cut short"))?;
        let text = text.strip_suffix(&[0]).unwrap_or(text);
        return Ok(Message::Provide(Some(String::from_utf8_lossy(text).replace("\r\n", "\n"))));
    }
    Err(bad("a clipboard message that does nothing"))
}

/// What Kagaz can do: text, in every direction.
pub fn caps() -> Vec<u8> {
    let mut out = (CAPS | TEXT | REQUEST | PEEK | NOTIFY | PROVIDE).to_be_bytes().to_vec();
    out.extend_from_slice(&TEXT_MAX.to_be_bytes());
    out
}

pub fn request() -> Vec<u8> {
    (REQUEST | TEXT).to_be_bytes().to_vec()
}

pub fn notify(has_text: bool) -> Vec<u8> {
    (NOTIFY | if has_text { TEXT } else { 0 }).to_be_bytes().to_vec()
}

/// Text as it travels: lines end in CR LF, and a zero byte ends it.
fn wire_text(text: &str) -> Vec<u8> {
    let mut out = text.replace("\r\n", "\n").replace('\n', "\r\n").into_bytes();
    out.push(0);
    out
}

/// How many bytes `text` is on the wire, to compare with what the other
/// side takes unasked.
pub fn wire_len(text: &str) -> usize {
    wire_text(text).len()
}

pub fn provide(text: &str) -> Vec<u8> {
    let wire = wire_text(text);
    let mut plain = (wire.len() as u32).to_be_bytes().to_vec();
    plain.extend(wire);
    let mut out = (PROVIDE | TEXT).to_be_bytes().to_vec();
    ZlibEncoder::new(&plain[..], Compression::default())
        .read_to_end(&mut out)
        .expect("packing from memory does not fail");
    out
}

/// The old way: each byte is one character.
pub fn from_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect::<String>().replace("\r\n", "\n")
}

/// The old way back: what Latin-1 has no letter for becomes "?".
pub fn to_latin1(text: &str) -> Vec<u8> {
    text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_comes_back_the_same() {
        for text in ["", "hello", "नमस्ते, दुनिया", "two\nlines\n", "a\r\nb", &"long ".repeat(5000)] {
            let want = text.replace("\r\n", "\n");
            assert_eq!(parse(&provide(text)).unwrap(), Message::Provide(Some(want)));
        }
    }

    #[test]
    fn lines_travel_with_cr_lf_and_a_zero_at_the_end() {
        assert_eq!(wire_text("a\nb"), b"a\r\nb\0");
        assert_eq!(wire_text("a\r\nb"), b"a\r\nb\0");
        assert_eq!(wire_len("a\nb"), 5);
    }

    #[test]
    fn the_small_messages() {
        assert_eq!(parse(&request()).unwrap(), Message::Request(TEXT));
        assert_eq!(parse(&notify(true)).unwrap(), Message::Notify(TEXT));
        assert_eq!(parse(&notify(false)).unwrap(), Message::Notify(0));
        let Message::Caps { flags, text_max } = parse(&caps()).unwrap() else { panic!() };
        assert_eq!(text_max, TEXT_MAX);
        assert!(flags & PROVIDE != 0 && flags & NOTIFY != 0 && flags & TEXT != 0);
        assert_eq!(parse(&PEEK.to_be_bytes()).unwrap(), Message::Peek);
    }

    #[test]
    fn broken_messages_are_errors() {
        assert!(parse(&[]).is_err());
        assert!(parse(&[0, 0, 0, 0]).is_err());
        let mut junk = (PROVIDE | TEXT).to_be_bytes().to_vec();
        junk.extend_from_slice(b"not zlib");
        assert!(parse(&junk).is_err());
        // Caps naming text but giving no size.
        assert!(parse(&(CAPS | TEXT).to_be_bytes()).is_err());
    }

    #[test]
    fn latin1_keeps_what_it_can() {
        assert_eq!(from_latin1(&[b'c', 0xe9, b'!']), "cé!");
        assert_eq!(to_latin1("cé! नम"), vec![b'c', 0xe9, b'!', b' ', b'?', b'?']);
    }
}
