#!/usr/bin/env bash
set -euo pipefail

APP_ID="dev.tiago.StatusFeedNotifier"
APP_NAME="status-feed-notifier"
INSTALL_DIR="${STATUS_FEED_NOTIFIER_INSTALL_DIR:-$HOME/.local/share/status-feed-notifier/app}"
BIN_DIR="${STATUS_FEED_NOTIFIER_BIN_DIR:-$HOME/.local/bin}"
DESKTOP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
LEGACY_ICON="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/${APP_ID}.svg"

rm -rf "$INSTALL_DIR"
rm -f "$BIN_DIR/$APP_NAME"
rm -f "$DESKTOP_DIR/${APP_ID}.desktop"
rm -f "$AUTOSTART_DIR/${APP_ID}.desktop"
rm -f "$LEGACY_ICON"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true

printf '[status-feed-notifier] uninstalled\n'
