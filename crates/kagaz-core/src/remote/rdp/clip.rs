//! Clipboard text over Remote Desktop. Neither side sends its text
//! unasked: each says "I have text", and hands it over when the other
//! asks, which a computer does when something is pasted there. Kagaz
//! asks at once when the computer says it has text, so that what was
//! copied there is on this device's clipboard without a further step.
//!
//! The library's clipboard channel calls into [`Backend`] while the
//! session handles what arrived; the session then does what was
//! [`Asked`], in order.

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest, FileContentsResponse,
    FormatDataRequest, FormatDataResponse, LockDataId,
};
use ironrdp_core::impl_as_any;
use std::collections::VecDeque;
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
}

pub type Queue = Arc<Mutex<VecDeque<Asked>>>;

#[derive(Debug)]
pub struct Backend {
    asked: Queue,
}

impl Backend {
    pub fn new() -> (Backend, Queue) {
        let asked = Queue::default();
        (Backend { asked: asked.clone() }, asked)
    }

    fn ask(&self, what: Asked) {
        self.asked.lock().unwrap_or_else(|e| e.into_inner()).push_back(what);
    }
}

impl_as_any!(Backend);

impl CliprdrBackend for Backend {
    // Files are not carried here: nothing is ever written there.
    fn temporary_directory(&self) -> &str {
        ""
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
    }

    fn on_ready(&mut self) {}

    fn on_request_format_list(&mut self) {
        self.ask(Asked::List);
    }

    fn on_process_negotiated_capabilities(&mut self, _capabilities: ClipboardGeneralCapabilityFlags) {}

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        if available_formats.iter().any(|f| f.id() == ClipboardFormatId::CF_UNICODETEXT) {
            self.ask(Asked::Fetch);
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.ask(Asked::Give(request.format));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if response.is_error() {
            return;
        }
        if let Ok(text) = response.to_unicode_string() {
            self.ask(Asked::Text(from_wire(&text)));
        }
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _data_id: LockDataId) {}
    fn on_unlock(&mut self, _data_id: LockDataId) {}
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
        let (mut backend, asked) = Backend::new();
        backend.on_request_format_list();
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_TEXT)]);
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        backend.on_format_data_response(FormatDataResponse::new_unicode_string("two\r\nlines"));
        backend.on_format_data_response(FormatDataResponse::new_error());
        let got: Vec<Asked> = asked.lock().unwrap().drain(..).collect();
        assert_eq!(
            got,
            vec![
                Asked::List,
                // Text in the old one-byte form alone is not fetched.
                Asked::Fetch,
                Asked::Give(ClipboardFormatId::CF_UNICODETEXT),
                Asked::Text("two\nlines".into()),
            ]
        );
    }
}
