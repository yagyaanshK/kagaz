//! Kagaz's VNC client against the test-pattern server: the picture that
//! arrives is the card that was sent, every way of packing it.

use super::server::{Heard, Options, Server};
use super::*;
use crate::remote::pattern;
use crate::remote::screen::{Piece, Shown};
use crate::remote::session::{LEFT, RIGHT};
use std::net::TcpListener;

const SOON: Duration = Duration::from_secs(10);

fn serve(options: Options) -> Server {
    Server::start(TcpListener::bind("127.0.0.1:0").unwrap(), options).unwrap()
}

fn still(width: u16, height: u16) -> Options {
    Options {
        width,
        height,
        ticks_per_second: 0,
        ..Options::default()
    }
}

fn connect(server: &Server, password: Option<&str>, settings: &Settings) -> Result<Session, OpenError> {
    let sign_in = SignIn {
        user: None,
        password: password.map(str::to_string),
    };
    open(server.addr, &sign_in, settings, Duration::from_secs(5))
}

/// Wait until `done` says so; false when it never did.
fn until(mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + SOON;
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// The largest difference in any colour of any pixel.
fn worst(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

fn shows(session: &Session, card: &[u8]) -> bool {
    until(|| session.screen.snapshot().2 == card)
}

/// A session's events as they come, kept for looking through.
struct Watch<'a> {
    session: &'a Session,
    shown: Shown,
    events: Vec<Event>,
    moves: usize,
}

impl<'a> Watch<'a> {
    fn new(session: &'a Session) -> Watch<'a> {
        Watch {
            session,
            shown: Shown::new(),
            events: Vec::new(),
            moves: 0,
        }
    }

    fn take(&mut self) {
        let taken = self.session.screen.take();
        self.moves += taken.pieces.iter().filter(|p| matches!(p, Piece::Move { .. })).count();
        self.shown.apply(&taken);
        self.events.extend(taken.events);
    }

    fn until(&mut self, mut done: impl FnMut(&Watch) -> bool) -> bool {
        let end = Instant::now() + SOON;
        while Instant::now() < end {
            self.session.screen.wait(Duration::from_millis(20));
            self.take();
            if done(self) {
                return true;
            }
        }
        false
    }
}

#[test]
fn des_as_the_books_have_it() {
    // The classic example: key 0123456789ABCDEF turns "Now is t" into
    // 3FA40E8A984D4815. A VNC password is that key with each byte's bits
    // the other way round.
    let password: Vec<u8> = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef].iter().map(|b| b.reverse_bits()).collect();
    let mut challenge = [0u8; 16];
    challenge[..8].copy_from_slice(b"Now is t");
    challenge[8..].copy_from_slice(b"Now is t");
    let got = des_response(&password, &challenge);
    assert_eq!(got[..8], [0x3f, 0xa4, 0x0e, 0x8a, 0x98, 0x4d, 0x48, 0x15]);
    assert_eq!(got[..8], got[8..]);
}

#[test]
fn the_card_arrives_whole_every_way_of_packing_it() {
    use decode::*;
    // An awkward size: not a multiple of any tile.
    let (w, h) = (333u16, 211u16);
    let card = pattern::card(w, h, 0);
    for how in [RAW, RRE, HEXTILE, ZRLE, TIGHT] {
        let server = serve(Options {
            force: Some(how),
            ..still(w, h)
        });
        let session = connect(&server, None, &Settings::default()).unwrap();
        assert_eq!(session.screen.size(), (w, h));
        assert_eq!(session.info.name, "Kagaz test pattern");
        assert!(shows(&session, &card), "packed as {how}: the picture never became the card");
    }
}

#[test]
fn what_the_viewer_prefers_is_what_it_gets() {
    let server = serve(still(200, 120));
    let session = connect(&server, None, &Settings::default()).unwrap();
    assert!(shows(&session, &pattern::card(200, 120, 0)));
    let heard = server.heard();
    let Some(Heard::Encodings(list)) = heard.iter().find(|h| matches!(h, Heard::Encodings(_))) else {
        panic!("no encodings heard");
    };
    assert_eq!(list[0], decode::TIGHT);
    // Sharpest by default: nothing may be lost, so no JPEG level is named.
    assert!(!list.iter().any(|e| (decode::JPEG_QUALITY_0..decode::JPEG_QUALITY_0 + 10).contains(e)));
}

#[test]
fn a_moving_card_is_followed_and_scrolling_arrives_as_moves() {
    let (w, h) = (320u16, 240u16);
    let server = serve(still(w, h));
    let session = connect(&server, None, &Settings::default()).unwrap();
    let mut watch = Watch::new(&session);
    assert!(watch.until(|w| w.shown.rgba == pattern::card(320, 240, 0)));
    for tick in 1..=6 {
        server.tick();
        let card = pattern::card(w, h, tick);
        assert!(watch.until(|w| w.shown.rgba == card), "tick {tick} never showed");
        // What is shown from the pieces is the screen itself.
        assert_eq!(watch.shown.rgba, session.screen.snapshot().2);
    }
    assert!(watch.moves >= 6, "scrolling came as {} moves", watch.moves);
    assert!(session.numbers.read().updates >= 7);
}

#[test]
fn every_version_of_the_greeting_with_and_without_a_password() {
    for version in ["003.003", "003.007", "003.008", "003.889", "004.001"] {
        for password in [None, Some("sesame")] {
            let server = serve(Options {
                version,
                password: password.map(str::to_string),
                ..still(64, 48)
            });
            let session = connect(&server, password, &Settings::default())
                .unwrap_or_else(|e| panic!("{version} with {password:?}: {e}"));
            assert!(shows(&session, &pattern::card(64, 48, 0)), "{version} with {password:?}");
        }
    }
}

#[test]
fn a_missing_or_wrong_password_is_told_apart() {
    for version in ["003.003", "003.007", "003.008"] {
        let server = serve(Options {
            version,
            password: Some("sesame".into()),
            ..still(64, 48)
        });
        assert!(matches!(
            connect(&server, None, &Settings::default()),
            Err(OpenError::NeedsPassword)
        ));
        assert!(
            matches!(connect(&server, Some("sesamy"), &Settings::default()), Err(OpenError::WrongPassword)),
            "{version}"
        );
        // Only the first eight letters count, as in every VNC.
        assert!(connect(&server, Some("sesame"), &Settings::default()).is_ok());
    }
}

#[test]
fn greetings_that_are_not_a_welcome() {
    let greet = |bytes: &[u8]| handshake(&mut &bytes[..], &mut Vec::new(), &SignIn::default());
    // Not VNC at all.
    assert!(matches!(greet(b"HTTP/1.1 400 Bad Request\r\n\r\n"), Err(OpenError::Other(_))));
    // Refused, with its reason.
    let mut refused = b"RFB 003.008\n\x00".to_vec();
    refused.extend_from_slice(&9u32.to_be_bytes());
    refused.extend_from_slice(b"too busy\n");
    match greet(&refused) {
        Err(OpenError::Refused(why)) => assert_eq!(why, "too busy"),
        other => panic!("{:?}", other.err()),
    }
    // Only kinds of sign-in Kagaz does not have.
    match greet(b"RFB 003.008\n\x02\x13\x1e") {
        Err(OpenError::Unsupported(why)) => {
            assert!(why.contains("VeNCrypt") && why.contains("Mac"), "{why}");
        }
        other => panic!("{:?}", other.err()),
    }
    // Cut off in the middle.
    assert!(greet(b"RFB 003").is_err());
}

#[test]
fn the_mouse_and_the_keyboard_arrive() {
    let server = serve(still(200, 150));
    let session = connect(&server, None, &Settings::default()).unwrap();
    session.send(Input::Pointer { x: 50, y: 60, buttons: 0 });
    session.send(Input::Pointer { x: 51, y: 61, buttons: LEFT });
    session.send(Input::Pointer { x: 51, y: 61, buttons: 0 });
    session.send(Input::Key { key: "A".into(), code: "KeyA".into(), down: true });
    // Shift let go first: the release still names the key that went down.
    session.send(Input::Key { key: "a".into(), code: "KeyA".into(), down: false });
    session.send(Input::Key { key: "Enter".into(), code: "Enter".into(), down: true });
    session.send(Input::Key { key: "Dead".into(), code: "Quote".into(), down: true });
    session.send(Input::Wheel { x: 51, y: 61, up: -2, right: 0 });
    let want = [
        Heard::Pointer { x: 51, y: 61, buttons: 1 },
        Heard::Pointer { x: 51, y: 61, buttons: 0 },
        Heard::Key { keysym: 0x41, down: true },
        Heard::Key { keysym: 0x41, down: false },
        Heard::Key { keysym: 0xff0d, down: true },
        // Two notches down: the wheel-down button twice.
        Heard::Pointer { x: 51, y: 61, buttons: 16 },
        Heard::Pointer { x: 51, y: 61, buttons: 0 },
        Heard::Pointer { x: 51, y: 61, buttons: 16 },
        Heard::Pointer { x: 51, y: 61, buttons: 0 },
    ];
    let acted = || -> Vec<Heard> {
        server.heard().into_iter().filter(|h| !matches!(h, Heard::Encodings(_))).collect()
    };
    assert!(until(|| acted().len() >= want.len()), "heard only {:?}", acted());
    let got = acted();
    // The first move may or may not have been folded into the next.
    let got = got.strip_prefix(&[Heard::Pointer { x: 50, y: 60, buttons: 0 }]).unwrap_or(&got);
    assert_eq!(got, want);
    // And the press shows on the card: red for the left button.
    session.send(Input::Pointer { x: 100, y: 100, buttons: LEFT });
    assert!(until(|| {
        let (w, _, rgba) = session.screen.snapshot();
        let at = (100 * w as usize + 100) * 4;
        rgba[at..at + 3] == [255, 0, 0]
    }));
}

#[test]
fn left_stays_left_where_the_buttons_are_swapped() {
    let server = serve(still(200, 150));
    let settings = Settings {
        swapped_buttons: true,
        ..Settings::default()
    };
    let session = connect(&server, None, &settings).unwrap();
    session.send(Input::Pointer { x: 10, y: 10, buttons: LEFT });
    session.send(Input::Pointer { x: 10, y: 10, buttons: 0 });
    session.send(Input::SwappedButtons(false));
    session.send(Input::Pointer { x: 10, y: 10, buttons: LEFT });
    let pressed = || -> Vec<u8> {
        server
            .heard()
            .iter()
            .filter_map(|h| match h {
                Heard::Pointer { buttons, .. } => Some(*buttons),
                _ => None,
            })
            .collect()
    };
    assert!(until(|| pressed().len() >= 3));
    // The device swaps it back: its right button is its left click.
    assert_eq!(pressed(), vec![RIGHT, 0, LEFT]);
}

#[test]
fn keys_still_down_are_let_go_when_the_session_closes() {
    let server = serve(still(64, 48));
    let session = connect(&server, None, &Settings::default()).unwrap();
    session.send(Input::Key { key: "Control".into(), code: "ControlLeft".into(), down: true });
    session.close();
    assert!(until(|| server.heard().contains(&Heard::Key { keysym: 0xffe3, down: false })));
}

#[test]
fn copied_text_travels_both_ways_in_any_script() {
    let server = serve(still(64, 48));
    let session = connect(&server, None, &Settings::default()).unwrap();
    let mut watch = Watch::new(&session);
    // From the device.
    server.copy("नमस्ते\nfrom the device");
    assert!(
        watch.until(|w| w.events.contains(&Event::Clipboard("नमस्ते\nfrom the device".into()))),
        "events: {:?}",
        watch.events
    );
    // To the device.
    session.send(Input::Clipboard("यहाँ से, with two\nlines".into()));
    assert!(until(|| server.heard().contains(&Heard::Text("यहाँ से, with two\nlines".into()))));
    // What came from the device is not sent back to it.
    session.send(Input::Clipboard("यहाँ से, with two\nlines".into()));
    session.send(Input::Clipboard("and once more".into()));
    assert!(until(|| server.heard().contains(&Heard::Text("and once more".into()))));
    let texts = server.heard().iter().filter(|h| matches!(h, Heard::Text(_))).count();
    assert_eq!(texts, 2);
    // Files have no way there over VNC, and that is said.
    session.send(Input::Files(vec!["report.pdf".into()]));
    let none = Event::Sending(Sending::Failed {
        why: "VNC carries no files".into(),
    });
    assert!(watch.until(|w| w.events.contains(&none)), "events: {:?}", watch.events);
}

#[test]
fn a_screen_that_changes_size_is_followed() {
    let server = serve(still(200, 120));
    let session = connect(&server, None, &Settings::default()).unwrap();
    let mut watch = Watch::new(&session);
    assert!(watch.until(|w| w.shown.rgba == pattern::card(200, 120, 0)));
    // A phone turned on its side.
    server.resize(120, 200);
    let card = pattern::card(120, 200, 0);
    assert!(watch.until(|w| (w.shown.width, w.shown.height) == (120, 200) && w.shown.rgba == card));
    assert_eq!(session.screen.size(), (120, 200));
}

#[test]
fn the_pointer_shape_and_the_delay_arrive() {
    let server = serve(still(64, 48));
    let session = connect(&server, None, &Settings::default()).unwrap();
    let mut watch = Watch::new(&session);
    assert!(watch.until(|w| w.events.iter().any(|e| matches!(e, Event::Pointer(p) if p.width == 12 && p.height == 18))));
    assert!(until(|| session.numbers.read().delay.is_some()));
    let reading = session.numbers.read();
    assert!(reading.delay.unwrap() < 1000);
    assert!(reading.received > 0 && reading.sent > 0);
}

#[test]
fn smoother_asks_for_jpeg_and_the_picture_stays_close() {
    let (w, h) = (256u16, 192u16);
    let server = serve(still(w, h));
    let settings = Settings {
        picture: Some(3),
        ..Settings::default()
    };
    let session = connect(&server, None, &settings).unwrap();
    // The flat bars at the top stay what they are, to within JPEG's loss
    // or exactly (flat areas are sent as one colour).
    let card = pattern::card(w, h, 0);
    let bars = 20 * w as usize * 4;
    assert!(until(|| worst(&session.screen.snapshot().2[..bars], &card[..bars]) < 40));
    let heard = server.heard();
    let Some(Heard::Encodings(list)) = heard.iter().find(|h| matches!(h, Heard::Encodings(_))) else {
        panic!("no encodings heard");
    };
    assert!(list.contains(&(decode::JPEG_QUALITY_0 + 7)), "{list:?}");
    assert_eq!(session.numbers.read().picture, 3);
    // Sharper again, in the middle of the session: asked for at once.
    session.send(Input::Picture(Some(0)));
    assert!(until(|| {
        server.heard().iter().filter(|h| matches!(h, Heard::Encodings(_))).count() >= 2
    }));
    assert_eq!(session.numbers.read().picture, 0);
}

#[test]
fn the_end_is_told_with_its_reason() {
    // Closed here.
    let server = serve(still(64, 48));
    let session = connect(&server, None, &Settings::default()).unwrap();
    let mut watch = Watch::new(&session);
    session.close();
    assert!(watch.until(|w| w.events.contains(&Event::Ended("closed".into()))));
    // The device goes away.
    let session = connect(&server, None, &Settings::default()).unwrap();
    let mut watch = Watch::new(&session);
    assert!(watch.until(|w| !w.shown.rgba.is_empty()));
    server.stop();
    assert!(
        watch.until(|w| w.events.contains(&Event::Ended("the device closed the connection".into()))),
        "events: {:?}",
        watch.events
    );
    assert_eq!(watch.events.iter().filter(|e| matches!(e, Event::Ended(_))).count(), 1);
}

#[test]
fn nothing_listening_is_an_error_in_words() {
    // A port that is taken but not listened on: nothing else can come to
    // listen there while the test runs.
    let held = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    held.bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into()).unwrap();
    let free = held.local_addr().unwrap().as_socket().unwrap();
    let got = open(free, &SignIn::default(), &Settings::default(), Duration::from_secs(2));
    match got {
        Err(OpenError::Other(why)) => assert_eq!(why, "nothing answers there"),
        other => panic!("{:?}", other.err()),
    }
}

#[test]
fn the_picture_settings_as_tight_levels() {
    assert_eq!(jpeg_level(0), None);
    assert_eq!(jpeg_level(1), Some(9));
    assert_eq!(jpeg_level(5), Some(5));
    assert_eq!(jpeg_level(10), Some(1));
    assert_eq!(picture_for(None), 0);
    assert_eq!(picture_for(Some(12)), 0);
    assert_eq!(picture_for(Some(48)), 2);
    assert_eq!(picture_for(Some(120)), 4);
    assert_eq!(picture_for(Some(400)), 6);
}
