//! Diagnostic: send one raw `requestData` through the cloud passthrough and
//! print the whole answer, for requests that are not plain device methods.
//!
//! `cargo run -p kagaz-core --example tapo_passthrough -- <device-id> '<requestData json>'`
//! The text `{TOKEN}` in the JSON is replaced by the session token.
use kagaz_core::cameras::tapo::Session;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::load(&Session::default_path()).expect("session");
    let cams = session.cameras().expect("cameras");
    let cam = cams
        .iter()
        .find(|c| c.device_id == args[1])
        .expect("camera");
    let text = args[2].replace("{TOKEN}", &session.token);
    let data: serde_json::Value = serde_json::from_str(&text).expect("requestData json");
    match session.passthrough_raw(&cam.device_id, &cam.app_server, data) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => println!("error: {e}"),
    }
}
