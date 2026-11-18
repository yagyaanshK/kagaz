//! The clipboard both ways, and files sent from the viewer.
//!
//! What is copied here is offered to the viewer; what the viewer copies
//! (text, a picture) is put on this computer's clipboard at once, as
//! Windows' own Remote Desktop does. Files the viewer sends (its Send a
//! file) are taken at once into a folder of their own and then put on
//! the clipboard, so that Paste in any folder here copies them there.

use ironrdp_cliprdr::backend::{ClipboardMessage, CliprdrBackend, CliprdrBackendFactory};
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags, FileContentsFlags,
    FileContentsRequest, FileContentsResponse, FileDescriptor, FormatDataRequest, FormatDataResponse, LockDataId,
};
use ironrdp_server::{CliprdrServerFactory, ServerEvent, ServerEventSender};
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

use crate::say;

/// What this computer's clipboard holds, as far as the viewer cares.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Holds {
    pub text: bool,
    pub dib: bool,
    pub png: bool,
}

/// This computer's clipboard.
pub trait Board: Send + Sync {
    fn text(&self) -> Option<String>;
    /// A picture in Windows' own form (`png` false) or as PNG.
    fn picture(&self, png: bool) -> Option<Vec<u8>>;
    fn put_text(&self, text: String);
    fn put_picture(&self, dib: Vec<u8>);
    fn put_files(&self, paths: Vec<PathBuf>);
    /// Told whenever someone here copies something.
    fn watch(&self, changed: Box<dyn Fn(Holds) + Send + Sync>);
    fn holds(&self) -> Holds;
}

/// The number this side gives PNG among what it offers.
const PNG_HERE: ClipboardFormatId = ClipboardFormatId(0xC0F1);
/// As much as is asked for at a time.
const PIECE: u64 = 1 << 20;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Wanted {
    Text,
    Dib,
}

/// Files being taken from the viewer, one after another.
#[derive(Default)]
struct Taking {
    files: Vec<(String, u64)>,
    kept: Option<u32>,
    index: usize,
    sized: bool,
    size: u64,
    position: u64,
    stream: u32,
    folder: PathBuf,
    out: Option<std::fs::File>,
    done: Vec<PathBuf>,
    began: Option<Instant>,
}

/// The clipboard channel's side here.
#[derive(Clone)]
pub struct Clip {
    board: Arc<dyn Board>,
    to_viewer: Arc<Mutex<Option<UnboundedSender<ServerEvent>>>>,
    /// What was asked of the viewer and is on its way, in order: its
    /// answers do not say what they answer.
    wanted: Arc<Mutex<VecDeque<Wanted>>>,
    taking: Arc<Mutex<Taking>>,
    /// What the device connected may do, and files refused whatever it
    /// may (`--no-files`).
    allowed: crate::paired::Allowed,
    no_files: bool,
    /// Where files from the viewer go, a folder for each sending.
    pub received: PathBuf,
}

impl std::fmt::Debug for Clip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Clip")
    }
}

impl Clip {
    pub fn new(board: Arc<dyn Board>, received: PathBuf, allowed: crate::paired::Allowed, no_files: bool) -> Clip {
        let clip = Clip {
            board,
            to_viewer: Arc::default(),
            wanted: Arc::default(),
            taking: Arc::default(),
            allowed,
            no_files,
            received,
        };
        let me = clip.clone();
        clip.board.watch(Box::new(move |holds| {
            if me.gives() {
                me.send(ClipboardMessage::SendInitiateCopy(formats(holds)))
            }
        }));
        clip
    }

    /// What is copied here goes to the viewer.
    fn gives(&self) -> bool {
        self.allowed.clipboard.load(std::sync::atomic::Ordering::Acquire)
    }

    /// What the viewer copies comes here: only for a device that may use
    /// this computer, not one that watches.
    fn takes(&self) -> bool {
        self.gives() && self.allowed.control.load(std::sync::atomic::Ordering::Acquire)
    }

    fn files(&self) -> bool {
        !self.no_files && self.allowed.files.load(std::sync::atomic::Ordering::Acquire)
    }

    fn send(&self, message: ClipboardMessage) {
        if let Some(sender) = self.to_viewer.lock().unwrap().as_ref() {
            let _ = sender.send(ServerEvent::Clipboard(message));
        }
    }

    /// Ask for the next piece of the file being taken, or finish.
    fn ask(&self, taking: &mut Taking) {
        let Some((_, size)) = taking.files.get(taking.index).cloned() else {
            let paths = std::mem::take(&mut taking.done);
            let secs = taking.began.map_or(0.0, |b| b.elapsed().as_secs_f64());
            say(&format!("received {} files in {secs:.1} s; on the clipboard to paste", paths.len()));
            if !paths.is_empty() {
                self.board.put_files(paths);
            }
            return;
        };
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
        let (name, _) = taking.files[taking.index].clone();
        let path = taking.folder.join(&name);
        if let Some(mut out) = taking.out.take() {
            let _ = out.flush();
        }
        if well {
            taking.done.push(path);
        } else {
            let _ = std::fs::remove_file(&path);
            say(&format!("a file did not arrive whole ({} of them)", taking.files.len()));
        }
        taking.index += 1;
        taking.sized = false;
        taking.position = 0;
        self.ask(taking);
    }
}

fn formats(holds: Holds) -> Vec<ClipboardFormat> {
    let mut list = Vec::new();
    if holds.text {
        list.push(ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT));
    }
    if holds.png {
        list.push(ClipboardFormat::new(PNG_HERE).with_name(ClipboardFormatName::new("PNG")));
    }
    if holds.dib {
        list.push(ClipboardFormat::new(ClipboardFormatId::CF_DIB));
    }
    list
}

/// A name the viewer gave a file, made safe as one file in one folder.
fn safe_name(name: &str, n: usize) -> String {
    let last = name.rsplit(['\\', '/']).next().unwrap_or("").trim();
    let clean: String = last.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') && !c.is_control()).collect();
    let clean = clean.trim_matches(['.', ' ']).to_string();
    if clean.is_empty() {
        format!("file {}", n + 1)
    } else {
        clean
    }
}

/// A new folder for one sending, named by the time.
fn folder_in(base: &Path) -> std::io::Result<PathBuf> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    for n in 0.. {
        let p = base.join(if n == 0 { format!("{secs}") } else { format!("{secs}-{n}") });
        if !p.exists() {
            std::fs::create_dir_all(&p)?;
            return Ok(p);
        }
    }
    unreachable!()
}

impl ServerEventSender for Clip {
    fn set_sender(&mut self, sender: UnboundedSender<ServerEvent>) {
        *self.to_viewer.lock().unwrap() = Some(sender);
    }
}

impl CliprdrBackendFactory for Clip {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        Box::new(self.clone())
    }
}

impl CliprdrServerFactory for Clip {}

ironrdp_core::impl_as_any!(Clip);

impl CliprdrBackend for Clip {
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
        let holds = if self.gives() { self.board.holds() } else { Holds::default() };
        self.send(ClipboardMessage::SendInitiateCopy(formats(holds)));
    }

    fn on_process_negotiated_capabilities(&mut self, _capabilities: ClipboardGeneralCapabilityFlags) {}

    // The viewer copied something: text and a picture are asked for at
    // once; files it sends are taken at once.
    fn on_remote_copy(&mut self, available: &[ClipboardFormat]) {
        if !self.takes() {
            return;
        }
        let files = available
            .iter()
            .find(|f| f.name().is_some_and(|n| n.value() == ClipboardFormatName::FILE_LIST.value()))
            .map(|f| f.id());
        if let Some(format) = files {
            if self.files() {
                self.send(ClipboardMessage::SendInitiatePaste(format));
            } else {
                say("files from the viewer refused: this device may not send them");
            }
            return;
        }
        if available.iter().any(|f| f.id() == ClipboardFormatId::CF_UNICODETEXT) {
            self.wanted.lock().unwrap().push_back(Wanted::Text);
            self.send(ClipboardMessage::SendInitiatePaste(ClipboardFormatId::CF_UNICODETEXT));
        } else if available.iter().any(|f| f.id() == ClipboardFormatId::CF_DIB) {
            // Windows' own form: every program here reads it.
            self.wanted.lock().unwrap().push_back(Wanted::Dib);
            self.send(ClipboardMessage::SendInitiatePaste(ClipboardFormatId::CF_DIB));
        }
    }

    // The list of what it sends: take the first file.
    fn on_remote_file_list(&mut self, files: &[FileDescriptor], clip_data_id: Option<u32>) {
        let mut taking = self.taking.lock().unwrap();
        let folder = match folder_in(&self.received) {
            Ok(f) => f,
            Err(e) => return say(&format!("there is nowhere to put the files: {e}")),
        };
        let mut names = std::collections::HashSet::new();
        let files: Vec<(String, u64)> = files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut name = safe_name(&f.name, i);
                while !names.insert(name.to_lowercase()) {
                    name = format!("{} ({})", name, i + 1);
                }
                (name, f.file_size.unwrap_or(0))
            })
            .collect();
        let total: u64 = files.iter().map(|f| f.1).sum();
        say(&format!("the viewer sends {} files, {} KB", files.len(), total / 1024));
        *taking = Taking { files, kept: clip_data_id, stream: taking.stream, folder, began: Some(Instant::now()), ..Taking::default() };
        self.ask(&mut taking);
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        let answer = if !self.gives() {
            None
        } else if request.format == ClipboardFormatId::CF_UNICODETEXT {
            self.board.text().map(|t| FormatDataResponse::new_unicode_string(&t))
        } else if request.format == PNG_HERE {
            self.board.picture(true).map(FormatDataResponse::new_data)
        } else if request.format == ClipboardFormatId::CF_DIB {
            self.board.picture(false).map(FormatDataResponse::new_data)
        } else {
            None
        };
        self.send(ClipboardMessage::SendFormatData(answer.unwrap_or_else(FormatDataResponse::new_error)));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let wanted = self.wanted.lock().unwrap().pop_front();
        if response.is_error() {
            return;
        }
        match wanted {
            Some(Wanted::Text) => {
                if let Ok(text) = response.to_unicode_string() {
                    self.board.put_text(text.trim_end_matches('\0').to_string());
                }
            }
            Some(Wanted::Dib) => self.board.put_picture(response.data().to_vec()),
            None => {}
        }
    }

    // Files copied here are not offered yet: the viewer does not take
    // files from a computer.
    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        self.send(ClipboardMessage::SendFileContentsResponse(FileContentsResponse::new_error(request.stream_id)));
    }

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
            let path = taking.folder.join(&taking.files[taking.index].0);
            match std::fs::File::create(&path) {
                Ok(f) => taking.out = Some(f),
                Err(_) => return self.file_over(&mut taking, false),
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
        let wrote = taking.out.as_mut().map(|o| o.write_all(piece).is_ok()).unwrap_or(false);
        if !wrote {
            return self.file_over(&mut taking, false);
        }
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

/// A clipboard that is only said, for the test card's run: lengths and
/// sizes, never what was copied.
#[derive(Default)]
pub struct Said {
    text: Mutex<Option<String>>,
}

impl Board for Said {
    fn text(&self) -> Option<String> {
        self.text.lock().unwrap().clone()
    }
    fn picture(&self, _png: bool) -> Option<Vec<u8>> {
        None
    }
    fn put_text(&self, text: String) {
        say(&format!("heard Text of {} characters", text.chars().count()));
        *self.text.lock().unwrap() = Some(text);
    }
    fn put_picture(&self, dib: Vec<u8>) {
        say(&format!("heard Picture of {} bytes", dib.len()));
    }
    fn put_files(&self, paths: Vec<PathBuf>) {
        for p in paths {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            say(&format!("heard File of {size} bytes"));
        }
    }
    fn watch(&self, _changed: Box<dyn Fn(Holds) + Send + Sync>) {}
    fn holds(&self) -> Holds {
        Holds { text: self.text.lock().unwrap().is_some(), ..Holds::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_in_their_folder() {
        assert_eq!(safe_name("..\\..\\Windows\\evil.exe", 0), "evil.exe");
        assert_eq!(safe_name("notes/plan.txt", 0), "plan.txt");
        assert_eq!(safe_name("..", 2), "file 3");
        assert_eq!(safe_name("a:b?.txt", 0), "ab.txt");
    }
}
