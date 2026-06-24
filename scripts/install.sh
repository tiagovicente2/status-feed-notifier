#!/usr/bin/env bash
set -euo pipefail

APP_ID="dev.tiago.StatusFeedNotifier"
APP_NAME="status-feed-notifier"
DISPLAY_NAME="Status Feed Notifier"
REPO="${STATUS_FEED_NOTIFIER_REPO:-tiagovicente2/status-feed-notifier}"
INSTALL_DIR="${STATUS_FEED_NOTIFIER_INSTALL_DIR:-$HOME/.local/share/status-feed-notifier/app}"
BIN_DIR="${STATUS_FEED_NOTIFIER_BIN_DIR:-$HOME/.local/bin}"
DESKTOP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
LEGACY_ICON="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/${APP_ID}.svg"

requested_version="${VERSION:-}"
archive_path=""
autostart=false

log() { printf '[status-feed-notifier] %s\n' "$*"; }
fail() { printf '[status-feed-notifier] error: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Status Feed Notifier Installer

Usage: scripts/install.sh [options]

Options:
  -h, --help              Show this help message
  -v, --version <version> Install a specific release version, for example 0.1.0
  -a, --archive <path>    Install from a local release archive
      --autostart         Start the app automatically when you log in

Examples:
  curl -fsSL https://raw.githubusercontent.com/tiagovicente2/status-feed-notifier/main/scripts/install.sh | bash
  curl -fsSL https://raw.githubusercontent.com/tiagovicente2/status-feed-notifier/main/scripts/install.sh | bash -s -- --version 0.1.0
  scripts/package-release.sh && scripts/install.sh --archive dist/status-feed-notifier-linux-x64.tar.gz
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    -v|--version)
      [[ -n "${2:-}" ]] || fail "--version requires a value"
      requested_version="$2"
      shift 2
      ;;
    -a|--archive)
      [[ -n "${2:-}" ]] || fail "--archive requires a value"
      archive_path="$2"
      shift 2
      ;;
    --autostart)
      autostart=true
      shift
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

command -v tar >/dev/null 2>&1 || fail "tar is required"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os" in
  linux) platform="linux" ;;
  *) fail "unsupported OS: $os" ;;
esac
case "$arch" in
  x86_64|amd64) arch="x64" ;;
  arm64|aarch64) arch="arm64" ;;
  *) fail "unsupported architecture: $arch" ;;
esac

artifact="${APP_NAME}-${platform}-${arch}.tar.gz"
tmp_dir=""

if [[ -z "$archive_path" ]]; then
  command -v curl >/dev/null 2>&1 || fail "curl is required"

  requested_version="${requested_version#v}"
  if [[ -z "$requested_version" ]]; then
    url="https://github.com/${REPO}/releases/latest/download/${artifact}"
    checksum_url="https://github.com/${REPO}/releases/latest/download/SHA256SUMS"
  else
    url="https://github.com/${REPO}/releases/download/v${requested_version}/${artifact}"
    checksum_url="https://github.com/${REPO}/releases/download/v${requested_version}/SHA256SUMS"
  fi

  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT
  archive_path="$tmp_dir/$artifact"

  log "downloading ${url}"
  curl -fL "$url" -o "$archive_path"

  if curl -fsL "$checksum_url" -o "$tmp_dir/SHA256SUMS"; then
    expected_checksum="$(awk -v artifact="$artifact" '$2 == artifact { print $1 }' "$tmp_dir/SHA256SUMS")"
    if [[ -n "$expected_checksum" ]]; then
      if command -v sha256sum >/dev/null 2>&1; then
        actual_checksum="$(sha256sum "$archive_path" | awk '{ print $1 }')"
      else
        actual_checksum="$(shasum -a 256 "$archive_path" | awk '{ print $1 }')"
      fi
      [[ "$actual_checksum" == "$expected_checksum" ]] || fail "checksum verification failed for $artifact"
      log "verified checksum for $artifact"
    else
      log "checksum file did not include $artifact; skipping verification"
    fi
  else
    log "checksums unavailable; skipping verification"
  fi
fi

[[ -f "$archive_path" ]] || fail "archive not found: $archive_path"

rm -rf "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
tar -xzf "$archive_path" -C "$INSTALL_DIR" --strip-components=1

launcher="$INSTALL_DIR/$APP_NAME"
[[ -x "$launcher" ]] || fail "app executable not found under $INSTALL_DIR"

mkdir -p "$BIN_DIR"
ln -sfn "$launcher" "$BIN_DIR/$APP_NAME"

icon_path="$INSTALL_DIR/resources/${APP_ID}.svg"
if [[ ! -f "$icon_path" ]]; then
  icon_path="$APP_NAME"
fi

mkdir -p "$DESKTOP_DIR"
desktop_file="$DESKTOP_DIR/${APP_ID}.desktop"
cat > "$desktop_file" <<EOF
[Desktop Entry]
Name=${DISPLAY_NAME}
Comment=Subscribe to RSS and Atom status feeds
Exec=${launcher}
Icon=${icon_path}
Terminal=false
Type=Application
Categories=Network;GTK;
StartupNotify=true
EOF

if [[ "$autostart" == true ]]; then
  mkdir -p "$AUTOSTART_DIR"
  cp "$desktop_file" "$AUTOSTART_DIR/${APP_ID}.desktop"
fi

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
rm -f "$LEGACY_ICON"

log "installed files: $INSTALL_DIR"
log "installed launcher: $BIN_DIR/$APP_NAME"
log "installed desktop entry: $desktop_file"
if [[ "$autostart" == true ]]; then
  log "enabled login autostart: $AUTOSTART_DIR/${APP_ID}.desktop"
fi
log "done"
