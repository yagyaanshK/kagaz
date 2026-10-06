//! TP-Link Tapo cameras over the TP-Link cloud: login, camera list, live
//! video via the relay. See `cloud` for the account side and `relay` for
//! the media side; `tls` trusts TP-Link's private certificate authority.

pub mod cloud;
pub mod relay;
pub mod serve;
pub mod tls;

pub use cloud::{Camera, CloudError, Session};
pub use relay::{request_relay, stream_preview, RelayError, RelayParams};
pub use serve::TsServer;
