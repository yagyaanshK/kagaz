#!/bin/bash
# Screen capture for TV mirroring, tested in a throwaway headless GNOME Shell
# with its own PipeWire: runtime, config, data and dconf live in a temporary
# folder, exported before dbus-run-session so every service inherits them.
# The real session's screen is never captured.
#
#   crates/kagaz-core/tests/headless-screen.sh     (needs gnome-shell, pipewire, wireplumber)
set -u
SRC="$(cd "$(dirname "$0")/../../.." && pwd)"
R=$(mktemp -d /tmp/kagaz-screen-XXXXXX)
export KAGAZ_TEST_SESSION=$R
stop_session() {
    local p pids=""
    for p in $(pgrep -u "$(id -u)"); do
        [ "$p" = $$ ] && continue
        { tr '\0' '\n' < /proc/$p/environ; } 2>/dev/null | grep -qx "KAGAZ_TEST_SESSION=$R" && pids="$pids $p"
    done
    [ -n "$pids" ] || return 0
    kill $pids 2>/dev/null; timeout 2 tail -f /dev/null; kill -9 $pids 2>/dev/null
    return 0
}
cleanup() {
    stop_session
    for m in $R/run/doc $R/run/gvfs; do grep -qs " $m " /proc/mounts && fusermount3 -u -z $m; done
    [ -n "${KEEP:-}" ] && { echo "kept $R"; return; }
    rm -rf "$R"
}
trap cleanup EXIT

export XDG_RUNTIME_DIR=$R/run XDG_CONFIG_HOME=$R/config XDG_DATA_HOME=$R/data XDG_CACHE_HOME=$R/cache
export XDG_STATE_HOME=$R/state GVFS_DISABLE_FUSE=1 GIO_USE_VFS=local
mkdir -p $XDG_RUNTIME_DIR $XDG_CONFIG_HOME $XDG_DATA_HOME $XDG_STATE_HOME
chmod 700 $XDG_RUNTIME_DIR
echo "user-db:kagaztest" > $R/dconf-profile; export DCONF_PROFILE=$R/dconf-profile
unset WAYLAND_DISPLAY DISPLAY PIPEWIRE_REMOTE

dbus-run-session -- bash -c "
    echo \$DBUS_SESSION_BUS_ADDRESS > $R/bus
    pipewire > $R/pipewire.log 2>&1 &
    sleep 0.5
    wireplumber > $R/wireplumber.log 2>&1 &
    exec gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x800 --unsafe-mode" \
    > $R/shell.log 2>&1 &

for i in $(seq 1 40); do
    [ -s $R/bus ] && gdbus introspect --address "$(cat $R/bus)" -d org.gnome.Mutter.ScreenCast \
        -o /org/gnome/Mutter/ScreenCast > /dev/null 2>&1 && break
    timeout 1 tail -f /dev/null
done
timeout 3 tail -f /dev/null

cd "$SRC"
DBUS_SESSION_BUS_ADDRESS="$(cat $R/bus)" KAGAZ_HEADLESS=1 \
    cargo test -p kagaz-core --lib tv::capture::tests::headless -- --ignored --nocapture --test-threads 1
status=$?
[ $status -ne 0 ] && { echo "--- shell.log"; tail -20 $R/shell.log; echo "--- pipewire.log"; tail -5 $R/pipewire.log; }
exit $status
