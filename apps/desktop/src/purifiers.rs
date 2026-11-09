//! Air purifiers (kagaz-core `purifiers`): the list the person added, kept
//! in `<config>/purifiers.toml`, a search of the network, and add, forget
//! and rename. Switching one and reading its air come once the protocol
//! is checked against a purifier.

use super::blocking;
use kagaz_core::purifiers::{self, Found, Purifiers};
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;

static LIST: Mutex<()> = Mutex::new(());

fn read() -> Result<Purifiers, String> {
    Purifiers::load(&Purifiers::default_path()).map_err(|e| e.to_string())
}

fn load() -> Result<Purifiers, String> {
    let _list = LIST.lock().map_err(|e| e.to_string())?;
    read()
}

fn update(change: impl FnOnce(&mut Purifiers) -> Result<(), String>) -> Result<Purifiers, String> {
    let _list = LIST.lock().map_err(|e| e.to_string())?;
    let mut list = read()?;
    change(&mut list)?;
    list.save(&Purifiers::default_path()).map_err(|e| e.to_string())?;
    Ok(list)
}

#[derive(Debug, Clone, Serialize)]
pub struct Scan {
    pub purifiers: Purifiers,
    /// Ids of the added ones that answered.
    pub reachable: Vec<String>,
    /// Answered, not added.
    pub found: Vec<Found>,
}

#[tauri::command]
pub fn purifiers_saved() -> Result<Purifiers, String> {
    load()
}

/// Look for purifiers for about `seconds`; the added ones that answered
/// get their address and time refreshed.
#[tauri::command]
pub async fn purifiers_scan(seconds: f32) -> Result<Scan, String> {
    blocking(move || {
        let found = purifiers::discover(Duration::from_secs_f32(seconds.clamp(1.0, 10.0)))
            .map_err(|e| format!("could not search the network: {e}"))?;
        let list = update(|list| {
            for f in &found {
                list.seen(f);
            }
            Ok(())
        })?;
        let reachable = found.iter().filter(|f| list.find(&f.id).is_some()).map(|f| f.id.clone()).collect();
        let found = found.into_iter().filter(|f| list.find(&f.id).is_none()).collect();
        Ok(Scan { purifiers: list, reachable, found })
    })
    .await
}

#[tauri::command]
pub fn purifier_add(found: Found, name: Option<String>) -> Result<Purifiers, String> {
    update(|list| {
        list.add(&found, name.as_deref().unwrap_or(""));
        Ok(())
    })
}

#[tauri::command]
pub fn purifier_forget(id: String) -> Result<Purifiers, String> {
    update(|list| list.forget(&id).map(|_| ()).map_err(|e| e.to_string()))
}

#[tauri::command]
pub fn purifier_rename(id: String, name: String) -> Result<Purifiers, String> {
    update(|list| list.rename(&id, &name).map_err(|e| e.to_string()))
}
