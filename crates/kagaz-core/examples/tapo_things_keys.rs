//! Diagnostic: the field names (not values) of the account's camera entries
//! in the cloud device list, for extending what Kagaz keeps of them.
use kagaz_core::cameras::tapo::Session;
fn main() {
    let session = Session::load(&Session::default_path()).expect("session");
    let things = session.things_raw().expect("things");
    for d in things
        .get("data")
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .take(2)
    {
        let mut keys: Vec<String> = d
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        println!("{}", keys.join(" "));
        for k in [
            "deviceType",
            "deviceModel",
            "deviceHwVer",
            "fwVer",
            "role",
            "status",
            "deviceMac",
        ] {
            if let Some(v) = d.get(k) {
                let shown = if k == "deviceMac" {
                    format!("<{} chars>", v.as_str().map(|s| s.len()).unwrap_or(0))
                } else {
                    v.to_string()
                };
                println!("  {k} = {shown}");
            }
        }
    }
}
