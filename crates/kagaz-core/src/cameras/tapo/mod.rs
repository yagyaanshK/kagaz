//! TP-Link Tapo cameras over the TP-Link cloud: login, camera list, live
//! video via the relay. See `cloud` for the account side and `relay` for
//! the media side; `tls` trusts TP-Link's private certificate authority.

pub mod audio;
pub mod cache;
pub mod clock;
pub mod cloud;
pub mod download;
pub mod recordings;
pub mod relay;
pub mod retime;
pub mod serve;
pub mod tls;

pub use cloud::{Camera, CloudError, Session};
pub use recordings::{Clip, Detection, Recordings, SdCard};
pub use relay::{
    request_relay, request_relay_for, stream_download, stream_playback, stream_preview, RelayError,
    RelayParams, STREAM_PLAYBACK, STREAM_PREVIEW,
};
pub use serve::{PlaybackState, TsServer};

/// The engine's sources for the account's cameras: a camera found on this
/// network (paired by hardware address) with the password kept is viewed
/// straight from the camera, as the app does on the same Wi-Fi; the rest
/// go through the relay. Each camera gets an HD and a VGA stream.
pub struct LocalSources {
    pub streams: Vec<crate::extras::go2rtc::StreamSource>,
    /// device id → local address, for the cameras reached directly.
    pub local: std::collections::HashMap<String, String>,
}

pub fn local_sources(server: &TsServer, cameras: &[Camera]) -> LocalSources {
    use crate::extras::go2rtc::StreamSource;
    let session = server.session();
    let mut local = std::collections::HashMap::new();
    if !session.local_password.is_empty() {
        let macs: Vec<String> = cameras
            .iter()
            .filter(|c| !c.mac.is_empty())
            .map(|c| c.mac.clone())
            .collect();
        let paired = super::local::find_by_mac(&macs, std::time::Duration::from_secs(3));
        for c in cameras {
            if let Some(ip) = paired.get(&c.mac) {
                local.insert(c.device_id.clone(), ip.to_string());
            }
        }
    }
    let mut streams = Vec::new();
    for c in cameras {
        match local.get(&c.device_id) {
            Some(ip) => {
                // go2rtc's own Tapo source: the cloud password, hashed as the
                // camera's firmware expects (it works out which).
                let url = format!(
                    "tapo://{}@{ip}",
                    crate::extras::go2rtc::url_encode(&session.local_password)
                );
                streams.push(StreamSource {
                    name: c.name.clone(),
                    url: url.clone(),
                });
                streams.push(StreamSource {
                    name: format!("{} vga", c.name),
                    url: format!("{url}?subtype=1"),
                });
            }
            None => {
                streams.push(StreamSource {
                    name: c.name.clone(),
                    url: server.url_for(&c.device_id),
                });
                streams.push(StreamSource {
                    name: format!("{} vga", c.name),
                    url: server.vga_url_for(&c.device_id),
                });
            }
        }
    }
    LocalSources { streams, local }
}
