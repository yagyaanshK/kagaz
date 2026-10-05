//! Brother's own way of finding Linux packages for a model, used for
//! Brother devices that have no entry in the database yet. Their installer
//! fetches `infs/<MODEL>` (model name without punctuation) which names the
//! driver families, then `infs/<family>.lnk` which names the package file
//! per packaging and architecture. Nothing here is pinned by checksum, so
//! the result is shown as "from Brother's server, unverified".

use super::{Entry, ModelSection, OsEntry, Package};
use std::collections::BTreeMap;
use std::time::Duration;

pub const INFS: &str = "https://download.brother.com/pub/com/linux/linux/infs";
pub const PACKAGES: &str = "https://download.brother.com/pub/com/linux/linux/packages";

/// Parse `KEY=value` lines (both the model inf and the .lnk files).
pub fn parse_kv(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.trim().split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Which `.lnk` key applies here.
pub fn lnk_key(kind: &str, arch: &str) -> String {
    let bits = if arch == "x86_64" || arch == "aarch64" {
        "64"
    } else {
        "32"
    };
    format!("{}{bits}", kind.to_ascii_uppercase())
}

/// Build an entry from the inf and the lnk files it points at.
pub fn entry_from(
    model_key: &str,
    model_name: &str,
    inf: &str,
    lnks: &BTreeMap<String, String>,
    kind: &str,
    arch: &str,
) -> Option<Entry> {
    let inf = parse_kv(inf);
    let families: Vec<&str> = ["SCANNER_DRV", "SCANKEY_DRV"]
        .iter()
        .filter_map(|k| inf.get(*k).map(String::as_str))
        .collect();
    if families.is_empty() {
        return None;
    }
    let key = lnk_key(kind, arch);
    let packages: Vec<Package> = families
        .iter()
        .filter_map(|family| {
            let file = parse_kv(lnks.get(*family)?).remove(&key)?;
            Some(Package {
                name: family.to_string(),
                kind: kind.to_string(),
                arch: arch.to_string(),
                url: format!("{PACKAGES}/{file}"),
                sha256: None,
                size: None,
            })
        })
        .collect();
    if packages.is_empty() {
        return None;
    }
    let has_skey = packages.iter().any(|p| p.name == "brscan-skey");
    let mut admin_steps = vec!["brsaneconfig4 -a name={name} model={model} ip={ip}".to_string()];
    let mut notes = vec![
        "Brother's scanner driver for Linux (brscan4), as named by Brother's download server for this model.".to_string(),
    ];
    if has_skey {
        admin_steps.push("if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then ufw allow from {ip} to any port 54925 proto udp comment 'Brother scan-key tool (Kagaz)'; fi".to_string());
        notes.push(
            "Includes the scan-key tool for the printer's Scan button (UDP port 54925)."
                .to_string(),
        );
    }
    Some(Entry {
        model: ModelSection {
            manufacturer: "Brother".into(),
            models: vec![model_name.to_string()],
            functions: vec!["scan".into()],
        },
        linux: Some(OsEntry {
            packages,
            admin_steps,
            user_steps: Vec::new(),
            remove_steps: vec![
                "brsaneconfig4 -r {name} || true".to_string(),
                if has_skey {
                    "dpkg -r brscan-skey brscan4".to_string()
                } else {
                    "dpkg -r brscan4".to_string()
                },
            ],
            remove_user_steps: Vec::new(),
            notes,
        }),
        windows: None,
        macos: None,
        source: format!("{INFS}/{model_key} (live lookup, not checksum-pinned)"),
    })
}

/// Ask Brother's server about `model_key` (e.g. "DCPL2540DW").
pub fn resolve(
    model_key: &str,
    model_name: &str,
    kind: &str,
    arch: &str,
) -> Result<Option<Entry>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build();
    let get = |url: &str| -> Result<Option<String>, String> {
        match agent.get(url).call() {
            Ok(r) => r.into_string().map(Some).map_err(|e| e.to_string()),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    };
    let Some(inf) = get(&format!("{INFS}/{model_key}"))? else {
        return Ok(None);
    };
    let inf_map = parse_kv(&inf);
    let mut lnks = BTreeMap::new();
    for key in ["SCANNER_DRV", "SCANKEY_DRV"] {
        if let Some(family) = inf_map.get(key) {
            if let Some(text) = get(&format!("{INFS}/{family}.lnk"))? {
                lnks.insert(family.clone(), text);
            }
        }
    }
    Ok(entry_from(model_key, model_name, &inf, &lnks, kind, arch))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INF: &str = include_str!("../../tests/fixtures/drivers/brother/DCPL2540DW");
    const SCAN_LNK: &str = include_str!("../../tests/fixtures/drivers/brother/brscan4.lnk");
    const SKEY_LNK: &str = include_str!("../../tests/fixtures/drivers/brother/brscan-skey.lnk");

    #[test]
    fn resolves_the_brother_packages_from_recordings() {
        let mut lnks = BTreeMap::new();
        lnks.insert("brscan4".to_string(), SCAN_LNK.to_string());
        lnks.insert("brscan-skey".to_string(), SKEY_LNK.to_string());
        let e = entry_from("DCPL2540DW", "DCP-L2540DW", INF, &lnks, "deb", "x86_64").unwrap();
        let linux = e.linux.unwrap();
        assert_eq!(
            linux.packages.iter().map(|p| p.url.as_str()).collect::<Vec<_>>(),
            vec![
                "https://download.brother.com/pub/com/linux/linux/packages/brscan4-0.4.11-1.amd64.deb",
                "https://download.brother.com/pub/com/linux/linux/packages/brscan-skey-0.3.4-0.amd64.deb",
            ]
        );
        assert!(linux.packages.iter().all(|p| p.sha256.is_none()));
        assert_eq!(linux.admin_steps.len(), 2);

        let e = entry_from("DCPL2540DW", "DCP-L2540DW", INF, &lnks, "rpm", "i686").unwrap();
        assert!(e.linux.unwrap().packages[0]
            .url
            .ends_with("brscan4-0.4.11-2.i386.rpm"));
        assert!(entry_from("X", "X", "PRINTERNAME=X\n", &lnks, "deb", "x86_64").is_none());
    }
}
