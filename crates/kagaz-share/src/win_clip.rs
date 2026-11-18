//! Windows: the clipboard, through a hidden window of its own. Windows
//! lets only a program with a window put things on the clipboard, and
//! tells that window whenever someone copies.

use crate::clip::{Board, Holds};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, RegisterClassW, HWND_MESSAGE, MSG,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WNDCLASSW,
};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;

/// What the window is asked to put on the clipboard.
enum Put {
    Text(String),
    Picture(Vec<u8>),
    Files(Vec<PathBuf>),
}

struct State {
    hwnd: Mutex<isize>,
    puts: Mutex<VecDeque<Put>>,
    changed: Mutex<Option<Box<dyn Fn(Holds) + Send + Sync>>>,
    png: u32,
    drop_effect: u32,
}

static STATE: OnceLock<Arc<State>> = OnceLock::new();

pub struct WinBoard {
    state: Arc<State>,
}

impl WinBoard {
    /// Starts the clipboard's window on a thread of its own.
    pub fn start() -> anyhow::Result<WinBoard> {
        let state = Arc::new(State {
            hwnd: Mutex::new(0),
            puts: Mutex::new(VecDeque::new()),
            changed: Mutex::new(None),
            png: unsafe { RegisterClipboardFormatW(w!("PNG")) },
            drop_effect: unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) },
        });
        STATE.set(state.clone()).map_err(|_| anyhow::anyhow!("the clipboard's window is already there"))?;
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            let class = w!("KagazShareClipboard");
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSW { lpfnWndProc: Some(window), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            let made = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                None,
            );
            let hwnd = match made {
                Ok(h) => h,
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow::anyhow!("no window for the clipboard: {e}")));
                    return;
                }
            };
            if let Some(s) = STATE.get() {
                *s.hwnd.lock().unwrap() = hwnd.0 as isize;
            }
            let _ = AddClipboardFormatListener(hwnd);
            let _ = ready_tx.send(Ok(()));
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        });
        ready_rx.recv()??;
        Ok(WinBoard { state })
    }

    fn wake(&self) {
        let hwnd = *self.state.hwnd.lock().unwrap();
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP, WPARAM(0), LPARAM(0));
        }
    }
}

/// The clipboard opened for a moment; Windows lends it to one program at
/// a time, so a few tries.
fn open(hwnd: Option<HWND>) -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(hwnd) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// Bytes into memory the clipboard can keep.
fn global(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let at = GlobalLock(mem) as *mut u8;
        if at.is_null() {
            let _ = GlobalFree(Some(mem));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len());
        let _ = GlobalUnlock(mem);
        Some(mem)
    }
}

fn set(format: u32, bytes: &[u8]) {
    if let Some(mem) = global(bytes) {
        unsafe {
            if SetClipboardData(format, Some(HANDLE(mem.0))).is_err() {
                let _ = GlobalFree(Some(mem));
            }
        }
    }
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect()
}

/// Puts what was asked for on the clipboard, as this window's.
fn put(state: &State, hwnd: HWND) {
    loop {
        let Some(next) = state.puts.lock().unwrap().pop_front() else { return };
        if !open(Some(hwnd)) {
            crate::say("the clipboard is busy with another program: nothing put on it");
            continue;
        }
        unsafe {
            let _ = EmptyClipboard();
        }
        match next {
            Put::Text(text) => set(CF_UNICODETEXT, &utf16(&text)),
            Put::Picture(dib) => set(CF_DIB, &dib),
            Put::Files(paths) => {
                // DROPFILES: where the names start, a point, two flags
                // (the last says the names are wide), then the names each
                // ending in a nought, and one more nought.
                let mut bytes = Vec::new();
                bytes.extend_from_slice(&20u32.to_le_bytes());
                bytes.extend_from_slice(&[0u8; 8]);
                bytes.extend_from_slice(&0u32.to_le_bytes());
                bytes.extend_from_slice(&1u32.to_le_bytes());
                for p in &paths {
                    bytes.extend_from_slice(&utf16(&p.to_string_lossy()));
                }
                bytes.extend_from_slice(&[0, 0]);
                set(CF_HDROP, &bytes);
                // Pasting copies them (1 is "copy").
                set(state.drop_effect, &1u32.to_le_bytes());
            }
        }
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn holds(state: &State) -> Holds {
    unsafe {
        Holds {
            text: IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok(),
            dib: IsClipboardFormatAvailable(CF_DIB).is_ok(),
            png: state.png != 0 && IsClipboardFormatAvailable(state.png).is_ok(),
        }
    }
}

unsafe extern "system" fn window(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(state) = STATE.get() else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_APP => {
            put(state, hwnd);
            LRESULT(0)
        }
        WM_CLIPBOARDUPDATE => {
            // Someone here copied something; what this window put there
            // itself came from the viewer and is not offered back.
            let ours = GetClipboardOwner().map(|o| o == hwnd).unwrap_or(false);
            if !ours {
                let now = holds(state);
                if let Some(changed) = state.changed.lock().unwrap().as_ref() {
                    changed(now);
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// What the clipboard holds in one format, as bytes.
fn read(format: u32) -> Option<Vec<u8>> {
    if !open(None) {
        return None;
    }
    let bytes = unsafe {
        match GetClipboardData(format) {
            Ok(handle) if !handle.is_invalid() => {
                let mem = HGLOBAL(handle.0);
                let size = GlobalSize(mem);
                let at = GlobalLock(mem) as *const u8;
                if at.is_null() || size == 0 {
                    None
                } else {
                    let v = std::slice::from_raw_parts(at, size).to_vec();
                    let _ = GlobalUnlock(mem);
                    Some(v)
                }
            }
            _ => None,
        }
    };
    unsafe {
        let _ = CloseClipboard();
    }
    bytes
}

impl Board for WinBoard {
    fn text(&self) -> Option<String> {
        let bytes = read(CF_UNICODETEXT)?;
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        Some(String::from_utf16_lossy(&units))
    }

    fn picture(&self, png: bool) -> Option<Vec<u8>> {
        read(if png { self.state.png } else { CF_DIB })
    }

    fn put_text(&self, text: String) {
        self.state.puts.lock().unwrap().push_back(Put::Text(text));
        self.wake();
    }

    fn put_picture(&self, dib: Vec<u8>) {
        self.state.puts.lock().unwrap().push_back(Put::Picture(dib));
        self.wake();
    }

    fn put_files(&self, paths: Vec<PathBuf>) {
        self.state.puts.lock().unwrap().push_back(Put::Files(paths));
        self.wake();
    }

    fn watch(&self, changed: Box<dyn Fn(Holds) + Send + Sync>) {
        *self.state.changed.lock().unwrap() = Some(changed);
    }

    fn holds(&self) -> Holds {
        holds(&self.state)
    }
}
