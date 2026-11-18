//! Windows: asking the person here, when this program runs on its own (in
//! Kagaz's window, Kagaz asks instead): Windows' own small question, on
//! top of everything, closed by itself after the wait.

use crate::paired::{Answer, Asker};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, MessageBoxW, PostMessageW, IDNO, IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNOCANCEL,
    WM_CLOSE,
};

pub struct Window;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Asker for Window {
    fn ask(&self, id: &str, name: &str, wait: Duration) -> Answer {
        // Its title is its own, so that it can be found and closed.
        let title = format!("Kagaz · {name} wants to connect ({id})");
        let text = format!(
            "{name} wants to see this screen.\n\nYes: it may also use the mouse and keys.\nNo: it may only watch.\nCancel: don't allow it.\n\nNot answered in {} seconds means don't allow.",
            wait.as_secs()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let t = title.clone();
        std::thread::spawn(move || {
            let (title, text) = (wide(&t), wide(&text));
            let chosen = unsafe {
                MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNOCANCEL | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND)
            };
            let _ = tx.send(if chosen == IDYES {
                Answer::Control
            } else if chosen == IDNO {
                Answer::Watch
            } else {
                Answer::Deny
            });
        });
        let until = Instant::now() + wait;
        loop {
            if let Ok(answer) = rx.recv_timeout(Duration::from_millis(200)) {
                return answer;
            }
            if Instant::now() >= until {
                let title = wide(&title);
                unsafe {
                    if let Ok(found) = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) {
                        let _ = PostMessageW(Some(found), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
                let _ = rx.recv_timeout(Duration::from_secs(2));
                return Answer::Deny;
            }
        }
    }
}
