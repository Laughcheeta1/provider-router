#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 || ! -f $1 || ! -x $1 || $2 != *.pkg ]]; then
    echo "Usage: $0 <provider-router-binary> <installer.pkg>" >&2
    exit 2
fi

binary=$1
output=$2
scripts=$(cd -- "$(dirname -- "$0")" && pwd)
directory=$(dirname -- "$output")
version_line=$("$binary" --version)
version=${version_line#provider-router }
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Expected a Provider Router binary with a three-part release version." >&2
    exit 1
fi
architecture=$(lipo -archs "$binary")
case "$architecture" in
    x86_64|arm64) ;;
    *)
        echo "Expected a single-architecture macOS binary." >&2
        exit 1
        ;;
esac
if [[ -n ${MACOS_NOTARY_PROFILE:-} &&
    ( -z ${MACOS_APPLICATION_SIGNING_IDENTITY:-} || -z ${MACOS_INSTALLER_SIGNING_IDENTITY:-} ) ]]; then
    echo "Notarization requires both macOS signing identities." >&2
    exit 1
fi

mkdir -p "$directory"
stage=$(mktemp -d "$directory/.provider-router-installer.XXXXXXXX")
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/scripts"
install -m 0755 "$binary" "$stage/scripts/provider-router"
install -m 0755 "$scripts/macos-postinstall.sh" "$stage/scripts/postinstall"

if [[ -n ${MACOS_APPLICATION_SIGNING_IDENTITY:-} ]]; then
    codesign --force --timestamp --options runtime \
        --sign "$MACOS_APPLICATION_SIGNING_IDENTITY" "$stage/scripts/provider-router"
    codesign --verify --strict "$stage/scripts/provider-router"
fi

identifier=org.providerrouter.installer
pkgbuild --identifier "$identifier" --version "$version" \
    --nopayload --scripts "$stage/scripts" "$stage/component.pkg"

cat > "$stage/Distribution.xml" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<installer-gui-script minSpecVersion="2">
    <title>Provider Router</title>
    <domains enable_anywhere="false" enable_currentUserHome="true" enable_localSystem="false"/>
    <options customize="never" hostArchitectures="$architecture"/>
    <choices-outline><line choice="provider-router"/></choices-outline>
    <choice id="provider-router" title="Provider Router">
        <pkg-ref id="$identifier"/>
    </choice>
    <pkg-ref id="$identifier" version="$version">component.pkg</pkg-ref>
</installer-gui-script>
EOF

set -- --distribution "$stage/Distribution.xml" --package-path "$stage"
if [[ -n ${MACOS_INSTALLER_SIGNING_IDENTITY:-} ]]; then
    set -- "$@" --sign "$MACOS_INSTALLER_SIGNING_IDENTITY"
fi
productbuild "$@" "$stage/provider-router.pkg"

if [[ -n ${MACOS_NOTARY_PROFILE:-} ]]; then
    xcrun notarytool submit "$stage/provider-router.pkg" --keychain-profile "$MACOS_NOTARY_PROFILE" --wait
    xcrun stapler staple "$stage/provider-router.pkg"
fi

mv -f "$stage/provider-router.pkg" "$output"
