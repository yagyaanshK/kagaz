//! Xiaomi (Mi Home) cameras. They stream over Xiaomi's own peer-to-peer
//! service, never RTSP; go2rtc (1.9.13 and later) plays them when it is given
//! the Mi account's user ID and pass token, fetching the per-connection keys
//! from Xiaomi's cloud while the video itself stays on the home network.
//!
//! This module does the account part: signing in (password with Xiaomi's
//! captcha and email or phone code; a phone number and texted code; or a QR
//! code), keeping the session, and listing the account's cameras.

pub mod account;
pub mod api;
pub mod cs2;
pub mod live;
pub mod local;
pub mod miss;
pub mod phone;

pub use account::{Login, QrLogin, Session, Step, XiaomiError};
pub use api::{Camera, Cloud};
pub use phone::PhoneLogin;
