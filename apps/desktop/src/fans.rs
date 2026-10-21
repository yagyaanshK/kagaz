//! Ceiling fans (Atomberg) for the window: the fans the person added, by
//! room, kept in `<config>/fans.toml` (kagaz-core `fans::store`), the fans
//! within reach, and the controls.
//!
//! Two locks. `RADIO`: one exchange with fans at a time, as the Bluetooth
//! adapter is shared and two at once make both fail. `LIST`: every change
//! to the list is load, change, save in one go, on the list as it is on
//! disk then. A scan or a command works on a copy while it talks to the
//! fans (seconds over Bluetooth) and notes what it learnt at the end, so a
//! rename or a move made meanwhile is kept.

use super::blocking;
use kagaz_core::fans::store::StoreError;
use kagaz_core::fans::{self, Command, FanState, Fans, Found, Reach};
use serde::Serialize;
use std::sync::Mutex;

static RADIO: Mutex<()> = Mutex::new(());

/// Keep one connection per fan while it is in use, closed after 45
/// seconds idle: a fan takes only so many new connections before it
/// stops answering until switched off at the wall (seen 2026-10-21).
pub fn start_links() {
    #[cfg(not(target_os = "android"))]
    {
        fans::ble::keep_links(true);
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            if let Ok(_radio) = RADIO.try_lock() {
                fans::ble::close_idle(std::time::Duration::from_secs(45));
            }
        });
    }
}

/// Let go of every fan (the window is going away).
pub fn close_links() {
    #[cfg(not(target_os = "android"))]
    fans::ble::close_all();
}
static LIST: Mutex<()> = Mutex::new(());

fn load() -> Result<Fans, String> {
    let _list = LIST.lock().map_err(|e| e.to_string())?;
    Fans::load(&Fans::default_path()).map_err(|e| e.to_string())
}

/// Change the saved list and return it as saved.
fn update(change: impl FnOnce(&mut Fans) -> Result<(), StoreError>) -> Result<Fans, String> {
    let _list = LIST.lock().map_err(|e| e.to_string())?;
    Fans::update(&Fans::default_path(), change)
        .map(|((), fans)| fans)
        .map_err(|e| e.to_string())
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
/// were seen, and list the rest. A Bluetooth fan tells its state only over
/// a connection, and fans have stopped answering after many connections,
/// so a fan is asked only when `fresh` (the person pressed Look again) or
/// when its last reading is more than ten minutes old.
#[tauri::command]
pub async fn fans_scan(seconds: f32, fresh: Option<bool>) -> Result<Scan, String> {
    let fresh = fresh.unwrap_or(false);
    blocking(move || {
        let _radio = RADIO.lock().map_err(|e| e.to_string())?;
        let (found, trouble) = fans::scan(seconds.clamp(1.0, 30.0));
        let known = update(|f| {
            f.note_seen(&found);
            Ok(())
        })?;
        let mut reachable: Vec<String> = found.iter().map(|f| f.id.clone()).collect();
        let now = fans::store::now();
        let to_ask: Vec<fans::Fan> = found
            .iter()
            .filter(|f| f.state.is_none())
            .filter_map(|f| known.find(&f.id).cloned())
            .filter(|f| {
                fresh
                    || f.last_state.is_none()
                    || f.last_seen.is_none_or(|t| now.saturating_sub(t) > 600)
            })
            .collect();
        let mut answers = Vec::new();
        for fan in to_ask {
            match fans::read(&fan) {
                Ok((state, reach)) => answers.push((fan.id, state, reach)),
                Err(_) => reachable.retain(|id| id != &fan.id),
            }
        }
        let fans = update(|f| {
            for (id, state, reach) in &answers {
                f.note_state(id, state, reach.label());
            }
            Ok(())
        })?;
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
    update(|f| f.add(&found, &name, &room).map(|_| ()))
}

#[tauri::command]
pub fn fan_forget(id: String) -> Result<Fans, String> {
    update(|f| f.forget(&id).map(|_| ()))
}

#[tauri::command]
pub fn fan_rename(id: String, name: String) -> Result<Fans, String> {
    update(|f| f.rename(&id, &name))
}

#[tauri::command]
pub fn fan_set_room(id: String, room: String) -> Result<Fans, String> {
    update(|f| f.set_room(&id, &room))
}

#[tauri::command]
pub fn room_add(name: String) -> Result<Fans, String> {
    update(|f| f.room_add(&name))
}

#[tauri::command]
pub fn room_rename(old: String, new: String) -> Result<Fans, String> {
    update(|f| f.room_rename(&old, &new))
}

#[tauri::command]
pub fn room_remove(name: String) -> Result<Fans, String> {
    update(|f| {
        f.room_remove(&name);
        Ok(())
    })
}

#[tauri::command]
pub fn room_order(order: Vec<String>) -> Result<Fans, String> {
    update(|f| {
        f.room_order(&order);
        Ok(())
    })
}

/// Tell a remembered fan something (or ask, with `read`) and bring its
/// state back.
#[tauri::command]
pub async fn fan_send(id: String, command: Command) -> Result<Outcome, String> {
    blocking(move || {
        let _radio = RADIO.lock().map_err(|e| e.to_string())?;
        let fan = load()?
            .find(&id)
            .cloned()
            .ok_or_else(|| format!("no fan \"{id}\""))?;
        let (state, reach) = fans::send(&fan, &command).map_err(|e| e.to_string())?;
        let fans = update(|f| {
            f.note_state(&fan.id, &state, reach.label());
            // Kagaz set the timer: it knows the moment exactly.
            if let Command::Timer(i) = command {
                let hours = fans::atomberg::TIMER_HOURS
                    .get(i as usize)
                    .copied()
                    .unwrap_or(0);
                f.note_timer(&fan.id, hours, fans::store::now());
            }
            Ok(())
        })?;
        Ok(Outcome { state, reach, fans })
    })
    .await
}
