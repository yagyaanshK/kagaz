//! The Xiaomi (Mi Home) account for the window: signing in by a texted
//! code, a password or a QR code (kagaz-core `cameras::xiaomi`), and, when
//! Xiaomi wants one of its own pages, that page in a window of its own that
//! carries the sign-in on and hands its token back at the end.

use super::blocking;
use base64::Engine;
use kagaz_core::cameras::xiaomi::{Login, PhoneLogin, QrLogin, Session, Step, XiaomiError};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

/// A sign-in waiting for its next answer.
enum Pending {
    Password(Login),
    Phone(PhoneLogin),
}

impl Pending {
    fn captcha(&mut self, text: &str) -> Result<Step, XiaomiError> {
        match self {
            Pending::Password(l) => l.captcha(text),
            Pending::Phone(l) => l.captcha(text),
        }
    }
    fn code(&mut self, code: &str) -> Result<Step, XiaomiError> {
        match self {
            Pending::Password(l) => l.code(code),
            Pending::Phone(l) => l.code(code),
        }
    }
    fn cookies(&self) -> Vec<(String, String)> {
        match self {
            Pending::Password(l) => l.cookies(),
            Pending::Phone(l) => l.cookies(),
        }
    }
    fn device_id(&self) -> String {
        match self {
            Pending::Password(l) => l.device_id().to_string(),
            Pending::Phone(l) => l.device_id().to_string(),
        }
    }
}

#[derive(Default)]
pub struct XiaomiState {
    pending: Arc<Mutex<Option<Pending>>>,
    /// Bumped by every new QR code and by Cancel: a wait that finds it
    /// changed gives up, and its result is not used.
    qr_round: Arc<AtomicU64>,
}

/// What the window shows next.
#[derive(Debug, Clone, Serialize)]
pub struct XiaomiStep {
    /// "done", "captcha", "code" or "page".
    step: &'static str,
    /// The captcha picture, as a data URL.
    image: String,
    /// Where the code went (masked), and how.
    to: String,
    by_email: bool,
    /// For "page": Xiaomi's page, and what it is for.
    url: String,
    title: String,
}

impl XiaomiStep {
    fn new(step: &'static str) -> XiaomiStep {
        XiaomiStep {
            step,
            image: String::new(),
            to: String::new(),
            by_email: false,
            url: String::new(),
            title: String::new(),
        }
    }
}

fn data_url(picture: &[u8]) -> String {
    let kind = if picture.starts_with(b"\x89PNG") { "png" } else { "jpeg" };
    format!(
        "data:image/{kind};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(picture)
    )
}

/// Take one step's result: save a finished sign-in, keep an unfinished one.
fn settle(slot: &Mutex<Option<Pending>>, pending: Pending, step: Result<Step, XiaomiError>) -> Result<XiaomiStep, String> {
    match step {
        Ok(Step::Done(session)) => {
            *slot.lock().unwrap() = None;
            session.save(&Session::default_path()).map_err(|e| e.to_string())?;
            Ok(XiaomiStep::new("done"))
        }
        Ok(Step::Captcha(picture)) => {
            *slot.lock().unwrap() = Some(pending);
            let mut s = XiaomiStep::new("captcha");
            s.image = data_url(&picture);
            Ok(s)
        }
        Ok(Step::Code { by_email, to }) => {
            *slot.lock().unwrap() = Some(pending);
            let mut s = XiaomiStep::new("code");
            s.by_email = by_email;
            s.to = to;
            Ok(s)
        }
        Err(XiaomiError::Page { url, title }) => {
            // Kept: the page window carries this sign-in on.
            *slot.lock().unwrap() = Some(pending);
            let mut s = XiaomiStep::new("page");
            s.url = url;
            s.title = title;
            Ok(s)
        }
        Err(e) => {
            // A wrong code or captcha can be tried again; anything else starts over.
            if matches!(e, XiaomiError::WrongCode) {
                *slot.lock().unwrap() = Some(pending);
            } else {
                *slot.lock().unwrap() = None;
            }
            Err(e.to_string())
        }
    }
}

fn old_device() -> Option<String> {
    Session::load(&Session::default_path()).ok().map(|s| s.device_id)
}

#[tauri::command]
pub fn xiaomi_signed_in() -> bool {
    Session::default_path().is_file()
}

/// Sign in with the account's email, phone number or Xiaomi ID and its password.
#[tauri::command]
pub async fn xiaomi_login_password(state: tauri::State<'_, XiaomiState>, user: String, password: String) -> Result<XiaomiStep, String> {
    let slot = state.pending.clone();
    blocking(move || {
        let mut login = Login::new(&user, &password, old_device().as_deref());
        let step = login.begin();
        settle(&slot, Pending::Password(login), step)
    })
    .await
}

/// Sign in with a phone number: Xiaomi texts a code to it.
#[tauri::command]
pub async fn xiaomi_login_phone(state: tauri::State<'_, XiaomiState>, phone: String) -> Result<XiaomiStep, String> {
    let slot = state.pending.clone();
    blocking(move || {
        let mut login = PhoneLogin::new(&phone).map_err(|e| e.to_string())?;
        let step = login.send();
        settle(&slot, Pending::Phone(login), step)
    })
    .await
}

#[tauri::command]
pub async fn xiaomi_login_captcha(state: tauri::State<'_, XiaomiState>, text: String) -> Result<XiaomiStep, String> {
    let slot = state.pending.clone();
    blocking(move || {
        let mut pending = slot.lock().unwrap().take().ok_or("This sign-in has ended; start again.")?;
        let step = pending.captcha(&text);
        settle(&slot, pending, step)
    })
    .await
}

#[tauri::command]
pub async fn xiaomi_login_code(state: tauri::State<'_, XiaomiState>, code: String) -> Result<XiaomiStep, String> {
    let slot = state.pending.clone();
    blocking(move || {
        let mut pending = slot.lock().unwrap().take().ok_or("This sign-in has ended; start again.")?;
        let step = pending.code(&code);
        settle(&slot, pending, step)
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
pub struct XiaomiQr {
    image: String,
    login_url: String,
    tips: String,
}

/// A QR code to sign in with. When it is scanned and confirmed the window
/// hears `xiaomi-signed-in`; when it fails or expires, `xiaomi-qr-failed`.
#[tauri::command]
pub async fn xiaomi_qr_start(app: AppHandle, state: tauri::State<'_, XiaomiState>) -> Result<XiaomiQr, String> {
    let round = state.qr_round.fetch_add(1, Ordering::SeqCst) + 1;
    let rounds = state.qr_round.clone();
    blocking(move || {
        let mut qr = QrLogin::start(old_device().as_deref()).map_err(|e| e.to_string())?;
        let shown = XiaomiQr {
            image: qr.image().map(|p| data_url(&p)).unwrap_or_default(),
            login_url: qr.login_url.clone(),
            tips: qr.tips.clone(),
        };
        std::thread::spawn(move || {
            let result = qr.wait_while(|| rounds.load(Ordering::SeqCst) == round);
            if rounds.load(Ordering::SeqCst) != round {
                return;
            }
            match result.and_then(|s| s.save(&Session::default_path())) {
                Ok(()) => {
                    let _ = app.emit("xiaomi-signed-in", ());
                }
                Err(e) => {
                    let _ = app.emit("xiaomi-qr-failed", e.to_string());
                }
            }
        });
        Ok(shown)
    })
    .await
}

/// Stop waiting for a QR code, and forget any sign-in in progress.
#[tauri::command]
pub fn xiaomi_cancel(state: tauri::State<'_, XiaomiState>) {
    state.qr_round.fetch_add(1, Ordering::SeqCst);
    *state.pending.lock().unwrap() = None;
}

#[tauri::command]
pub fn xiaomi_logout(state: tauri::State<'_, XiaomiState>) -> Result<(), String> {
    xiaomi_cancel(state);
    match std::fs::remove_file(Session::default_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Open Xiaomi's own page for the sign-in in progress, in a private window
/// that carries its cookies. When the page sends the person on to Xiaomi's
/// service (the end of a sign-in), the account's token is read from the
/// window, kept, and the window closed; the main window hears
/// `xiaomi-signed-in`.
#[tauri::command]
pub async fn xiaomi_open_page(app: AppHandle, state: tauri::State<'_, XiaomiState>, url: String) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = (app, state, url);
        Err("Xiaomi's page cannot open in the phone app; sign in by text message or QR code instead.".into())
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let target = url::Url::parse(&url).map_err(|e| e.to_string())?;
        let xiaomi = target.scheme() == "https"
            && target.host_str().is_some_and(|h| h == "xiaomi.com" || h.ends_with(".xiaomi.com"));
        if !xiaomi {
            return Err("that is not one of Xiaomi's pages".into());
        }
        let (cookies, device_id) = {
            let guard = state.pending.lock().unwrap();
            let p = guard.as_ref().ok_or("This sign-in has ended; start again.")?;
            (p.cookies(), p.device_id())
        };
        const LABEL: &str = "xiaomi-page";
        if let Some(old) = app.get_webview_window(LABEL) {
            let _ = old.destroy();
        }
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = app.clone();
        let slot = state.pending.clone();
        let window = tauri::WebviewWindowBuilder::new(&app, LABEL, tauri::WebviewUrl::External("about:blank".parse().unwrap()))
            .title("Xiaomi account")
            .inner_size(480.0, 760.0)
            .incognito(true)
            .on_navigation(move |u| {
                // The end of a sign-in: Xiaomi hands over to its service.
                if u.host_str() == Some("sts.api.io.mi.com") && !finished.swap(true, Ordering::SeqCst) {
                    let (handle, slot, device_id) = (handle.clone(), slot.clone(), device_id.clone());
                    // Reading cookies must not happen on the window's own thread.
                    std::thread::spawn(move || take_token(&handle, &slot, &device_id));
                }
                true
            })
            .build()
            .map_err(|e| e.to_string())?;
        for (name, value) in cookies {
            let cookie = tauri::webview::Cookie::build((name, value))
                .domain("account.xiaomi.com")
                .path("/")
                .secure(true)
                .build();
            let _ = window.set_cookie(cookie);
        }
        window.navigate(target).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn take_token(app: &AppHandle, slot: &Mutex<Option<Pending>>, device_id: &str) {
    let Some(window) = app.get_webview_window("xiaomi-page") else { return };
    let account = url::Url::parse("https://account.xiaomi.com/").expect("url");
    let cookies = window.cookies_for_url(account).unwrap_or_default();
    let find = |name: &str| cookies.iter().find(|c| c.name() == name).map(|c| c.value().to_string());
    match (find("userId"), find("passToken")) {
        (Some(user_id), Some(pass_token)) if !pass_token.is_empty() => {
            let session = Session {
                user_id,
                pass_token,
                device_id: device_id.to_string(),
                regions: Vec::new(),
            };
            match session.save(&Session::default_path()) {
                Ok(()) => {
                    *slot.lock().unwrap() = None;
                    let _ = app.emit("xiaomi-signed-in", ());
                }
                Err(e) => {
                    let _ = app.emit("xiaomi-qr-failed", e.to_string());
                }
            }
            let _ = window.destroy();
        }
        _ => {
            let _ = app.emit(
                "xiaomi-qr-failed",
                "Xiaomi's page finished, but the sign-in did not come back from it. Try the text message or QR code instead.".to_string(),
            );
        }
    }
}
