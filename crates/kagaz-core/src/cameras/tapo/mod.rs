//! TP-Link Tapo cameras over the TP-Link cloud: login, camera list, live
//! video via the relay. See `cloud` for the account side and `relay` for
//! the media side; `tls` trusts TP-Link's private certificate authority.

pub mod audio;
pub mod cloud;
pub mod download;
pub mod recordings;
pub mod relay;
pub mod serve;
pub mod tls;

pub use cloud::{Camera, CloudError, Session};
pub use recordings::{Clip, Recordings, SdCard};
pub use relay::{
    request_relay, request_relay_for, stream_download, stream_playback, stream_preview, RelayError,
    RelayParams, STREAM_PLAYBACK, STREAM_PREVIEW,
};
pub use serve::{PlaybackState, TsServer};
