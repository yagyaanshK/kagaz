//! Kagaz's Remote Desktop session against a test server that shows the
//! test card: `tools/rdp-test-server`, a program of its own (see its
//! manifest for why), started here. No real computer's screen is
//! involved. Where that program has not been built these tests say so and
//! pass without testing anything:
//!
//!   cargo build --manifest-path tools/rdp-test-server/Cargo.toml

use super::*;
use crate::remote::pattern;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

const SOON: Duration = Duration::from_secs(20);

struct TestServer {
    child: Child,
    input: ChildStdin,
    addr: SocketAddr,
    /// The fingerprint of the certificate it shows.
    fingerprint: String,
    heard: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    fn heard(&self) -> Vec<String> {
        self.heard.lock().unwrap().clone()
    }

    fn tick(&mut self, tick: u32) {
        writeln!(self.input, "tick {tick}").unwrap();
    }

    /// Text is copied "on the computer"; `\n` in it is a line's end.
    fn copy(&mut self, text: &str) {
        writeln!(self.input, "copy {text}").unwrap();
    }

    fn resize(&mut self, width: u16, height: u16) {
        writeln!(self.input, "size {width} {height}").unwrap();
    }

    /// What a viewer offers is pasted "on the computer", which waits so
    /// many milliseconds before it asks for each piece.
    fn paste(&mut self, wait: u32) {
        writeln!(self.input, "paste {wait}").unwrap();
    }
}

/// A folder of files to send, gone when the test is over. What is in
/// them is made here: nobody's files.
struct Folder(std::path::PathBuf);

impl Folder {
    fn new(test: &str) -> Folder {
        let path = std::env::temp_dir().join(format!("kagaz-rdp-test-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Folder(path)
    }

    fn file(&self, name: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Bytes that repeat nowhere near a piece's length.
fn made(length: usize) -> Vec<u8> {
    (0..length).map(|i| (i * 31 + i / 251) as u8).collect()
}

/// What the test server says of a file that arrived whole.
fn arrived(name: &str, contents: &[u8]) -> String {
    use sha2::Digest as _;
    let sum: String = sha2::Sha256::digest(contents).iter().map(|b| format!("{b:02x}")).collect();
    format!("File({name:?}, {}, {sum})", contents.len())
}

/// Collects what a session tells, to look through.
struct Told<'a> {
    session: &'a Session,
    events: Vec<Event>,
}

impl Told<'_> {
    fn of(session: &Session) -> Told<'_> {
        Told {
            session,
            events: Vec::new(),
        }
    }

    fn has(&mut self, want: &Event) -> bool {
        until(|| {
            self.events.extend(self.session.screen.take().events);
            self.events.contains(want)
        })
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The test server's program: named by `KAGAZ_RDP_TEST_SERVER`, or in
/// the build folder the tests themselves were built into.
fn program() -> Option<std::path::PathBuf> {
    if let Some(named) = std::env::var_os("KAGAZ_RDP_TEST_SERVER") {
        return Some(named.into());
    }
    let name = if cfg!(windows) { "kagaz-rdp-test-server.exe" } else { "kagaz-rdp-test-server" };
    // The build folder is some levels above the test's own program.
    let test = std::env::current_exe().ok()?;
    test.ancestors().skip(1).take(6).map(|dir| dir.join(name)).find(|built| built.is_file())
}

/// A server for "asha" with the password "sesame", on a port of its own;
/// none where its program has not been built.
fn serve(width: u16, height: u16) -> Option<TestServer> {
    serve_with(width, height, &[])
}

/// With `--fit` the server draws its screen at the size the viewer asks
/// for, as a real computer does.
fn serve_with(width: u16, height: u16, more: &[&str]) -> Option<TestServer> {
    let Some(program) = program() else {
        eprintln!("skipped: build tools/rdp-test-server to test Remote Desktop sessions");
        return None;
    };
    let mut child = Command::new(program)
        .args(["--size", &format!("{width}x{height}")])
        .args(more)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the test server starts");
    let input = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut word = |start: &str| -> String {
        let line = lines.next().expect("the test server says where it listens").unwrap();
        line.strip_prefix(start).unwrap_or_else(|| panic!("expected `{start}…`, got `{line}`")).to_string()
    };
    let addr: SocketAddr = word("listening ").parse().unwrap();
    let fingerprint = word("fingerprint ");
    let heard = Arc::new(Mutex::new(Vec::new()));
    {
        let heard = heard.clone();
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                if let Some(what) = line.strip_prefix("heard ") {
                    heard.lock().unwrap().push(what.to_string());
                }
            }
        });
    }
    Some(TestServer {
        child,
        input,
        addr,
        fingerprint,
        heard,
    })
}

fn connect(server: &TestServer, user: Option<&str>, password: Option<&str>, trust: &Trust) -> Result<Session, OpenError> {
    let sign_in = SignIn {
        user: user.map(str::to_string),
        password: password.map(str::to_string),
    };
    open(server.addr, &sign_in, &Settings::default(), trust, None, Duration::from_secs(10))
}

fn until(mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + SOON;
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// How far two pictures are apart on average, a colour of a pixel.
fn apart(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 255.0;
    }
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>() / a.len() as f64
}

/// The flat bars at the top of the card: any codec keeps those close.
fn bars_apart(session: &Session, card: &[u8], width: u16) -> f64 {
    let (w, _, rgba) = session.screen.snapshot();
    let rows = 16 * width as usize * 4;
    if w != width || rgba.len() < rows {
        return 255.0;
    }
    apart(&rgba[..rows], &card[..rows])
}

#[test]
fn the_card_arrives_and_follows_the_ticks() {
    let (w, h) = (320u16, 240u16);
    let Some(mut server) = serve(w, h) else { return };
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    assert_eq!(session.info.speaks, Speaks::Rdp);
    assert!(session.info.encrypted);
    let card = pattern::card(w, h, 0);
    assert!(until(|| session.screen.size() == (w, h) && bars_apart(&session, &card, w) < 6.0));
    // The whole picture is the card, to within what the codec loses.
    assert!(until(|| apart(&session.screen.snapshot().2, &card) < 12.0), "{}", apart(&session.screen.snapshot().2, &card));
    // The lowest band holds the tick in squares: it changes with it.
    let before = session.screen.snapshot().2;
    server.tick(9);
    let card = pattern::card(w, h, 9);
    assert!(until(|| apart(&session.screen.snapshot().2, &card) < 12.0));
    assert!(session.screen.snapshot().2 != before);
    assert!(session.numbers.read().received > 0);
}

#[test]
fn who_signs_in_is_asked_for_and_checked() {
    let Some(server) = serve(64, 64) else { return };
    assert!(matches!(connect(&server, None, None, &Trust::default()), Err(OpenError::NeedsUser)));
    assert!(matches!(connect(&server, Some(" "), Some("x"), &Trust::default()), Err(OpenError::NeedsUser)));
    assert!(matches!(
        connect(&server, Some("asha"), None, &Trust::default()),
        Err(OpenError::NeedsPassword)
    ));
    match connect(&server, Some("asha"), Some("wrong"), &Trust::default()) {
        Err(OpenError::WrongPassword) => {}
        other => panic!("a wrong password gave {:?}", other.err()),
    }
    assert!(connect(&server, Some("asha"), Some("sesame"), &Trust::default()).is_ok());
}

#[test]
fn the_certificate_is_told_and_a_changed_one_stops_the_sign_in() {
    let Some(server) = serve(64, 64) else { return };
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let identity = session.info.identity.clone().unwrap();
    assert_eq!(identity.certificate.as_deref(), Some(server.fingerprint.as_str()));
    assert_eq!(identity.name.as_deref(), Some("rcgen self signed cert"));
    drop(session);
    // The one remembered: fine.
    let same = Trust {
        known: Some(server.fingerprint.clone()),
        accept_new: false,
    };
    assert!(connect(&server, Some("asha"), Some("sesame"), &same).is_ok());
    // Another one remembered: stopped before the password is sent, even a
    // wrong password is not what is reported.
    let other = Trust {
        known: Some("00".repeat(32)),
        accept_new: false,
    };
    assert!(matches!(
        connect(&server, Some("asha"), Some("wrong"), &other),
        Err(OpenError::CertificateChanged)
    ));
    // The person takes the new one.
    let taken = Trust {
        known: Some("00".repeat(32)),
        accept_new: true,
    };
    assert!(connect(&server, Some("asha"), Some("sesame"), &taken).is_ok());
}

#[test]
fn the_mouse_and_the_keyboard_arrive() {
    let Some(server) = serve(200, 150) else { return };
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    session.send(Input::Pointer { x: 51, y: 61, buttons: 0 });
    session.send(Input::Pointer { x: 51, y: 61, buttons: LEFT });
    session.send(Input::Pointer { x: 51, y: 61, buttons: 0 });
    session.send(Input::Pointer { x: 52, y: 61, buttons: RIGHT });
    session.send(Input::Pointer { x: 52, y: 61, buttons: 0 });
    session.send(Input::Key { key: "a".into(), code: "KeyA".into(), down: true });
    session.send(Input::Key { key: "a".into(), code: "KeyA".into(), down: false });
    session.send(Input::Key { key: "Delete".into(), code: "Delete".into(), down: true });
    session.send(Input::Key { key: "Delete".into(), code: "Delete".into(), down: false });
    // No key of the board: the letter itself.
    session.send(Input::Key { key: "न".into(), code: "".into(), down: true });
    session.send(Input::Wheel { x: 52, y: 61, up: -1, right: 0 });
    let want = [
        "Move { x: 51, y: 61 }",
        "LeftPressed",
        "LeftReleased",
        "Move { x: 52, y: 61 }",
        "RightPressed",
        "RightReleased",
        "Pressed { code: 30, extended: false }",
        "Released { code: 30, extended: false }",
        "Pressed { code: 83, extended: true }",
        "Released { code: 83, extended: true }",
        "UnicodePressed(2344)",
        "VerticalScroll { value: -120 }",
    ];
    // Each press or release comes with a move to where it happened.
    let acted = || -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for h in server.heard() {
            if h.starts_with("Move") && out.last() == Some(&h) {
                continue;
            }
            if h.starts_with("Move") && out.iter().rev().find(|x| x.starts_with("Move")) == Some(&h) {
                continue;
            }
            out.push(h);
        }
        out
    };
    assert!(until(|| acted().len() >= want.len()), "heard {:?}", server.heard());
    assert_eq!(acted(), want);
}

#[test]
fn left_stays_left_where_the_buttons_are_swapped() {
    let Some(server) = serve(64, 64) else { return };
    let sign_in = SignIn {
        user: Some("asha".into()),
        password: Some("sesame".into()),
    };
    let settings = Settings {
        swapped_buttons: true,
        ..Settings::default()
    };
    let session = open(server.addr, &sign_in, &settings, &Trust::default(), None, Duration::from_secs(10)).unwrap();
    session.send(Input::Pointer { x: 5, y: 5, buttons: LEFT });
    session.send(Input::Pointer { x: 5, y: 5, buttons: 0 });
    assert!(until(|| server.heard().iter().any(|h| h == "RightReleased")), "heard {:?}", server.heard());
    assert!(!server.heard().iter().any(|h| h.starts_with("Left")));
}

#[test]
fn a_screen_that_changes_size_is_followed() {
    let Some(mut server) = serve(320, 240) else { return };
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let card = pattern::card(320, 240, 0);
    assert!(until(|| bars_apart(&session, &card, 320) < 6.0));
    server.resize(400, 300);
    let card = pattern::card(400, 300, 0);
    assert!(
        until(|| session.screen.size() == (400, 300) && bars_apart(&session, &card, 400) < 6.0),
        "size {:?}",
        session.screen.size()
    );
}

#[test]
fn the_screen_is_drawn_to_fit_the_viewer_and_follows_it() {
    let Some(server) = serve_with(320, 240, &["--fit"]) else { return };
    let sign_in = SignIn {
        user: Some("asha".into()),
        password: Some("sesame".into()),
    };
    // An odd width is not a width Remote Desktop has: one less.
    let view = View {
        width: 641,
        height: 400,
        scale: 200,
    };
    let fit = Settings::default();
    let session = open(server.addr, &sign_in, &fit, &Trust::default(), Some(view), Duration::from_secs(10)).unwrap();
    assert!(session.info.resizes);
    let card = pattern::card(640, 400, 0);
    assert!(
        until(|| session.screen.size() == (640, 400) && bars_apart(&session, &card, 640) < 6.0),
        "size {:?}",
        session.screen.size()
    );
    // The window is made larger: the screen is drawn again at that size.
    session.send(Input::Resize { width: 800, height: 500 });
    let card = pattern::card(800, 500, 0);
    assert!(
        until(|| session.screen.size() == (800, 500) && bars_apart(&session, &card, 800) < 6.0),
        "size {:?}",
        session.screen.size()
    );
    // And the mouse still arrives after the screen started over.
    session.send(Input::Pointer { x: 700, y: 450, buttons: 0 });
    assert!(until(|| server.heard().iter().any(|h| h == "Move { x: 700, y: 450 }")), "heard {:?}", server.heard());
    // Made smaller again (full screen left), and then changed twice in
    // a row: each time the screen follows, down to the last size.
    for sizes in [&[(500u16, 320u16)][..], &[(900, 600), (420, 300)]] {
        for (width, height) in sizes {
            session.send(Input::Resize { width: *width, height: *height });
        }
        let (width, height) = *sizes.last().unwrap();
        let card = pattern::card(width, height, 0);
        assert!(
            until(|| session.screen.size() == (width, height) && bars_apart(&session, &card, width) < 6.0),
            "wanted {width}x{height}, size {:?}",
            session.screen.size()
        );
    }
}

/// As close as H.264 keeps the card on average: it carries colour at
/// half the size, which costs the card's fine stripes and sharp edges.
const CLOSE: f64 = 12.0;

/// How far the whole picture is from the card, on average.
fn card_apart(session: &Session, width: u16, height: u16, tick: u32) -> f64 {
    let (w, h, rgba) = session.screen.snapshot();
    if (w, h) != (width, height) {
        return 255.0;
    }
    apart(&rgba, &pattern::card(width, height, tick))
}

#[test]
fn the_newer_picture_arrives_as_h264_and_each_frame_is_acknowledged() {
    let Some(mut server) = serve_with(320, 240, &["--h264"]) else { return };
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    // The card, close to what was sent: H.264 keeps colour at half the
    // size, so the edges between colours give a little.
    assert!(
        until(|| card_apart(&session, 320, 240, 0) < CLOSE),
        "apart {} at size {:?}, heard {:?}",
        card_apart(&session, 320, 240, 0),
        session.screen.size(),
        server.heard()
    );
    let heard = server.heard();
    assert!(heard.iter().any(|h| h.starts_with("PipelineReady(V10")), "heard {heard:?}");
    assert!(heard.iter().any(|h| h.starts_with("Frame(0, ")), "heard {heard:?}");
    assert!(until(|| server.heard().contains(&"FrameAck".to_string())), "heard {:?}", server.heard());
    // It moves: the next picture is the card at that tick, not the first.
    server.tick(40);
    assert!(until(|| card_apart(&session, 320, 240, 40) < CLOSE), "apart {}", card_apart(&session, 320, 240, 40));
    eprintln!(
        "h264: {:.2} from the card it shows, {:.2} from the card before, bars {:.2}",
        card_apart(&session, 320, 240, 40),
        card_apart(&session, 320, 240, 0),
        bars_apart(&session, &pattern::card(320, 240, 40), 320)
    );
    assert!(card_apart(&session, 320, 240, 0) > 2.0 * card_apart(&session, 320, 240, 40));
    // The flat colours come out as they went in: the right colour rule.
    assert!(bars_apart(&session, &pattern::card(320, 240, 40), 320) < 3.0);
    // How it travelled is told, for the log.
    let via = session.numbers.via();
    assert!(via.starts_with("newer picture 10.") && via.contains("H.264"), "via {via}");
    assert!(session.numbers.read().work > 0 || session.numbers.read().updates > 0);
    // And the mouse arrives as before.
    session.send(Input::Pointer { x: 30, y: 40, buttons: 0 });
    assert!(until(|| server.heard().iter().any(|h| h == "Move { x: 30, y: 40 }")), "heard {:?}", server.heard());
}

#[test]
fn the_newer_picture_follows_the_viewers_size() {
    let Some(server) = serve_with(320, 240, &["--h264", "--fit"]) else { return };
    let sign_in = SignIn {
        user: Some("asha".into()),
        password: Some("sesame".into()),
    };
    // An odd height: H.264 itself only does even ones.
    let view = View {
        width: 400,
        height: 301,
        scale: 100,
    };
    let session =
        open(server.addr, &sign_in, &Settings::default(), &Trust::default(), Some(view), Duration::from_secs(10)).unwrap();
    assert!(until(|| card_apart(&session, 400, 301, 0) < CLOSE), "size {:?}, heard {:?}", session.screen.size(), server.heard());
    for (width, height) in [(640u16, 480u16), (360, 260)] {
        session.send(Input::Resize { width, height });
        assert!(
            until(|| card_apart(&session, width, height, 0) < CLOSE),
            "wanted {width}x{height}: apart {} at size {:?}, heard {:?}",
            card_apart(&session, width, height, 0),
            session.screen.size(),
            server.heard()
        );
    }
}

#[test]
fn the_older_picture_is_used_when_asked_for() {
    let Some(server) = serve_with(320, 240, &["--h264"]) else { return };
    let sign_in = SignIn {
        user: Some("asha".into()),
        password: Some("sesame".into()),
    };
    let older = Settings {
        older_picture: true,
        ..Settings::default()
    };
    let session = open(server.addr, &sign_in, &older, &Trust::default(), None, Duration::from_secs(10)).unwrap();
    assert_eq!(session.numbers.via(), "older picture");
    // This test server sends nothing but H.264, so no picture comes; the
    // channel for it is never opened.
    std::thread::sleep(Duration::from_millis(600));
    assert!(!server.heard().iter().any(|h| h.starts_with("PipelineReady")), "heard {:?}", server.heard());
}

#[test]
fn its_own_size_is_kept_when_the_person_asks_for_that() {
    let Some(server) = serve_with(320, 240, &["--fit"]) else { return };
    let sign_in = SignIn {
        user: Some("asha".into()),
        password: Some("sesame".into()),
    };
    let own = Settings {
        own_size: true,
        ..Settings::default()
    };
    let view = View {
        width: 640,
        height: 400,
        scale: 100,
    };
    let session = open(server.addr, &sign_in, &own, &Trust::default(), Some(view), Duration::from_secs(10)).unwrap();
    assert!(!session.info.resizes);
    assert!(until(|| session.screen.size() == OWN_SIZE));
    session.send(Input::Resize { width: 800, height: 500 });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(session.screen.size(), OWN_SIZE);
}

#[test]
fn sizes_and_scales_remote_desktop_can_have() {
    assert_eq!(fitting(641, 400), (640, 400));
    assert_eq!(fitting(10, 10), (200, 200));
    assert_eq!(fitting(20000, 9000), (8192, 8192));
    assert_eq!(fitting_scale(50), 100);
    assert_eq!(fitting_scale(250), 250);
    assert_eq!(fitting_scale(900), 500);
}

#[test]
fn copied_text_travels_both_ways_in_any_script() {
    let Some(mut server) = serve(64, 64) else { return };
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let mut events = Vec::new();
    let mut seen = |want: &Event| -> bool {
        until(|| {
            events.extend(session.screen.take().events);
            events.contains(want)
        })
    };
    // Copied on the computer: here without a further step, its lines
    // ending the way they do here.
    std::thread::sleep(Duration::from_millis(300));
    server.copy(r"नमस्ते\nfrom the computer");
    assert!(seen(&Event::Clipboard("नमस्ते\nfrom the computer".into())), "no text from the computer");
    // Copied here: the computer has it when it asks, with its own line
    // ends.
    session.send(Input::Clipboard("यहाँ से\ntwo lines".into()));
    let want = format!("Text({:?})", "यहाँ से\r\ntwo lines");
    assert!(until(|| server.heard().contains(&want)), "heard {:?}", server.heard());
    // The same text again is not offered again.
    session.send(Input::Clipboard("यहाँ से\ntwo lines".into()));
    session.send(Input::Clipboard("and once more".into()));
    let want = format!("Text({:?})", "and once more");
    assert!(until(|| server.heard().contains(&want)), "heard {:?}", server.heard());
    assert_eq!(server.heard().iter().filter(|h| h.starts_with("Text(")).count(), 2);
}

#[test]
fn files_wait_on_the_computers_clipboard_and_go_when_pasted() {
    let Some(mut server) = serve(64, 64) else { return };
    let folder = Folder::new("pasted");
    let notes = "नमस्ते, notes\nof two lines\n".as_bytes();
    let big = made(300_000);
    let paths = vec![
        folder.file("notes.txt", notes),
        folder.file("big.bin", &big),
        folder.file("empty.bin", b""),
        // A name a Windows computer would refuse.
        folder.file("what: when?.txt", b"soon"),
    ];
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let mut told = Told::of(&session);
    // Offered at once, before the clipboard's channel is even ready.
    session.send(Input::Files(paths));
    let names: Vec<String> = ["notes.txt", "big.bin", "empty.bin", "what_ when_.txt"].map(String::from).into();
    let offered = Event::Sending(Sending::Offered {
        names,
        bytes: (notes.len() + big.len() + 4) as u64,
    });
    assert!(told.has(&offered), "told {:?}", told.events);
    assert!(until(|| server.heard().contains(&"Offer".to_string())), "heard {:?}", server.heard());
    // Nothing goes until it is pasted there.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!server.heard().iter().any(|h| h.starts_with("File")), "heard {:?}", server.heard());
    server.paste(0);
    assert!(until(|| server.heard().contains(&"PasteDone".to_string())), "heard {:?}", server.heard());
    let heard = server.heard();
    // The computer sees names and sizes, not where the files are kept.
    let list = format!(
        "FileList([(\"notes.txt\", {}), (\"big.bin\", 300000), (\"empty.bin\", 0), (\"what_ when_.txt\", 4)])",
        notes.len()
    );
    assert!(heard.contains(&list), "heard {heard:?}");
    for (name, contents) in [("notes.txt", notes), ("big.bin", &big[..]), ("empty.bin", b""), ("what_ when_.txt", b"soon")] {
        assert!(heard.contains(&arrived(name, contents)), "{name}: heard {heard:?}");
        let taken = Event::Sending(Sending::Taken {
            name: name.into(),
            size: contents.len() as u64,
        });
        assert!(told.has(&taken), "{name}: told {:?}", told.events);
    }
    // Pasted once more, they go once more.
    server.paste(0);
    assert!(until(|| server.heard().iter().filter(|h| *h == "PasteDone").count() == 2), "heard {:?}", server.heard());
    assert_eq!(server.heard().iter().filter(|h| **h == arrived("big.bin", &big)).count(), 2);
}

#[test]
fn a_file_on_its_way_arrives_whole_when_something_else_is_copied() {
    let Some(mut server) = serve(64, 64) else { return };
    let folder = Folder::new("meanwhile");
    let big = made(400_000);
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let mut told = Told::of(&session);
    session.send(Input::Files(vec![folder.file("big.bin", &big)]));
    assert!(until(|| server.heard().contains(&"Offer".to_string())), "heard {:?}", server.heard());
    // Pasted over a slow link; while it goes, text is copied here.
    server.paste(40);
    assert!(until(|| server.heard().iter().any(|h| h.starts_with("FileList"))), "heard {:?}", server.heard());
    session.send(Input::Clipboard("copied meanwhile".into()));
    assert!(told.has(&Event::Sending(Sending::Withdrawn)), "told {:?}", told.events);
    let text = format!("Text({:?})", "copied meanwhile");
    assert!(until(|| server.heard().contains(&text)), "heard {:?}", server.heard());
    assert!(until(|| server.heard().contains(&arrived("big.bin", &big))), "heard {:?}", server.heard());
    // The text was there before the file was all in.
    let heard = server.heard();
    let at = |what: &str| heard.iter().position(|h| h == what).unwrap();
    assert!(at(&text) < at(&arrived("big.bin", &big)), "heard {heard:?}");
    // How far it had got was told on the way, and that it is all there.
    let taken = Event::Sending(Sending::Taken {
        name: "big.bin".into(),
        size: 400_000,
    });
    assert!(told.has(&taken), "told {:?}", told.events);
    assert!(
        told.events.iter().any(|e| matches!(e, Event::Sending(Sending::Going { name, .. }) if name == "big.bin")),
        "told {:?}",
        told.events
    );
}

#[test]
fn an_offer_ends_when_the_computer_copies_something_of_its_own() {
    let Some(mut server) = serve(64, 64) else { return };
    let folder = Folder::new("theirs");
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let mut told = Told::of(&session);
    session.send(Input::Files(vec![folder.file("notes.txt", b"notes")]));
    assert!(until(|| server.heard().contains(&"Offer".to_string())), "heard {:?}", server.heard());
    server.copy("the computer's own");
    assert!(told.has(&Event::Sending(Sending::Withdrawn)), "told {:?}", told.events);
    assert!(told.has(&Event::Clipboard("the computer's own".into())), "told {:?}", told.events);
    // The same files can be offered again.
    session.send(Input::Files(vec![folder.0.join("notes.txt")]));
    assert!(until(|| server.heard().iter().filter(|h| *h == "Offer").count() == 2), "heard {:?}", server.heard());
    server.paste(0);
    assert!(until(|| server.heard().contains(&arrived("notes.txt", b"notes"))), "heard {:?}", server.heard());
}

#[test]
fn the_text_already_here_does_not_push_offered_files_aside() {
    let Some(mut server) = serve(64, 64) else { return };
    let folder = Folder::new("aside");
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let mut told = Told::of(&session);
    session.send(Input::Clipboard("copied before".into()));
    let text = format!("Text({:?})", "copied before");
    assert!(until(|| server.heard().contains(&text)), "heard {:?}", server.heard());
    session.send(Input::Files(vec![folder.file("notes.txt", b"notes")]));
    assert!(until(|| server.heard().contains(&"Offer".to_string())), "heard {:?}", server.heard());
    // The viewer says again what is on the clipboard here, as it does
    // whenever the person comes back to it: the files stay on offer.
    session.send(Input::Clipboard("copied before".into()));
    std::thread::sleep(Duration::from_millis(300));
    server.paste(0);
    assert!(until(|| server.heard().contains(&arrived("notes.txt", b"notes"))), "heard {:?}", server.heard());
    assert!(!told.events.contains(&Event::Sending(Sending::Withdrawn)));
    // Asked for by the person, the same text takes the files' place.
    session.send(Input::ClipboardAsked("copied before".into()));
    assert!(told.has(&Event::Sending(Sending::Withdrawn)), "told {:?}", told.events);
    assert!(until(|| server.heard().iter().filter(|h| **h == text).count() == 2), "heard {:?}", server.heard());
    // And asked for once more, it is not said once more.
    session.send(Input::ClipboardAsked("copied before".into()));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.heard().iter().filter(|h| **h == text).count(), 2);
}

#[test]
fn files_that_cannot_be_sent_are_said_in_words() {
    let Some(server) = serve(64, 64) else { return };
    let folder = Folder::new("refused");
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let mut told = Told::of(&session);
    let failed = |why: &str| Event::Sending(Sending::Failed { why: why.into() });
    session.send(Input::Files(vec![folder.0.join("gone.txt")]));
    assert!(told.has(&failed("gone.txt is not there")), "told {:?}", told.events);
    std::fs::create_dir(folder.0.join("album")).unwrap();
    session.send(Input::Files(vec![folder.file("one.txt", b"1"), folder.0.join("album")]));
    assert!(told.has(&failed("album is a folder; only files can be sent yet")), "told {:?}", told.events);
    session.send(Input::Files(Vec::new()));
    assert!(told.has(&failed("no file was chosen")), "told {:?}", told.events);
    // None of that reached the computer.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!server.heard().contains(&"Offer".to_string()), "heard {:?}", server.heard());
}

#[test]
fn a_computer_that_takes_no_files_is_said_in_words() {
    let Some(server) = serve_with(64, 64, &["--no-files"]) else { return };
    let folder = Folder::new("no-files");
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    let mut told = Told::of(&session);
    session.send(Input::Files(vec![folder.file("notes.txt", b"notes")]));
    let failed = Event::Sending(Sending::Failed {
        why: "this device does not take files over its clipboard".into(),
    });
    assert!(told.has(&failed), "told {:?}", told.events);
    // Text still travels.
    session.send(Input::Clipboard("text all the same".into()));
    let want = format!("Text({:?})", "text all the same");
    assert!(until(|| server.heard().contains(&want)), "heard {:?}", server.heard());
}

#[test]
fn the_end_is_told_and_keys_are_let_go() {
    let Some(server) = serve(64, 64) else { return };
    let session = connect(&server, Some("asha"), Some("sesame"), &Trust::default()).unwrap();
    session.send(Input::Key { key: "Control".into(), code: "ControlLeft".into(), down: true });
    assert!(until(|| server.heard().iter().any(|h| h == "Pressed { code: 29, extended: false }")));
    session.close();
    let mut ended = None;
    assert!(until(|| {
        for e in session.screen.take().events {
            if let Event::Ended(why) = e {
                ended = Some(why);
            }
        }
        ended.is_some()
    }));
    assert_eq!(ended.as_deref(), Some("closed"));
    assert!(until(|| server.heard().iter().any(|h| h == "Released { code: 29, extended: false }")), "heard {:?}", server.heard());
}

#[test]
fn nothing_listening_is_an_error_in_words() {
    // A port that is taken but not listened on: nothing else can come to
    // listen there while the test runs.
    let held = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    held.bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into()).unwrap();
    let free = held.local_addr().unwrap().as_socket().unwrap();
    let sign_in = SignIn {
        user: Some("asha".into()),
        password: Some("sesame".into()),
    };
    match open(free, &sign_in, &Settings::default(), &Trust::default(), None, Duration::from_secs(2)) {
        Err(OpenError::Other(why)) => assert_eq!(why, "nothing answers there"),
        other => panic!("{:?}", other.err()),
    }
}

#[test]
fn the_reason_for_an_end_reads_as_a_sentence() {
    assert_eq!(ended_words(""), "the device closed the session");
    assert_eq!(ended_words("User Requested Disconnect."), "the device closed the session (user requested disconnect)");
}
