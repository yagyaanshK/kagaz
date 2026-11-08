#!/bin/sh
# Put Kagaz in the Linux desktop's app list and dock with its own icon, for
# a build run from this checkout (a packaged .deb or AppImage does this
# itself). Writes only under ~/.local/share; remove with --remove.
#
#   scripts/install-desktop-entry.sh [path/to/kagaz-desktop]   (default: target/release, else target/debug)
#   scripts/install-desktop-entry.sh --remove
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
# The entry's file and icon name. The window's Wayland app id / X11 class is the
# name of the binary that runs, which differs between a cargo build (kagaz-desktop)
# and a release binary (kagaz-desktop-linux-x64): StartupWMClass follows the binary.
id=kagaz-desktop
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"

if [ "${1:-}" = "--remove" ]; then
    rm -f "$apps/$id.desktop"
    for size in 32 128 256 512; do rm -f "$icons/${size}x${size}/apps/$id.png"; done
    command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
    echo "Removed the Kagaz desktop entry."
    exit 0
fi

bin=${1:-}
if [ -z "$bin" ]; then
    for b in "$here/target/release/kagaz-desktop" "$here/target/debug/kagaz-desktop"; do
        if [ -x "$b" ]; then bin=$b; break; fi
    done
fi
if [ -z "$bin" ] || [ ! -x "$bin" ]; then
    echo "No kagaz-desktop build found; run: cargo build -p kagaz-desktop" >&2
    exit 1
fi

for size in 32 128 256 512; do
    case $size in
        32) src=32x32.png ;; 128) src=128x128.png ;; 256) src=256x256.png ;; 512) src=icon.png ;;
    esac
    mkdir -p "$icons/${size}x${size}/apps"
    cp "$here/apps/desktop/icons/$src" "$icons/${size}x${size}/apps/$id.png"
done
wmclass=$(basename "$bin")
# The entry runs a small script that is always on the system disk: GNOME
# drops an entry whose program is missing when the Shell starts, and a
# build on a drive that mounts a moment after login is missing just then.
# The script waits for the drive, then runs the build.
bindir="${XDG_DATA_HOME:-$HOME/.local/share}/../bin"
bindir=$(cd "$(dirname "$bindir")" && pwd)/$(basename "$bindir")
mkdir -p "$bindir"
cat > "$bindir/$id" <<RUN
#!/bin/sh
# Runs Kagaz from its build (written by scripts/install-desktop-entry.sh).
bin="$bin"
n=0
while [ ! -x "\$bin" ] && [ \$n -lt 40 ]; do sleep 0.5; n=\$((n + 1)); done
if [ ! -x "\$bin" ]; then
    command -v notify-send >/dev/null && notify-send "Kagaz" "The build is not there: \$bin (is the drive mounted?)"
    exit 1
fi
exec "\$bin" "\$@"
RUN
chmod +x "$bindir/$id"
mkdir -p "$apps"
cat > "$apps/$id.desktop" <<ENTRY
[Desktop Entry]
Type=Application
Name=Kagaz
Comment=Printers, scanners and cameras on your network
Exec=$bindir/$id
Icon=$id
StartupWMClass=$wmclass
Categories=Utility;
Terminal=false
ENTRY
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true
echo "Kagaz is in the app list with its icon ($bindir/$id runs $bin)."
