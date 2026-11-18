//! Wi-Fi Direct on Linux, through NetworkManager's D-Bus interface: finding
//! TVs that wait in their screen-sharing mode (on a network or not), and a
//! link of this computer's own to one, made as the signed-in user.
//!
//! The link is a volatile connection: NetworkManager forgets it when it
//! ends. While it is up, the TV is group owner and hands out an address,
//! then connects to this computer's RTSP port named in the Wi-Fi Display
//! information Kagaz sends.

use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::Connection;
use dbus::Path;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const P2P: &str = "org.freedesktop.NetworkManager.Device.WifiP2P";
const PEER: &str = "org.freedesktop.NetworkManager.WifiP2PPeer";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const TIMEOUT: Duration = Duration::from_secs(10);

/// A device seen over Wi-Fi Direct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    /// Its Wi-Fi Direct address (kept in memory, never in logs).
    pub address: String,
    /// Signal strength, 0 to 100.
    pub strength: u8,
    /// It sends Wi-Fi Display information: a TV or adapter that takes
    /// screen mirroring.
    pub display: bool,
    /// Its Wi-Fi Display information says it can take a session now.
    pub available: bool,
}

/// Kagaz's own Wi-Fi Display information: a source, a session available,
/// RTSP on port 7236, up to 50 Mbit/s.
pub fn source_information(rtsp_port: u16) -> Vec<u8> {
    let p = rtsp_port.to_be_bytes();
    vec![0x00, 0x00, 0x06, 0x00, 0x10, p[0], p[1], 0x00, 0x32]
}

/// Reads the device information subelement: (is a display, session available).
fn read_information(ies: &[u8]) -> (bool, bool) {
    let mut at = 0;
    while at + 3 <= ies.len() {
        let id = ies[at];
        let len = u16::from_be_bytes([ies[at + 1], ies[at + 2]]) as usize;
        let value = &ies[(at + 3).min(ies.len())..(at + 3 + len).min(ies.len())];
        if id == 0 && value.len() >= 2 {
            let info = u16::from_be_bytes([value[0], value[1]]);
            let kind = info & 0b11; // 0 source, 1 primary sink, 2 secondary, 3 both
            let available = (info >> 4) & 0b11 == 0b01;
            return (kind != 0, available);
        }
        at += 3 + len;
    }
    (false, false)
}

fn bus() -> Result<Connection, String> {
    Connection::new_system().map_err(|e| format!("NetworkManager could not be reached: {e}"))
}

/// NetworkManager's Wi-Fi Direct device, if this computer has one.
fn p2p_device(c: &Connection) -> Result<Path<'static>, String> {
    let nm = c.with_proxy(NM, NM_PATH, TIMEOUT);
    let devices: Vec<Path<'static>> = nm
        .get(NM, "AllDevices")
        .map_err(|e| format!("NetworkManager did not list its devices: {e}"))?;
    for d in devices {
        let p = c.with_proxy(NM, d.clone(), TIMEOUT);
        let kind: u32 = p.get("org.freedesktop.NetworkManager.Device", "DeviceType").unwrap_or(0);
        // NM_DEVICE_TYPE_WIFI_P2P
        if kind == 30 {
            return Ok(d);
        }
    }
    Err("this computer's Wi-Fi has no Wi-Fi Direct, or NetworkManager does not manage it".into())
}

fn read_peer(c: &Connection, path: &Path) -> Option<Peer> {
    let p = c.with_proxy(NM, path.clone(), TIMEOUT);
    let name: String = p.get(PEER, "Name").ok()?;
    let address: String = p.get(PEER, "HwAddress").ok()?;
    let strength: u8 = p.get(PEER, "Strength").unwrap_or(0);
    let ies: Vec<u8> = p.get(PEER, "WfdIEs").unwrap_or_default();
    let (display, available) = read_information(&ies);
    Some(Peer { name, address, strength, display, available })
}

/// Looks for Wi-Fi Direct devices for up to `wait`, returning early once
/// `enough` says the list is enough.
pub fn find(wait: Duration, enough: impl Fn(&[Peer]) -> bool) -> Result<Vec<Peer>, String> {
    let c = bus()?;
    let dev = p2p_device(&c)?;
    let p = c.with_proxy(NM, dev.clone(), TIMEOUT);
    let mut options: PropMap = HashMap::new();
    options.insert("timeout".into(), Variant(Box::new(wait.as_secs().clamp(1, 600) as i32)));
    let _: () = p
        .method_call(P2P, "StartFind", (options,))
        .map_err(|e| format!("Wi-Fi Direct search did not start: {e}"))?;
    let end = Instant::now() + wait;
    let mut peers = loop {
        let paths: Vec<Path<'static>> = p.get(P2P, "Peers").unwrap_or_default();
        let peers: Vec<Peer> = paths.iter().filter_map(|x| read_peer(&c, x)).collect();
        if enough(&peers) || Instant::now() >= end {
            break peers;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let _: Result<(), _> = p.method_call(P2P, "StopFind", ());
    peers.sort_by(|a, b| b.strength.cmp(&a.strength));
    Ok(peers)
}

/// A Wi-Fi Direct link that is up. Dropping it ends the link.
pub struct Link {
    c: Connection,
    active: Path<'static>,
}

impl Link {
    pub fn end(self) {}
}

impl Drop for Link {
    fn drop(&mut self) {
        let nm = self.c.with_proxy(NM, NM_PATH, TIMEOUT);
        let _: Result<(), _> = nm.method_call(NM, "DeactivateConnection", (self.active.clone(),));
    }
}

/// Joins the TV's Wi-Fi Direct group (or makes one), saying this computer
/// is a screen-mirroring source with RTSP on `rtsp_port`. Waits until the
/// link has an address, at most `wait`.
pub fn connect(peer: &Peer, rtsp_port: u16, wait: Duration) -> Result<Link, String> {
    let c = bus()?;
    let dev = p2p_device(&c)?;
    let user = std::env::var("USER").unwrap_or_default();

    fn v<T: RefArg + 'static>(x: T) -> Variant<Box<dyn RefArg>> {
        Variant(Box::new(x))
    }
    let mut connection: PropMap = HashMap::new();
    connection.insert("id".into(), v(format!("Kagaz to {}", peer.name)));
    connection.insert("type".into(), v("wifi-p2p".to_string()));
    connection.insert("autoconnect".into(), v(false));
    if !user.is_empty() {
        connection.insert("permissions".into(), v(vec![format!("user:{user}:")]));
    }
    let mut wifi_p2p: PropMap = HashMap::new();
    wifi_p2p.insert("peer".into(), v(peer.address.clone()));
    wifi_p2p.insert("wfd-ies".into(), v(source_information(rtsp_port)));
    let mut ipv4: PropMap = HashMap::new();
    ipv4.insert("method".into(), v("auto".to_string()));
    let mut ipv6: PropMap = HashMap::new();
    ipv6.insert("method".into(), v("disabled".to_string()));
    let mut settings: HashMap<&str, PropMap> = HashMap::new();
    settings.insert("connection", connection);
    settings.insert("wifi-p2p", wifi_p2p);
    settings.insert("ipv4", ipv4);
    settings.insert("ipv6", ipv6);
    let mut options: PropMap = HashMap::new();
    options.insert("persist".into(), v("volatile".to_string()));

    let nm = c.with_proxy(NM, NM_PATH, Duration::from_secs(30));
    let (_saved, active, _result): (Path<'static>, Path<'static>, PropMap) = nm
        .method_call(NM, "AddAndActivateConnection2", (settings, dev, Path::from("/"), options))
        .map_err(|e| format!("NetworkManager would not start the Wi-Fi Direct link: {e}"))?;
    let link = Link { c, active };

    let end = Instant::now() + wait;
    loop {
        let a = link.c.with_proxy(NM, link.active.clone(), TIMEOUT);
        match a.get::<u32>(ACTIVE, "State") {
            // NM_ACTIVE_CONNECTION_STATE_ACTIVATED
            Ok(2) => return Ok(link),
            Ok(3) | Ok(4) | Err(_) => {
                return Err("the TV did not accept the Wi-Fi Direct link. Is its screen sharing on?".into())
            }
            _ => {}
        }
        if Instant::now() >= end {
            return Err("the Wi-Fi Direct link took too long; the TV may be busy or too far away".into());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_lg_information() {
        // As NetworkManager showed the TV's: dual role, available, port 7236.
        let lg = [0u8, 0, 6, 1, 19, 28, 68, 0, 50];
        assert_eq!(read_information(&lg), (true, true));
        // Kagaz's own: a source, so not a display.
        assert_eq!(read_information(&source_information(7236)), (false, true));
        assert_eq!(source_information(7236), vec![0, 0, 6, 0, 0x10, 0x1c, 0x44, 0, 0x32]);
    }
}
