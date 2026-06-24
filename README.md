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

Install it as a per-user desktop app:

```bash
scripts/install.sh
```

This installs:

```text
~/.local/bin/status-feed-notifier
~/.local/share/applications/dev.tiago.StatusFeedNotifier.desktop
~/.local/share/icons/hicolor/scalable/apps/dev.tiago.StatusFeedNotifier.svg
```

To also start it when you log in:

```bash
scripts/install.sh --autostart
```

Uninstall:

```bash
scripts/uninstall.sh
```

Runtime data is stored in:

```text
~/.local/share/status-feed-notifier/feeds.sqlite3
```

## Notes

- Notifications are sent only while the app is running.
- A newly added feed is seeded on first check without notifying for historical entries.
- New entries found on later checks trigger desktop notifications.
- The UI supports adding/removing feeds, manual refresh, refresh interval, and opening entry links in the default browser.
