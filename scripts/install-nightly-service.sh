#!/bin/sh
# Run `kagaz tapo nightly` as a user service on an always-on computer: it
# copies the cameras' footage every night and watches their clocks. Writes
# only ~/.config/systemd/user/kagaz-nightly.service; remove with --remove.
# Log in first (`kagaz tapo login`), and choose the recordings folder in
# ~/.config/kagaz/settings.toml if it should not be the default.
#
#   scripts/install-nightly-service.sh [nightly options...]
#     e.g. scripts/install-nightly-service.sh --except "Garden" --window 20:00-24:00
#   scripts/install-nightly-service.sh --remove
#
# The service keeps running after you log out only with lingering on:
#   loginctl enable-linger "$USER"
# Its messages: journalctl --user -u kagaz-nightly -f
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/kagaz-nightly.service"

if [ "${1:-}" = "--remove" ]; then
    systemctl --user disable --now kagaz-nightly.service 2>/dev/null || true
    rm -f "$unit"
    systemctl --user daemon-reload
    echo "Removed the Kagaz nightly service."
    exit 0
fi

bin=
for b in "$here/target/release/kagaz" "$here/target/debug/kagaz"; do
    if [ -x "$b" ]; then bin=$b; break; fi
done
if [ -z "$bin" ]; then
    echo "No kagaz build found; run: cargo build --release -p kagaz-cli" >&2
    exit 1
fi

# Quote each option for systemd's ExecStart.
args=
for a in "$@"; do
    args="$args \"$(printf '%s' "$a" | sed 's/["\\]/\\&/g')\""
done

mkdir -p "$(dirname "$unit")"
cat > "$unit" <<EOF
[Unit]
Description=Kagaz: nightly copy of the cameras' recordings and clock watcher
After=network-online.target
Wants=network-online.target

[Service]
ExecStart="$bin" tapo nightly$args
Restart=on-failure
RestartSec=60

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload
systemctl --user enable --now kagaz-nightly.service
echo "Installed and started: $unit"
echo "Messages: journalctl --user -u kagaz-nightly -f"
if ! loginctl show-user "$USER" -p Linger 2>/dev/null | grep -q yes; then
    echo "To keep it running after you log out: loginctl enable-linger $USER"
fi
