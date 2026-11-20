//! Files the computer copied, fetched here over its clipboard channel
//! (copy there, paste here): their list, then each file a piece at a
//! time, written to disk as it comes (never all of a file in memory),
//! into a folder of their own here for this computer's clipboard. One
//! piece is asked for at a time: on a local network a piece is a few
//! milliseconds there and back.

use ironrdp_cliprdr::pdu::{ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileDescriptor};
use std::io::Write;
use std::path::PathBuf;

/// Fetched without asking up to this much; more, the person says so.
pub const MOST_UNASKED: u64 = 512 * 1024 * 1024;
/// One piece asked for.
const PIECE: u32 = 1024 * 1024;

/// A fetch under way.
#[derive(Debug)]
pub struct Fetch {
    files: Vec<FileDescriptor>,
    /// The computer's number for the copy (it keeps it while it is had).
    data_id: Option<u32>,
    /// The folder they come into.
    pub dir: PathBuf,
    index: usize,
    out: Option<Out>,
    /// The piece asked for and not yet come.
    waiting: Option<u32>,
    pub total: u64,
    pub done: u64,
    /// What was copied there (files and folders at the top), here.
    pub top: Vec<PathBuf>,
}

#[derive(Debug)]
struct Out {
    file: std::fs::File,
    size: u64,
    at: u64,
}

/// How a piece went.
pub enum Took {
    /// Written; more to come (or not this fetch's).
    More,
    Failed(String),
}

fn is_folder(f: &FileDescriptor) -> bool {
    f.attributes.is_some_and(|a| a.contains(ClipboardFileAttributes::DIRECTORY))
}

/// What the computer copied, at the top (not inside a folder of it).
pub fn top_names(files: &[FileDescriptor]) -> Vec<String> {
    files.iter().filter(|f| f.relative_path.as_deref().is_none_or(str::is_empty)).map(|f| f.name.clone()).collect()
}

/// The size of all of it.
pub fn bytes(files: &[FileDescriptor]) -> u64 {
    files.iter().filter(|f| !is_folder(f)).filter_map(|f| f.file_size).sum()
}

impl Fetch {
    pub fn new(files: Vec<FileDescriptor>, data_id: Option<u32>) -> std::io::Result<Fetch> {
        let dir = received_folder()?;
        let total = bytes(&files);
        let top = top_names(&files).into_iter().map(|n| dir.join(n)).collect();
        Ok(Fetch { files, data_id, dir, index: 0, out: None, waiting: None, total, done: 0, top })
    }

    /// Where a file of the list goes here: inside its folders, by the
    /// names the channel has already made safe (no "..", no drive).
    fn path(&self, f: &FileDescriptor) -> PathBuf {
        let mut p = self.dir.clone();
        if let Some(inside) = &f.relative_path {
            for part in inside.split(['\\', '/']).filter(|c| !c.is_empty() && *c != "." && *c != "..") {
                p.push(part);
            }
        }
        p.join(&f.name)
    }

    /// The next piece to ask for (as `stream`), if none is awaited; none
    /// when all of it is here, or a piece is on its way.
    pub fn next(&mut self, stream: u32) -> std::io::Result<Option<FileContentsRequest>> {
        if self.waiting.is_some() {
            return Ok(None);
        }
        loop {
            let Some(f) = self.files.get(self.index) else { return Ok(None) };
            let path = self.path(f);
            if is_folder(f) {
                std::fs::create_dir_all(&path)?;
                self.index += 1;
                continue;
            }
            if self.out.is_none() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let file = std::fs::File::create(&path)?;
                self.out = Some(Out { file, size: f.file_size.unwrap_or(0), at: 0 });
            }
            let Some(out) = self.out.as_ref() else { continue };
            if out.at >= out.size {
                self.out = None;
                self.index += 1;
                continue;
            }
            let want = (out.size - out.at).min(u64::from(PIECE)) as u32;
            self.waiting = Some(stream);
            return Ok(Some(FileContentsRequest {
                stream_id: stream,
                index: self.index as i32,
                flags: FileContentsFlags::RANGE,
                position: out.at,
                requested_size: want,
                data_id: self.data_id,
            }));
        }
    }

    /// All of it is here.
    pub fn finished(&self) -> bool {
        self.index >= self.files.len() && self.waiting.is_none()
    }

    /// A piece came (none: the computer could not give it).
    pub fn took(&mut self, stream: u32, data: Option<Vec<u8>>) -> Took {
        if self.waiting != Some(stream) {
            return Took::More;
        }
        self.waiting = None;
        let Some(data) = data else { return Took::Failed("the computer did not give a piece of a file".into()) };
        let Some(out) = self.out.as_mut() else { return Took::More };
        if data.is_empty() {
            return Took::Failed("the computer gave an empty piece of a file".into());
        }
        let data = &data[..data.len().min((out.size - out.at) as usize)];
        if let Err(e) = out.file.write_all(data) {
            return Took::Failed(format!("a file could not be written here ({e})"));
        }
        out.at += data.len() as u64;
        self.done += data.len() as u64;
        Took::More
    }
}

/// A folder of its own for what is fetched, under Kagaz's cache: kept
/// after the session (a paste can come later), gone after a day.
fn received_folder() -> std::io::Result<PathBuf> {
    let base = crate::paths::cache_dir().join("received");
    std::fs::create_dir_all(&base)?;
    if let Ok(entries) = std::fs::read_dir(&base) {
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        for e in entries.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > day);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = base.join(format!("{now:x}-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_folders_come_piece_by_piece_into_their_places() {
        let mut folder = FileDescriptor::new("docs");
        folder.attributes = Some(ClipboardFileAttributes::DIRECTORY);
        let mut inner = FileDescriptor::new("b.txt");
        inner.relative_path = Some("docs".into());
        inner.file_size = Some(3);
        let mut big = FileDescriptor::new("a.bin");
        big.file_size = Some(u64::from(PIECE) + 5);
        let files = vec![folder, inner, big];
        assert_eq!(top_names(&files), vec!["docs".to_string(), "a.bin".to_string()]);
        let mut f = Fetch::new(files, Some(7)).unwrap();
        assert_eq!(f.total, u64::from(PIECE) + 8);
        let r = f.next(1).unwrap().unwrap();
        assert_eq!((r.index, r.position, r.requested_size, r.data_id), (1, 0, 3, Some(7)));
        assert!(f.next(2).unwrap().is_none(), "one piece at a time");
        assert!(matches!(f.took(1, Some(b"abc".to_vec())), Took::More));
        let r = f.next(2).unwrap().unwrap();
        assert_eq!((r.index, r.position, r.requested_size), (2, 0, PIECE));
        assert!(matches!(f.took(2, Some(vec![1; PIECE as usize])), Took::More));
        let r = f.next(3).unwrap().unwrap();
        assert_eq!((r.position, r.requested_size), (u64::from(PIECE), 5));
        assert!(matches!(f.took(3, Some(vec![2; 5])), Took::More));
        assert!(f.next(4).unwrap().is_none());
        assert!(f.finished());
        assert_eq!(std::fs::read(f.dir.join("docs").join("b.txt")).unwrap(), b"abc");
        assert_eq!(std::fs::metadata(f.dir.join("a.bin")).unwrap().len(), u64::from(PIECE) + 5);
        assert_eq!(f.top, vec![f.dir.join("docs"), f.dir.join("a.bin")]);
        let _ = std::fs::remove_dir_all(&f.dir);
    }
}
