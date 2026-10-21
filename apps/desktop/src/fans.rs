//! Ceiling fans (Atomberg) for the window: the fans the person added, by
//! room, kept in `<config>/fans.toml` (kagaz-core `fans::store`), the fans
//! within reach, and the controls. One exchange with a fan at a time: the
//! Bluetooth adapter is shared, and two at once make both fail.

use super::blocking;
use kagaz_core::fans::{self, Command, FanState, Fans, Found, Reach};
use serde::Serialize;
use std::sync::Mutex;

static BUSY: Mutex<()> = Mutex::new(());

fn load() -> Result<Fans, String> {
    Fans::load(&Fans::default_path()).map_err(|e| e.to_string())
}

fn save(fans: &Fans) -> Result<(), String> {
    fans.save(&Fans::default_path()).map_err(|e| e.to_string())
}

/// What a scan found: the remembered fans, refreshed, with which of them
/// answered; the fans within reach that are not remembered; and the
/// Bluetooth trouble, if any.
#[derive(Debug, Clone, Serialize)]
pub struct Scan {
    pub fans: Fans,
    pub reachable: Vec<String>,
    pub found: Vec<Found>,
    pub bluetooth: Option<String>,
}

/// A fan answered a command: its state, how it was reached, and the list
/// with that noted.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub state: FanState,
    pub reach: Reach,
    pub fans: Fans,
}

#[tauri::command]
pub fn fans_saved() -> Result<Fans, String> {
    load()
}

/// Look for fans for about `seconds`, refresh the remembered ones that
/// were seen (asking each over Bluetooth how it is), and list the rest.
#[tauri::command]
pub async fn fans_scan(seconds: f32) -> Result<Scan, String> {
    blocking(move || {
        let _one = BUSY.lock().map_err(|e| e.to_string())?;
        let mut fans = load()?;
        let (found, trouble) = fans::scan(seconds.clamp(1.0, 30.0));
        fans.note_seen(&found);
        let mut reachable: Vec<String> = found.iter().map(|f| f.id.clone()).collect();
        let to_ask: Vec<fans::Fan> = found
            .iter()
            .filter(|f| f.state.is_none())
            .filter_map(|f| fans.find(&f.id).cloned())
            .collect();
        for fan in to_ask {
            match fans::read(&fan) {
                Ok((state, reach)) => fans.note_state(&fan.id, &state, reach.label()),
                Err(_) => reachable.retain(|id| id != &fan.id),
            }
        }
        save(&fans)?;
        let others = found
            .into_iter()
            .filter(|f| !fans.is_remembered(&f.id))
            .collect();
        Ok(Scan {
            fans,
            reachable,
            found: others,
            bluetooth: trouble.map(|e| e.to_string()),
        })
    })
    .await
}

#[tauri::command]
pub fn fan_add(found: Found, name: String, room: String) -> Result<Fans, String> {
    let mut fans = load()?;
    fans.add(&found, &name, &room).map_err(|e| e.to_string())?;
    save(&fans)?;
    Ok(fans)
}

#[tauri::command]
pub fn fan_forget(id: String) -> Result<Fans, String> {
    let mut fans = load()?;
    fans.forget(&id).map_err(|e| e.to_string())?;
    save(&fans)?;
    Ok(fans)
}

#[tauri::command]
pub fn fan_rename(id: String, name: String) -> Result<Fans, String> {
    let mut fans = load()?;
    fans.rename(&id, &name).map_err(|e| e.to_string())?;
    save(&fans)?;
    Ok(fans)
}

#[tauri::command]
pub fn fan_set_room(id: String, room: String) -> Result<Fans, String> {
    let mut fans = load()?;
    fans.set_room(&id, &room).map_err(|e| e.to_string())?;
    save(&fans)?;
    Ok(fans)
}

#[tauri::command]
pub fn room_add(name: String) -> Result<Fans, String> {
    let mut fans = load()?;
    fans.room_add(&name).map_err(|e| e.to_string())?;
    save(&fans)?;
    Ok(fans)
}

#[tauri::command]
pub fn room_rename(old: String, new: String) -> Result<Fans, String> {
    let mut fans = load()?;
    fans.room_rename(&old, &new).map_err(|e| e.to_string())?;
    save(&fans)?;
    Ok(fans)
}

#[tauri::command]
pub fn room_remove(name: String) -> Result<Fans, String> {
    let mut fans = load()?;
    fans.room_remove(&name);
    save(&fans)?;
    Ok(fans)
}

#[tauri::command]
pub fn room_order(order: Vec<String>) -> Result<Fans, String> {
    let mut fans = load()?;
    fans.room_order(&order);
    save(&fans)?;
    Ok(fans)
}

/// Tell a remembered fan something (or ask, with `read`) and bring its
/// state back.
#[tauri::command]
pub async fn fan_send(id: String, command: Command) -> Result<Outcome, String> {
    blocking(move || {
        let _one = BUSY.lock().map_err(|e| e.to_string())?;
        let mut fans = load()?;
        let fan = fans
            .find(&id)
            .cloned()
            .ok_or_else(|| format!("no fan \"{id}\""))?;
        let (state, reach) = fans::send(&fan, &command).map_err(|e| e.to_string())?;
        fans.note_state(&fan.id, &state, reach.label());
        save(&fans)?;
        Ok(Outcome { state, reach, fans })
    })
    .await
}
