//! Finding devices. Each method returns partial `Device`s; `discover` runs the
//! methods and merges the results into one entry per physical device.

pub mod mdns;
pub mod wsd;

use crate::device::{merge_devices, Device};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    /// How long to listen for answers.
    pub timeout: Duration,
    pub use_mdns: bool,
    pub use_wsd: bool,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(3),
            use_mdns: true,
            use_wsd: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    #[error("mDNS: {0}")]
    Mdns(String),
    #[error("WS-Discovery: {0}")]
    Wsd(#[from] std::io::Error),
}

/// Run every enabled discovery method and return one `Device` per physical device.
pub fn discover(opts: &DiscoverOptions) -> Result<Vec<Device>, DiscoverError> {
    let mut found = Vec::new();
    let mut errors = Vec::new();

    // Both methods wait for the full timeout, so run them side by side.
    std::thread::scope(|s| {
        let mdns_handle = opts
            .use_mdns
            .then(|| s.spawn(|| mdns::browse(opts.timeout)));
        let wsd_handle = opts.use_wsd.then(|| s.spawn(|| wsd::probe(opts.timeout)));
        if let Some(h) = mdns_handle {
            match h.join().expect("mdns thread panicked") {
                Ok(v) => found.extend(v),
                Err(e) => errors.push(DiscoverError::Mdns(e)),
            }
        }
        if let Some(h) = wsd_handle {
            match h.join().expect("wsd thread panicked") {
                Ok(v) => found.extend(v),
                Err(e) => errors.push(DiscoverError::Wsd(e)),
            }
        }
    });

    if found.is_empty() {
        if let Some(e) = errors.into_iter().next() {
            return Err(e);
        }
    }
    Ok(merge_devices(found))
}
