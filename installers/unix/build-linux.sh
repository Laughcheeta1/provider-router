#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 || ! -f $1 || ! -x $1 || $2 != *.sh ]]; then
    echo "Usage: $0 <provider-router-binary> <installer.sh>" >&2
    exit 2
fi

binary=$1
output=$2
scripts=$(cd -- "$(dirname -- "$0")" && pwd)
directory=$(dirname -- "$output")
mkdir -p -- "$directory"
staged=$(mktemp "$directory/.provider-router-installer.XXXXXXXX")
trap 'rm -f -- "$staged"' EXIT

{
    cat "$scripts/linux-installer.sh"
    printf "\ninstall_router <<'__PROVIDER_ROUTER_PAYLOAD__'\n"
    gzip -n -c -- "$binary" | base64
    printf '%s\n' '__PROVIDER_ROUTER_PAYLOAD__'
} > "$staged"

chmod 0755 "$staged"
mv -f -- "$staged" "$output"
