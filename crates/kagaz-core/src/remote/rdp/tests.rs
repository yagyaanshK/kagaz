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

    fn resize(&mut self, width: u16, height: u16) {
        writeln!(self.input, "size {width} {height}").unwrap();
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
