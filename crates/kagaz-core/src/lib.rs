//! Kagaz core: find printers and scanners, understand what they can do, and
//! talk to them. The CLI and the desktop app are thin layers over this crate.

pub mod cameras;
pub mod device;
pub mod discovery;
pub mod drivers;
pub mod explain;
pub mod ipp;
pub mod localtime;
pub mod output;
pub mod paths;
pub mod print;
pub mod scan;
pub mod select;

pub use device::{Device, Protocol, Service, UsbInfo};
pub use discovery::{discover, DiscoverOptions};
pub use explain::{explain, open_ports, Explanation, Host, Os, Status, Verdict};
pub use select::{find, SelectError};
