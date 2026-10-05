//! Kagaz core: find printers and scanners, understand what they can do, and
//! talk to them. The CLI and the desktop app are thin layers over this crate.

pub mod device;
pub mod discovery;

pub use device::{Device, Protocol, Service, UsbInfo};
pub use discovery::{discover, DiscoverOptions};
