#!/bin/sh
set -eu

uid=$(/usr/bin/id -u)
if [ "$uid" -eq 0 ]; then
    echo "Install Provider Router for your current user account." >&2
    exit 1
fi

# Installer can supply a different environment from the user's login shell.
HOME=$(/usr/bin/dscacheutil -q user -a uid "$uid" | /usr/bin/sed -n 's/^dir: //p')
case "$HOME" in
    /*) ;;
    *)
        echo "Could not find the current user's home directory." >&2
        exit 1
        ;;
esac
export HOME

scripts=$(CDPATH= cd "$(/usr/bin/dirname "$0")" && pwd)
applications=$HOME/Applications
destination="$applications/Provider Router.command"
/bin/mkdir -p "$applications"
if [ -d "$destination" ]; then
    echo "The Provider Router launcher path is already a directory." >&2
    exit 1
fi
launcher=$(/usr/bin/mktemp "$applications/.provider-router-launcher.XXXXXXXX")
trap '/bin/rm -f "$launcher"' 0
trap 'exit 1' HUP INT TERM

cat > "$launcher" <<'EOF'
#!/bin/sh
exec "$HOME/.local/bin/provider-router" "$@"
EOF
/bin/chmod 0755 "$launcher"

"$scripts/provider-router" __install --destination "$HOME/.local/bin/provider-router"
/bin/mv -f "$launcher" "$destination"
