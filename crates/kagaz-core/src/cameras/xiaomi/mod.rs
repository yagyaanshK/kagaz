//! Xiaomi (Mi Home) cameras. They stream over Xiaomi's own peer-to-peer
//! service, never RTSP, and Kagaz speaks it itself: the account
//! ([`account`], [`phone`]: password, texted code or QR code), the cloud
//! ([`api`]: the cameras, and each connection's keys), the camera found on
//! this network ([`local`]), the transport ([`cs2`]) and the encrypted
//! streaming on it ([`miss`]), put together as a live view ([`live`]).
//! The video itself stays on the home network.

pub mod account;
pub mod api;
pub mod card;
pub mod cs2;
pub mod live;
pub mod local;
pub mod miss;
pub mod phone;

pub use account::{Login, QrLogin, Session, Step, XiaomiError};
pub use api::{Camera, Cloud};
pub use phone::PhoneLogin;
