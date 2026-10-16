#!/bin/sh
# Download a prebuilt plugin and register it through the selected host.
set -eu
umask 077
VERSION=0.1.0
fail() { printf 'Traversiq: %s\n' "$*" >&2; exit 1; }

[ "$#" -eq 1 ] || fail 'Usage: sh install.sh codex|claude'
case "$1" in codex|claude) host=$1 ;; *) fail 'Choose codex or claude.' ;; esac
for tool in "$host" curl unzip mktemp; do
    command -v "$tool" >/dev/null 2>&1 || fail "Install $tool first."
done
if command -v sha256sum >/dev/null 2>&1; then
    checksum_tool=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    checksum_tool=shasum
else
    fail 'Install sha256sum or shasum first.'
fi
case "$(uname -s):$(uname -m)" in
    Darwin:arm64) target=aarch64-apple-darwin ;;
    Darwin:x86_64) target=x86_64-apple-darwin ;;
    Linux:x86_64) target=x86_64-unknown-linux-gnu ;;
    Linux:aarch64|Linux:arm64) target=aarch64-unknown-linux-gnu ;;
    *) fail 'Supported platforms: macOS and Linux on Apple Silicon/ARM64 or x86-64.' ;;
esac
name=traversiq-$VERSION-$target
base=${XDG_DATA_HOME:-"$HOME/.local/share"}/traversiq
mkdir -p "$base"
# Serialize installs so an existing version is never overwritten or partially replaced.
lock=$base/.installing
mkdir "$lock" 2>/dev/null || fail "Another install is running. If it crashed, remove $lock and retry."
staging=
cleanup() {
    if [ -n "$staging" ]; then rm -rf "$staging"; fi
    rmdir "$lock"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
staging=$(mktemp -d "$base/.download.XXXXXX")
url=https://github.com/OsnielT/Traversiq/releases/download/v$VERSION
printf 'Downloading Traversiq %s for %s...\n' "$VERSION" "$target"
for asset in "$name.zip" "$name.zip.sha256"; do
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time 300 "$url/$asset" -o "$staging/$asset"
done
expected=$(awk 'NR == 1 { print $1 }' "$staging/$name.zip.sha256")
[ "${#expected}" -eq 64 ] || fail 'Invalid release checksum.'
case "$expected" in *[!0-9a-f]*) fail 'Invalid release checksum.' ;; esac
if [ "$checksum_tool" = shasum ]; then
    actual=$(shasum -a 256 "$staging/$name.zip")
else
    actual=$(sha256sum "$staging/$name.zip")
fi
[ "${actual%% *}" = "$expected" ] || fail 'Checksum mismatch; nothing was installed.'
install_dir=$base/$name
if [ -e "$install_dir" ] || [ -L "$install_dir" ]; then
    [ ! -L "$install_dir" ] && [ -f "$install_dir/.archive-sha256" ] && \
        [ "$(cat "$install_dir/.archive-sha256")" = "$expected" ] && \
        [ -x "$install_dir/plugins/traversiq/bin/traversiq" ] || \
        fail "Existing installation differs; leave it intact and inspect $install_dir."
else
    unzip -q "$staging/$name.zip" -d "$staging/unpacked"
    unpacked=$staging/unpacked/$name
    [ -f "$unpacked/.agents/plugins/marketplace.json" ] && \
        [ -f "$unpacked/.claude-plugin/marketplace.json" ] && \
        [ -f "$unpacked/plugins/traversiq/bin/traversiq" ] || fail 'Incomplete plugin archive.'
    chmod 755 "$unpacked/plugins/traversiq/bin/traversiq"
    printf '%s\n' "$expected" > "$unpacked/.archive-sha256"
    mv "$unpacked" "$install_dir"
fi
printf 'Registering local marketplace at %s\n' "$install_dir"
# Keep this directory even if host registration fails: the host may already reference it.
"$host" plugin marketplace add "$install_dir"
if [ "$host" = codex ]; then
    codex plugin add traversiq@traversiq-local
else
    claude plugin install traversiq@traversiq-local
fi
printf 'Restart %s and ask Traversiq to set up your project.\n' "$host"
if [ "$host" = codex ]; then
    printf 'Review and trust the Traversiq hook through /hooks.\n'
else
    printf 'Use /traversiq:setup to choose your connected projects.\n'
fi
