//! What the phone does differently. Android has no print queues an app can
//! reach and no folder a scan can simply be written to, so:
//!
//! - printing through "the installed driver" means Android's print dialog,
//!   which uses whatever print service (vendor plugin, Mopria) the phone
//!   has; the document goes to it as a PDF;
//! - a scan is written to the app's cache, then copied into Downloads/Kagaz
//!   where the Files app and other apps see it.
//!
//! The Kotlin half is `KagazPlugin` in the Android project
//! (gen/android/app/src/main/java/life/omniverse/kagaz/KagazPlugin.kt); it
//! also holds Wi-Fi's multicast lock so discovery hears mDNS and WSD.

use kagaz_core::output::{self, Format, OutputOptions};
use kagaz_core::print::{Fit, PrintRequest};
use kagaz_core::scan::{ColorMode, Page};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime, Wry};

struct Phone<R: Runtime>(PluginHandle<R>);

/// The plugin that connects to the Kotlin side.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("kagaz-phone")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle = api.register_android_plugin("life.omniverse.kagaz", "KagazPlugin")?;
                app.manage(Phone(handle));
            }
            #[cfg(not(target_os = "android"))]
            let _ = (app, api);
            Ok(())
        })
        .build()
}

/// Kotlin's side of the ring and the saved sign-in (KagazNative.kt),
/// called directly: plugin calls go through the app's window, and Android
/// takes that away when the window closes while the doorbell line runs on.
#[cfg(target_os = "android")]
mod jvm {
    use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
    use jni::{JNIEnv, JavaVM};
    use std::sync::OnceLock;

    static KAGAZ: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

    /// KagazNative.attach, from MainActivity before the app starts.
    #[no_mangle]
    pub extern "system" fn Java_life_omniverse_kagaz_KagazNative_hello(env: JNIEnv, class: JClass) {
        if KAGAZ.get().is_some() {
            return;
        }
        if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) {
            let _ = KAGAZ.set((vm, class));
        }
    }

    /// Run `f` with this thread attached to Java; a Java exception becomes an error.
    fn call<T>(f: impl FnOnce(&mut JNIEnv, &JClass) -> jni::errors::Result<T>) -> Result<T, String> {
        let (vm, class) = KAGAZ.get().ok_or("Android is not reachable yet")?;
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let class: &JClass = class.as_obj().into();
        let out = f(&mut env, class);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        out.map_err(|e| e.to_string())
    }

    pub fn call_changed(r: &super::Ring) {
        let _ = call(|env, class| {
            let phase = JObject::from(env.new_string(&r.phase)?);
            let peer = JObject::from(env.new_string(&r.peer)?);
            let tone = JObject::from(env.new_string(&r.tone)?);
            env.call_static_method(
                class,
                "callChanged",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ZZ)V",
                &[
                    JValue::Object(&phase),
                    JValue::Object(&peer),
                    JValue::Object(&tone),
                    JValue::Bool(r.silent.into()),
                    JValue::Bool(r.vibrate.into()),
                ],
            )?;
            Ok(())
        });
    }

    pub fn secret_get(name: &str) -> Result<Option<String>, String> {
        call(|env, class| {
            let name = JObject::from(env.new_string(name)?);
            let value = env
                .call_static_method(class, "secretGet", "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&name)])?
                .l()?;
            if value.is_null() {
                return Ok(None);
            }
            let value: String = env.get_string(&JString::from(value))?.into();
            Ok(Some(value))
        })
    }

    pub fn secret_set(name: &str, value: &str) -> Result<(), String> {
        call(|env, class| {
            let name = JObject::from(env.new_string(name)?);
            let value = JObject::from(env.new_string(value)?);
            env.call_static_method(
                class,
                "secretSet",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(&name), JValue::Object(&value)],
            )?;
            Ok(())
        })
    }

    pub fn secret_delete(name: &str) -> Result<(), String> {
        call(|env, class| {
            let name = JObject::from(env.new_string(name)?);
            env.call_static_method(class, "secretDelete", "(Ljava/lang/String;)V", &[JValue::Object(&name)])?;
            Ok(())
        })
    }

    /// A static `(…)String` method's answer.
    fn text(env: &mut JNIEnv, value: jni::objects::JValueOwned) -> jni::errors::Result<String> {
        let value = value.l()?;
        if value.is_null() {
            return Ok(String::new());
        }
        Ok(env.get_string(&JString::from(value))?.into())
    }

    /// KagazBle.scan: "ok:" and a JSON list of the named devices heard, or
    /// "err:<code>:<detail>".
    pub fn ble_scan(millis: i32) -> Result<String, String> {
        call(|env, class| {
            let out = env.call_static_method(class, "bleScan", "(I)Ljava/lang/String;", &[JValue::Int(millis)])?;
            text(env, out)
        })
    }

    /// KagazBle.exchange, on the kept connection to `address`: "ok:" and
    /// the state characteristic's text, or "err:<code>:<detail>".
    pub fn ble_exchange(address: &str, uuids: [String; 3], command: Option<&str>) -> Result<String, String> {
        call(|env, class| {
            let address = JObject::from(env.new_string(address)?);
            let [service, write, read] = uuids;
            let service = JObject::from(env.new_string(service)?);
            let write = JObject::from(env.new_string(write)?);
            let read = JObject::from(env.new_string(read)?);
            let command = match command {
                Some(c) => JObject::from(env.new_string(c)?),
                None => JObject::null(),
            };
            let out = env.call_static_method(
                class,
                "bleExchange",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[
                    JValue::Object(&address),
                    JValue::Object(&service),
                    JValue::Object(&write),
                    JValue::Object(&read),
                    JValue::Object(&command),
                ],
            )?;
            text(env, out)
        })
    }

    pub fn ble_close(address: &str) {
        let _ = call(|env, class| {
            let address = JObject::from(env.new_string(address)?);
            env.call_static_method(class, "bleClose", "(Ljava/lang/String;)V", &[JValue::Object(&address)])?;
            Ok(())
        });
    }
}

/// A remembered secret, from the Android Keystore-sealed store.
pub fn secret_get(name: &str) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    return jvm::secret_get(name);
    #[cfg(not(target_os = "android"))]
    {
        let _ = name;
        Err("this phone's secure storage is not available".into())
    }
}

pub fn secret_set(name: &str, value: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    return jvm::secret_set(name, value);
    #[cfg(not(target_os = "android"))]
    {
        let _ = (name, value);
        Err("this phone's secure storage is not available".into())
    }
}

pub fn secret_delete(name: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    return jvm::secret_delete(name);
    #[cfg(not(target_os = "android"))]
    {
        let _ = name;
        Err("this phone's secure storage is not available".into())
    }
}

/// The phone's Bluetooth for the fans (kagaz-core `fans::ble` on Android),
/// through KagazBle.kt.
#[cfg(target_os = "android")]
pub fn fan_radio() -> Box<dyn kagaz_core::fans::ble::Radio> {
    Box::new(FanRadio)
}

#[cfg(target_os = "android")]
struct FanRadio;

/// Kotlin's answer: "ok:<text>", or "err:<code>:<detail>" with the codes
/// KagazBle.kt gives.
#[cfg(target_os = "android")]
fn fan_answer(out: Result<String, String>) -> Result<String, kagaz_core::fans::FanError> {
    use kagaz_core::fans::FanError;
    let out = out.map_err(FanError::Bluetooth)?;
    if let Some(text) = out.strip_prefix("ok:") {
        return Ok(text.to_string());
    }
    let rest = out.strip_prefix("err:").unwrap_or(&out);
    let (code, detail) = rest.split_once(':').unwrap_or((rest, ""));
    Err(match code {
        "none" => FanError::NoBluetooth,
        "off" => FanError::BluetoothOff,
        "denied" => FanError::Bluetooth(
            "Kagaz is not allowed to use Bluetooth; allow Nearby devices for Kagaz in the phone's Settings, then try again"
                .into(),
        ),
        "reach" => FanError::OutOfReach,
        "busy" => FanError::Busy,
        "refuses" => FanError::Refuses,
        _ => FanError::Bluetooth(detail.to_string()),
    })
}

#[cfg(target_os = "android")]
impl kagaz_core::fans::ble::Radio for FanRadio {
    fn scan(&self, seconds: f32) -> Result<Vec<kagaz_core::fans::ble::Heard>, kagaz_core::fans::FanError> {
        #[derive(Deserialize)]
        struct One {
            address: String,
            name: String,
            rssi: Option<i16>,
        }
        let text = fan_answer(jvm::ble_scan((seconds * 1000.0) as i32))?;
        let heard: Vec<One> =
            serde_json::from_str(&text).map_err(|e| kagaz_core::fans::FanError::Bluetooth(e.to_string()))?;
        Ok(heard
            .into_iter()
            .map(|h| kagaz_core::fans::ble::Heard {
                address: h.address,
                name: h.name,
                rssi: h.rssi,
            })
            .collect())
    }

    fn exchange(&self, address: &str, command: Option<&str>) -> Result<String, kagaz_core::fans::FanError> {
        use kagaz_core::fans::atomberg::{BLE_COMMAND, BLE_SERVICE, BLE_STATE};
        let uuids = [BLE_SERVICE.to_string(), BLE_COMMAND.to_string(), BLE_STATE.to_string()];
        fan_answer(jvm::ble_exchange(address, uuids, command))
    }

    fn close(&self, address: &str) {
        jvm::ble_close(address);
    }
}

fn phone(app: &AppHandle) -> Result<&PluginHandle<Wry>, String> {
    app.try_state::<Phone<Wry>>()
        .map(|s| &s.inner().0)
        .ok_or_else(|| "this phone's print and file services are not available".to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrintArgs<'a> {
    path: &'a str,
    job_name: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveArgs<'a> {
    path: &'a str,
    name: &'a str,
    mime: &'a str,
    folder: Option<String>,
    label: Option<String>,
}

#[derive(Deserialize)]
struct Saved {
    location: String,
}

fn scratch(name: &str) -> Result<PathBuf, String> {
    let dir = kagaz_core::paths::cache_dir().join("phone");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(name))
}

/// Pages as one PDF in the cache.
fn pages_pdf(pages: &[Page], color: bool, name: &str) -> Result<PathBuf, String> {
    let path = scratch(name)?;
    output::write(
        pages,
        &OutputOptions {
            format: Format::Pdf,
            color: if color {
                ColorMode::Color
            } else {
                ColorMode::Gray
            },
            quality: Some(90),
            max_bytes: None,
        },
        &path,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

fn open_dialog(
    app: &AppHandle,
    pdf: &Path,
    job_name: &str,
) -> Result<super::printing::Sent, String> {
    let path = pdf.to_str().ok_or("unusable file name")?;
    phone(app)?
        .run_mobile_plugin::<()>("print", PrintArgs { path, job_name })
        .map_err(|e| e.to_string())?;
    Ok(super::printing::Sent::new(
        "opened the phone's print dialog; copies, sides and paper are chosen there".into(),
    ))
}

/// Print a PDF, JPEG or PNG through the phone's print dialog.
pub fn print_document(
    app: &AppHandle,
    path: &Path,
    req: &PrintRequest,
) -> Result<super::printing::Sent, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let job_name = req.job_name.clone().unwrap_or_else(|| {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Kagaz print")
            .to_string()
    });
    if data.starts_with(b"%PDF") {
        return open_dialog(app, path, &job_name);
    }
    // A picture becomes a one-page PDF sized so it fits A4 or Letter.
    let img = image::load_from_memory(&data).map_err(|e| e.to_string())?;
    let dpi = ((img.width() as f32 / 8.0).max(img.height() as f32 / 10.5)).ceil() as u32;
    let page = Page {
        mime: if data.starts_with(&[0xFF, 0xD8]) {
            "image/jpeg"
        } else {
            "image/png"
        },
        data,
        dpi: dpi.max(72),
        color: ColorMode::Color,
    };
    let pdf = pages_pdf(&[page], req.color, "print.pdf")?;
    open_dialog(app, &pdf, &job_name)
}

/// Print scanned pages through the phone's print dialog.
pub fn print_scans(
    app: &AppHandle,
    pages: &[Page],
    req: &PrintRequest,
    _fit: Fit,
) -> Result<super::printing::Sent, String> {
    let pdf = pages_pdf(pages, req.color, "copy.pdf")?;
    open_dialog(app, &pdf, req.job_name.as_deref().unwrap_or("Kagaz copy"))
}

/// Copy a finished file into the chosen folder, else Downloads/Kagaz;
/// returns where it went.
pub fn publish(app: &AppHandle, path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("unusable file name")?;
    let mime = match path.extension().and_then(|e| e.to_str()) {
        Some("pdf") => "application/pdf",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("mp4") => "video/mp4",
        _ => "application/octet-stream",
    };
    let folder = super::saving::phone_folder();
    let saved: Saved = phone(app)?
        .run_mobile_plugin(
            "saveToDownloads",
            SaveArgs {
                path: path.to_str().ok_or("unusable file name")?,
                name,
                mime,
                folder: folder.clone().map(|f| f.0),
                label: folder.map(|f| f.1),
            },
        )
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(path);
    Ok(saved.location)
}

#[derive(Serialize)]
struct Nothing {}

#[derive(Serialize)]
struct FolderArgs {
    folder: Option<String>,
}

/// Where saved files go (the chosen folder, else Kagaz's in Downloads),
/// in the Files app.
pub fn show_downloads(app: &AppHandle) -> Result<(), String> {
    let folder = super::saving::phone_folder().map(|f| f.0);
    phone(app)?.run_mobile_plugin::<()>("showDownloads", FolderArgs { folder }).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct Picked {
    uri: Option<String>,
    label: Option<String>,
}

/// Android's folder picker: the folder's handle and name, or none when
/// the person backs out.
pub fn pick_folder(app: &AppHandle) -> Result<Option<(String, String)>, String> {
    let p: Picked = phone(app)?.run_mobile_plugin("pickFolder", Nothing {}).map_err(|e| e.to_string())?;
    Ok(p.uri.map(|u| (u, p.label.unwrap_or_else(|| "Your folder".into()))))
}

/// Give back the right to write to a folder no longer used.
pub fn forget_folder(app: &AppHandle, uri: &str) {
    if let Ok(p) = phone(app) {
        let _ = p.run_mobile_plugin::<()>("forgetFolder", FolderArgs { folder: Some(uri.to_string()) });
    }
}

#[derive(Serialize)]
struct SpeakerArgs {
    on: bool,
}

/// Call sound on the loudspeaker (unless a headset is connected), or back
/// to the phone's own choice.
pub fn loudspeaker(app: &AppHandle, on: bool) -> Result<(), String> {
    phone(app)?.run_mobile_plugin::<()>("loudspeaker", SpeakerArgs { on }).map_err(|e| e.to_string())
}

/// A doorbell call takes the phone's call audio (call mode and the sound,
/// also from another app's call), or gives it back.
pub fn call_audio(app: &AppHandle, on: bool) -> Result<(), String> {
    phone(app)?.run_mobile_plugin::<()>("callAudio", SpeakerArgs { on }).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct On {
    on: bool,
}

/// Whether doorbell calls ring in Do Not Disturb (the person's switch in
/// Android's settings).
pub fn ring_in_dnd(app: &AppHandle) -> Result<bool, String> {
    let r: On = phone(app)?.run_mobile_plugin("ringInDnd", Nothing {}).map_err(|e| e.to_string())?;
    Ok(r.on)
}

/// Android's page for that switch.
pub fn ring_settings(app: &AppHandle) -> Result<(), String> {
    phone(app)?.run_mobile_plugin::<()>("ringSettings", Nothing {}).map_err(|e| e.to_string())
}

/// Where a file the person will save goes first: the cache, then
/// [`publish`] moves it to Downloads.
pub fn save_path(name: &str) -> Result<String, String> {
    Ok(scratch(name)?.display().to_string())
}

#[derive(Serialize)]
struct ListenArgs {
    on: bool,
}

/// Keep the doorbell line alive in the background (a foreground service
/// with its own small notification), or let it go. Only with the window
/// open: the plugin needs it, and with the window closed the service is
/// already what keeps Kagaz running.
pub fn keep_listening(app: &AppHandle, on: bool) {
    if app.webview_windows().is_empty() {
        return;
    }
    if let Ok(p) = phone(app) {
        let _ = p.run_mobile_plugin::<()>("keepListening", ListenArgs { on });
    }
}

/// One line's call, as the phone should show it.
#[derive(Clone, Default, PartialEq)]
pub struct Ring {
    /// "ringing", "idle" or a call in progress ("talking", "in_call"…).
    pub phase: String,
    pub peer: String,
    /// The tone's file to ring with; empty for the phone's own ringtone.
    pub tone: String,
    /// The notification only, no sound.
    pub silent: bool,
    pub vibrate: bool,
}

/// The lines that can ring: calls through OneTouch (calls.rs) and door
/// stations on the network (doorbell.rs).
#[derive(Clone, Copy)]
pub enum Line {
    Calls = 0,
    Door = 1,
}

/// Each line's call, and what the phone shows now.
static RINGS: std::sync::Mutex<([Option<Ring>; 2], Option<Ring>)> = std::sync::Mutex::new(([None, None], None));

/// A line's call changed. The phone shows one: a ring first, then a call in
/// progress; it stops ringing, and leaves the lock screen, only when both
/// lines are quiet. Callers keep their changes in order (see their update).
pub fn call_changed(line: Line, ring: Ring) {
    let mut rings = RINGS.lock().unwrap();
    rings.0[line as usize] = Some(ring);
    let busy = |r: &&Ring| r.phase != "idle" && !r.phase.is_empty();
    let all = rings.0.iter().flatten();
    let shown = all
        .clone()
        .find(|r| r.phase == "ringing")
        .or_else(|| all.clone().find(busy))
        .cloned()
        .unwrap_or_else(|| Ring {
            phase: "idle".into(),
            ..Default::default()
        });
    if rings.1.as_ref() == Some(&shown) {
        return;
    }
    rings.1 = Some(shown.clone());
    #[cfg(target_os = "android")]
    jvm::call_changed(&shown);
}
