//! Where the files the person saves go: a camera's screenshots and
//! recordings, and on a phone scans and downloaded footage too. A folder
//! they choose (Settings), else Downloads/Kagaz. On a computer the save
//! dialogs open there; on a phone a file is made in the cache and then
//! copied into the folder (Android's own way for shared folders).

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

#[cfg(desktop)]
use tauri_plugin_dialog::DialogExt;

#[derive(Debug, Clone, Serialize)]
pub struct Folder {
    /// As the person knows it: a path, or "Downloads/Kagaz".
    label: String,
    /// Chosen by the person (else the default).
    custom: bool,
}

/// The phone's chosen folder: Android's handle for it and its name.
#[cfg(mobile)]
pub fn phone_folder() -> Option<(String, String)> {
    let uri = kagaz_core::paths::setting("save_folder_uri")?;
    let label = kagaz_core::paths::setting("save_folder_label").unwrap_or_else(|| "Your folder".into());
    Some((uri, label))
}

fn folder() -> Folder {
    #[cfg(mobile)]
    {
        match phone_folder() {
            Some((_, label)) => Folder { label, custom: true },
            None => Folder {
                label: "Downloads/Kagaz".into(),
                custom: false,
            },
        }
    }
    #[cfg(desktop)]
    {
        Folder {
            label: kagaz_core::paths::saves_dir().display().to_string(),
            custom: kagaz_core::paths::setting("save_dir").is_some(),
        }
    }
}

/// The folder new files go to on a computer (made when missing).
pub fn dir() -> PathBuf {
    let dir = kagaz_core::paths::saves_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[tauri::command]
pub fn save_folder() -> Folder {
    folder()
}

/// Choose another folder; the same one back when the person cancels.
#[tauri::command]
pub async fn save_folder_choose(app: AppHandle) -> Result<Folder, String> {
    super::blocking(move || {
        #[cfg(mobile)]
        {
            if let Some((uri, label)) = super::mobile::pick_folder(&app)? {
                if let Some((old, _)) = phone_folder() {
                    if old != uri {
                        super::mobile::forget_folder(&app, &old);
                    }
                }
                set("save_folder_uri", Some(&uri))?;
                set("save_folder_label", Some(&label))?;
            }
        }
        #[cfg(desktop)]
        {
            let picked = app
                .dialog()
                .file()
                .set_title("Save pictures and videos in")
                .set_directory(dir())
                .blocking_pick_folder();
            if let Some(p) = picked.and_then(|p| p.into_path().ok()) {
                set("save_dir", Some(&p.display().to_string()))?;
            }
        }
        Ok(folder())
    })
    .await
}

/// Back to Downloads/Kagaz.
#[tauri::command]
pub fn save_folder_reset(app: AppHandle) -> Result<Folder, String> {
    #[cfg(mobile)]
    {
        if let Some((uri, _)) = phone_folder() {
            super::mobile::forget_folder(&app, &uri);
        }
        set("save_folder_uri", None)?;
        set("save_folder_label", None)?;
    }
    #[cfg(desktop)]
    {
        let _ = &app;
        set("save_dir", None)?;
    }
    Ok(folder())
}

/// Show the folder in the file manager (on a phone, the Files app).
#[tauri::command]
pub async fn save_folder_open(app: AppHandle) -> Result<(), String> {
    #[cfg(mobile)]
    {
        super::mobile::show_downloads(&app)
    }
    #[cfg(desktop)]
    {
        let _ = app;
        super::blocking(|| super::open_folder(&dir())).await
    }
}

fn set(key: &str, value: Option<&str>) -> Result<(), String> {
    kagaz_core::paths::set_setting(key, value).map_err(|e| format!("could not save the setting: {e}"))
}

/// A file name without characters a file system or Android would refuse.
pub fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_.()".contains(c) { c } else { '-' })
        .collect();
    let s = s.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if s.is_empty() { "Kagaz".into() } else { s }
}

/// Where to write a new file called `name`: in the folder on a computer
/// (with " (2)" and so on when the name is taken), in the cache on a
/// phone until [`keep`] copies it over.
pub fn new_file(name: &str) -> Result<PathBuf, String> {
    let name = safe_name(name);
    #[cfg(mobile)]
    {
        super::mobile::save_path(&name).map(PathBuf::from)
    }
    #[cfg(desktop)]
    {
        let dir = dir();
        let path = dir.join(&name);
        if !path.exists() {
            return Ok(path);
        }
        let p = Path::new(&name);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("Kagaz");
        let ext = p.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
        (2..1000)
            .map(|n| dir.join(format!("{stem} ({n}){ext}")))
            .find(|p| !p.exists())
            .ok_or_else(|| "too many files with that name".into())
    }
}

/// A file written at [`new_file`]'s path is done: on a phone it goes into
/// the folder. Returns where it is, as the person would find it.
pub fn keep(app: &AppHandle, path: &Path) -> Result<String, String> {
    #[cfg(mobile)]
    {
        super::mobile::publish(app, path)
    }
    #[cfg(desktop)]
    {
        let _ = app;
        Ok(path.display().to_string())
    }
}

/// Save a picture (a camera's screenshot) the page made; returns where.
#[tauri::command]
pub async fn save_picture(app: AppHandle, name: String, data: String) -> Result<String, String> {
    use base64::Engine;
    super::blocking(move || {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|_| "the picture could not be read".to_string())?;
        let path = new_file(&name)?;
        std::fs::write(&path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
        keep(&app, &path)
    })
    .await
}
