#!/bin/sh
# opencode-rs installer. Detects your OS and CPU, downloads the matching release
# binary, verifies its checksum, and installs it.
#
#   curl -fsSL https://raw.githubusercontent.com/exgg1453/opencode-rs/main/install.sh | sh
#
# Options (environment variables):
#   OCRS_INSTALL_DIR   where to install (default: /usr/local/bin if writable, else ~/.local/bin)
#   OCRS_VERSION       a release tag such as v0.1.2 (default: latest)
set -eu

REPO="exgg1453/opencode-rs"
BIN="opencode"

say()  { printf '%s\n' "$*"; }
err()  { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# --- pick the asset for this machine --------------------------------------
os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)
    case "$arch" in
      x86_64|amd64)        asset="opencode-linux-64bit" ;;
      aarch64|arm64)       asset="opencode-linux-arm64" ;;
      *) err "unsupported Linux architecture: $arch" ;;
    esac ;;
  Darwin)
    case "$arch" in
      arm64)               asset="opencode-macos-apple-silicon" ;;
      x86_64)              asset="opencode-macos-intel" ;;
      *) err "unsupported macOS architecture: $arch" ;;
    esac ;;
  *)
    err "unsupported OS: $os. On Windows, download the .zip from https://github.com/$REPO/releases/latest" ;;
esac

# --- download tool --------------------------------------------------------
if have curl; then
  dl() { curl -fsSL "$1" -o "$2"; }
elif have wget; then
  dl() { wget -qO "$2" "$1"; }
else
  err "need curl or wget to download"
fi

ver="${OCRS_VERSION:-latest}"
if [ "$ver" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$ver"
fi

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t ocrs)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading $asset ($os/$arch, $ver)..."
dl "$base/$asset.tar.gz" "$tmp/$asset.tar.gz" || err "download failed; is $ver a published release?"

# --- verify checksum ------------------------------------------------------
if dl "$base/$asset.tar.gz.sha256" "$tmp/$asset.tar.gz.sha256" 2>/dev/null; then
  want=$(awk '{print $1}' "$tmp/$asset.tar.gz.sha256")
  if have sha256sum; then
    got=$(sha256sum "$tmp/$asset.tar.gz" | awk '{print $1}')
  elif have shasum; then
    got=$(shasum -a 256 "$tmp/$asset.tar.gz" | awk '{print $1}')
  else
    got=""
    say "warning: no sha256 tool found, skipping checksum verification"
  fi
  if [ -n "$got" ] && [ "$got" != "$want" ]; then
    err "checksum mismatch — refusing to install (expected $want, got $got)"
  fi
  [ -n "$got" ] && say "Checksum verified."
else
  say "warning: no checksum file for this release, skipping verification"
fi

# --- unpack ---------------------------------------------------------------
tar -xzf "$tmp/$asset.tar.gz" -C "$tmp" || err "failed to extract archive"
src="$tmp/$asset/$BIN"
[ -f "$src" ] || src=$(find "$tmp" -type f -name "$BIN" | head -n1)
[ -n "$src" ] && [ -f "$src" ] || err "binary not found in archive"
chmod +x "$src"

# --- choose an install directory -----------------------------------------
if [ -n "${OCRS_INSTALL_DIR:-}" ]; then
  dir="$OCRS_INSTALL_DIR"
elif [ -w /usr/local/bin ] 2>/dev/null; then
  dir="/usr/local/bin"
elif [ -d /usr/local/bin ] && have sudo; then
  dir="/usr/local/bin"; use_sudo=1
else
  dir="$HOME/.local/bin"
fi
mkdir -p "$dir" 2>/dev/null || { [ "${use_sudo:-0}" = 1 ] && sudo mkdir -p "$dir"; }

if [ "${use_sudo:-0}" = 1 ]; then
  say "Installing to $dir (needs sudo)..."
  sudo install -m 0755 "$src" "$dir/$BIN"
else
  install -m 0755 "$src" "$dir/$BIN" 2>/dev/null || cp "$src" "$dir/$BIN"
fi

say ""
say "Installed $BIN to $dir/$BIN"
"$dir/$BIN" --version 2>/dev/null || true

# --- PATH hint ------------------------------------------------------------
case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    say ""
    say "note: $dir is not on your PATH. Add this to your shell profile:"
    say "    export PATH=\"$dir:\$PATH\"" ;;
esac

say ""
say "Next: opencode auth login anthropic   # then run: opencode"
