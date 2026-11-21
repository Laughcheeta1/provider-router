#!/bin/sh
set -eu

install_router() {
    if [ "$(uname -s)" != Linux ] || [ "$(id -u)" -eq 0 ]; then
        echo "Run this installer on Linux from your regular user account." >&2
        exit 1
    fi
    case ${HOME:-} in
        /*) ;;
        *)
            echo "The installer needs an absolute HOME directory." >&2
            exit 1
            ;;
    esac

    umask 077
    bin_dir=$HOME/.local/bin
    mkdir -p "$bin_dir"
    stage=$(mktemp -d "$bin_dir/.provider-router-installer.XXXXXXXX")
    trap 'rm -rf "$stage"' 0
    trap 'exit 1' HUP INT TERM

    base64 -d > "$stage/provider-router.gz"
    gzip -dc "$stage/provider-router.gz" > "$stage/provider-router"
    chmod 0700 "$stage/provider-router"
    "$stage/provider-router" __install --destination "$bin_dir/provider-router"

    printf 'To open Provider Router, run "%s/provider-router".\n' "$bin_dir"
}
