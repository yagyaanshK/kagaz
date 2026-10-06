//! Cameras. Local ones speak ONVIF and RTSP; Tapo cameras at other
//! locations are reached the way the Tapo app reaches them, through
//! TP-Link's cloud relay, with the account's own login.

pub mod layout;
pub mod local;
pub mod tapo;

pub use layout::{Group, Layout, LayoutError};
