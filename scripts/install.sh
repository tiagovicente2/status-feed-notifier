#!/usr/bin/env bash
set -euo pipefail

APP_ID="dev.tiago.StatusFeedNotifier"
BIN_NAME="status-feed-notifier"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${HOME}/.local/bin"
DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-${HOME}/.config}"
AUTOSTART=false

usage() {
  cat <<EOF
Usage: scripts/install.sh [--autostart]

Installs ${BIN_NAME} for the current Linux user.

Options:
  --autostart   Start the app automatically when you log in
  -h, --help    Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --autostart)
      AUTOSTART=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

cargo build --release --manifest-path "${ROOT_DIR}/Cargo.toml"

install -Dm755 "${ROOT_DIR}/target/release/${BIN_NAME}" "${BIN_DIR}/${BIN_NAME}"
install -Dm644 \
  "${ROOT_DIR}/packaging/${APP_ID}.svg" \
  "${DATA_HOME}/icons/hicolor/scalable/apps/${APP_ID}.svg"

desktop_file="$(mktemp)"
sed "s|@EXEC@|${BIN_DIR}/${BIN_NAME}|g" \
  "${ROOT_DIR}/packaging/${APP_ID}.desktop.in" > "${desktop_file}"
install -Dm644 "${desktop_file}" "${DATA_HOME}/applications/${APP_ID}.desktop"

if [[ "${AUTOSTART}" == true ]]; then
  install -Dm644 "${desktop_file}" "${CONFIG_HOME}/autostart/${APP_ID}.desktop"
fi

rm -f "${desktop_file}"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "${DATA_HOME}/applications" >/dev/null 2>&1 || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q "${DATA_HOME}/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Installed ${BIN_NAME} to ${BIN_DIR}/${BIN_NAME}"
echo "Installed launcher to ${DATA_HOME}/applications/${APP_ID}.desktop"
if [[ "${AUTOSTART}" == true ]]; then
  echo "Enabled login autostart at ${CONFIG_HOME}/autostart/${APP_ID}.desktop"
fi
