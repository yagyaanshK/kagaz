//! From a database entry to an installed driver: a plan the person can read
//! in full, downloads verified against the pinned checksum, then the
//! administrator steps through the OS's own prompt (pkexec or sudo on
//! Linux) and the user steps as the user.

use super::{Entry, OsEntry, Package};
use crate::{Device, Os};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedPackage {
    pub name: String,
    pub url: String,
    pub file: PathBuf,
    pub size: Option<u64>,
    /// Pinned checksum, when the database has one.
    pub sha256: Option<String>,
    /// Already present and verified in the download folder.
    pub cached: bool,
}

/// Everything that will happen, in the order it will happen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub device: String,
    pub source: String,
    pub notes: Vec<String>,
    pub packages: Vec<PlannedPackage>,
    /// The exact shell script run as administrator.
    pub admin_script: String,
    /// The exact shell script run as the user afterwards.
    pub user_script: String,
    pub remove_script: String,
    pub remove_user_script: String,
    pub download_dir: PathBuf,
    pub already_installed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Downloading { name: String, bytes: Option<u64> },
    Verified { name: String, cached: bool },
    Unverified { name: String },
    AdminStep,
    UserStep,
    Done,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no driver entry for {0}")]
    NoEntry(String),
    #[error("the entry has nothing for {0}")]
    NoOs(String),
    #[error("download failed for {name}: {reason}")]
    Download { name: String, reason: String },
    #[error(
        "{name} does not match the checksum the database knows (got {got}); not installing it"
    )]
    Checksum { name: String, got: String },
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the administrator step failed (exit {0}); nothing more was done")]
    AdminFailed(i32),
    #[error("the user step failed (exit {0})")]
    UserFailed(i32),
    #[error("no way to ask for administrator rights: neither pkexec nor sudo is available")]
    NoElevation,
}

/// Values filled into `{ip}`, `{model}`, `{name}`, `{kagaz}`, `{scans_dir}`.
#[derive(Debug, Clone)]
pub struct Context {
    pub ip: String,
    pub model: String,
    pub name: String,
    pub kagaz: PathBuf,
    pub scans_dir: PathBuf,
    pub download_dir: PathBuf,
}

impl Context {
    pub fn for_device(device: &Device) -> Context {
        let model = device.model.clone().unwrap_or_else(|| device.name.clone());
        let model = device
            .manufacturer
            .as_deref()
            .and_then(|mf| model.strip_prefix(mf))
            .map(|m| m.trim().to_string())
            .unwrap_or(model);
        let model = model.trim_end_matches(" series").to_string();
        Context {
            ip: device
                .addresses
                .iter()
                .find(|a| a.is_ipv4())
                .or(device.addresses.first())
                .map(|a| a.to_string())
                .unwrap_or_default(),
            name: model.replace(' ', "-"),
            model,
            kagaz: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kagaz")),
            scans_dir: crate::paths::scans_dir(),
            download_dir: crate::paths::download_dir(),
        }
    }

    pub fn fill(&self, template: &str) -> String {
        template
            .replace("{ip}", &self.ip)
            .replace("{model}", &self.model)
            .replace("{name}", &self.name)
            .replace("{kagaz}", &self.kagaz.display().to_string())
            .replace("{scans_dir}", &self.scans_dir.display().to_string())
    }
}

/// Which packaging this Linux uses, from what is on the PATH.
pub fn package_kind(os: Os) -> &'static str {
    match os {
        Os::Linux => {
            if which("dpkg") {
                "deb"
            } else if which("rpm") {
                "rpm"
            } else {
                "sh"
            }
        }
        Os::Windows => "exe",
        Os::MacOs => "pkg",
        Os::Other => "sh",
    }
}

pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "x86" => "i686",
        other => other,
    }
}

fn which(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

/// Build the plan without touching the network or the system.
pub fn plan(
    entry: &Entry,
    os: Os,
    ctx: &Context,
    device_title: &str,
) -> Result<Plan, InstallError> {
    let os_entry: &OsEntry = entry
        .for_os(os)
        .ok_or_else(|| InstallError::NoOs(os.label().to_string()))?;
    let kind = package_kind(os);
    let my_arch = arch();
    let packages: Vec<&Package> = os_entry
        .packages
        .iter()
        .filter(|p| p.kind == kind && (p.arch == "any" || p.arch == my_arch))
        .collect();
    if packages.is_empty() && os_entry.admin_steps.is_empty() {
        return Err(InstallError::NoOs(format!(
            "{} with {kind} packages on {my_arch}",
            os.label()
        )));
    }
    let planned: Vec<PlannedPackage> = packages
        .iter()
        .map(|p| {
            let file = ctx.download_dir.join(file_name(&p.url));
            let cached = p
                .sha256
                .as_deref()
                .map(|sum| file.is_file() && sha256_file(&file).ok().as_deref() == Some(sum))
                .unwrap_or(false);
            PlannedPackage {
                name: p.name.clone(),
                url: p.url.clone(),
                file,
                size: p.size,
                sha256: p.sha256.clone(),
                cached,
            }
        })
        .collect();

    let install_lines: Vec<String> = match kind {
        "deb" if !planned.is_empty() => vec![format!(
            "dpkg -i {}",
            planned
                .iter()
                .map(|p| quote(&p.file))
                .collect::<Vec<_>>()
                .join(" ")
        )],
        "rpm" if !planned.is_empty() => vec![format!(
            "rpm -Uvh {}",
            planned
                .iter()
                .map(|p| quote(&p.file))
                .collect::<Vec<_>>()
                .join(" ")
        )],
        "sh" => planned
            .iter()
            .map(|p| format!("sh {}", quote(&p.file)))
            .collect(),
        _ => Vec::new(),
    };
    let admin_script = script(
        install_lines
            .into_iter()
            .chain(os_entry.admin_steps.iter().map(|s| ctx.fill(s))),
    );
    let user_script = script(os_entry.user_steps.iter().map(|s| ctx.fill(s)));
    let remove_script = script(os_entry.remove_steps.iter().map(|s| ctx.fill(s)));
    let remove_user_script = script(os_entry.remove_user_steps.iter().map(|s| ctx.fill(s)));
    let already_installed = planned
        .iter()
        .filter(|p| package_installed(kind, &p.name))
        .map(|p| p.name.clone())
        .collect();
    Ok(Plan {
        device: device_title.to_string(),
        source: entry.source.clone(),
        notes: os_entry.notes.iter().map(|n| ctx.fill(n)).collect(),
        packages: planned,
        admin_script,
        user_script,
        remove_script,
        remove_user_script,
        download_dir: ctx.download_dir.clone(),
        already_installed,
    })
}

fn script(lines: impl Iterator<Item = String>) -> String {
    let body: Vec<String> = lines.collect();
    if body.is_empty() {
        return String::new();
    }
    format!("set -e\n{}\n", body.join("\n"))
}

fn quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

fn file_name(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("package")
        .to_string()
}

fn package_installed(kind: &str, name: &str) -> bool {
    let out = match kind {
        "deb" => Command::new("dpkg-query")
            .args(["-W", "-f", "${Status}", name])
            .output(),
        "rpm" => Command::new("rpm").args(["-q", name]).output(),
        _ => return false,
    };
    out.map(|o| {
        o.status.success() && String::from_utf8_lossy(&o.stdout).contains("install ok installed")
            || (kind == "rpm" && o.status.success())
    })
    .unwrap_or(false)
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Download every package that is not already cached and verify the ones
/// with a pinned checksum. Unpinned packages are reported, not refused.
pub fn download(plan: &Plan, on_event: &mut dyn FnMut(Event)) -> Result<(), InstallError> {
    std::fs::create_dir_all(&plan.download_dir).map_err(|source| InstallError::Io {
        path: plan.download_dir.clone(),
        source,
    })?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .build();
    for p in &plan.packages {
        if p.cached {
            on_event(Event::Verified {
                name: p.name.clone(),
                cached: true,
            });
            continue;
        }
        on_event(Event::Downloading {
            name: p.name.clone(),
            bytes: p.size,
        });
        let response = agent
            .get(&p.url)
            .call()
            .map_err(|e| InstallError::Download {
                name: p.name.clone(),
                reason: e.to_string(),
            })?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| InstallError::Download {
                name: p.name.clone(),
                reason: e.to_string(),
            })?;
        let tmp = p.file.with_extension("part");
        std::fs::File::create(&tmp)
            .and_then(|mut f| f.write_all(&bytes))
            .map_err(|source| InstallError::Io {
                path: tmp.clone(),
                source,
            })?;
        if let Some(expected) = &p.sha256 {
            let got = sha256_file(&tmp).map_err(|source| InstallError::Io {
                path: tmp.clone(),
                source,
            })?;
            if &got != expected {
                let _ = std::fs::remove_file(&tmp);
                return Err(InstallError::Checksum {
                    name: p.name.clone(),
                    got,
                });
            }
            on_event(Event::Verified {
                name: p.name.clone(),
                cached: false,
            });
        } else {
            on_event(Event::Unverified {
                name: p.name.clone(),
            });
        }
        std::fs::rename(&tmp, &p.file).map_err(|source| InstallError::Io {
            path: p.file.clone(),
            source,
        })?;
    }
    Ok(())
}

/// Run `script` as administrator through pkexec (graphical prompt) or sudo.
pub fn run_as_admin(script: &str) -> Result<(), InstallError> {
    if script.trim().is_empty() {
        return Ok(());
    }
    let dir = std::env::temp_dir();
    let path = dir.join(format!("kagaz-admin-{}.sh", std::process::id()));
    std::fs::write(&path, script).map_err(|source| InstallError::Io {
        path: path.clone(),
        source,
    })?;
    let graphical =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    let status = if graphical && which("pkexec") {
        Command::new("pkexec")
            .args(["sh", &path.display().to_string()])
            .status()
    } else if which("sudo") {
        Command::new("sudo")
            .args(["sh", &path.display().to_string()])
            .status()
    } else {
        let _ = std::fs::remove_file(&path);
        return Err(InstallError::NoElevation);
    };
    let _ = std::fs::remove_file(&path);
    let status = status.map_err(|source| InstallError::Io {
        path: PathBuf::from("pkexec/sudo"),
        source,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(InstallError::AdminFailed(status.code().unwrap_or(-1)))
    }
}

/// Run `script` as the current user.
pub fn run_as_user(script: &str) -> Result<(), InstallError> {
    if script.trim().is_empty() {
        return Ok(());
    }
    let status = Command::new("sh")
        .args(["-c", script])
        .status()
        .map_err(|source| InstallError::Io {
            path: PathBuf::from("sh"),
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(InstallError::UserFailed(status.code().unwrap_or(-1)))
    }
}

/// The whole thing after consent: download, verify, admin steps, user steps.
pub fn install(plan: &Plan, on_event: &mut dyn FnMut(Event)) -> Result<(), InstallError> {
    download(plan, on_event)?;
    on_event(Event::AdminStep);
    run_as_admin(&plan.admin_script)?;
    if !plan.user_script.trim().is_empty() {
        on_event(Event::UserStep);
        run_as_user(&plan.user_script)?;
    }
    on_event(Event::Done);
    Ok(())
}

/// Undo: the remove steps as administrator, then as the user.
pub fn remove(plan: &Plan, on_event: &mut dyn FnMut(Event)) -> Result<(), InstallError> {
    on_event(Event::AdminStep);
    run_as_admin(&plan.remove_script)?;
    if !plan.remove_user_script.trim().is_empty() {
        on_event(Event::UserStep);
        run_as_user(&plan.remove_user_script)?;
    }
    on_event(Event::Done);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers;

    fn brother_device() -> Device {
        Device {
            name: "Brother DCP-L2540DW series".into(),
            manufacturer: Some("Brother".into()),
            model: Some("DCP-L2540DW series".into()),
            addresses: vec!["198.51.100.167".parse().unwrap()],
            ..Device::default()
        }
    }

    fn ctx(dir: &Path) -> Context {
        Context {
            ip: "198.51.100.167".into(),
            model: "DCP-L2540DW".into(),
            name: "DCP-L2540DW".into(),
            kagaz: PathBuf::from("/usr/bin/kagaz"),
            scans_dir: PathBuf::from("/home/me/Scans"),
            download_dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn context_fills_placeholders() {
        let c = Context::for_device(&brother_device());
        assert_eq!(c.ip, "198.51.100.167");
        assert_eq!(c.model, "DCP-L2540DW");
        assert_eq!(c.name, "DCP-L2540DW");
        assert_eq!(
            ctx(Path::new("/tmp")).fill("brsaneconfig4 -a name={name} model={model} ip={ip} > {scans_dir} via {kagaz}"),
            "brsaneconfig4 -a name=DCP-L2540DW model=DCP-L2540DW ip=198.51.100.167 > /home/me/Scans via /usr/bin/kagaz"
        );
    }

    #[test]
    fn brother_plan_on_linux_reads_right() {
        let entries = drivers::load().unwrap();
        let entry = drivers::find(&entries, &brother_device()).unwrap();
        let dir = std::env::temp_dir().join(format!("kagaz-plan-{}", std::process::id()));
        let p = plan(entry, Os::Linux, &ctx(&dir), "Brother DCP-L2540DW series").unwrap();
        if package_kind(Os::Linux) == "deb" {
            assert_eq!(p.packages.len(), 2);
            assert!(p.packages.iter().all(|x| x.sha256.is_some() && !x.cached));
            assert!(p.admin_script.starts_with("set -e\ndpkg -i '"));
            assert!(p
                .admin_script
                .contains("brsaneconfig4 -a name=DCP-L2540DW model=DCP-L2540DW ip=198.51.100.167"));
            assert!(p.admin_script.contains(
                "FILE=\"bash /opt/brother/scanner/brscan-skey/script/kagaz-scantofile.sh\""
            ));
            assert!(p.user_script.contains("autostart/brscan-skey.desktop"));
            assert!(p.remove_script.contains("dpkg -r brscan-skey brscan4"));
            assert!(p.notes.iter().any(|n| n.contains("/home/me/Scans")));
        }
        assert!(plan(entry, Os::Windows, &ctx(&dir), "x").is_err());
    }

    #[test]
    fn checksums_and_file_names() {
        let dir = std::env::temp_dir().join(format!("kagaz-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            file_name("https://x/y/brscan4-0.4.11-1.amd64.deb"),
            "brscan4-0.4.11-1.amd64.deb"
        );
        assert_eq!(quote(Path::new("/tmp/it's")), "'/tmp/it'\\''s'");
    }
}
