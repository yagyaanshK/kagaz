//! Diagnostic: call one device method through the cloud passthrough and print
//! the raw result, for finding out what a camera supports.
//!
//! `cargo run -p kagaz-core --example tapo_raw -- <device-id> <method> '<params json>'`
use kagaz_core::cameras::tapo::Session;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::load(&Session::default_path()).expect("session");
    let cams = session.cameras().expect("cameras");
    let cam = cams
        .iter()
        .find(|c| c.device_id == args[1])
        .expect("camera");
    let params: serde_json::Value = serde_json::from_str(&args[3]).expect("params json");
    match session.device_request(&cam.device_id, &cam.app_server, &args[2], params) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => println!("error: {e}"),
    }
}
