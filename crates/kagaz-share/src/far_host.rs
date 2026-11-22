//! From anywhere, on the computer (remote/far.rs says how it keeps the
//! computer closed to all but its paired devices): the door, its port
//! opened in the router while the person has it on, and what was found,
//! for Kagaz to show and for a paired device to be told.
//!
//! The router is asked the ways routers are (UPnP, then PCP and NAT-PMP)
//! for the door's port only, never the Remote Desktop port, for ten
//! minutes at a time, renewed while this runs: when it stops, however it
//! stops, the opening closes by itself soon after.
//!
//! Where it can be knocked at is posted in BitTorrent's address book for
//! each paired device (far.rs says how only that device can find it),
//! when it changes and every twenty minutes.

use crate::far::{self, Door, Pair, Role};
use crate::paired::Paired;
use crate::say;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How the computer can be reached from afar now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    /// "looking" at first; then "router" (the router lets Kagaz in) or
    /// "none" (no way in found on this connection).
    pub state: String,
    /// Where a paired device can knock.
    pub addresses: Vec<SocketAddr>,
}

pub type Shared = Arc<Mutex<Found>>;

/// How long the router keeps the opening, and how often it is renewed.
const LEASE: u32 = 10 * 60;
const RENEW: Duration = Duration::from_secs(4 * 60);
/// How often a computer with no way in looks again (it may have moved).
const AGAIN: Duration = Duration::from_secs(5 * 60);

/// The paired devices, as the door knows them.
fn pairs(paired: &Paired) -> Vec<Pair> {
    paired.store.lock().unwrap().devices.iter().map(|d| Pair::new(&d.id, &d.secret)).collect()
}

/// Start it, inside the runtime: the door on `port` (UDP), connections
/// through it going on to `rdp`. `told` hears each change of what was
/// found.
pub fn start(
    port: u16,
    der: Vec<u8>,
    key: Vec<u8>,
    rdp: SocketAddr,
    paired: Arc<Paired>,
    told: impl Fn(&Found) + Send + Sync + 'static,
) -> anyhow::Result<Shared> {
    let door = Door::open(Role::Host, port)?;
    door.set_pairs(pairs(&paired));
    let provider = Arc::new(quinn::rustls::crypto::aws_lc_rs::default_provider());
    let endpoint = far::host_endpoint(door.clone(), der, key, provider).map_err(anyhow::Error::msg)?;
    tokio::spawn(far::serve(endpoint, rdp));
    // Devices paired or removed meanwhile.
    {
        let door = door.clone();
        let paired = paired.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                door.set_pairs(pairs(&paired));
            }
        });
    }
    let found: Shared = Arc::new(Mutex::new(Found { state: "looking".into(), addresses: Vec::new() }));
    told(&found.lock().unwrap());
    {
        let (found, paired) = (found.clone(), paired.clone());
        std::thread::Builder::new().name("far-book".into()).spawn(move || book(found, paired))?;
    }
    let shared = found.clone();
    let port = door.port();
    tokio::spawn(async move {
        loop {
            let opened = open(port).await;
            *OPEN.lock().unwrap() = opened.as_ref().map(|o| (o.outside.port(), o.how.clone()));
            let now = match &opened {
                Some(o) => Found { state: "router".into(), addresses: vec![o.outside] },
                None => Found { state: "none".into(), addresses: Vec::new() },
            };
            if *found.lock().unwrap() != now {
                *found.lock().unwrap() = now.clone();
                told(&now);
            }
            match opened {
                Some(mut o) => loop {
                    tokio::time::sleep(RENEW).await;
                    if !o.renew().await {
                        say("the router's opening could not be renewed: asking again");
                        break;
                    }
                },
                None => tokio::time::sleep(AGAIN).await,
            }
        }
    });
    Ok(shared)
}

/// Post where it can be knocked at, for each paired device, when that
/// changes (or the devices, or the day) and every twenty minutes.
#[allow(deprecated)]
fn book(found: Shared, paired: Arc<Paired>) {
    let dht = loop {
        match mainline::Dht::client() {
            Ok(d) => break d,
            Err(e) => {
                say(&format!("the address book could not be reached ({e}): trying again in a minute"));
                std::thread::sleep(Duration::from_secs(60));
            }
        }
    };
    let mut last: Option<(Vec<SocketAddr>, usize, u64, std::time::Instant)> = None;
    loop {
        let addresses = found.lock().unwrap().addresses.clone();
        let pairs = pairs(&paired);
        let day = far::unix_now() / far::DAY;
        let due = match &last {
            None => true,
            Some((a, n, d, t)) => *a != addresses || *n != pairs.len() || *d != day || t.elapsed() > Duration::from_secs(20 * 60),
        };
        if due && !addresses.is_empty() && !pairs.is_empty() {
            let ok = pairs.iter().filter(|p| far::post(&dht, p, far::BOOK, &addresses).is_ok()).count();
            say(&format!("where it can be reached, posted in the address book for {ok} of {} paired devices", pairs.len()));
            if ok > 0 {
                last = Some((addresses, pairs.len(), day, std::time::Instant::now()));
            }
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

/// The opening now, to close as this ends.
static OPEN: Mutex<Option<(u16, How)>> = Mutex::new(None);

/// Close the router's opening now (as this program ends; otherwise it
/// closes by itself within ten minutes).
pub fn close() {
    let Some((port, how)) = OPEN.lock().unwrap().take() else { return };
    match how {
        How::Upnp { gateway, .. } => {
            let _ = gateway.remove_port(igd_next::PortMappingProtocol::UDP, port);
        }
        How::Pcp(mapping) => {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                let _ = rt.block_on(async { tokio::time::timeout(Duration::from_secs(2), mapping.try_drop()).await });
            }
        }
    }
}

/// An opening in the router.
struct Opened {
    outside: SocketAddr,
    how: How,
}

#[derive(Clone)]
enum How {
    Upnp { gateway: igd_next::Gateway, local: SocketAddr },
    Pcp(crab_nat::PortMapping),
}

impl Opened {
    async fn renew(&mut self) -> bool {
        let port = self.outside.port();
        match &mut self.how {
            How::Upnp { gateway, local } => {
                let (gateway, local) = (gateway.clone(), *local);
                tokio::task::spawn_blocking(move || {
                    gateway.add_port(igd_next::PortMappingProtocol::UDP, port, local, LEASE, "Kagaz").is_ok()
                })
                .await
                .unwrap_or(false)
            }
            How::Pcp(mapping) => mapping.renew().await.is_ok(),
        }
    }
}

/// This computer's own address on its network and its router's.
fn home() -> Option<(Ipv4Addr, Ipv4Addr)> {
    let i = netdev::get_default_interface().ok()?;
    let mine = i.ipv4.first()?.addr();
    let router = *i.gateway.as_ref()?.ipv4.first()?;
    Some((mine, router))
}

/// Ask the router to send the door's port to this computer; an opening
/// only counts when the router's own address is one the internet reaches
/// (not when the internet provider shares it among homes).
async fn open(port: u16) -> Option<Opened> {
    let (mine, router) = home()?;
    let local = SocketAddr::new(IpAddr::V4(mine), port);
    // UPnP.
    let upnp = tokio::task::spawn_blocking(move || {
        let mut options = igd_next::SearchOptions::default();
        options.timeout = Some(Duration::from_secs(3));
        let gateway = igd_next::search_gateway(options).ok()?;
        let outside_ip = gateway.get_external_ip().ok()?;
        if !far::public(outside_ip) {
            say("the router's own address is shared by the internet provider: its opening would lead nowhere");
            return None;
        }
        let outside_port = match gateway.add_port(igd_next::PortMappingProtocol::UDP, port, local, LEASE, "Kagaz") {
            Ok(()) => port,
            Err(_) => gateway.add_any_port(igd_next::PortMappingProtocol::UDP, local, LEASE, "Kagaz").ok()?,
        };
        Some(Opened { outside: SocketAddr::new(outside_ip, outside_port), how: How::Upnp { gateway, local } })
    })
    .await
    .ok()
    .flatten();
    if let Some(o) = upnp {
        say(&format!("the router opened UDP {} for Kagaz (UPnP)", o.outside.port()));
        return Some(o);
    }
    // PCP, then NAT-PMP.
    let gateway = crab_nat::GatewayAddress::IpV4(router);
    let options = crab_nat::PortMappingOptions {
        external_port: NonZeroU16::new(port),
        lifetime_seconds: Some(LEASE),
        timeout_config: Some(crab_nat::TimeoutConfig {
            initial_timeout: Duration::from_millis(250),
            max_retries: 2,
            max_retry_timeout: Some(Duration::from_secs(1)),
        }),
    };
    let mapping = crab_nat::PortMapping::new(gateway, IpAddr::V4(mine), crab_nat::InternetProtocol::Udp, NonZeroU16::new(port)?, options)
        .await
        .ok()?;
    let outside_ip = match mapping.mapping_type() {
        crab_nat::PortMappingType::Pcp { external_ip, .. } => external_ip,
        crab_nat::PortMappingType::NatPmp => IpAddr::V4(crab_nat::natpmp::external_address(gateway, None).await.ok()?),
    };
    if !far::public(outside_ip) {
        say("the router's own address is shared by the internet provider: its opening would lead nowhere");
        let _ = mapping.try_drop().await;
        return None;
    }
    let outside = SocketAddr::new(outside_ip, mapping.external_port().get());
    say(&format!("the router opened UDP {} for Kagaz (PCP or NAT-PMP)", outside.port()));
    Some(Opened { outside, how: How::Pcp(mapping) })
}
