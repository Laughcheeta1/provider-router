#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 || ! -x $1 || ! -x $2 ]]; then
    echo "Usage: $0 <older-provider-router-binary> <newer-provider-router-binary>" >&2
    exit 2
fi

scripts=$(cd -- "$(dirname -- "$0")" && pwd)
old_version=$("$1" --version)
new_version=$("$2" --version)
work=$(mktemp -d)
export HOME="$work/home with spaces"
export XDG_CONFIG_HOME="$HOME/config"
export XDG_RUNTIME_DIR="$work/runtime"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
unset PROVIDER_ROUTER_DATA_DIR PROVIDER_ROUTER_INSTALL_EXECUTABLE
mkdir -m 0700 "$HOME" "$XDG_RUNTIME_DIR"
installed=$HOME/.local/bin/provider-router
original_pid=

cleanup() {
    if [[ -x $installed ]]; then
        "$installed" stop >/dev/null 2>&1 || true
    fi
    if [[ -n $original_pid ]]; then
        kill "$original_pid" 2>/dev/null || true
        wait "$original_pid" 2>/dev/null || true
    fi
    rm -rf -- "$work"
}
trap cleanup EXIT

older=$work/older.sh
newer=$work/newer.sh
bash "$scripts/build-linux.sh" "$1" "$older"
bash "$scripts/build-linux.sh" "$2" "$newer"
sh -n "$older"
sh "$older"
[[ $("$installed" --version) == "$old_version" ]]
"$installed" status | jq -e '.endpoint == null and .startup_mode == "startup"' >/dev/null

port=$(python3 - <<'PY'
import socket

with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
)
"$installed" network set --port "$port" >/dev/null
"$installed" startup keep-after-open >/dev/null
printf '%s\n' 'installer-test-key' | "$installed" providers add openrouter --key-stdin >/dev/null
"$installed" clients create "Installer test" >/dev/null
configuration=$XDG_CONFIG_HOME/provider-router-rust/router.json
cp "$configuration" "$work/configuration.before"

expect_version() {
    local version=$1
    for _ in {1..60}; do
        if curl --noproxy '*' --silent --fail --max-time 1 "http://127.0.0.1:$port/health" |
            jq -e --arg version "$version" '.version == $version' >/dev/null; then
            return
        fi
        sleep 0.1
    done
    cat "$work/router.log" >&2
    echo "The router did not serve requests as version $version." >&2
    return 1
}

"$installed" serve > "$work/router.log" 2>&1 &
original_pid=$!
expect_version "${old_version#provider-router }"
sh "$newer"
[[ $("$installed" --version) == "$new_version" ]]
expect_version "${new_version#provider-router }"
cmp "$work/configuration.before" "$configuration"

if sh "$older" > "$work/downgrade.log" 2>&1; then
    echo "The older installer unexpectedly succeeded." >&2
    exit 1
fi
[[ $("$installed" --version) == "$new_version" ]]
expect_version "${new_version#provider-router }"
cmp "$work/configuration.before" "$configuration"

"$installed" stop
sh "$newer"
"$installed" status | jq -e '.endpoint == null' >/dev/null
cmp "$work/configuration.before" "$configuration"
printf '%s\n' "Verified fresh install, running upgrade, downgrade rejection, and idle reinstall."
