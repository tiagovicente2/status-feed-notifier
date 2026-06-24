#!/usr/bin/env bash
set -euo pipefail

APP_ID="dev.tiago.StatusFeedNotifier"
BIN_NAME="status-feed-notifier"
BIN_DIR="${HOME}/.local/bin"
DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-${HOME}/.config}"

rm -f "${BIN_DIR}/${BIN_NAME}"
rm -f "${DATA_HOME}/applications/${APP_ID}.desktop"
rm -f "${DATA_HOME}/icons/hicolor/scalable/apps/${APP_ID}.svg"
rm -f "${CONFIG_HOME}/autostart/${APP_ID}.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "${DATA_HOME}/applications" >/dev/null 2>&1 || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q "${DATA_HOME}/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Uninstalled ${BIN_NAME}"
