//! Embed the driver database (`drivers/**/*.toml` at the repository root)
//! into the binary, so an installed Kagaz carries it.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
    let mut files: Vec<PathBuf> = Vec::new();
    collect(&root, &mut files);
    // An empty database would build fine and ship without any driver; a
    // folder that could not be read (a shared drive blinking) must not.
    assert!(
        !files.is_empty(),
        "no driver files found under {}; is the drivers folder readable?",
        root.display()
    );
    files.sort();
    let mut out = String::from("/// (relative path, contents) of every drivers/*.toml at build time.\npub static EMBEDDED: &[(&str, &str)] = &[\n");
    for f in &files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let abs = f.canonicalize().unwrap();
        out.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            rel,
            abs.to_string_lossy()
        ));
        println!("cargo:rerun-if-changed={}", abs.display());
    }
    out.push_str("];\n");
    println!("cargo:rerun-if-changed={}", root.display());
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("drivers_embedded.rs");
    fs::write(dest, out).unwrap();
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "toml") {
            out.push(p);
        }
    }
}
