use adw::prelude::*;
use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use feed_rs::model::Entry as ParsedFeedEntry;
use gtk::gio;
use gtk::glib;
use reqwest::blocking::Client;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "dev.tiago.StatusFeedNotifier";
const DEFAULT_FEED_URL: &str = "https://status.claude.com/history.atom";
const USER_AGENT: &str = "StatusFeedNotifier/0.1 (+https://status.claude.com/)";

#[derive(Debug, Clone)]
struct Feed {
    id: i64,
    url: String,
    title: Option<String>,
    last_checked: Option<i64>,
}

#[derive(Debug, Clone)]
struct StoredEntry {
    feed_title: String,
    title: String,
    url: Option<String>,
    updated: Option<String>,
    summary: String,
}

#[derive(Debug, Clone)]
struct NewEntry {
    key: String,
    feed_title: String,
    title: String,
    summary: String,
}

#[derive(Debug)]
struct PollOutcome {
    feeds: Vec<Feed>,
    entries: Vec<StoredEntry>,
    notifications: Vec<NewEntry>,
    errors: Vec<String>,
}

#[derive(Debug)]
enum PollMessage {
    Finished(Result<PollOutcome, String>),
}

struct AppWidgets {
    app: adw::Application,
    db_path: PathBuf,
    sender: Sender<PollMessage>,
    polling: Cell<bool>,
    last_poll: Cell<i64>,
    feed_entry: gtk::Entry,
    feed_list: gtk::ListBox,
    entry_list: gtk::ListBox,
    refresh_button: gtk::Button,
    status_label: gtk::Label,
    interval_spin: gtk::SpinButton,
    notifications_switch: gtk::Switch,
}

struct Store {
    conn: Connection,
}

#[derive(Debug)]
struct FetchedFeed {
    title: Option<String>,
    entries: Vec<FetchedEntry>,
}

#[derive(Debug)]
struct FetchedEntry {
    key: String,
    title: String,
    url: Option<String>,
    updated: Option<String>,
    summary: String,
}

fn main() -> glib::ExitCode {
    adw::init().expect("failed to initialize libadwaita");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let db_path = match app_db_path().and_then(|path| {
        let store = Store::open(&path)?;
        store.init()?;
        store.seed_default_feed()?;
        Ok(path)
    }) {
        Ok(path) => path,
        Err(err) => {
            show_startup_error(app, &err.to_string());
            return;
        }
    };

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Status Feed Notifier")
        .default_width(920)
        .default_height(640)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title = gtk::Label::builder()
        .label("Status Feed Notifier")
        .css_classes(["title"])
        .build();
    header.set_title_widget(Some(&title));

    let refresh_button = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Refresh feeds")
        .build();
    header.pack_end(&refresh_button);
    toolbar_view.add_top_bar(&header);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);
    toolbar_view.set_content(Some(&root));

    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_wide_handle(true);
    split.set_resize_start_child(false);
    split.set_shrink_start_child(false);
    root.append(&split);

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 12);
    sidebar.set_size_request(300, -1);
    split.set_start_child(Some(&sidebar));

    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let feed_entry = gtk::Entry::builder()
        .placeholder_text("RSS or Atom feed URL")
        .hexpand(true)
        .build();
    let add_button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add feed")
        .build();
    add_row.append(&feed_entry);
    add_row.append(&add_button);
    sidebar.append(&add_row);

    let feed_list = gtk::ListBox::new();
    feed_list.set_selection_mode(gtk::SelectionMode::None);
    feed_list.add_css_class("boxed-list");
    let feed_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&feed_list)
        .build();
    sidebar.append(&feed_scroll);

    let settings_group = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let interval_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let interval_label = gtk::Label::builder()
        .label("Refresh minutes")
        .xalign(0.0)
        .hexpand(true)
        .build();
    let interval_spin = gtk::SpinButton::with_range(1.0, 60.0, 1.0);
    interval_spin.set_value(5.0);
    interval_spin.set_width_chars(3);
    interval_row.append(&interval_label);
    interval_row.append(&interval_spin);

    let notifications_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let notifications_label = gtk::Label::builder()
        .label("Desktop notifications")
        .xalign(0.0)
        .hexpand(true)
        .build();
    let notifications_switch = gtk::Switch::builder().active(true).build();
    notifications_row.append(&notifications_label);
    notifications_row.append(&notifications_switch);
    settings_group.append(&interval_row);
    settings_group.append(&notifications_row);
    sidebar.append(&settings_group);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    split.set_end_child(Some(&content));

    let status_label = gtk::Label::builder()
        .label("Ready")
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    content.append(&status_label);

    let entry_list = gtk::ListBox::new();
    entry_list.set_selection_mode(gtk::SelectionMode::None);
    entry_list.add_css_class("boxed-list");
    let entry_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&entry_list)
        .build();
    content.append(&entry_scroll);

    let (sender, receiver) = mpsc::channel();
    let ui = Rc::new(AppWidgets {
        app: app.clone(),
        db_path,
        sender,
        polling: Cell::new(false),
        last_poll: Cell::new(0),
        feed_entry,
        feed_list,
        entry_list,
        refresh_button,
        status_label,
        interval_spin,
        notifications_switch,
    });

    install_actions(app, &window);
    connect_controls(&ui, &add_button);
    render_from_store(&ui);
    attach_poll_receiver(&ui, receiver);
    attach_auto_refresh(&ui);

    start_poll(&ui);
    window.set_content(Some(&toolbar_view));
    window.present();
}

fn show_startup_error(app: &adw::Application, message: &str) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Status Feed Notifier")
        .default_width(520)
        .default_height(180)
        .build();
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    box_.set_margin_top(24);
    box_.set_margin_bottom(24);
    box_.set_margin_start(24);
    box_.set_margin_end(24);
    box_.append(
        &gtk::Label::builder()
            .label("Unable to start Status Feed Notifier")
            .css_classes(["title-2"])
            .xalign(0.0)
            .build(),
    );
    box_.append(
        &gtk::Label::builder()
            .label(message)
            .wrap(true)
            .xalign(0.0)
            .build(),
    );
    window.set_content(Some(&box_));
    window.present();
}

fn install_actions(app: &adw::Application, window: &adw::ApplicationWindow) {
    let show_action = gio::SimpleAction::new("show-window", None);
    let window = window.clone();
    show_action.connect_activate(move |_, _| {
        window.present();
    });
    app.add_action(&show_action);
}

fn connect_controls(ui: &Rc<AppWidgets>, add_button: &gtk::Button) {
    let ui_for_refresh = Rc::clone(ui);
    ui.refresh_button.connect_clicked(move |_| {
        start_poll(&ui_for_refresh);
    });

    let ui_for_add = Rc::clone(ui);
    add_button.connect_clicked(move |_| {
        add_feed_from_entry(&ui_for_add);
    });

    let ui_for_entry = Rc::clone(ui);
    ui.feed_entry.connect_activate(move |_| {
        add_feed_from_entry(&ui_for_entry);
    });
}

fn attach_poll_receiver(ui: &Rc<AppWidgets>, receiver: std::sync::mpsc::Receiver<PollMessage>) {
    let receiver = Rc::new(receiver);
    let ui = Rc::clone(ui);
    glib::timeout_add_seconds_local(1, move || {
        while let Ok(message) = receiver.try_recv() {
            match message {
                PollMessage::Finished(result) => handle_poll_result(&ui, result),
            }
        }
        glib::ControlFlow::Continue
    });
}

fn attach_auto_refresh(ui: &Rc<AppWidgets>) {
    let ui = Rc::clone(ui);
    glib::timeout_add_seconds_local(30, move || {
        let interval_seconds = (ui.interval_spin.value_as_int().max(1) as i64) * 60;
        let elapsed = now_unix() - ui.last_poll.get();
        if elapsed >= interval_seconds {
            start_poll(&ui);
        }
        glib::ControlFlow::Continue
    });
}

fn add_feed_from_entry(ui: &Rc<AppWidgets>) {
    let url = ui.feed_entry.text().trim().to_string();
    if url.is_empty() {
        return;
    }

    if !(url.starts_with("https://") || url.starts_with("http://")) {
        ui.status_label
            .set_text("Feed URL must start with http:// or https://");
        return;
    }

    match Store::open(&ui.db_path).and_then(|store| store.add_feed(&url)) {
        Ok(()) => {
            ui.feed_entry.set_text("");
            render_from_store(ui);
            start_poll(ui);
        }
        Err(err) => {
            ui.status_label
                .set_text(&format!("Could not add feed: {err}"));
        }
    }
}

fn remove_feed(ui: &Rc<AppWidgets>, feed_id: i64) {
    match Store::open(&ui.db_path).and_then(|store| store.remove_feed(feed_id)) {
        Ok(()) => {
            render_from_store(ui);
            start_poll(ui);
        }
        Err(err) => {
            ui.status_label
                .set_text(&format!("Could not remove feed: {err}"));
        }
    }
}

fn start_poll(ui: &Rc<AppWidgets>) {
    if ui.polling.replace(true) {
        return;
    }

    ui.refresh_button.set_sensitive(false);
    ui.status_label.set_text("Checking feeds...");

    let db_path = ui.db_path.clone();
    let sender = ui.sender.clone();
    std::thread::spawn(move || {
        let result = poll_all(&db_path).map_err(|err| err.to_string());
        let _ = sender.send(PollMessage::Finished(result));
    });
}

fn handle_poll_result(ui: &Rc<AppWidgets>, result: Result<PollOutcome, String>) {
    ui.polling.set(false);
    ui.refresh_button.set_sensitive(true);
    ui.last_poll.set(now_unix());

    match result {
        Ok(outcome) => {
            render_feeds(ui, &outcome.feeds);
            render_entries(ui, &outcome.entries);
            if ui.notifications_switch.is_active() {
                send_notifications(ui, &outcome.notifications);
            }

            let feed_count = outcome.feeds.len();
            let new_count = outcome.notifications.len();
            if outcome.errors.is_empty() {
                ui.status_label.set_text(&format!(
                    "Checked {feed_count} feed(s), {new_count} new item(s)"
                ));
            } else {
                ui.status_label.set_text(&format!(
                    "Checked {feed_count} feed(s), {new_count} new item(s), {} error(s)",
                    outcome.errors.len()
                ));
            }
        }
        Err(err) => {
            ui.status_label.set_text(&format!("Refresh failed: {err}"));
        }
    }
}

fn send_notifications(ui: &Rc<AppWidgets>, entries: &[NewEntry]) {
    for entry in entries {
        let body = compact_spaces(&format!(
            "{}\n{}",
            entry.feed_title,
            first_sentence(&entry.summary)
        ));
        let notification = gio::Notification::new(&entry.title);
        notification.set_body(Some(&body));
        notification.set_default_action("app.show-window");
        ui.app.send_notification(Some(&entry.key), &notification);
    }
}

fn render_from_store(ui: &Rc<AppWidgets>) {
    match Store::open(&ui.db_path).and_then(|store| {
        let feeds = store.list_feeds()?;
        let entries = store.list_recent_entries(80)?;
        Ok((feeds, entries))
    }) {
        Ok((feeds, entries)) => {
            render_feeds(ui, &feeds);
            render_entries(ui, &entries);
        }
        Err(err) => {
            ui.status_label
                .set_text(&format!("Could not read local store: {err}"));
        }
    }
}

fn render_feeds(ui: &Rc<AppWidgets>, feeds: &[Feed]) {
    clear_list_box(&ui.feed_list);

    if feeds.is_empty() {
        ui.feed_list.append(&empty_row("No feeds yet"));
        return;
    }

    for feed in feeds {
        let row = gtk::ListBoxRow::new();
        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        box_.set_margin_top(10);
        box_.set_margin_bottom(10);
        box_.set_margin_start(10);
        box_.set_margin_end(10);

        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text_box.set_hexpand(true);

        let title = gtk::Label::builder()
            .label(feed.title.as_deref().unwrap_or("Untitled feed"))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        title.add_css_class("heading");
        let url = gtk::Label::builder()
            .label(&feed.url)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        url.add_css_class("dim-label");

        text_box.append(&title);
        text_box.append(&url);

        let remove_button = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Remove feed")
            .valign(gtk::Align::Center)
            .build();
        remove_button.add_css_class("flat");
        let ui_for_remove = Rc::clone(ui);
        let feed_id = feed.id;
        remove_button.connect_clicked(move |_| {
            remove_feed(&ui_for_remove, feed_id);
        });

        box_.append(&text_box);
        box_.append(&remove_button);
        row.set_child(Some(&box_));
        ui.feed_list.append(&row);
    }
}

fn render_entries(ui: &Rc<AppWidgets>, entries: &[StoredEntry]) {
    clear_list_box(&ui.entry_list);

    if entries.is_empty() {
        ui.entry_list
            .append(&empty_row("No feed entries have been stored yet"));
        return;
    }

    for entry in entries {
        let row = gtk::ListBoxRow::new();
        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        box_.set_margin_top(12);
        box_.set_margin_bottom(12);
        box_.set_margin_start(12);
        box_.set_margin_end(12);

        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text_box.set_hexpand(true);

        let title = gtk::Label::builder()
            .label(&entry.title)
            .xalign(0.0)
            .wrap(true)
            .build();
        title.add_css_class("heading");

        let meta_text = match &entry.updated {
            Some(updated) if !updated.is_empty() => format!("{} · {}", entry.feed_title, updated),
            _ => entry.feed_title.clone(),
        };
        let meta = gtk::Label::builder()
            .label(&meta_text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        meta.add_css_class("dim-label");

        let summary = gtk::Label::builder()
            .label(first_sentence(&entry.summary))
            .xalign(0.0)
            .wrap(true)
            .build();
        summary.add_css_class("dim-label");

        text_box.append(&title);
        text_box.append(&meta);
        if !entry.summary.is_empty() {
            text_box.append(&summary);
        }

        let open_button = gtk::Button::builder()
            .icon_name("adw-external-link-symbolic")
            .tooltip_text("Open entry")
            .valign(gtk::Align::Center)
            .sensitive(entry.url.is_some())
            .build();
        open_button.add_css_class("flat");
        if let Some(url) = entry.url.clone() {
            open_button.connect_clicked(move |_| {
                let _ = gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>);
            });
        }

        box_.append(&text_box);
        box_.append(&open_button);
        row.set_child(Some(&box_));
        ui.entry_list.append(&row);
    }
}

fn empty_row(label: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let label = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    label.add_css_class("dim-label");
    row.set_child(Some(&label));
    row
}

fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

impl Store {
    fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening SQLite store at {}", path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(Self { conn })
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS feeds (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                url TEXT NOT NULL UNIQUE,
                title TEXT,
                last_checked INTEGER
            );

            CREATE TABLE IF NOT EXISTS entries (
                key TEXT PRIMARY KEY,
                feed_id INTEGER NOT NULL,
                feed_url TEXT NOT NULL,
                feed_title TEXT NOT NULL,
                title TEXT NOT NULL,
                url TEXT,
                updated TEXT,
                summary TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                notified INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY(feed_id) REFERENCES feeds(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_entries_created_at
                ON entries(created_at DESC);
            ",
        )?;
        Ok(())
    }

    fn seed_default_feed(&self) -> Result<()> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM feeds", [], |row| row.get(0))?;
        if count == 0 {
            self.add_feed(DEFAULT_FEED_URL)?;
        }
        Ok(())
    }

    fn add_feed(&self, url: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO feeds (url, title, last_checked) VALUES (?1, NULL, NULL)",
            params![url],
        )?;
        Ok(())
    }

    fn remove_feed(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM feeds WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn list_feeds(&self) -> Result<Vec<Feed>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, url, title, last_checked FROM feeds ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(Feed {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                last_checked: row.get(3)?,
            })
        })?;

        let mut feeds = Vec::new();
        for row in rows {
            feeds.push(row?);
        }
        Ok(feeds)
    }

    fn list_recent_entries(&self, limit: usize) -> Result<Vec<StoredEntry>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT feed_title, title, url, updated, summary
            FROM entries
            ORDER BY created_at DESC
            LIMIT ?1
            ",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(StoredEntry {
                feed_title: row.get(0)?,
                title: row.get(1)?,
                url: row.get(2)?,
                updated: row.get(3)?,
                summary: row.get(4)?,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    fn update_feed_after_poll(&self, feed_id: i64, title: Option<&str>, now: i64) -> Result<()> {
        self.conn.execute(
            "
            UPDATE feeds
            SET title = COALESCE(?1, title),
                last_checked = ?2
            WHERE id = ?3
            ",
            params![title, now, feed_id],
        )?;
        Ok(())
    }

    fn insert_entry(
        &self,
        feed: &Feed,
        feed_title: &str,
        entry: &FetchedEntry,
        notify: bool,
        now: i64,
    ) -> Result<bool> {
        self.conn.execute(
            "
            INSERT OR IGNORE INTO entries
                (key, feed_id, feed_url, feed_title, title, url, updated, summary, created_at, notified)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                entry.key,
                feed.id,
                feed.url,
                feed_title,
                entry.title,
                entry.url,
                entry.updated,
                entry.summary,
                now,
                if notify { 1 } else { 0 },
            ],
        )?;
        Ok(self.conn.changes() > 0)
    }
}

fn poll_all(db_path: &Path) -> Result<PollOutcome> {
    let store = Store::open(db_path)?;
    store.init()?;
    let feeds = store.list_feeds()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(USER_AGENT)
        .build()?;

    let mut notifications = Vec::new();
    let mut errors = Vec::new();
    let now = now_unix();

    for feed in &feeds {
        match fetch_feed(&client, &feed.url) {
            Ok(fetched) => {
                let feed_title = fetched
                    .title
                    .as_deref()
                    .or(feed.title.as_deref())
                    .unwrap_or(&feed.url)
                    .to_string();
                let should_notify = feed.last_checked.is_some();

                for entry in &fetched.entries {
                    let inserted =
                        store.insert_entry(feed, &feed_title, entry, should_notify, now)?;
                    if inserted && should_notify {
                        notifications.push(NewEntry {
                            key: entry.key.clone(),
                            feed_title: feed_title.clone(),
                            title: entry.title.clone(),
                            summary: entry.summary.clone(),
                        });
                    }
                }

                store.update_feed_after_poll(feed.id, fetched.title.as_deref(), now)?;
            }
            Err(err) => errors.push(format!("{}: {err}", feed.url)),
        }
    }

    let feeds = store.list_feeds()?;
    let entries = store.list_recent_entries(80)?;
    Ok(PollOutcome {
        feeds,
        entries,
        notifications,
        errors,
    })
}

fn fetch_feed(client: &Client, url: &str) -> Result<FetchedFeed> {
    let bytes = client
        .get(url)
        .send()
        .with_context(|| format!("requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("fetching {url}"))?
        .bytes()
        .with_context(|| format!("reading response body for {url}"))?;

    let feed = feed_rs::parser::parse(bytes.as_ref())
        .with_context(|| format!("parsing feed response from {url}"))?;
    let title = feed.title.map(|text| clean_markup(&text.content));
    let entries = feed
        .entries
        .into_iter()
        .take(50)
        .map(|entry| fetched_entry(url, entry))
        .collect();

    Ok(FetchedFeed { title, entries })
}

fn fetched_entry(feed_url: &str, entry: ParsedFeedEntry) -> FetchedEntry {
    let title = entry
        .title
        .map(|text| clean_markup(&text.content))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Untitled update".to_string());

    let url = entry.links.first().map(|link| link.href.clone());
    let updated = entry
        .updated
        .or(entry.published)
        .map(|date| date.to_rfc3339());
    let raw_summary = entry
        .summary
        .map(|text| text.content)
        .or_else(|| entry.content.and_then(|content| content.body))
        .unwrap_or_default();
    let summary = clean_markup(&raw_summary);
    let stable_id = if entry.id.trim().is_empty() {
        format!(
            "{}|{}|{}|{}",
            feed_url,
            url.as_deref().unwrap_or_default(),
            updated.as_deref().unwrap_or_default(),
            title
        )
    } else {
        format!("{feed_url}|{}", entry.id)
    };

    FetchedEntry {
        key: sha256_hex(&stable_id),
        title,
        url,
        updated,
        summary,
    }
}

fn app_db_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "tiago", "status-feed-notifier")
        .ok_or_else(|| anyhow!("could not determine XDG data directory"))?;
    fs::create_dir_all(dirs.data_dir())
        .with_context(|| format!("creating data directory {}", dirs.data_dir().display()))?;
    Ok(dirs.data_dir().join("feeds.sqlite3"))
}

fn sha256_hex(input: &str) -> String {
    format!("{:x}", Sha256::digest(input.as_bytes()))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn first_sentence(text: &str) -> &str {
    const MAX_CHARS: usize = 220;
    let text = text.trim();
    if text.chars().count() <= MAX_CHARS {
        return text;
    }

    let mut end = 0;
    for (idx, ch) in text.char_indices() {
        if text[..idx].chars().count() >= MAX_CHARS {
            break;
        }
        end = idx + ch.len_utf8();
    }
    text[..end].trim_end()
}

fn clean_markup(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_tag = false;

    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                output.push(' ');
            }
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }

    decode_entities(&compact_spaces(&output))
}

fn compact_spaces(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_simple_markup_and_entities() {
        assert_eq!(
            clean_markup("<p>Claude &amp; API <strong>updates</strong></p>"),
            "Claude & API updates"
        );
    }

    #[test]
    fn falls_back_to_stable_hash() {
        let hash = sha256_hex("feed|entry");
        assert_eq!(hash.len(), 64);
    }
}
