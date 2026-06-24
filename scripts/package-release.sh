#!/usr/bin/env bash
set -euo pipefail

APP_ID="dev.tiago.StatusFeedNotifier"
APP_NAME="status-feed-notifier"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux) platform="linux" ;;
  *) echo "unsupported OS: $os" >&2; exit 1 ;;
esac
case "$arch" in
  x86_64|amd64) arch="x64" ;;
  arm64|aarch64) arch="arm64" ;;
  *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac

artifact_dir="${APP_NAME}-${platform}-${arch}"
archive="${artifact_dir}.tar.gz"

cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"

rm -rf "$DIST_DIR/$artifact_dir"
mkdir -p "$DIST_DIR/$artifact_dir/resources"
install -m 755 "$ROOT_DIR/target/release/$APP_NAME" "$DIST_DIR/$artifact_dir/$APP_NAME"
install -m 644 "$ROOT_DIR/packaging/${APP_ID}.svg" "$DIST_DIR/$artifact_dir/resources/${APP_ID}.svg"

tar -czf "$DIST_DIR/$archive" -C "$DIST_DIR" "$artifact_dir"
(
  cd "$DIST_DIR"
  sha256sum *.tar.gz > SHA256SUMS
)

printf 'Built %s\n' "$DIST_DIR/$archive"
