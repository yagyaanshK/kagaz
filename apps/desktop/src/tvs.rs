//! TVs & displays in the window (kagaz-core `tv`): the TVs the person
//! added, kept in `<config>/tvs.json`; a search of the network; and showing
//! this device's screen on one. On a computer the screen comes through the
//! system's capture (the desktop's own dialog on Linux); on a phone through
//! Android's prompt and the phone's own encoder (KagazTv.kt), handed over
//! below. Beside Kagaz's own sender, the system's own is offered.

use super::blocking;
use kagaz_core::tv::{self, Way};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// A TV the person added.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Saved {
    pub name: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Where it was last seen on the network.
    #[serde(default)]
    pub address: Option<String>,
}

/// A TV found nearby, for the window.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub name: String,
    pub model: Option<String>,
    pub address: Option<String>,
    /// What it takes, in words.
    pub ways: Vec<String>,
    /// It takes Kagaz's screen mirroring over the network right now
    /// (Miracast or Google Cast).
    pub mirror: bool,
    pub added: bool,
}

static LIST: Mutex<()> = Mutex::new(());

fn path() -> std::path::PathBuf {
    kagaz_core::paths::config_dir().join("tvs.json")
}

fn read() -> Vec<Saved> {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn update(change: impl FnOnce(&mut Vec<Saved>)) -> Result<Vec<Saved>, String> {
    let _list = LIST.lock().map_err(|e| e.to_string())?;
    let mut list = read();
    change(&mut list);
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("could not save the TVs: {e}"))?;
    Ok(list)
}

#[tauri::command]
pub fn tvs_saved() -> Vec<Saved> {
    let _list = LIST.lock();
    read()
}

/// The TVs that answer on this network within a few seconds; added ones get
/// their address refreshed.
#[tauri::command]
pub async fn tvs_search() -> Result<Vec<Found>, String> {
    blocking(|| {
        let tvs = tv::nearby(Duration::from_secs(4), false)?;
        let saved = update(|list| {
            for t in &tvs {
                if let Some(s) = list.iter_mut().find(|s| s.name == t.name) {
                    s.address = t.address.map(|a| a.to_string());
                    if t.model.is_some() {
                        s.model = t.model.clone();
                    }
                }
            }
        })?;
        Ok(tvs
            .into_iter()
            .map(|t| Found {
                added: saved.iter().any(|s| s.name == t.name),
                mirror: t.has(Way::MiracastNetwork) || t.has(Way::GoogleCast),
                ways: t.ways.iter().map(|w| w.label().to_string()).collect(),
                address: t.address.map(|a| a.to_string()),
                model: t.model,
                name: t.name,
            })
            .collect())
    })
    .await
}

#[tauri::command]
pub fn tv_add(name: String, model: Option<String>, address: Option<String>) -> Result<Vec<Saved>, String> {
    update(|list| {
        if !list.iter().any(|s| s.name == name) {
            list.push(Saved { name, model, address });
        }
    })
}

#[tauri::command]
pub fn tv_forget(name: String) -> Result<Vec<Saved>, String> {
    update(|list| list.retain(|s| s.name != name))
}

/// What a session tells the window ("tv-session" events).
#[derive(Debug, Clone, Serialize)]
struct News {
    tv: String,
    line: String,
    /// "starting", "playing", "ended" or "failed".
    phase: &'static str,
}

static STOP: AtomicBool = AtomicBool::new(false);
static SHOWING: Mutex<Option<String>> = Mutex::new(None);

/// The TV this device's screen is on now, if any.
#[tauri::command]
pub fn tv_showing() -> Option<String> {
    SHOWING.lock().ok().and_then(|s| s.clone())
}

/// Shows this device's screen on the TV called `name`. `what`: "screen"
/// (the desktop asks which on Linux), "window" (Linux), or "extend" (a new
/// monitor for the TV, Linux). Runs until stopped; news comes as
/// "tv-session" events. On a phone, Android's consent comes first, from the
/// page (plugin `kagaz-tv|consent`).
#[tauri::command]
pub fn tv_show(app: AppHandle, name: String, what: String) -> Result<(), String> {
    {
        let mut showing = SHOWING.lock().map_err(|e| e.to_string())?;
        if let Some(on) = showing.as_ref() {
            return Err(format!("this screen is already on {on}"));
        }
        *showing = Some(name.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("kagaz-tv".into())
        .spawn(move || {
            let say = |line: String, phase: &'static str| {
                // Also in the system's log (logcat on a phone), for finding faults.
                eprintln!("tv: [{phase}] {line}");
                let _ = app.emit("tv-session", News { tv: name.clone(), line, phase });
            };
            say("Starting...".into(), "starting");
            let result = session(&name, &what, &say);
            match result {
                Ok(line) => say(line, "ended"),
                Err(e) => say(e, "failed"),
            }
            if let Ok(mut s) = SHOWING.lock() {
                *s = None;
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn session(name: &str, what: &str, say: &dyn Fn(String, &'static str)) -> Result<String, String> {
    use kagaz_core::tv::miracast::{video, Options};
    use kagaz_core::tv::send::{self, Said};
    let engine = video::Engine::from_settings();
    let mut pictures = pictures(what, engine)?;
    let opts = Options {
        name: "Kagaz".into(),
        max_height: 1080,
        engine,
        priority: video::Priority::from_settings(),
        length: None,
        wait: Duration::from_secs(30),
    };
    let report = send::mirror_named(name, None, &opts, pictures.as_mut(), &STOP, |s| match s {
        Said::Line(l) => say(l, "starting"),
        // A note is for the record (the phone's log), not the page.
        Said::Event(kagaz_core::tv::miracast::Event::Note(line)) => eprintln!("tv: [note] {line}"),
        Said::Event(e) => {
            let playing = matches!(e, kagaz_core::tv::miracast::Event::Playing(_) | kagaz_core::tv::miracast::Event::Progress { .. });
            if let Some(l) = send::describe(&e) {
                say(l.trim().to_string(), if playing { "playing" } else { "starting" });
            }
        }
    })?;
    Ok(send::ended(&report))
}

/// Where the pictures come from on this system.
fn pictures(
    what: &str,
    engine: kagaz_core::tv::miracast::video::Engine,
) -> Result<Box<dyn kagaz_core::tv::miracast::Pictures>, String> {
    #[cfg(target_os = "linux")]
    {
        use kagaz_core::tv::capture::{Choice, Screen};
        let choice = match what {
            "window" => Choice::Window,
            "extend" => Choice::Extend { width: 1920, height: 1080 },
            _ => Choice::Screen,
        };
        let screen = Screen::ask(choice, engine.wants_gpu_pictures())?;
        screen.first_picture(Duration::from_secs(5)).ok_or("the desktop sent no picture of the screen")?;
        Ok(Box::new(screen))
    }
    #[cfg(windows)]
    {
        let _ = engine;
        if what != "screen" {
            return Err("on Windows Kagaz shows the main screen so far".into());
        }
        let mut screen = kagaz_core::tv::capture_win::Screen::main()?;
        screen.first_picture(Duration::from_secs(5)).ok_or("Windows sent no picture of the screen")?;
        Ok(Box::new(screen))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = engine;
        if what != "screen" {
            return Err("on a Mac Kagaz shows the main display so far".into());
        }
        let screen = kagaz_core::tv::capture_mac::Screen::main()?;
        screen.first_picture(Duration::from_secs(5)).ok_or("macOS sent no picture of the screen")?;
        Ok(Box::new(screen))
    }
    #[cfg(target_os = "android")]
    {
        let _ = (what, engine);
        Ok(Box::new(kagaz_core::tv::miracast::external::PhoneScreen::new()))
    }
    #[cfg(target_os = "ios")]
    {
        let _ = (what, engine);
        Err("showing an iPhone's screen on a TV is AirPlay's, from Control Centre".into())
    }
}

/// Ends the session (the TV is told).
#[tauri::command]
pub fn tv_stop() {
    STOP.store(true, Ordering::SeqCst);
}

/// The system's own sender beside Kagaz's (on a phone the page asks the
/// plugin instead).
#[tauri::command]
pub fn tv_system_sender() -> Result<String, String> {
    #[cfg(not(target_os = "ios"))]
    {
        tv::open_system_sender()
    }
    #[cfg(target_os = "ios")]
    {
        Ok("Control Centre, Screen Mirroring, then the TV (TVs with AirPlay).".into())
    }
}

/// The phone's own steps (KagazTvPlugin.kt): "consent" (Android's prompt
/// to share the screen), "systemCast" (Android's Cast or Samsung's Smart
/// View), "dex" (Samsung DeX). Says what happened.
#[tauri::command]
pub async fn tv_phone(app: AppHandle, what: String) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        if !matches!(what.as_str(), "consent" | "systemCast" | "dex") {
            return Err(format!("no phone step called {what}"));
        }
        blocking(move || {
            let phone = app.try_state::<TvPhone>().ok_or("the phone's TV side is not ready")?;
            let said: Said = phone.0.run_mobile_plugin(&what, Nothing {}).map_err(|e| e.to_string())?;
            Ok(said.said.unwrap_or_default())
        })
        .await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, what);
        Err("this is for Kagaz on a phone".into())
    }
}

/// How a phone session's switches stand (KagazTv.kt): whether the TV gets
/// a blur in place of the screen, and where the sound plays.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Switches {
    pub live: bool,
    pub screen_hidden: bool,
    pub sound_on_tv: bool,
    /// Whether this phone can send its sound at all.
    #[serde(default)]
    pub sound: bool,
    /// The sound is wanted on the TV but the app playing keeps it to the
    /// phone (Android hands over silence for it).
    #[serde(default)]
    pub sound_kept: bool,
    /// Android can offer the quick panel's tiles itself (13 and later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiles: Option<bool>,
    /// The floating button is on (and Android allows it); `None` in news
    /// of a switch, which leaves it as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floating: Option<bool>,
}

/// The switches as they stand; `None` on a computer, which has none yet.
#[tauri::command]
pub async fn tv_controls(app: AppHandle) -> Result<Option<Switches>, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        blocking(move || {
            let phone = app.try_state::<TvPhone>().ok_or("the phone's TV side is not ready")?;
            phone.0.run_mobile_plugin("controls", Nothing {}).map(Some).map_err(|e| e.to_string())
        })
        .await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(None)
    }
}

/// Turns a switch: "screen" (on: shown), "sound" (on: on the TV), "all"
/// (on: everything shown), "float" (the floating button), "tiles"
/// (Android's offer to add the quick panel's tiles).
#[tauri::command]
pub async fn tv_control(app: AppHandle, what: String, on: bool) -> Result<Option<Switches>, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        #[derive(Serialize)]
        struct Turn {
            what: String,
            on: bool,
        }
        blocking(move || {
            let phone = app.try_state::<TvPhone>().ok_or("the phone's TV side is not ready")?;
            phone.0.run_mobile_plugin("control", Turn { what, on }).map(Some).map_err(|e| e.to_string())
        })
        .await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, what, on);
        Ok(None)
    }
}

#[cfg(target_os = "android")]
struct TvPhone(tauri::plugin::PluginHandle<tauri::Wry>);

#[cfg(target_os = "android")]
#[derive(Serialize)]
struct Nothing {}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
struct Said {
    said: Option<String>,
}

/// The phone's side (KagazTvPlugin.kt for consent and Android's own Cast).
pub fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("kagaz-tv")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = _api.register_android_plugin("app.kagaz", "KagazTvPlugin")?;
                _app.manage(TvPhone(handle));
                let _ = phone::APP.set(_app.clone());
                phone::connect();
            }
            Ok(())
        })
        .build()
}

/// The phone's encoder (KagazTv.kt), both ways.
#[cfg(target_os = "android")]
mod phone {
    use jni::objects::{GlobalRef, JByteArray, JClass, JString, JValue};
    use jni::sys::jboolean;
    use jni::{JNIEnv, JavaVM};
    use kagaz_core::tv::miracast::external;
    use std::sync::OnceLock;

    static KAGAZ_TV: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();
    /// The app, to tell its page of a change made outside it.
    pub static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

    /// The switches changed (from the notification, a tile, the floating
    /// button or the page), or a session began or ended.
    #[no_mangle]
    pub extern "system" fn Java_app_kagaz_KagazTv_changed(
        _env: JNIEnv,
        _class: JClass,
        live: jboolean,
        screen_hidden: jboolean,
        sound_on_tv: jboolean,
        sound_kept: jboolean,
    ) {
        use tauri::Emitter;
        if let Some(app) = APP.get() {
            let _ = app.emit(
                "tv-controls",
                super::Switches {
                    live: live != 0,
                    screen_hidden: screen_hidden != 0,
                    sound_on_tv: sound_on_tv != 0,
                    sound: true,
                    sound_kept: sound_kept != 0,
                    tiles: None,
                    floating: None,
                },
            );
        }
    }

    /// KagazTv.attach, when the plugin loads.
    #[no_mangle]
    pub extern "system" fn Java_app_kagaz_KagazTv_hello(env: JNIEnv, class: JClass) {
        if KAGAZ_TV.get().is_some() {
            return;
        }
        if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) {
            let _ = KAGAZ_TV.set((vm, class));
        }
    }

    /// An encoded picture from the phone's encoder.
    #[no_mangle]
    pub extern "system" fn Java_app_kagaz_KagazTv_frame(env: JNIEnv, _class: JClass, data: JByteArray, key: jboolean) {
        if let Ok(bytes) = env.convert_byte_array(&data) {
            external::push(bytes, key != 0);
        }
    }

    /// One Opus packet of the phone's sound.
    #[no_mangle]
    pub extern "system" fn Java_app_kagaz_KagazTv_sound(env: JNIEnv, _class: JClass, data: JByteArray) {
        if let Ok(bytes) = env.convert_byte_array(&data) {
            external::push_sound(bytes);
        }
    }

    /// The person stopped sharing from the notification (or Android did).
    #[no_mangle]
    pub extern "system" fn Java_app_kagaz_KagazTv_stopAsked(_env: JNIEnv, _class: JClass) {
        super::tv_stop();
    }

    fn call<T>(f: impl FnOnce(&mut JNIEnv, &JClass) -> jni::errors::Result<T>) -> Result<T, String> {
        let (vm, class) = KAGAZ_TV.get().ok_or("the phone's screen sender is not ready yet")?;
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let class: &JClass = class.as_obj().into();
        let out = f(&mut env, class);
        // Name the Java exception, not just that there was one.
        let mut thrown = None;
        if env.exception_check().unwrap_or(false) {
            let e = env.exception_occurred().ok();
            let _ = env.exception_clear();
            if let Some(e) = e {
                thrown = env
                    .call_method(&e, "toString", "()Ljava/lang/String;", &[])
                    .and_then(|v| v.l())
                    .ok()
                    .and_then(|o| env.get_string(&JString::from(o)).ok().map(String::from));
                let _ = env.exception_clear();
            }
        }
        out.map_err(|e| thrown.unwrap_or_else(|| e.to_string()))
    }

    pub fn connect() {
        external::connect(
            |w| {
                let said = call(|env, class| {
                    let v = env.call_static_method(
                        class,
                        "start",
                        "(IIII)Ljava/lang/String;",
                        &[
                            JValue::Int(w.width as i32),
                            JValue::Int(w.height as i32),
                            JValue::Int(w.fps as i32),
                            JValue::Int(w.bits_per_second as i32),
                        ],
                    )?;
                    let o = v.l()?;
                    if o.is_null() {
                        return Ok(String::new());
                    }
                    Ok(env.get_string(&JString::from(o))?.into())
                })?;
                if said.is_empty() {
                    Ok(())
                } else {
                    Err(format!("the phone's encoder did not start: {said}"))
                }
            },
            || {
                let _ = call(|env, class| env.call_static_method(class, "keyFrame", "()V", &[]).map(|_| ()));
            },
            || {
                let _ = call(|env, class| env.call_static_method(class, "stop", "()V", &[]).map(|_| ()));
            },
            || call(|env, class| env.call_static_method(class, "canSound", "()Z", &[])?.z()).unwrap_or(false),
        );
    }
}
