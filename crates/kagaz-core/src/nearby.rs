//! Everything on the local network, for Home's "Found on your network": every
//! address that answers, with what it calls itself and what it probably is
//! (a phone, a computer, a TV, the router). Each tool still finds its own
//! devices its own way; this is the rest, so the person sees what is there.
//!
//! All at once, for a few seconds, and only on this device's own networks:
//! a knock on a few TCP ports of every address (an answer or a refusal both
//! mean something is there, and the knock fills the neighbour table), mDNS
//! (every service type announced), SSDP (`ssdp:all`, then each device's
//! description), NetBIOS names (Windows computers, Samba), ONVIF's probe
//! (cameras) and a ping (phones that refuse every connection answer it). The neighbour
//! table gives hardware addresses on computers; Android 10 and later hides
//! it from apps, so there a device is known by its name and address only.
//! The maker comes from the IEEE's public list of hardware address blocks
//! (`data/oui.tsv.gz`, made by `data/oui.py`).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

/// One address on the network and what it says about itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Host {
    pub ip: Ipv4Addr,
    /// "aa:bb:cc:dd:ee:ff", where the neighbour table gives it.
    pub mac: Option<String>,
    /// From the hardware address's block.
    pub maker: Option<String>,
    /// A made-up hardware address (phones and laptops use one per network),
    /// which says nothing of the maker.
    pub private_mac: bool,
    /// The best name it gave: its own friendly name, mDNS or NetBIOS name.
    pub name: Option<String>,
    pub model: Option<String>,
    /// mDNS service types without the decoration ("googlecast", "airplay").
    pub services: Vec<String>,
    /// Ports of the knock that took the connection.
    pub open: Vec<u16>,
    pub router: bool,
    pub this_device: bool,
    /// What it most likely is: router, this, phone, tablet, computer, tv,
    /// speaker, printer, camera, intercom, console, storage, smart-home,
    /// unknown.
    pub kind: String,
    /// Its system where something gave it away ("Ubuntu", "Windows",
    /// "macOS", "Android", "iOS"; "Ubuntu 26.04" from Kagaz on it).
    pub os: Option<String>,
    /// "laptop" or "desktop" where known (this device; Kagaz on another
    /// computer says; a Mac's model).
    pub form: Option<String>,
    /// Each port that took a connection and what answers there.
    pub ports: Vec<Port>,
}

/// A port that takes connections and what answers on it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Port {
    pub port: u16,
    /// "SSH", "web page", "Remote Desktop"…; "unknown" where nothing said.
    pub what: String,
    /// What it said of itself: an SSH greeting, a page's title.
    pub detail: Option<String>,
}

impl Default for Host {
    fn default() -> Self {
        Host {
            ip: Ipv4Addr::UNSPECIFIED,
            mac: None,
            maker: None,
            private_mac: false,
            name: None,
            model: None,
            services: Vec::new(),
            open: Vec::new(),
            router: false,
            this_device: false,
            kind: String::new(),
            os: None,
            form: None,
            ports: Vec::new(),
        }
    }
}

/// The ports knocked: web (most devices with a page), cameras' video
/// (RTSP), and the port every iPhone and iPad keeps open.
const KNOCK: [u16; 4] = [80, 443, 554, 62078];

/// mDNS service types asked for by name, beside whatever the meta-query
/// turns up (some devices answer only for their own types).
const SERVICE_TYPES: &[&str] = &[
    "_http._tcp.local.",
    "_device-info._tcp.local.",
    "_workstation._tcp.local.",
    "_smb._tcp.local.",
    "_ssh._tcp.local.",
    "_sftp-ssh._tcp.local.",
    "_rfb._tcp.local.",
    "_googlecast._tcp.local.",
    "_airplay._tcp.local.",
    "_raop._tcp.local.",
    "_companion-link._tcp.local.",
    "_apple-mobdev2._tcp.local.",
    "_spotify-connect._tcp.local.",
    "_androidtvremote2._tcp.local.",
    "_amzn-wplay._tcp.local.",
    "_sonos._tcp.local.",
    "_hap._tcp.local.",
    "_matter._tcp.local.",
    "_matterc._udp.local.",
    "_ipp._tcp.local.",
    "_printer._tcp.local.",
    "_uscan._tcp.local.",
    "_adisk._tcp.local.",
    "_miio._udp.local.",
    "_esphomelib._tcp.local.",
    "_home-assistant._tcp.local.",
    "_hue._tcp.local.",
    "_nvstream._tcp.local.",
];
const META: &str = "_services._dns-sd._udp.local.";

/// Look for `wait` (about four seconds is enough) and return every address
/// that showed itself, this device and the router included, by address.
pub fn sweep(wait: Duration) -> Vec<Host> {
    let nets = local_networks();
    let (knocked, mdns, ssdp, netbios, onvif, pinged) = std::thread::scope(|s| {
        let knock = s.spawn(|| knock_all(&nets, Duration::from_millis(1500)));
        let mdns = s.spawn(|| mdns_all(wait));
        let ssdp = s.spawn(|| ssdp_all(wait.min(Duration::from_millis(2500))));
        let netbios = s.spawn(|| netbios_all(&nets, wait.min(Duration::from_millis(1500))));
        let onvif = s.spawn(|| crate::cameras::local::probe(wait.min(Duration::from_secs(2))).unwrap_or_default());
        let pinged = s.spawn(|| ping_all(&nets, wait.min(Duration::from_millis(3000))));
        (
            knock.join().unwrap_or_default(),
            mdns.join().unwrap_or_default(),
            ssdp.join().unwrap_or_default(),
            netbios.join().unwrap_or_default(),
            onvif.join().unwrap_or_default(),
            pinged.join().unwrap_or_default(),
        )
    });
    let table = crate::cameras::local::neighbours();
    let gateway = gateway();
    let mine: Vec<Ipv4Addr> = nets.iter().map(|(ip, _)| *ip).collect();

    let mut hosts: BTreeMap<Ipv4Addr, Host> = BTreeMap::new();
    fn on<'a>(hosts: &'a mut BTreeMap<Ipv4Addr, Host>, nets: &[(Ipv4Addr, u8)], ip: Ipv4Addr) -> Option<&'a mut Host> {
        if !nets.iter().any(|(me, prefix)| same_net(ip, *me, *prefix)) {
            return None;
        }
        Some(hosts.entry(ip).or_insert_with(|| Host { ip, ..Default::default() }))
    }
    for (ip, open) in knocked {
        if let Some(h) = on(&mut hosts, &nets, ip) {
            h.open = open;
        }
    }
    for (ip, seen) in mdns {
        if let Some(h) = on(&mut hosts, &nets, ip) {
            h.services = seen.services.into_iter().collect();
            h.model = h.model.take().or(seen.model);
            h.os = h.os.take().or(seen.os);
            h.form = h.form.take().or(seen.form);
            h.name = h.name.take().or(seen.name).or(seen.host);
        }
    }
    for (ip, d) in ssdp {
        if let Some(h) = on(&mut hosts, &nets, ip) {
            if h.name.is_none() {
                h.name = d.name;
            }
            if h.model.is_none() {
                h.model = d.model;
            }
            if d.kind.is_some() {
                h.services.push(d.kind.unwrap());
            }
        }
    }
    for (ip, name) in netbios {
        if let Some(h) = on(&mut hosts, &nets, ip) {
            if h.name.is_none() {
                h.name = Some(name);
            }
            h.services.push("netbios".into());
        }
    }
    // Whatever answered a ping: phones that refuse every connection do.
    for ip in pinged {
        let _ = on(&mut hosts, &nets, ip);
    }
    // Cameras that answered ONVIF's probe, with their model.
    for cam in onvif {
        if let Some(h) = on(&mut hosts, &nets, cam.address) {
            h.services.push("onvif".into());
            if h.model.is_none() {
                h.model = cam.hardware();
            }
            if h.name.is_none() {
                h.name = cam.name();
            }
        }
    }
    // The neighbour table: every address that answered anything, even
    // phones that refuse every port.
    for (ip, mac) in &table {
        if let Some(h) = on(&mut hosts, &nets, *ip) {
            h.mac = Some(colons(mac));
        }
    }
    // Names given when asked by address: a Linux computer (Avahi), a Mac or
    // an iPhone, which announce no service by default, still answer this.
    // Android answers only "Android".
    // Asked once the rest is in, of what answered without a name only: a
    // question for every address at once is mostly lost (the Wi-Fi holds
    // the group's packets for sleeping devices and drops a long queue).
    // At the same time, every one found is knocked on at the well-known
    // ports of [`SERVICES`] (what it takes connections for), and those that
    // take SSH say hello (OpenSSH names the system).
    let unnamed: Vec<Ipv4Addr> = hosts.values().filter(|h| h.name.is_none() && !mine.contains(&h.ip)).map(|h| h.ip).collect();
    let found: Vec<Ipv4Addr> = hosts.keys().copied().collect();
    let (named, served) = std::thread::scope(|s| {
        let names = s.spawn(|| mdns_names(&nets, &unnamed, Duration::from_millis(800)));
        let served = knock(
            found.iter().flat_map(|ip| SERVICES.iter().map(move |(p, _)| (*ip, *p))).collect(),
            Duration::from_millis(1000),
        );
        (names.join().unwrap_or_default(), served)
    });
    for (ip, port) in served {
        if let (Some(port), Some(h)) = (port, hosts.get_mut(&ip)) {
            if !h.open.contains(&port) {
                h.open.push(port);
            }
        }
    }
    let ssh: Vec<Ipv4Addr> = hosts.values().filter(|h| h.open.contains(&22)).map(|h| h.ip).collect();
    let hellos: Vec<(Ipv4Addr, Option<String>)> = std::thread::scope(|s| {
        let asked: Vec<_> = ssh.iter().map(|ip| (*ip, s.spawn(move || greeting(*ip, 22, Duration::from_millis(900))))).collect();
        asked.into_iter().map(|(ip, t)| (ip, t.join().ok().flatten())).collect()
    });
    for (ip, hello) in hellos {
        if let (Some(hello), Some(h)) = (hello, hosts.get_mut(&ip)) {
            h.ports.push(Port { port: 22, what: "SSH".into(), detail: Some(hello) });
        }
    }
    for (ip, name) in named {
        if let Some(h) = on(&mut hosts, &nets, ip) {
            // "Android", "Android-2"…: an Android phone's only name.
            let lower = name.to_ascii_lowercase();
            if lower == "android" || lower.strip_prefix("android-").is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())) {
                h.services.push("android".into());
            } else {
                if h.name.is_none() {
                    h.name = Some(name);
                }
                h.services.push("mdns-host".into());
            }
        }
    }
    for ip in mine.iter().copied() {
        if let Some(h) = on(&mut hosts, &nets, ip) {
            h.this_device = true;
        }
    }
    // The router: the default route's gateway; where the system keeps that
    // from apps (Android), each network's first address when it answered.
    let routers: Vec<Ipv4Addr> = match gateway {
        Some(gw) => vec![gw],
        None => nets
            .iter()
            .map(|(ip, prefix)| Ipv4Addr::from((u32::from(*ip) & (u32::MAX << (32 - *prefix as u32))) | 1))
            .filter(|ip| hosts.contains_key(ip))
            .collect(),
    };
    for gw in routers {
        if let Some(h) = hosts.get_mut(&gw) {
            h.router = true;
        }
    }
    let mut out: Vec<Host> = hosts.into_values().collect();
    for h in &mut out {
        h.name = h.name.take().map(|n| percent_decode(&n)).filter(|n| !n.trim().is_empty());
        if let Some(mac) = &h.mac {
            h.private_mac = is_private(mac);
            if !h.private_mac {
                h.maker = maker(mac).map(str::to_string);
            }
        }
        h.services.sort();
        h.services.dedup();
        h.open.sort();
        h.open.dedup();
        label_ports(h);
        if h.this_device {
            h.os = this_os();
            h.form = this_form().map(str::to_string);
        }
        h.kind = guess(h).to_string();
        h.os = os_of(h);
        if h.kind == "unknown" && matches!(h.os.as_deref().map(os_family), Some("Windows" | "macOS" | "Linux")) {
            h.kind = "computer".into();
        }
        if h.form.is_none() {
            h.form = form_of(h).map(str::to_string);
        }
    }
    out
}

/// Each open port with what answers there: what was heard (an SSH
/// greeting) or the well-known use of the port; Kagaz as a screen for
/// Cast's port on a Kagaz computer.
fn label_ports(h: &mut Host) {
    let kagaz = h.model.as_deref() == Some("Kagaz");
    for &port in &h.open {
        if h.ports.iter().any(|p| p.port == port) {
            continue;
        }
        let what = if kagaz && port == 8009 { "Kagaz screen" } else { service_name(port).unwrap_or("unknown") };
        h.ports.push(Port { port, what: what.into(), detail: None });
    }
    h.ports.sort_by_key(|p| p.port);
}

/// The well-known use of a port.
pub fn service_name(port: u16) -> Option<&'static str> {
    SERVICES.iter().find(|(p, _)| *p == port).map(|(_, w)| *w)
}

/// The first line a service says by itself on connecting (SSH, FTP and
/// mail do), within `wait`.
fn greeting(ip: Ipv4Addr, port: u16, wait: Duration) -> Option<String> {
    let mut tcp = std::net::TcpStream::connect_timeout(&SocketAddr::new(IpAddr::V4(ip), port), wait).ok()?;
    tcp.set_read_timeout(Some(wait)).ok()?;
    let mut buf = [0u8; 256];
    let n = tcp.read(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// A system named in what a service said of itself.
fn os_in(text: &str) -> Option<&'static str> {
    let t = text.to_ascii_lowercase();
    [
        ("ubuntu", "Ubuntu"),
        ("raspbian", "Raspberry Pi OS"),
        ("debian", "Debian"),
        ("fedora", "Fedora"),
        ("freebsd", "FreeBSD"),
        ("for_windows", "Windows"),
        ("microsoft", "Windows"),
        ("darwin", "macOS"),
    ]
    .iter()
    .find(|(w, _)| t.contains(w))
    .map(|(_, o)| *o)
}

/// The family of a system's name, for the guess: Windows, macOS, Linux,
/// or the name itself.
fn os_family(os: &str) -> &str {
    match os.split_whitespace().next().unwrap_or(os) {
        "Ubuntu" | "Debian" | "Fedora" | "Raspberry" | "Linux" | "Mint" | "Arch" => "Linux",
        other => other,
    }
}

/// Its system, from what it showed: Kagaz's own word, an SSH greeting or
/// web server, an iPhone's or Android's own port or name, a Mac's model,
/// Windows' own ports (or a NetBIOS name from a computer without Avahi).
fn os_of(h: &Host) -> Option<String> {
    if h.os.is_some() {
        return h.os.clone();
    }
    if let Some(o) = h.ports.iter().filter_map(|p| p.detail.as_deref()).find_map(os_in) {
        return Some(o.into());
    }
    let has = |s: &str| h.services.iter().any(|x| x == s);
    let model = h.model.as_deref().unwrap_or("");
    if h.open.contains(&62078) || has("apple-mobdev2") {
        return Some("iOS".into());
    }
    if has("android") || has("adb") || has("adb-tls-connect") {
        return Some("Android".into());
    }
    if ["MacBook", "iMac", "Macmini", "MacPro", "MacStudio", "Mac1"].iter().any(|m| model.starts_with(m)) || h.open.contains(&548) {
        return Some("macOS".into());
    }
    if h.open.contains(&135) || h.open.contains(&5357) || (has("netbios") && !has("mdns-host") && h.kind == "computer") {
        return Some("Windows".into());
    }
    None
}

/// Laptop or desktop where it shows: a Mac's model.
fn form_of(h: &Host) -> Option<&'static str> {
    let model = h.model.as_deref().unwrap_or("");
    if model.starts_with("MacBook") {
        Some("laptop")
    } else if ["iMac", "Macmini", "MacPro", "MacStudio"].iter().any(|m| model.starts_with(m)) {
        Some("desktop")
    } else {
        None
    }
}

/// This device's system, with its version where it gives one
/// ("Ubuntu 26.04").
pub fn this_os() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/etc/os-release").ok()?;
        let field = |k: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
                .map(|v| v.trim_matches('"').to_string())
                .filter(|v| !v.is_empty())
        };
        let name = field("NAME").unwrap_or_else(|| "Linux".into());
        let name = name.strip_suffix(" GNU/Linux").unwrap_or(&name).to_string();
        Some(match field("VERSION_ID") {
            Some(v) => format!("{name} {v}"),
            None => name,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some(
            match std::env::consts::OS {
                "android" => "Android",
                "ios" => "iOS",
                "macos" => "macOS",
                "windows" => "Windows",
                other => other,
            }
            .to_string(),
        )
    }
}

/// Whether this device is a laptop or a desktop, where the system says:
/// the chassis on Linux, the model on a Mac, a battery on Windows.
pub fn this_form() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    {
        let kind: u32 = std::fs::read_to_string("/sys/class/dmi/id/chassis_type").ok()?.trim().parse().ok()?;
        match kind {
            8 | 9 | 10 | 14 | 30 | 31 | 32 => Some("laptop"),
            3..=7 | 13 | 15 | 16 | 35 | 36 => Some("desktop"),
            _ => None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl").args(["-n", "hw.model"]).output().ok()?;
        let model = String::from_utf8_lossy(&out.stdout);
        Some(if model.contains("Book") { "laptop" } else { "desktop" })
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut st: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { GetSystemPowerStatus(&mut st) } == 0 {
            return None;
        }
        // 128: no system battery; 255: unknown.
        match st.BatteryFlag {
            255 => None,
            f if f & 128 != 0 => Some("desktop"),
            _ => Some("laptop"),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

// ---------- what it probably is ----------

/// The likeliest kind of device, from what it showed: its services, open
/// ports, name and maker. A guess for an icon and a word, never for
/// anything Kagaz does with it.
pub fn guess(h: &Host) -> &'static str {
    let has = |s: &str| h.services.iter().any(|x| x == s);
    let name = h.name.as_deref().unwrap_or("").to_ascii_lowercase();
    let model = h.model.as_deref().unwrap_or("").to_ascii_lowercase();
    let maker = h.maker.as_deref().unwrap_or("");
    let words = format!("{name} {model}");
    let any = |list: &[&str]| list.iter().any(|w| words.contains(w));
    if h.this_device {
        return "this";
    }
    if h.router {
        return "router";
    }
    if has("ipp") || has("printer") || has("uscan") || has("upnp-printer") {
        return "printer";
    }
    // Kagaz on a computer, listed as a screen (Cast): a computer.
    if model == "kagaz" {
        return "computer";
    }
    if has("onvif") || h.open.contains(&2020) {
        return "camera";
    }
    // What a closer look's ports say (look_closer).
    // A TV takes print jobs' ports too (LG's webOS keeps 515 open).
    let tv = has("googlecast") || has("airplay") || has("upnp-tv") || has("androidtvremote2") || h.open.contains(&8008) || h.open.contains(&8009);
    if !tv && h.open.iter().any(|p| [9100, 631, 515].contains(p)) {
        return "printer";
    }
    if h.open.contains(&1400) {
        return "speaker";
    }
    if h.open.contains(&8009) || h.open.contains(&8008) {
        return "tv";
    }
    if h.open.contains(&6668) {
        return "smart-home";
    }
    if h.open.contains(&3389) || h.open.contains(&5900) {
        return "computer";
    }
    if any(&["ipad"]) {
        return "tablet";
    }
    if any(&["iphone", "android", "galaxy", "pixel", "redmi", "oneplus", "phone"]) {
        return "phone";
    }
    if any(&["macbook", "imac", "mac mini", "macmini", "macpro", "laptop", "desktop", "-pc", "thinkpad"]) {
        return "computer";
    }
    if any(&["esp32", "esp8266", "esp-", "shelly", "tasmota", "tuya", "sonoff", "wled"]) {
        return "smart-home";
    }
    if any(&["camera", "cam", "ipc", "nvr", "dvr"]) || h.open.contains(&554) {
        return "camera";
    }
    if has("googlecast") || has("androidtvremote2") || has("amzn-wplay") || has("upnp-tv") || any(&["tv", "bravia", "chromecast", "fire stick", "roku"]) {
        return if any(&["nest audio", "nest mini", "home mini", "speaker"]) { "speaker" } else { "tv" };
    }
    if has("sonos") || has("spotify-connect") || has("raop") && !has("airplay") || any(&["speaker", "echo", "homepod", "soundbar"]) {
        return "speaker";
    }
    if any(&["playstation", "xbox", "nintendo"]) || maker == "Nintendo" {
        return "console";
    }
    if has("adisk") || any(&["nas", "diskstation", "synology", "wd my"]) || maker == "Synology" {
        return "storage";
    }
    if has("hap") || has("matter") || has("matterc") || has("miio") || has("esphomelib") || has("hue") || matches!(maker, "Espressif" | "Tuya" | "MXCHIP" | "iComm" | "Beken" | "Bouffalo Lab" | "WinnerMicro" | "Gaoshengda") {
        return "smart-home";
    }
    if h.open.contains(&62078) || has("apple-mobdev2") || has("android") {
        return "phone";
    }
    if has("netbios") || has("smb") || has("ssh") || has("sftp-ssh") || has("rfb") || has("workstation") || has("companion-link") || has("nvstream") {
        return "computer";
    }
    if has("airplay") {
        return "tv";
    }
    // Named itself when asked by address and nothing else: phones (iPhones
    // keep 62078 open, Android says "Android") and gadgets are out above,
    // which leaves Linux computers and Macs.
    if has("mdns-host") {
        return "computer";
    }
    // A made-up hardware address, nothing announced: almost always a phone
    // or a laptop that keeps to itself.
    if h.private_mac {
        return "phone";
    }
    match maker {
        "Apple" | "Samsung" | "Xiaomi" | "Oppo" | "Vivo" | "Realme" | "OnePlus" | "Motorola" | "Huawei" | "Honor" | "Nokia" | "Google" | "Infinix" | "Tecno" | "Transsion" => "phone",
        "Intel" | "Realtek" | "Dell" | "Lenovo" | "ASUS" | "Acer" | "HP" | "Microsoft" | "Lite-On" | "AzureWave" | "Foxconn" | "Wistron" | "Quanta" | "Compal" | "Gigabyte" | "MSI" => "computer",
        "Raspberry Pi" => "computer",
        "Hikvision" | "Dahua" | "EZVIZ" | "Imou" | "Wyze" => "camera",
        "Akuvox" => "intercom",
        _ => "unknown",
    }
}

// ---------- a closer look at one device ----------

/// The ports a closer look knocks on, each with what it means.
pub const SERVICES: &[(u16, &str)] = &[
    (80, "web page"),
    (443, "web page (https)"),
    (8080, "web page"),
    (8443, "web page (https)"),
    (22, "SSH"),
    (3389, "Remote Desktop"),
    (5900, "VNC screen sharing"),
    (445, "Windows file sharing"),
    (139, "Windows file sharing"),
    (548, "Apple file sharing"),
    (631, "IPP printing"),
    (9100, "printing"),
    (515, "LPD printing"),
    (554, "camera video (RTSP)"),
    (8554, "camera video (RTSP)"),
    (2020, "ONVIF camera control"),
    (8008, "Google Cast"),
    (8009, "Google Cast"),
    (7000, "AirPlay"),
    (1400, "Sonos"),
    (5060, "SIP calls"),
    (1883, "MQTT"),
    (6668, "Tuya smart home"),
    (55443, "Yeelight"),
    (5000, "NAS or media server"),
    (32400, "Plex"),
    (8123, "Home Assistant"),
    (53, "DNS"),
    (21, "FTP"),
    (23, "Telnet"),
    (62078, "iPhone or iPad sync"),
    (135, "Windows remote calls"),
    (5357, "Windows network discovery"),
    (3283, "Apple Remote Desktop"),
    (5555, "Android debugging (adb)"),
    (8000, "web page"),
    (8081, "web page"),
    (8888, "web page"),
    (3000, "web page"),
    (9000, "web page"),
    (5001, "NAS (https)"),
    (9090, "web console (Cockpit)"),
    (8384, "Syncthing"),
    (8096, "Jellyfin"),
    (1080, "SOCKS proxy"),
    (3128, "web proxy"),
    (8118, "web proxy (Privoxy)"),
    (9050, "Tor (SOCKS proxy)"),
    (9150, "Tor Browser (SOCKS proxy)"),
    (2049, "NFS file sharing"),
    (25, "mail (SMTP)"),
    (3306, "MySQL database"),
    (5432, "PostgreSQL database"),
    (6379, "Redis"),
    (11434, "Ollama (AI models)"),
    (7236, "Miracast"),
];

/// What a closer look found: the device again, its ports each with what
/// answers there, its web page's title and server.
#[derive(Debug, Clone, Serialize)]
pub struct Closer {
    pub host: Host,
    /// (port, what answers there), as [`Host::ports`] says it.
    pub services: Vec<(u16, String)>,
    pub title: Option<String>,
    pub server: Option<String>,
    /// How many ports were knocked on.
    pub checked: u32,
}

/// Ask one device what it is: knock on every port of [`SERVICES`] at once,
/// ask each one that takes a connection what it is (an SSH greeting, a web
/// page's title and server), and guess again with all of it. A couple of
/// seconds.
pub fn look_closer(h: Host) -> Closer {
    let ports: Vec<u16> = SERVICES.iter().map(|(p, _)| *p).collect();
    closer(h, ports, Duration::from_millis(1200))
}

/// Every port of one device, 1 to 65535, each that takes a connection asked
/// what it is. Several seconds on a computer, half a minute on a phone.
pub fn every_port(h: Host) -> Closer {
    closer(h, (1..=65535).collect(), Duration::from_millis(900))
}

fn closer(mut h: Host, ports: Vec<u16>, timeout: Duration) -> Closer {
    let checked = ports.len() as u32;
    let targets: Vec<(Ipv4Addr, u16)> = ports.into_iter().map(|p| (h.ip, p)).collect();
    let mut open: Vec<u16> = knock(targets, timeout).into_iter().filter_map(|(_, p)| p).collect();
    open.sort();
    open.dedup();
    // Each asked at once, a few dozen at a time.
    let ip = h.ip;
    let mut found: Vec<Port> = Vec::new();
    for group in open.chunks(48) {
        found.extend(std::thread::scope(|s| {
            let asked: Vec<_> = group.iter().map(|p| s.spawn(move || identify(ip, *p))).collect();
            asked.into_iter().filter_map(|t| t.join().ok()).collect::<Vec<_>>()
        }));
    }
    let web = found.iter().find(|p| p.what.starts_with("web page") && p.detail.is_some());
    let title = web.and_then(|p| p.detail.clone()).filter(|d| !d.starts_with("server: "));
    let server = found
        .iter()
        .filter_map(|p| p.detail.as_deref())
        .find_map(|d| d.split(" · ").find_map(|x| x.strip_prefix("server: ")))
        .map(str::to_string);
    for p in &open {
        if !h.open.contains(p) {
            h.open.push(*p);
        }
    }
    h.open.sort();
    // What was heard replaces the well-known guess; ports that did not
    // answer this time stay as they were.
    for p in found {
        h.ports.retain(|q| q.port != p.port);
        h.ports.push(p);
    }
    label_ports(&mut h);
    // A page's title names a gadget ("Smart Plug"), not a computer's
    // own pages.
    if h.model.is_none() && h.os.is_none() && !h.open.iter().any(|p| [22, 3389, 5900, 445].contains(p)) {
        h.model = title.clone();
    }
    if !h.this_device {
        h.kind = guess(&h).to_string();
        h.os = os_of(&Host { os: None, ..h.clone() }).or(h.os.take());
        if h.kind == "unknown" && matches!(h.os.as_deref().map(os_family), Some("Windows" | "macOS" | "Linux")) {
            h.kind = "computer".into();
        }
    }
    let services = h.ports.iter().map(|p| (p.port, p.what.clone())).collect();
    Closer { host: h, services, title, server, checked }
}

/// What answers on one port: a service that speaks first (SSH, VNC, FTP,
/// mail) by what it says, else a web page by its title (http, then https
/// with whatever certificate it shows), else the port's well-known use.
/// Ports whose service waits for its own kind of hello are named by number.
fn identify(ip: Ipv4Addr, port: u16) -> Port {
    let known = service_name(port);
    let named = |what: &str, detail: Option<String>| Port { port, what: what.into(), detail };
    if matches!(port, 53 | 135 | 139 | 445 | 554 | 8554 | 3389 | 5060 | 1883 | 62078 | 6668 | 9100 | 515 | 2049 | 1080 | 9050 | 9150 | 5432 | 3306 | 6379) {
        return named(known.unwrap_or("unknown"), None);
    }
    if let Some(line) = greeting(ip, port, Duration::from_millis(700)) {
        let what = if line.starts_with("SSH-") {
            "SSH"
        } else if line.starts_with("RFB ") {
            "VNC screen sharing"
        } else if line.starts_with("220") {
            known.unwrap_or("FTP or mail")
        } else {
            known.unwrap_or("unknown")
        };
        return named(what, Some(line));
    }
    // https first where it is the custom; a plain request that an https
    // port turns away ("The plain HTTP request was sent to HTTPS port") is
    // asked again over https.
    let order = if matches!(port, 443 | 8443 | 5001 | 9443) { [true, false] } else { [false, true] };
    for https in order {
        let Some((title, server)) = web_page(ip, port, https) else { continue };
        if !https && title.as_deref().is_some_and(|t| t.to_ascii_lowercase().contains("https")) {
            if let Some((t, s)) = web_page(ip, port, true) {
                return named("web page (https)", web_detail(t, s));
            }
        }
        // A port with a use of its own that answers over http (AirPlay,
        // Cast, media sharing) keeps that name.
        let upnp = server.as_deref().is_some_and(|s| s.contains("UPnP") || s.contains("DLNA"));
        let what = match known {
            Some(k) if !k.starts_with("web page") => k,
            _ if upnp => "media sharing and control (UPnP)",
            _ if https => "web page (https)",
            _ => "web page",
        };
        return named(what, web_detail(title, server));
    }
    named(known.unwrap_or("unknown"), None)
}

/// "<title> · server: <server>", whichever there are.
fn web_detail(title: Option<String>, server: Option<String>) -> Option<String> {
    match (title, server) {
        (Some(t), Some(s)) => Some(format!("{t} · server: {s}")),
        (Some(t), None) => Some(t),
        (None, Some(s)) => Some(format!("server: {s}")),
        (None, None) => None,
    }
}

/// A web page's title and server line, when the port serves one.
fn web_page(ip: Ipv4Addr, port: u16, https: bool) -> Option<(Option<String>, Option<String>)> {
    let mut agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(1500)).redirects(2);
    if https {
        agent = agent.tls_config(crate::remote::reach::any_certificate_tls()?);
    }
    let url = format!("{}://{}:{}/", if https { "https" } else { "http" }, ip, port);
    let resp = match agent.build().get(&url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(_) => return None,
    };
    let server = resp.header("server").map(str::to_string).filter(|s| !s.is_empty());
    let mut body = String::new();
    let _ = resp.into_reader().take(64 * 1024).read_to_string(&mut body);
    Some((page_title(&body), server))
}

/// A page's <title>, its entities and spacing tidied.
fn page_title(html: &str) -> Option<String> {
    let low = html.to_ascii_lowercase();
    let start = low.find("<title")?;
    let open_end = start + low[start..].find('>')? + 1;
    let end = open_end + low[open_end..].find("</title")?;
    let t = html[open_end..end]
        .replace("&amp;", "&")
        .replace("&nbsp;", " ")
        .replace("&#39;", "'")
        .replace("&quot;", "\"");
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty() && t.len() < 120).then_some(t)
}

// ---------- the knock ----------

/// Each address of the local networks, knocked on each of [`KNOCK`]: the
/// addresses that took or refused a connection, with the ports that took it.
fn knock_all(nets: &[(Ipv4Addr, u8)], timeout: Duration) -> Vec<(Ipv4Addr, Vec<u16>)> {
    let mut targets: Vec<(Ipv4Addr, u16)> = Vec::new();
    for (ip, prefix) in nets {
        for host in hosts(*ip, *prefix) {
            for port in KNOCK {
                targets.push((host, port));
            }
        }
    }
    let mut out: BTreeMap<Ipv4Addr, BTreeSet<u16>> = BTreeMap::new();
    for (host, port) in knock(targets, timeout) {
        let ports = out.entry(host).or_default();
        if let Some(p) = port {
            ports.insert(p);
        }
    }
    out.into_iter().map(|(ip, p)| (ip, p.into_iter().collect())).collect()
}

/// Every knock from one thread: non-blocking connections, up to about a
/// thousand under way at once, each new one started as another ends
/// (refused, taken, or past `timeout`), all waited on together (poll). In
/// the phone's app a hundred knocking threads beside the other searches
/// finished only part of the network (2026-11-22); this needs none, and
/// keeps a whole device's 65535 ports from going out in one burst (most of
/// which were lost). (host, Some(port)) for a connection taken, (host, None)
/// for one refused.
#[cfg(unix)]
fn knock(targets: Vec<(Ipv4Addr, u16)>, timeout: Duration) -> Vec<(Ipv4Addr, Option<u16>)> {
    // On Android each connect goes by the system's network service (about
    // 1.5 ms each, one after another): a whole device's ports took 99 s
    // from one thread. Shared among a thread per core, a few at most.
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    if targets.len() < 1024 || threads < 2 {
        return knock_poll(targets, timeout, 1);
    }
    let mut shards: Vec<Vec<(Ipv4Addr, u16)>> = vec![Vec::new(); threads];
    for (i, t) in targets.into_iter().enumerate() {
        shards[i % threads].push(t);
    }
    std::thread::scope(|s| {
        let running: Vec<_> = shards.into_iter().map(|shard| s.spawn(move || knock_poll(shard, timeout, threads))).collect();
        running.into_iter().flat_map(|t| t.join().unwrap_or_default()).collect()
    })
}

/// One thread's share of a knock, `of` sharing the open-file limit.
#[cfg(unix)]
fn knock_poll(targets: Vec<(Ipv4Addr, u16)>, timeout: Duration, of: usize) -> Vec<(Ipv4Addr, Option<u16>)> {
    use socket2::{Domain, Socket, Type};
    use std::os::fd::AsRawFd;
    let mut found = Vec::new();
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    let mut soft = 1024;
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
        soft = limit.rlim_cur as usize;
        // A terminal often allows 1024 open files; raise it to 4096 where
        // the hard limit lets (a phone's app has 32768).
        if limit.rlim_cur < 4096 && limit.rlim_max > limit.rlim_cur {
            let raised = libc::rlimit { rlim_cur: limit.rlim_max.min(4096), rlim_max: limit.rlim_max };
            if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
                soft = raised.rlim_cur as usize;
            }
        }
    }
    // About 1100 under way in all: more at once loses some (a laptop's
    // 3800 missed two of a computer's three open ports).
    let window = (soft.saturating_sub(256).min(1100) / of.max(1)).max(64);
    let mut queue = targets.into_iter();
    let mut open: Vec<(Socket, Ipv4Addr, u16, Instant)> = Vec::new();
    loop {
        while open.len() < window {
            let Some((host, port)) = queue.next() else { break };
            let Ok(sock) = Socket::new(Domain::IPV4, Type::STREAM, None) else { continue };
            if sock.set_nonblocking(true).is_err() {
                continue;
            }
            let addr = SocketAddr::new(IpAddr::V4(host), port);
            match sock.connect(&addr.into()) {
                Ok(()) => found.push((host, Some(port))),
                Err(e) if e.raw_os_error() == Some(libc::EINPROGRESS) || e.kind() == std::io::ErrorKind::WouldBlock => {
                    open.push((sock, host, port, Instant::now()))
                }
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => found.push((host, None)),
                Err(_) => {}
            }
        }
        if open.is_empty() {
            break;
        }
        let mut fds: Vec<libc::pollfd> = open
            .iter()
            .map(|(s, _, _, _)| libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLOUT, revents: 0 })
            .collect();
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 20) };
        if n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            break;
        }
        let now = Instant::now();
        let mut still = Vec::with_capacity(open.len());
        for ((sock, host, port, at), fd) in open.into_iter().zip(fds) {
            if fd.revents == 0 {
                if now.duration_since(at) < timeout {
                    still.push((sock, host, port, at));
                }
                continue;
            }
            match sock.take_error() {
                Ok(None) => found.push((host, Some(port))),
                Ok(Some(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => found.push((host, None)),
                _ => {}
            }
        }
        open = still;
    }
    found
}

/// Elsewhere (Windows), a few dozen threads, one blocking knock each.
#[cfg(not(unix))]
fn knock(targets: Vec<(Ipv4Addr, u16)>, timeout: Duration) -> Vec<(Ipv4Addr, Option<u16>)> {
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};
    let targets = Arc::new(Mutex::new(targets));
    let (tx, rx) = mpsc::channel();
    let threads = (targets.lock().map(|t| t.len()).unwrap_or(0) / 8).clamp(16, 512);
    for _ in 0..threads {
        let (targets, tx) = (targets.clone(), tx.clone());
        std::thread::spawn(move || loop {
            let next = targets.lock().ok().and_then(|mut t| t.pop());
            let Some((host, port)) = next else { break };
            let addr = SocketAddr::new(IpAddr::V4(host), port);
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(_) => {
                    let _ = tx.send((host, Some(port)));
                }
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    let _ = tx.send((host, None));
                }
                Err(_) => {}
            }
        });
    }
    drop(tx);
    rx.into_iter().collect()
}

// ---------- mDNS ----------

#[derive(Default)]
struct Announced {
    services: BTreeSet<String>,
    /// Its own friendly name (a Cast device's "fn", an instance name).
    name: Option<String>,
    /// "<host>.local." without the decoration.
    host: Option<String>,
    model: Option<String>,
    /// Kagaz on another computer, while it is a screen: its system and
    /// whether it is a laptop.
    os: Option<String>,
    form: Option<String>,
}

fn mdns_all(wait: Duration) -> HashMap<Ipv4Addr, Announced> {
    let mut out: HashMap<Ipv4Addr, Announced> = HashMap::new();
    let Ok(daemon) = mdns_sd::ServiceDaemon::new() else {
        return out;
    };
    let mut asked: Vec<(String, mdns_sd::Receiver<mdns_sd::ServiceEvent>)> = Vec::new();
    for ty in std::iter::once(META).chain(SERVICE_TYPES.iter().copied()) {
        if let Ok(rx) = daemon.browse(ty) {
            asked.push((ty.to_string(), rx));
        }
    }
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        let mut more: Vec<String> = Vec::new();
        let mut got = false;
        for (ty, rx) in &asked {
            while let Ok(event) = rx.try_recv() {
                got = true;
                match event {
                    mdns_sd::ServiceEvent::ServiceFound(_, found) if ty == META => {
                        if found.ends_with(".local.") && !asked.iter().any(|(t, _)| *t == found) && !more.contains(&found) {
                            more.push(found);
                        }
                    }
                    mdns_sd::ServiceEvent::ServiceResolved(info) => {
                        let service = short_type(&info.ty_domain);
                        let instance = info
                            .fullname
                            .strip_suffix(info.ty_domain.as_str())
                            .map(|s| s.trim_end_matches('.').replace("\\032", " "))
                            .filter(|s| !s.is_empty());
                        let txt = |k: &str| info.txt_properties.get_property_val_str(k).map(str::to_string).filter(|v| !v.is_empty());
                        for addr in &info.addresses {
                            let IpAddr::V4(ip) = addr.to_ip_addr() else { continue };
                            let a = out.entry(ip).or_default();
                            a.services.insert(service.clone());
                            if a.host.is_none() {
                                let host = info.host.trim_end_matches('.').trim_end_matches(".local");
                                if !host.is_empty() {
                                    a.host = Some(host.to_string());
                                }
                            }
                            // A friendly name: Cast's "fn", else the instance
                            // name of a service that names the device (not a
                            // printer queue or a web page's title).
                            if let Some(f) = txt("fn") {
                                a.name = Some(f);
                            } else if a.name.is_none() && !matches!(service.as_str(), "http" | "ipp" | "printer" | "uscan" | "miio" | "hap" | "matter" | "matterc" | "adb" | "adb-tls-connect" | "adb-tls-pairing") {
                                if let Some(i) = &instance {
                                    // RAOP names are "<mac>@<name>".
                                    a.name = Some(i.split_once('@').map(|(_, n)| n.to_string()).unwrap_or_else(|| i.clone()));
                                }
                            }
                            if a.model.is_none() {
                                a.model = txt("md").or_else(|| txt("model")).or_else(|| txt("am")).or_else(|| txt("ty"));
                            }
                            if let Some(o) = txt("kz-os") {
                                a.os = Some(o);
                            }
                            if let Some(f) = txt("kz-form") {
                                a.form = Some(f);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for ty in more {
            if let Ok(rx) = daemon.browse(&ty) {
                asked.push((ty, rx));
            }
        }
        if !got {
            std::thread::sleep(Duration::from_millis(60));
        }
    }
    for (ty, _) in &asked {
        let _ = daemon.stop_browse(ty);
    }
    let _ = daemon.shutdown();
    out
}

fn short_type(ty: &str) -> String {
    ty.trim_start_matches('_')
        .split("._")
        .next()
        .unwrap_or(ty)
        .to_string()
}

/// Ask these addresses their names over mDNS (a reverse lookup,
/// "<d>.<c>.<b>.<a>.in-addr.arpa", sent to the mDNS group from an ordinary
/// port so the answers come straight back here). Names only, ".local" off.
fn mdns_names(nets: &[(Ipv4Addr, u8)], ips: &[Ipv4Addr], wait: Duration) -> HashMap<Ipv4Addr, String> {
    use socket2::{Domain, Protocol, Socket, Type};
    let mut out = HashMap::new();
    let Ok(sock) = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) else {
        return out;
    };
    if sock.bind(&SocketAddr::from(([0, 0, 0, 0], 0)).into()).is_err() {
        return out;
    }
    let sock: UdpSocket = sock.into();
    let _ = sock.set_read_timeout(Some(Duration::from_millis(150)));
    if ips.is_empty() {
        return out;
    }
    let group = SocketAddr::from(([224, 0, 0, 251], 5353));
    for (me, prefix) in nets {
        let _ = socket2::SockRef::from(&sock).set_multicast_if_v4(me);
        for (i, ip) in ips.iter().filter(|ip| same_net(**ip, *me, *prefix)).enumerate() {
            let _ = sock.send_to(&reverse_query(i as u16, *ip), group);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let start = Instant::now();
    let mut buf = [0u8; 1500];
    while start.elapsed() < wait {
        let Ok((n, _)) = sock.recv_from(&mut buf) else { continue };
        if let Some((ip, name)) = reverse_answer(&buf[..n]) {
            out.insert(ip, name);
            if out.len() == ips.len() {
                break;
            }
        }
    }
    out
}

fn reverse_query(id: u16, ip: Ipv4Addr) -> Vec<u8> {
    let mut q = Vec::with_capacity(48);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    let o = ip.octets();
    for label in [o[3].to_string(), o[2].to_string(), o[1].to_string(), o[0].to_string(), "in-addr".into(), "arpa".into()] {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&[0, 12, 0, 1]);
    q
}

/// A DNS name at `at`, following compression; and where the name ends.
fn dns_name(msg: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end = None;
    for _ in 0..64 {
        let len = *msg.get(at)? as usize;
        if len == 0 {
            return Some((labels.join("."), end.unwrap_or(at + 1)));
        }
        if len & 0xc0 == 0xc0 {
            let to = ((len & 0x3f) << 8) | *msg.get(at + 1)? as usize;
            end.get_or_insert(at + 2);
            at = to;
            continue;
        }
        labels.push(String::from_utf8_lossy(msg.get(at + 1..at + 1 + len)?).into_owned());
        at += 1 + len;
    }
    None
}

/// The address asked about and the name in the first PTR answer.
fn reverse_answer(msg: &[u8]) -> Option<(Ipv4Addr, String)> {
    let count = |at: usize| u16::from_be_bytes([msg[at], msg[at + 1]]) as usize;
    if msg.len() < 12 || msg[2] & 0x80 == 0 {
        return None;
    }
    let (questions, answers) = (count(4), count(6));
    let mut at = 12;
    for _ in 0..questions {
        at = dns_name(msg, at)?.1 + 4;
    }
    for _ in 0..answers {
        let (owner, end) = dns_name(msg, at)?;
        let head = msg.get(end..end + 10)?;
        let (kind, len) = (u16::from_be_bytes([head[0], head[1]]), u16::from_be_bytes([head[8], head[9]]) as usize);
        let data = end + 10;
        if kind == 12 {
            let parts: Vec<&str> = owner.strip_suffix(".in-addr.arpa")?.split('.').collect();
            if parts.len() == 4 {
                let o: Vec<u8> = parts.iter().rev().filter_map(|p| p.parse().ok()).collect();
                if o.len() == 4 {
                    let name = dns_name(msg, data)?.0;
                    let name = name.strip_suffix(".local").unwrap_or(&name).to_string();
                    if !name.is_empty() {
                        return Some((Ipv4Addr::new(o[0], o[1], o[2], o[3]), name));
                    }
                }
            }
        }
        at = data + len;
    }
    None
}

// ---------- SSDP ----------

#[derive(Default, Debug)]
struct Described {
    name: Option<String>,
    model: Option<String>,
    /// "upnp-tv", "upnp-printer", … from its device type.
    kind: Option<String>,
}

fn ssdp_all(wait: Duration) -> HashMap<Ipv4Addr, Described> {
    let mut out = HashMap::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return out;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = sock.set_multicast_ttl_v4(2);
    let search = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: ssdp:all\r\n\r\n";
    let _ = sock.send_to(search.as_bytes(), "239.255.255.250:1900");
    let start = Instant::now();
    let mut locations: HashMap<Ipv4Addr, Vec<String>> = HashMap::new();
    let mut buf = [0u8; 2048];
    while start.elapsed() < wait {
        if start.elapsed() > Duration::from_millis(600) && start.elapsed() < Duration::from_millis(800) {
            let _ = sock.send_to(search.as_bytes(), "239.255.255.250:1900");
        }
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        let IpAddr::V4(ip) = from.ip() else { continue };
        let text = String::from_utf8_lossy(&buf[..n]);
        if let Some(loc) = header(&text, "location") {
            let list = locations.entry(ip).or_default();
            if !list.contains(&loc) && list.len() < 3 {
                list.push(loc);
            }
        }
    }
    // Each device's description, all at once, a short wait each.
    let (tx, rx) = mpsc::channel();
    for (ip, locs) in locations {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut best = Described::default();
            for loc in locs {
                // Only the device's own address: a description elsewhere
                // is not asked for.
                if !loc.contains(&ip.to_string()) {
                    continue;
                }
                if let Some(d) = describe(&loc) {
                    let better = best.name.is_none() || d.kind.is_some() && best.kind.is_none();
                    if better {
                        best = d;
                    }
                }
            }
            let _ = tx.send((ip, best));
        });
    }
    drop(tx);
    let deadline = Instant::now() + Duration::from_millis(1500);
    while let Ok((ip, d)) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if d.name.is_some() || d.model.is_some() {
            out.insert(ip, d);
        }
    }
    out
}

fn header(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
    })
}

fn describe(location: &str) -> Option<Described> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(1200)).build();
    let resp = agent.get(location).call().ok()?;
    let mut body = String::new();
    resp.into_reader().take(64 * 1024).read_to_string(&mut body).ok()?;
    Some(parse_description(&body))
}

/// A UPnP device description: its friendly name, model and kind.
fn parse_description(xml: &str) -> Described {
    let mut d = Described::default();
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return d;
    };
    let Some(device) = doc.descendants().find(|n| n.tag_name().name() == "device") else {
        return d;
    };
    let field = |name: &str| {
        device
            .children()
            .find(|n| n.tag_name().name() == name)
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    d.name = field("friendlyName");
    let maker = field("manufacturer").unwrap_or_default();
    d.model = match (field("modelName"), maker.is_empty()) {
        (Some(m), false) if !m.to_lowercase().contains(&maker.to_lowercase()) => Some(format!("{maker} {m}")),
        (m, _) => m,
    };
    let ty = field("deviceType").unwrap_or_default().to_lowercase();
    d.kind = if ty.contains("mediarenderer") || ty.contains("tvdevice") || ty.contains("dial") {
        Some("upnp-tv".into())
    } else if ty.contains("printer") {
        Some("upnp-printer".into())
    } else if ty.contains("internetgatewaydevice") {
        Some("upnp-router".into())
    } else if ty.contains("mediaserver") {
        Some("upnp-media".into())
    } else {
        None
    };
    d
}

// ---------- ping ----------

/// Ping every address once (ICMP echo) and return the ones that answered.
/// An unprivileged ping socket: Android lets every app have one, as macOS
/// does; on a Linux computer that does not allow it (ping_group_range),
/// nothing, and the knock and the neighbour table do the work.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos", target_os = "ios"))]
fn ping_all(nets: &[(Ipv4Addr, u8)], wait: Duration) -> Vec<Ipv4Addr> {
    use socket2::{Domain, Protocol, Socket, Type};
    let mut out = Vec::new();
    let Ok(sock) = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4)) else {
        return out;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(100)));
    let all: Vec<Ipv4Addr> = nets.iter().flat_map(|(ip, prefix)| hosts(*ip, *prefix)).collect();
    // Three rounds over the wait, each to what has not answered yet: on a
    // phone's Wi-Fi beside the other searches, much of one burst is lost
    // (the lookups of the hardware addresses, mostly).
    let rounds = 3u32;
    let mut sent = 0u32;
    let start = Instant::now();
    let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 512];
    while start.elapsed() < wait {
        if sent < rounds && start.elapsed() >= wait * sent / (rounds + 1) {
            sent += 1;
            let echo = echo_request(0x4b47, sent as u16);
            for (i, host) in all.iter().filter(|h| !out.contains(*h)).enumerate() {
                let _ = sock.send_to(&echo, &SocketAddr::new(IpAddr::V4(*host), 0).into());
                if i % 32 == 31 {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        // An echo reply (type 0); macOS hands the IP header too.
        let bytes: Vec<u8> = buf[..n].iter().map(|b| unsafe { b.assume_init() }).collect();
        let icmp = if bytes.first().map(|b| b >> 4) == Some(4) { &bytes[((bytes[0] & 0x0f) as usize * 4).min(n)..] } else { &bytes[..] };
        if icmp.first() != Some(&0) {
            continue;
        }
        if let Some(addr) = from.as_socket_ipv4() {
            if !out.contains(addr.ip()) {
                out.push(*addr.ip());
            }
        }
    }
    out
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", target_os = "ios")))]
fn ping_all(_nets: &[(Ipv4Addr, u8)], _wait: Duration) -> Vec<Ipv4Addr> {
    Vec::new()
}

/// An ICMP echo request with its checksum.
fn echo_request(id: u16, seq: u16) -> Vec<u8> {
    let mut p = vec![8, 0, 0, 0];
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(b"kagaz");
    let mut sum: u32 = 0;
    for pair in p.chunks(2) {
        sum += u32::from(pair[0]) << 8 | u32::from(*pair.get(1).unwrap_or(&0));
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    let c = !(sum as u16);
    p[2..4].copy_from_slice(&c.to_be_bytes());
    p
}

// ---------- NetBIOS ----------

/// Ask every address its NetBIOS name (a Windows computer or Samba answers
/// with its computer name).
fn netbios_all(nets: &[(Ipv4Addr, u8)], wait: Duration) -> HashMap<Ipv4Addr, String> {
    let mut out = HashMap::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return out;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(150)));
    let query = netbios_query(0x4b47);
    for (ip, prefix) in nets {
        for host in hosts(*ip, *prefix) {
            let _ = sock.send_to(&query, (host, 137));
        }
    }
    let start = Instant::now();
    let mut buf = [0u8; 1024];
    while start.elapsed() < wait {
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        let IpAddr::V4(ip) = from.ip() else { continue };
        if let Some(name) = netbios_name(&buf[..n]) {
            out.insert(ip, name);
        }
    }
    out
}

/// A node status request for the name "*".
fn netbios_query(id: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(50);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    q.push(32);
    // "*" padded with nuls, each half-byte as 'A' + value.
    let mut name = [0u8; 16];
    name[0] = b'*';
    for b in name {
        q.push(b'A' + (b >> 4));
        q.push(b'A' + (b & 0x0f));
    }
    q.push(0);
    q.extend_from_slice(&[0x00, 0x21, 0x00, 0x01]);
    q
}

/// The computer's name from a node status answer: the first unique name of
/// type 0x00 (the workstation).
fn netbios_name(reply: &[u8]) -> Option<String> {
    // Header 12, the name echoed (34), type, class, TTL, length = 56,
    // then the count of names, 18 bytes each.
    let count = *reply.get(56)? as usize;
    for i in 0..count {
        let at = 57 + i * 18;
        let entry = reply.get(at..at + 18)?;
        let (kind, flags) = (entry[15], entry[16]);
        let group = flags & 0x80 != 0;
        if kind == 0 && !group {
            let name = String::from_utf8_lossy(&entry[..15]).trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

// ---------- the network ----------

/// This device's IPv4 networks small enough to look over (a home network,
/// not a /16), as (own address, prefix length).
pub fn local_networks() -> Vec<(Ipv4Addr, u8)> {
    let mut out = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                let prefix = u32::from(v4.netmask).count_ones() as u8;
                // Not the link-local, nor a VPN's point-to-point network.
                if (22..=30).contains(&prefix) && !v4.ip.is_link_local() && !out.iter().any(|(ip, _)| *ip == v4.ip) {
                    out.push((v4.ip, prefix));
                }
            }
        }
    }
    out
}

fn same_net(a: Ipv4Addr, b: Ipv4Addr, prefix: u8) -> bool {
    let mask = u32::MAX << (32 - prefix as u32);
    u32::from(a) & mask == u32::from(b) & mask
}

fn hosts(ip: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    let mask = u32::MAX << (32 - prefix as u32);
    let net = u32::from(ip) & mask;
    (1..(!mask)).map(|i| Ipv4Addr::from(net | i)).filter(|h| *h != ip).collect()
}

/// The router: the default route's gateway.
fn gateway() -> Option<Ipv4Addr> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let text = std::fs::read_to_string("/proc/net/route").ok()?;
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() > 2 && cols[1] == "00000000" {
                let raw = u32::from_str_radix(cols[2], 16).ok()?;
                return Some(Ipv4Addr::from(u32::from_be(raw)));
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("route").args(["-n", "get", "default"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.trim().strip_prefix("gateway:").and_then(|g| g.trim().parse().ok()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("route")
            .args(["print", "-4", "0.0.0.0"])
            .creation_flags(0x0800_0000)
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            (cols.len() >= 3 && cols[0] == "0.0.0.0" && cols[1] == "0.0.0.0").then(|| cols[2].parse().ok()).flatten()
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
    {
        None
    }
}

/// "%5BLG%5D%20webOS%20TV" as it was meant: "[LG] webOS TV".
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

// ---------- hardware addresses ----------

fn colons(mac: &str) -> String {
    let m = crate::cameras::local::normalise_mac(mac);
    m.as_bytes()
        .chunks(2)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join(":")
}

/// A locally administered address: made up by the device (a phone's
/// private address for this Wi-Fi), so its first half names no maker.
pub fn is_private(mac: &str) -> bool {
    let m = crate::cameras::local::normalise_mac(mac);
    u8::from_str_radix(m.get(..2).unwrap_or("00"), 16).map(|b| b & 0x02 != 0).unwrap_or(false)
}

static OUI: &[u8] = include_bytes!("../data/oui.tsv.gz");

fn oui_table() -> &'static [(u32, String)] {
    static TABLE: OnceLock<Vec<(u32, String)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut text = String::new();
        if flate2::read::GzDecoder::new(OUI).read_to_string(&mut text).is_err() {
            return Vec::new();
        }
        text.lines()
            .filter_map(|l| {
                let (block, name) = l.split_once('\t')?;
                Some((u32::from_str_radix(block, 16).ok()?, name.to_string()))
            })
            .collect()
    })
}

/// The maker of a hardware address's block, from the IEEE's list.
pub fn maker(mac: &str) -> Option<&'static str> {
    let m = crate::cameras::local::normalise_mac(mac);
    let block = u32::from_str_radix(m.get(..6)?, 16).ok()?;
    let table = oui_table();
    table.binary_search_by_key(&block, |(b, _)| *b).ok().map(|i| table[i].1.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makers_come_from_the_ieee_list() {
        // 3C:5A:B4 is one of Google's blocks, F0:18:98 one of Apple's.
        assert_eq!(maker("3c:5a:b4:00:00:01"), Some("Google"));
        assert_eq!(maker("F0-18-98-12-34-56"), Some("Apple"));
        assert_eq!(maker("not a mac"), None);
    }

    #[test]
    fn a_made_up_address_is_private() {
        assert!(is_private("da:a1:19:00:00:01"));
        assert!(is_private("02:00:00:00:00:00"));
        assert!(!is_private("f0:18:98:12:34:56"));
    }

    #[test]
    fn netbios_query_and_answer() {
        let q = netbios_query(1);
        assert_eq!(q.len(), 50);
        assert_eq!(&q[13..15], b"CK"); // '*' is 0x2a: 'A'+2, 'A'+10
        // An answer with one group name and one unique workstation name.
        let mut r = vec![0u8; 56];
        r.push(2);
        let mut entry = |name: &str, kind: u8, flags: u8| {
            let mut e = [b' '; 18];
            e[..name.len()].copy_from_slice(name.as_bytes());
            e[15] = kind;
            e[16] = flags;
            e[17] = 0;
            r.extend_from_slice(&e);
        };
        entry("WORKGROUP", 0, 0x80);
        entry("STUDY-PC", 0, 0x04);
        assert_eq!(netbios_name(&r).as_deref(), Some("STUDY-PC"));
    }

    #[test]
    fn a_name_asked_for_by_address() {
        let ip = Ipv4Addr::new(192, 0, 2, 15);
        let q = reverse_query(7, ip);
        assert_eq!(dns_name(&q, 12).map(|(n, _)| n).as_deref(), Some("15.2.0.192.in-addr.arpa"));
        // The answer as Avahi sends it: the question echoed, then a PTR
        // whose owner points back at the question's name.
        let mut r = q.clone();
        r[2] = 0x84;
        r[7] = 1;
        r.extend_from_slice(&[0xc0, 12, 0, 12, 0, 1, 0, 0, 0, 10]);
        let target = b"\x09study-box\x05local\x00";
        r.extend_from_slice(&(target.len() as u16).to_be_bytes());
        r.extend_from_slice(target);
        assert_eq!(reverse_answer(&r), Some((ip, "study-box".to_string())));
        // A question with no answer, and a query (not an answer), say nothing.
        assert_eq!(reverse_answer(&q), None);
        let named = |n: &str, s: &str| Host { name: Some(n.into()), services: vec![s.into()], ..Default::default() };
        assert_eq!(guess(&named("study-box", "mdns-host")), "computer");
        assert_eq!(guess(&named("LGwebOSTV-1", "mdns-host")), "tv");
        assert_eq!(guess(&named("shelly1-ab12", "mdns-host")), "smart-home");
        assert_eq!(guess(&Host { services: vec!["android".into()], ..Default::default() }), "phone");
    }

    #[test]
    fn systems_and_ports_say_what_it_is() {
        assert_eq!(os_in("SSH-2.0-OpenSSH_10.2p1 Ubuntu-2ubuntu3.8"), Some("Ubuntu"));
        assert_eq!(os_in("SSH-2.0-OpenSSH_for_Windows_9.5"), Some("Windows"));
        assert_eq!(os_in("server: Microsoft-IIS/10.0"), Some("Windows"));
        assert_eq!(os_in("SSH-2.0-dropbear_2022.83"), None);
        assert_eq!(os_family("Ubuntu 26.04"), "Linux");
        let with = |open: &[u16], services: &[&str]| Host {
            open: open.to_vec(),
            services: services.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        // A TV keeps a printing port open: still a TV.
        assert_eq!(guess(&with(&[515, 8008, 8009], &[])), "tv");
        assert_eq!(guess(&with(&[515], &[])), "printer");
        // Windows by its own ports; a NetBIOS name without Avahi.
        let mut pc = with(&[135, 445], &["netbios"]);
        pc.kind = guess(&pc).into();
        assert_eq!(os_of(&pc).as_deref(), Some("Windows"));
        // Kagaz on a computer, a screen over Cast: a computer, its port named.
        let mut k = with(&[8009], &["googlecast"]);
        k.model = Some("Kagaz".into());
        assert_eq!(guess(&k), "computer");
        label_ports(&mut k);
        assert_eq!(k.ports[0].what, "Kagaz screen");
        // An SSH greeting names the system.
        let mut b = with(&[22], &["mdns-host"]);
        b.ports.push(Port { port: 22, what: "SSH".into(), detail: Some("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13".into()) });
        assert_eq!(os_of(&b).as_deref(), Some("Ubuntu"));
        // A page title is no game console's name.
        let mut t = with(&[8080], &[]);
        t.name = Some("LightSwitch Panel".into());
        assert_ne!(guess(&t), "console");
    }

    #[test]
    fn echo_request_checksum_sums_to_ffff() {
        let p = echo_request(0x1234, 7);
        let mut sum: u32 = 0;
        for pair in p.chunks(2) {
            sum += u32::from(pair[0]) << 8 | u32::from(*pair.get(1).unwrap_or(&0));
        }
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        assert_eq!(sum, 0xffff);
        assert_eq!(p[0], 8);
    }

    #[test]
    fn upnp_description() {
        let xml = r#"<?xml version="1.0"?><root xmlns="urn:schemas-upnp-org:device-1-0"><device>
            <deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
            <friendlyName>Living room TV</friendlyName><manufacturer>LG Electronics</manufacturer>
            <modelName>OLED55</modelName></device></root>"#;
        let d = parse_description(xml);
        assert_eq!(d.name.as_deref(), Some("Living room TV"));
        assert_eq!(d.model.as_deref(), Some("LG Electronics OLED55"));
        assert_eq!(d.kind.as_deref(), Some("upnp-tv"));
    }

    #[test]
    fn guesses() {
        let base = Host { ip: Ipv4Addr::new(192, 0, 2, 20), ..Default::default() };
        let iphone = Host { open: vec![62078], ..base.clone() };
        assert_eq!(guess(&iphone), "phone");
        let private = Host { private_mac: true, ..base.clone() };
        assert_eq!(guess(&private), "phone");
        let cast = Host { services: vec!["googlecast".into()], name: Some("Bedroom TV".into()), ..base.clone() };
        assert_eq!(guess(&cast), "tv");
        let pc = Host { services: vec!["netbios".into()], name: Some("STUDY".into()), ..base.clone() };
        assert_eq!(guess(&pc), "computer");
        let plug = Host { maker: Some("Espressif".into()), ..base.clone() };
        assert_eq!(guess(&plug), "smart-home");
        let router = Host { router: true, open: vec![80], ..base.clone() };
        assert_eq!(guess(&router), "router");
        assert_eq!(guess(&base), "unknown");
        assert_eq!(short_type("_googlecast._tcp.local."), "googlecast");
        assert_eq!(percent_decode("%5BLG%5D%20webOS%20TV"), "[LG] webOS TV");
        assert_eq!(percent_decode("100%"), "100%");
        let rdp = Host { open: vec![3389], ..base.clone() };
        assert_eq!(guess(&rdp), "computer");
        let tuya = Host { open: vec![6668], ..base.clone() };
        assert_eq!(guess(&tuya), "smart-home");
        assert_eq!(page_title("<html><TITLE>\n Router &amp; Wi-Fi </title>").as_deref(), Some("Router & Wi-Fi"));
        assert_eq!(page_title("<html>no title"), None);
    }
}
