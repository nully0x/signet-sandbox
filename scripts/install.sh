#!/bin/sh
# Install, upgrade, or remove the signet CLI from GitHub releases.
# Covers linux x86_64/arm64 and macOS x86_64/arm64; no windows build.
#
# Canonical install (the site serves this same script):
#   curl -fsSL https://nully0x.github.io/signet-sandbox/install.sh | sh
#
#   install.sh                     install or upgrade to the latest release
#   install.sh --version v0.1.2    install a specific release
#   install.sh uninstall           remove the binary (profile data is kept)
#
# Installs into $SIGNET_INSTALL_DIR (default ~/.local/bin); no sudo.
set -eu

REPO="nully0x/signet-sandbox"
API="https://api.github.com/repos/$REPO"
BIN_DIR="${SIGNET_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        die "need curl or wget to download the release"
    fi
}

target_triple() {
    case "$(uname -s)" in
        Linux) os=unknown-linux-gnu ;;
        Darwin) os=apple-darwin ;;
        *) die "unsupported OS: $(uname -s) (this installer covers linux and macOS)" ;;
    esac
    case "$(uname -m)" in
        x86_64|amd64) arch=x86_64 ;;
        aarch64|arm64) arch=aarch64 ;;
        *) die "unsupported architecture: $(uname -m)" ;;
    esac
    printf '%s-%s' "$arch" "$os"
}

sha_of() { # sha_of <file> — print the sha256 hex digest
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

latest_tag() {
    work=$(mktemp)
    # A failed fetch must not vanish silently inside a command
    # substitution: check it explicitly and die with a message.
    if ! fetch "$API/releases/latest" "$work"; then
        rm -f "$work"
        die "could not reach the GitHub API to find the latest release"
    fi
    tag=$(sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' "$work" | head -n 1)
    rm -f "$work"
    [ -n "$tag" ] || die "could not read the latest release tag from the GitHub API"
    printf '%s' "$tag"
}

install_binary() {
    tag="$1"
    [ -n "$tag" ] || die "no version given and no latest release found"
    triple=$(target_triple)
    asset="signet-$tag-$triple.tar.gz"

    current=$("$BIN_DIR/signet" --version 2>/dev/null | awk '{print $2}') || current=""
    if [ -n "$current" ] && [ "v$current" = "$tag" ]; then
        say "signet $current is already installed — nothing to do"
        return 0
    fi

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT INT TERM
    say "downloading $asset"
    fetch "$API/releases/download/$tag/$asset" "$work/$asset" \
        || die "download failed — is $tag published for $triple?"
    fetch "$API/releases/download/$tag/$asset.sha256" "$work/check.sha256" \
        || die "checksum file not found: $asset.sha256"
    want=$(cut -d' ' -f1 "$work/check.sha256" | tr -d ' \t')
    got=$(sha_of "$work/$asset")
    [ "$want" = "$got" ] || die "checksum mismatch for $asset (want $want, got $got)"

    tar -xzf "$work/$asset" -C "$work"
    mkdir -p "$BIN_DIR"
    mv "$work/signet" "$BIN_DIR/signet"
    chmod 755 "$BIN_DIR/signet"
    rm -rf "$work"
    trap - EXIT INT TERM

    say "installed $BIN_DIR/signet ($tag)"
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *)
            say "note: $BIN_DIR is not on your PATH — add it:"
            say "  export PATH=\"$BIN_DIR:\$PATH\""
            ;;
    esac
}

uninstall_binary() {
    if [ -f "$BIN_DIR/signet" ]; then
        rm -f "$BIN_DIR/signet"
        say "removed $BIN_DIR/signet"
    else
        die "$BIN_DIR/signet is not installed"
    fi
    say "profile data at ~/.signet is kept; delete it manually if you want a full cleanup"
}

version_arg=""
action="install"
while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || die "--version needs a value"
            version_arg="$2"
            shift 2
            ;;
        install) shift ;;
        uninstall)
            action="uninstall"
            shift
            ;;
        *) die "unknown argument: $1 (usage: install.sh [--version vX.Y.Z] [install|uninstall])" ;;
    esac
done

if [ "$action" = uninstall ]; then
    uninstall_binary
else
    tag="$version_arg"
    if [ -z "$tag" ]; then
        tag=$(latest_tag)
    fi
    install_binary "$tag"
fi
