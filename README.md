# Status Feed Notifier

A small Rust + GTK4/libadwaita desktop app for subscribing to RSS and Atom status feeds and receiving native desktop notifications.

The app starts with Claude Status as the default feed:

```text
https://status.claude.com/history.atom
```

## Run

```bash
cargo run
```

## Install on Linux

Install the latest GitHub release as a per-user desktop app:

```bash
curl -fsSL https://raw.githubusercontent.com/tiagovicente2/status-feed-notifier/main/scripts/install.sh | bash
```

This installs:

```text
~/.local/share/status-feed-notifier/app/
~/.local/bin/status-feed-notifier -> ~/.local/share/status-feed-notifier/app/status-feed-notifier
~/.local/share/applications/dev.tiago.StatusFeedNotifier.desktop
```

To also start it when you log in:

```bash
curl -fsSL https://raw.githubusercontent.com/tiagovicente2/status-feed-notifier/main/scripts/install.sh | bash -s -- --autostart
```

For local development, build and install a release archive from this checkout:

```bash
scripts/package-release.sh
scripts/install.sh --archive dist/status-feed-notifier-linux-x64.tar.gz
```

Uninstall the app files:

```bash
scripts/uninstall.sh
```

Uninstalling does not remove feed data.

Runtime data is stored in:

```text
~/.local/share/status-feed-notifier/feeds.sqlite3
```

## Notes

- Notifications are sent only while the app is running.
- A newly added feed is seeded on first check without notifying for historical entries.
- New entries found on later checks trigger desktop notifications.
- The UI supports adding/removing feeds, manual refresh, refresh interval, and opening entry links in the default browser.
