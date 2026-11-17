//! The clipboard over Remote Desktop: text both ways, and files from
//! here to the computer. Neither side sends anything unasked: each says
//! what it has, and hands it over when the other asks, which a computer
//! does when something is pasted there. Kagaz asks at once when the
//! computer says it has text, so that what was copied there is on this
//! device's clipboard without a further step.
//!
//! Files offered from here ([`Offer`]) sit on the computer's clipboard
//! as names and sizes; when they are pasted there, the computer reads
//! each one piece by piece and Kagaz answers from the file on disk. The
//! computer sees the names, never where the files are kept here.
//!
//! The library's clipboard channel calls into [`Backend`] while the
//! session handles what arrived; the session then does what was
//! [`Asked`], in order.

use ironrdp_cliprdr::backend::CliprdrBackend;
use super::super::screen::Sending;
use ironrdp_cliprdr::pdu::{
    ClipboardFileAttributes, ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
    FileContentsFlags, FileContentsRequest, FileContentsResponse, FileDescriptor, FormatDataRequest,
    FormatDataResponse, LockDataId,
};
use ironrdp_core::impl_as_any;
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What the clipboard channel asks of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// The computer wants to know what is on the clipboard here.
    List,
    /// The computer says it has text: fetch it.
    Fetch,
    /// The computer wants what is here, in this format.
    Give(ClipboardFormatId),
    /// The computer's text arrived.
    Text(String),
    /// The computer says it has a picture: fetch it, in this format.
    FetchPicture(ClipboardFormatId),
    /// The computer's picture arrived, as PNG.
    Picture(Vec<u8>),
    /// Something new is here to offer, or the channel has just become
    /// ready to be told of it.
    Mine,
    /// The computer copied something of its own: what was offered from
    /// here is no longer on its clipboard.
    Theirs,
    /// The computer reads a piece of an offered file.
    Contents(FileContentsRequest),
    /// The computer wants the files offered now kept to be had under this
    /// number, whatever is copied next; and later, that it is done.
    Lock(u32),
    Unlock(u32),
}

/// What the session and the channel both look at.
#[derive(Debug, Default)]
pub struct Shared {
    pub asked: Mutex<VecDeque<Asked>>,
    /// The computer takes files over its clipboard (known once the two
    /// have said what each can do).
    pub files: AtomicBool,
}

impl Shared {
    pub fn ask(&self, what: Asked) {
        self.asked.lock().unwrap_or_else(|e| e.into_inner()).push_back(what);
    }

    pub fn next(&self) -> Option<Asked> {
        self.asked.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
    }
}

/// What was asked of the computer and is on its way, in the order asked:
/// its answers come back without saying what they answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Text,
    /// A picture as PNG, or in one of the two forms Windows keeps
    /// pictures in.
    Png,
    Dib,
    DibV5,
}

/// The number Kagaz gives PNG among the formats it offers (Windows goes
/// by the name; the number only has to be told apart here).
pub const PNG_HERE: ClipboardFormatId = ClipboardFormatId(0xC0F0);

/// The largest picture taken from the computer's clipboard.
const MOST_PICTURE: usize = 256 << 20;

/// What the computer's clipboard is told of a picture offered from here:
/// the form every Windows program reads, and PNG for those that keep
/// see-through parts.
pub fn picture_formats() -> [ClipboardFormat; 2] {
    [
        ClipboardFormat::new(ClipboardFormatId::CF_DIB),
        ClipboardFormat::new(PNG_HERE).with_name(ClipboardFormatName::new("PNG")),
    ]
}

/// A picture offered from here (PNG), in the format the computer asks
/// for; none for a format it was not offered in.
pub fn picture_as(png: &[u8], format: ClipboardFormatId) -> Option<Vec<u8>> {
    if format == PNG_HERE {
        Some(png.to_vec())
    } else if format == ClipboardFormatId::CF_DIB {
        ironrdp_cliprdr_format::bitmap::png_to_cf_dib(png).ok()
    } else if format == ClipboardFormatId::CF_DIBV5 {
        ironrdp_cliprdr_format::bitmap::png_to_cf_dibv5(png).ok()
    } else {
        None
    }
}

#[derive(Debug)]
pub struct Backend {
    shared: Arc<Shared>,
    wanted: VecDeque<Wanted>,
}

impl Backend {
    pub fn new() -> (Backend, Arc<Shared>) {
        let shared = Arc::new(Shared::default());
        let backend = Backend {
            shared: shared.clone(),
            wanted: VecDeque::new(),
        };
        (backend, shared)
    }

    fn ask(&self, what: Asked) {
        self.shared.ask(what);
    }
}

/// What Kagaz can do with files over the clipboard: hand them over piece
/// by piece, of any size, under names alone, and keep an offer to be had
/// while something else is copied.
const FILE_ABILITIES: ClipboardGeneralCapabilityFlags = ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
    .union(ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS)
    .union(ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED)
    .union(ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA);

impl_as_any!(Backend);

impl CliprdrBackend for Backend {
    // Files go from here to the computer only: nothing is ever written
    // here, so there is no folder for it.
    fn temporary_directory(&self) -> &str {
        ""
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        FILE_ABILITIES
    }

    fn on_ready(&mut self) {
        self.ask(Asked::Mine);
    }

    fn on_request_format_list(&mut self) {
        self.ask(Asked::List);
    }

    fn on_process_negotiated_capabilities(&mut self, capabilities: ClipboardGeneralCapabilityFlags) {
        let files = capabilities.contains(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED);
        self.shared.files.store(files, Ordering::Release);
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        // A list with files in it may be the computer saying back what
        // was offered from here; anything else is its own.
        let files = available_formats
            .iter()
            .any(|f| f.name().is_some_and(|n| n.value() == ClipboardFormatName::FILE_LIST.value()));
        if !files && !available_formats.is_empty() {
            self.ask(Asked::Theirs);
        }
        if available_formats.iter().any(|f| f.id() == ClipboardFormatId::CF_UNICODETEXT) {
            self.wanted.push_back(Wanted::Text);
            self.ask(Asked::Fetch);
        }
        // A picture: as PNG where the computer has it so (smaller, and it
        // keeps see-through parts), else in Windows' own form. The plain
        // form before the newer one, whose see-through channel a
        // screenshot leaves empty.
        let named = |name: &str| {
            available_formats.iter().find(|f| f.name().is_some_and(|n| n.value().eq_ignore_ascii_case(name)))
        };
        let numbered = |id: ClipboardFormatId| available_formats.iter().find(|f| f.id() == id);
        let picture = named("PNG")
            .or_else(|| named("image/png"))
            .map(|f| (f.id(), Wanted::Png))
            .or_else(|| numbered(ClipboardFormatId::CF_DIB).map(|f| (f.id(), Wanted::Dib)))
            .or_else(|| numbered(ClipboardFormatId::CF_DIBV5).map(|f| (f.id(), Wanted::DibV5)));
        if let (Some((id, form)), false) = (picture, files) {
            self.wanted.push_back(form);
            self.ask(Asked::FetchPicture(id));
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.ask(Asked::Give(request.format));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        // Answers come in the order of the asking.
        let Some(wanted) = self.wanted.pop_front() else { return };
        if response.is_error() {
            return;
        }
        let data = response.data();
        if data.len() > MOST_PICTURE {
            return;
        }
        use ironrdp_cliprdr_format::bitmap::{dib_to_png, dibv5_to_png};
        let picture = match wanted {
            Wanted::Text => {
                if let Ok(text) = response.to_unicode_string() {
                    self.ask(Asked::Text(from_wire(&text)));
                }
                return;
            }
            Wanted::Png => data.starts_with(b"\x89PNG\r\n\x1a\n").then(|| data.to_vec()),
            Wanted::Dib => dib_to_png(data).ok(),
            Wanted::DibV5 => dibv5_to_png(data).ok(),
        };
        if let Some(png) = picture {
            self.ask(Asked::Picture(png));
        }
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        self.ask(Asked::Contents(request));
    }
    // Kagaz asks the computer for no files.
    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, data_id: LockDataId) {
        self.ask(Asked::Lock(data_id.0));
    }
    fn on_unlock(&mut self, data_id: LockDataId) {
        self.ask(Asked::Unlock(data_id.0));
    }
}

/// One file on offer.
#[derive(Debug)]
pub struct Offered {
    path: PathBuf,
    /// Its name as the computer sees it.
    pub name: String,
    pub size: u64,
    /// When it was last changed, as Windows counts time.
    changed: Option<u64>,
    /// How far the computer has read it.
    sent: AtomicU64,
}

/// The files offered to the computer at one time.
#[derive(Debug)]
pub struct Offer {
    pub files: Vec<Offered>,
}

/// The most handed over in answer to one read; a computer asks for far
/// less at a time.
const MOST_AT_ONCE: u64 = 8 << 20;

/// A name a Windows computer can keep a file under: none of the signs it
/// reserves, no dot or space at the end, none of its device names.
fn name_there(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_control() || r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    out.truncate(out.trim_end_matches(['.', ' ']).len());
    if out.is_empty() {
        out.push_str("file");
    }
    if ironrdp_cliprdr::is_windows_device_name(&out) {
        out.insert(0, '_');
    }
    // 259 signs at most, as Windows counts them; the ending is kept.
    let units = |s: &str| s.encode_utf16().count();
    if units(&out) > 200 {
        let ending = out.rfind('.').filter(|at| units(&out[*at..]) <= 20).map(|at| out[at..].to_string());
        let ending = ending.unwrap_or_default();
        let mut stem: String = out[..out.len() - ending.len()].to_string();
        while units(&stem) + units(&ending) > 200 {
            stem.pop();
        }
        out = stem + &ending;
    }
    out
}

/// "report.pdf" for the second file of that name: "report (2).pdf".
fn numbered(name: &str, n: usize) -> String {
    match name.rfind('.').filter(|at| *at > 0) {
        Some(at) => format!("{} ({n}){}", &name[..at], &name[at..]),
        None => format!("{name} ({n})"),
    }
}

impl Offer {
    /// The files at these paths, as they are now. A folder, or something
    /// that cannot be read, is said by name.
    pub fn of(paths: &[PathBuf]) -> Result<Offer, String> {
        if paths.is_empty() {
            return Err("no file was chosen".into());
        }
        let mut files: Vec<Offered> = Vec::new();
        for path in paths {
            let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
            let about = std::fs::metadata(path).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!("{shown} is not there"),
                std::io::ErrorKind::PermissionDenied => format!("{shown} may not be read"),
                _ => format!("{shown} could not be read ({e})"),
            })?;
            if about.is_dir() {
                return Err(format!("{shown} is a folder; only files can be sent yet"));
            }
            if !about.is_file() {
                return Err(format!("{shown} is not a file"));
            }
            std::fs::File::open(path).map_err(|_| format!("{shown} may not be read"))?;
            let plain = name_there(&shown);
            let taken = |name: &str| files.iter().any(|f| f.name.to_lowercase() == name.to_lowercase());
            let name = (1..)
                .map(|n| if n == 1 { plain.clone() } else { numbered(&plain, n) })
                .find(|name| !taken(name))
                .expect("a number is free");
            let changed = about
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                // Windows counts tenths of a microsecond from the year 1601.
                .map(|d| (d.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(d.subsec_nanos() / 100));
            files.push(Offered {
                path: path.clone(),
                name,
                size: about.len(),
                changed,
                sent: AtomicU64::new(0),
            });
        }
        Ok(Offer { files })
    }

    pub fn names(&self) -> Vec<String> {
        self.files.iter().map(|f| f.name.clone()).collect()
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// What the computer's clipboard is told of them: names, sizes, when
    /// each was last changed.
    pub fn descriptors(&self) -> Vec<FileDescriptor> {
        self.files
            .iter()
            .map(|f| {
                let d = FileDescriptor::new(f.name.clone())
                    .with_attributes(ClipboardFileAttributes::NORMAL)
                    .with_file_size(f.size);
                match f.changed {
                    Some(at) => d.with_last_write_time(at),
                    None => d,
                }
            })
            .collect()
    }

    /// The answer to one read by the computer, and what that says of how
    /// the file is getting on.
    pub fn answer(&self, request: &FileContentsRequest) -> (FileContentsResponse<'static>, Option<Sending>) {
        let failed = || FileContentsResponse::new_error(request.stream_id);
        let Some(file) = usize::try_from(request.index).ok().and_then(|i| self.files.get(i)) else {
            return (failed(), None);
        };
        let taken = || Sending::Taken {
            name: file.name.clone(),
            size: file.size,
        };
        if request.flags.contains(FileContentsFlags::SIZE) {
            // An empty file is all there once its size is known.
            let news = (file.size == 0).then(taken);
            return (FileContentsResponse::new_size_response(request.stream_id, file.size), news);
        }
        let length = u64::from(request.requested_size).min(MOST_AT_ONCE).min(file.size.saturating_sub(request.position));
        let read = || -> std::io::Result<Vec<u8>> {
            let mut from = std::fs::File::open(&file.path)?;
            from.seek(SeekFrom::Start(request.position))?;
            let mut data = Vec::with_capacity(length as usize);
            from.take(length).read_to_end(&mut data)?;
            Ok(data)
        };
        let data = match read() {
            Ok(data) => data,
            Err(e) => {
                let why = format!("{} could not be read ({e})", file.name);
                return (failed(), Some(Sending::Failed { why }));
            }
        };
        // A read past the end hands over nothing and says nothing.
        if data.is_empty() {
            return (FileContentsResponse::new_data_response(request.stream_id, data), None);
        }
        let end = request.position + data.len() as u64;
        // From the start again: pasted once more.
        if request.position == 0 {
            file.sent.store(0, Ordering::Relaxed);
        }
        let before = file.sent.fetch_max(end, Ordering::Relaxed);
        let news = if end >= file.size && before < file.size {
            Some(taken())
        } else if end > before {
            Some(Sending::Going {
                name: file.name.clone(),
                sent: end,
                size: file.size,
            })
        } else {
            None
        };
        (FileContentsResponse::new_data_response(request.stream_id, data), news)
    }
}

/// Text as Windows keeps it: lines end in CR LF.
pub fn to_wire(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// And back: lines end in LF, nothing after the text's end.
pub fn from_wire(text: &str) -> String {
    text.trim_end_matches('\0').replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_end_the_way_each_side_keeps_them() {
        assert_eq!(to_wire("a\nb"), "a\r\nb");
        assert_eq!(to_wire("a\r\nb\n"), "a\r\nb\r\n");
        assert_eq!(from_wire("a\r\nb\0"), "a\nb");
        assert_eq!(from_wire("नमस्ते"), "नमस्ते");
    }

    #[test]
    fn the_channel_asks_in_order() {
        let (mut backend, shared) = Backend::new();
        backend.on_request_format_list();
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_TEXT)]);
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        backend.on_format_data_response(FormatDataResponse::new_unicode_string("two\r\nlines"));
        backend.on_format_data_response(FormatDataResponse::new_error());
        let got: Vec<Asked> = shared.asked.lock().unwrap().drain(..).collect();
        assert_eq!(
            got,
            vec![
                Asked::List,
                // Text in the old one-byte form alone is not fetched; it is
                // the computer's own all the same.
                Asked::Theirs,
                Asked::Theirs,
                Asked::Fetch,
                Asked::Give(ClipboardFormatId::CF_UNICODETEXT),
                Asked::Text("two\nlines".into()),
            ]
        );
    }

    #[test]
    fn names_a_windows_computer_can_keep() {
        assert_eq!(name_there("report.pdf"), "report.pdf");
        assert_eq!(name_there("what: when?.txt"), "what_ when_.txt");
        assert_eq!(name_there(r"a\b/c*d|e<f>g.txt"), "a_b_c_d_e_f_g.txt");
        assert_eq!(name_there("ends with a dot. "), "ends with a dot");
        assert_eq!(name_there("..."), "file");
        assert_eq!(name_there("CON.txt"), "_CON.txt");
        assert_eq!(name_there("रिपोर्ट.pdf"), "रिपोर्ट.pdf");
        let long = format!("{}.tar.gz", "x".repeat(400));
        let short = name_there(&long);
        assert_eq!(short.encode_utf16().count(), 200);
        assert!(short.ends_with(".gz"));
        assert_eq!(numbered("report.pdf", 2), "report (2).pdf");
        assert_eq!(numbered("notes", 3), "notes (3)");
        assert_eq!(numbered(".profile", 2), ".profile (2)");
    }

    #[test]
    fn two_files_of_one_name_get_a_number() {
        let folder = std::env::temp_dir().join(format!("kagaz-clip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("a")).unwrap();
        std::fs::create_dir_all(folder.join("b")).unwrap();
        let paths = [folder.join("a/report.pdf"), folder.join("b/Report.pdf"), folder.join("b/other.pdf")];
        for (path, contents) in paths.iter().zip(["one", "two!", "three"]) {
            std::fs::write(path, contents).unwrap();
        }
        let offer = Offer::of(&paths).unwrap();
        assert_eq!(offer.names(), ["report.pdf", "Report (2).pdf", "other.pdf"]);
        assert_eq!(offer.bytes(), 12);
        let said = offer.descriptors();
        assert_eq!(said[1].file_size, Some(4));
        assert_eq!(said[1].relative_path, None);
        assert!(said[1].last_write_time.is_some_and(|at| at > 130_000_000_000_000_000));

        // A read from the middle, past the end, and of a file not offered.
        let read = |index: i32, position: u64, requested_size: u32| {
            offer.answer(&FileContentsRequest {
                stream_id: 7,
                index,
                flags: FileContentsFlags::RANGE,
                position,
                requested_size,
                data_id: None,
            })
        };
        let (answer, news) = read(2, 1, 3);
        assert_eq!(answer.data(), b"hre");
        assert_eq!(news, Some(Sending::Going { name: "other.pdf".into(), sent: 4, size: 5 }));
        let (answer, news) = read(2, 4, 100);
        assert_eq!(answer.data(), b"e");
        assert_eq!(news, Some(Sending::Taken { name: "other.pdf".into(), size: 5 }));
        // The same end once more is not news; from the start again it is.
        assert_eq!(read(2, 4, 100).1, None);
        assert_eq!(read(2, 0, 100).1, Some(Sending::Taken { name: "other.pdf".into(), size: 5 }));
        let (answer, news) = read(2, 9, 4);
        assert!(!answer.is_error() && answer.data().is_empty());
        assert_eq!(news, None);
        assert!(read(3, 0, 4).0.is_error());
        assert!(read(-1, 0, 4).0.is_error());
        // A file taken away since it was offered.
        std::fs::remove_file(&paths[0]).unwrap();
        let (answer, news) = read(0, 0, 3);
        assert!(answer.is_error());
        assert!(matches!(news, Some(Sending::Failed { why }) if why.starts_with("report.pdf could not be read")));
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
