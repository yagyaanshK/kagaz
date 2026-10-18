//! A silent ring on a computer (Settings: the notification only): instead of
//! the window jumping forward with a chime, the desktop shows a quiet
//! notification; clicking it brings the window up on the ring, to answer
//! or not. It goes when the ring ends.

use tauri::AppHandle;

#[cfg(target_os = "linux")]
mod imp {
    use notify_rust::{ActionResponse, Hint, Notification, NotificationHandle, Timeout};
    use std::sync::Mutex;
    use tauri::AppHandle;

    static SHOWN: Mutex<Option<NotificationHandle>> = Mutex::new(None);

    pub fn show(app: &AppHandle, peer: &str) {
        close();
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
        *SHOWN.lock().unwrap() = Some(handle);
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = notify_rust::handle_action(id, |r: &ActionResponse| {
                if let ActionResponse::Custom(_) = r {
                    super::super::calls::open_ring(&app);
                }
            });
        });
    }

    pub fn close() {
        if let Some(h) = SHOWN.lock().unwrap().take() {
            h.close();
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use tauri::AppHandle;
    use tauri_winrt_notification::{Duration, Toast};

    pub fn show(app: &AppHandle, peer: &str) {
        let app = app.clone();
        let _ = Toast::new(Toast::POWERSHELL_APP_ID)
            .title(&format!("{peer} is ringing"))
            .text1("Click to open the call.")
            .sound(None)
            .duration(Duration::Long)
            .on_activated(move |_| {
                super::super::calls::open_ring(&app);
                Ok(())
            })
            .show();
    }

    /// A toast leaves by itself (Duration::Long).
    pub fn close() {}
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod imp {
    use tauri::{AppHandle, Manager};

    /// No notification here yet: the Dock icon asks for attention once.
    pub fn show(app: &AppHandle, _peer: &str) {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }

    pub fn close() {}
}

pub fn show(app: &AppHandle, peer: &str) {
    imp::show(app, peer);
}

pub fn close() {
    imp::close();
}
