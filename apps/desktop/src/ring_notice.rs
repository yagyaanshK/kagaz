//! A silent ring on a computer (Settings: the notification only): instead of
//! the window jumping forward with a chime, the desktop shows a quiet
//! notification; clicking it brings the window up on the ring, to answer
//! or not. It goes when the ring ends. Each line that rings ("calls" for
//! OneTouch, "door" for door stations on the network) has its own.

use tauri::AppHandle;

#[cfg(target_os = "linux")]
mod imp {
    use notify_rust::{ActionResponse, Hint, Notification, NotificationHandle, Timeout};
    use std::sync::Mutex;
    use tauri::AppHandle;

    static SHOWN: Mutex<Vec<(&'static str, NotificationHandle)>> = Mutex::new(Vec::new());

    pub fn show(app: &AppHandle, line: &'static str, peer: &str) {
        close(line);
        let Ok(handle) = Notification::new()
            .appname("Kagaz")
            .summary(&format!("{peer} is ringing"))
            .body("Click to open the call.")
            .icon("kagaz-desktop")
            .hint(Hint::DesktopEntry("kagaz-desktop".into()))
            .hint(Hint::Category("call.incoming".into()))
            .hint(Hint::SuppressSound(true))
            .action("default", "Open")
            .timeout(Timeout::Never)
            .show()
        else {
            return;
        };
        let id = handle.id();
        SHOWN.lock().unwrap().push((line, handle));
        // Ends with a click or when the notification closes.
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = notify_rust::handle_action(id, |r: &ActionResponse| {
                if let ActionResponse::Custom(_) = r {
                    super::super::calls::open_ring(&app);
                }
            });
        });
    }

    pub fn close(line: &'static str) {
        let mut shown = SHOWN.lock().unwrap();
        while let Some(i) = shown.iter().position(|(l, _)| *l == line) {
            shown.remove(i).1.close();
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::Mutex;
    use tauri::AppHandle;
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    /// Windows shows toasts only for an app it has registered, which an
    /// unpacked Kagaz is not; PowerShell always is (the toast crates do this).
    const APP_ID: &str =
        "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe";
    const GROUP: &str = "kagaz-ring";

    /// The toasts on screen; kept so their click handler lives as long.
    static SHOWN: Mutex<Vec<(&'static str, ToastNotification)>> = Mutex::new(Vec::new());

    fn xml_text(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    pub fn show(app: &AppHandle, line: &'static str, peer: &str) {
        close(line);
        let shown = (|| -> windows::core::Result<ToastNotification> {
            // A call's toast stays on screen until clicked or removed; silent.
            let xml = format!(
                "<toast scenario=\"incomingCall\"><visual><binding template=\"ToastGeneric\">\
                 <text>{} is ringing</text><text>Click to open the call.</text>\
                 </binding></visual><audio silent=\"true\"/></toast>",
                xml_text(peer)
            );
            let doc = XmlDocument::new()?;
            doc.LoadXml(&HSTRING::from(xml))?;
            let toast = ToastNotification::CreateToastNotification(&doc)?;
            toast.SetTag(&HSTRING::from(line))?;
            toast.SetGroup(&HSTRING::from(GROUP))?;
            let app = app.clone();
            toast.Activated(&TypedEventHandler::new(move |_, _| {
                super::super::calls::open_ring(&app);
                Ok(())
            }))?;
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)?;
            Ok(toast)
        })();
        if let Ok(toast) = shown {
            SHOWN.lock().unwrap().push((line, toast));
        }
    }

    /// Off the screen and out of the notification centre: a click on an
    /// ended ring would open nothing.
    pub fn close(line: &'static str) {
        SHOWN.lock().unwrap().retain(|(l, _)| *l != line);
        if let Ok(history) = ToastNotificationManager::History() {
            let _ = history.RemoveGroupedTagWithId(
                &HSTRING::from(line),
                &HSTRING::from(GROUP),
                &HSTRING::from(APP_ID),
            );
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod imp {
    use tauri::{AppHandle, Manager};

    /// No notification here yet: the Dock icon asks for attention once.
    pub fn show(app: &AppHandle, _line: &'static str, _peer: &str) {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }

    pub fn close(_line: &'static str) {}
}

pub fn show(app: &AppHandle, line: &'static str, peer: &str) {
    imp::show(app, line, peer);
}

pub fn close(line: &'static str) {
    imp::close(line);
}
