//! App-wide choices on the Settings page that Kagaz's parts read from
//! settings.toml themselves (the command line sets the same ones).

/// The settings the window may read and change, with the values each takes
/// (the first is what an unset one means).
const ALLOWED: &[(&str, &[&str])] = &[
    // Optimise for battery or for performance (sending the screen to a TV:
    // the low-power encoder and at most 30 pictures a second, or not).
    ("power", &["performance", "battery"]),
    // Which encoder sends the screen to a TV.
    ("tv_encoder", &["auto", "hardware", "software"]),
    // This computer as a screen: off at each start unless chosen here
    // (here.rs; its switch on the TVs page lasts until Kagaz closes).
    ("screen_here_at_start", &["off", "on"]),
];

fn allowed(key: &str) -> Result<&'static [&'static str], String> {
    ALLOWED.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).ok_or_else(|| format!("no setting called {key}"))
}

/// The setting's value (its default when unset).
#[tauri::command]
pub fn pref_get(key: String) -> Result<String, String> {
    let values = allowed(&key)?;
    Ok(kagaz_core::paths::setting(&key).filter(|v| values.contains(&v.as_str())).unwrap_or_else(|| values[0].to_string()))
}

/// Changes the setting (the default is stored as unset).
#[tauri::command]
pub fn pref_set(key: String, value: String) -> Result<(), String> {
    let values = allowed(&key)?;
    if !values.contains(&value.as_str()) {
        return Err(format!("{value} is not a choice for {key}"));
    }
    let stored = (value != values[0]).then_some(value.as_str());
    kagaz_core::paths::set_setting(&key, stored).map_err(|e| format!("could not save the setting: {e}"))
}
