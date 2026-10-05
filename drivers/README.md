# Driver database

One TOML file per model family, under `<manufacturer>/`. Only links to the
vendor's own servers, never mirrored binaries. Every file lists, per OS, the
official packages with the SHA-256 of the version it was checked with, the
steps the installer runs as administrator, and the manual steps the vendor
requires afterwards. `kagaz driver <device>` reads these; files are embedded
into the binary at build time, and `KAGAZ_DRIVERS_DIR` points at a folder to
read instead while you work on an entry.

Fields:

```toml
[model]
manufacturer = "Brother"
models = ["DCP-L2540DW"]        # matched after removing spaces, dashes and "series", case-insensitive
functions = ["scan"]            # what the driver adds: print, scan, scan-button

[[linux.packages]]              # also [[windows.packages]], [[macos.packages]]
name = "brscan4"
kind = "deb"                    # deb | rpm | exe | pkg | dmg | sh
arch = "x86_64"                 # x86_64 | aarch64 | i686 | any
url = "https://..."
sha256 = "..."                  # omit if the vendor publishes none; Kagaz then warns
size = 111246

[linux]
admin_steps = [ "..." ]         # shell, run as root after the packages, with {ip} {model} {name} {kagaz} {scans_dir} filled in
user_steps = [ "..." ]          # shell, run as the user afterwards
remove_steps = [ "..." ]        # as root, to undo
remove_user_steps = [ "..." ]   # as the user, to undo
notes = [ "..." ]               # plain words shown before consent
```
