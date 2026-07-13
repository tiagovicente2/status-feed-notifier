use adw::prelude::*;
use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use feed_rs::model::Entry as ParsedFeedEntry;
use gtk::gio;
use gtk::glib;
use reqwest::blocking::Client;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "dev.tiago.StatusFeedNotifier";
const DISPLAY_NAME: &str = "Status Feed Notifier";
const DESKTOP_COMMENT: &str = "Subscribe to RSS and Atom status feeds";
const DEFAULT_FEED_URL: &str = "https://status.claude.com/history.atom";
const USER_AGENT: &str = "StatusFeedNotifier/0.1 (+https://status.claude.com/)";

thread_local! {
    static APP_STATE: RefCell<Option<Rc<AppWidgets>>> = const { RefCell::new(None) };
    static STARTUP_OPTIONS: Cell<StartupOptions> = const { Cell::new(StartupOptions { start_hidden: false }) };
}

#[derive(Debug, Clone, Copy)]
struct StartupOptions {
    start_hidden: bool,
}

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
    notifications: Vec<NewEntry>,
    errors: Vec<String>,
}

#[derive(Debug)]
enum PollMessage {
    Finished(Result<PollOutcome, String>),
}

#[derive(Debug)]
enum UiCommand {
    ShowWindow,
    Refresh,
    Quit,
}

struct AppWidgets {
    app: adw::Application,
    window: adw::ApplicationWindow,
    db_path: PathBuf,
    sender: Sender<PollMessage>,
    polling: Cell<bool>,
    last_poll: Cell<i64>,
    selected_feed_id: Cell<Option<i64>>,
    feed_entry: gtk::Entry,
    feed_list: gtk::ListBox,
    entry_list: gtk::ListBox,
    refresh_button: gtk::Button,
    status_label: gtk::Label,
    interval_minutes: Cell<i32>,
    interval_value_label: gtk::Label,
    notifications_check: gtk::CheckButton,
    _hold_guard: gio::ApplicationHoldGuard,
    _tray_handle: Option<ksni::blocking::Handle<StatusTray>>,
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

struct StatusTray {
    sender: Sender<UiCommand>,
    icon_name: String,
    icon_theme_path: String,
}

#[derive(Clone, Copy)]
enum SymbolicIcon {
    Add,
    ExternalLink,
    Minus,
    Quit,
    Refresh,
    Remove,
    Settings,
}

fn main() -> glib::ExitCode {
    if let StartupCommand::Exit(code) = parse_startup_args(std::env::args().skip(1)) {
        return code;
    }

    adw::init().expect("failed to initialize libadwaita");

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_command_line(|app, command_line| {
        match parse_startup_args(
            command_line
                .arguments()
                .into_iter()
                .skip(1)
                .map(|arg| arg.to_string_lossy().to_string()),
        ) {
            StartupCommand::Run(options) => {
                STARTUP_OPTIONS.with(|startup_options| startup_options.set(options));
                app.activate();
                glib::ExitCode::SUCCESS
            }
            StartupCommand::Exit(code) => code,
        }
    });
    app.connect_activate(|app| {
        let options = STARTUP_OPTIONS.with(Cell::get);
        build_ui(app, options);
    });
    app.run()
}

enum StartupCommand {
    Run(StartupOptions),
    Exit(glib::ExitCode),
}

fn parse_startup_args(args: impl IntoIterator<Item = String>) -> StartupCommand {
    let mut start_hidden = false;

    for arg in args {
        match arg.as_str() {
            "--background" => start_hidden = true,
            "--version" | "-V" => {
                println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return StartupCommand::Exit(glib::ExitCode::SUCCESS);
            }
            "--help" | "-h" => {
                println!(
                    "{}\n\nUsage: status-feed-notifier [--background] [--version]",
                    env!("CARGO_PKG_DESCRIPTION")
                );
                return StartupCommand::Exit(glib::ExitCode::SUCCESS);
            }
            other => {
                eprintln!("unknown option: {other}");
                return StartupCommand::Exit(glib::ExitCode::FAILURE);
            }
        }
    }

    StartupCommand::Run(StartupOptions { start_hidden })
}

fn build_ui(app: &adw::Application, options: StartupOptions) {
    if let Some(window) = app.windows().first() {
        if options.start_hidden {
            window.set_visible(false);
        } else {
            window.present();
        }
        return;
    }

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
        .default_width(1040)
        .default_height(680)
        .build();
    window.set_hide_on_close(true);

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title = gtk::Label::builder()
        .label("Status Feed Notifier")
        .css_classes(["title"])
        .build();
    header.set_title_widget(Some(&title));

    let refresh_button = icon_button(SymbolicIcon::Refresh, "Refresh feeds");
    let settings_button = icon_button(SymbolicIcon::Settings, "Settings");
    let quit_button = icon_button(SymbolicIcon::Quit, "Quit");
    header.pack_end(&quit_button);
    header.pack_end(&settings_button);
    header.pack_end(&refresh_button);
    toolbar_view.add_top_bar(&header);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_margin_top(24);
    root.set_margin_bottom(24);
    root.set_margin_start(24);
    root.set_margin_end(24);
    toolbar_view.set_content(Some(&root));

    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_wide_handle(true);
    split.set_vexpand(true);
    split.set_resize_start_child(false);
    split.set_shrink_start_child(false);
    root.append(&split);

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 16);
    sidebar.set_size_request(340, -1);
    sidebar.set_margin_end(12);
    split.set_start_child(Some(&sidebar));

    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let feed_entry = gtk::Entry::builder()
        .placeholder_text("RSS or Atom feed URL")
        .hexpand(true)
        .build();
    let add_button = icon_button(SymbolicIcon::Add, "Add feed");
    add_row.append(&feed_entry);
    add_row.append(&add_button);
    sidebar.append(&add_row);

    let feed_list = gtk::ListBox::new();
    feed_list.set_selection_mode(gtk::SelectionMode::Single);
    feed_list.add_css_class("boxed-list");
    let feed_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&feed_list)
        .build();
    sidebar.append(&feed_scroll);

    let settings_group = gtk::Box::new(gtk::Orientation::Vertical, 14);
    settings_group.set_margin_start(4);
    settings_group.set_margin_end(4);
    let interval_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let interval_label = gtk::Label::builder()
        .label("Refresh minutes")
        .xalign(0.0)
        .hexpand(true)
        .build();
    let interval_control = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    interval_control.add_css_class("linked");
    let interval_decrease_button = icon_button(SymbolicIcon::Minus, "Decrease refresh interval");
    let interval_value_label = gtk::Label::builder()
        .label("5")
        .width_chars(2)
        .xalign(0.5)
        .build();
    let interval_value_frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    interval_value_frame.set_size_request(44, 34);
    interval_value_frame.add_css_class("button");
    interval_value_frame.append(&interval_value_label);
    let interval_increase_button = icon_button(SymbolicIcon::Add, "Increase refresh interval");
    interval_control.append(&interval_decrease_button);
    interval_control.append(&interval_value_frame);
    interval_control.append(&interval_increase_button);
    interval_row.append(&interval_label);
    interval_row.append(&interval_control);

    let notifications_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let notifications_label = gtk::Label::builder()
        .label("Desktop notifications")
        .xalign(0.0)
        .hexpand(true)
        .build();
    let notifications_check = gtk::CheckButton::builder()
        .active(true)
        .valign(gtk::Align::Center)
        .tooltip_text("Desktop notifications")
        .build();
    notifications_row.append(&notifications_label);
    notifications_row.append(&notifications_check);
    settings_group.append(&interval_row);
    settings_group.append(&notifications_row);
    sidebar.append(&settings_group);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.set_margin_start(12);
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
    let (ui_sender, ui_receiver) = mpsc::channel();
    let tray_handle = start_tray(ui_sender);
    let ui = Rc::new(AppWidgets {
        app: app.clone(),
        window: window.clone(),
        db_path,
        sender,
        polling: Cell::new(false),
        last_poll: Cell::new(0),
        selected_feed_id: Cell::new(None),
        feed_entry,
        feed_list,
        entry_list,
        refresh_button,
        status_label,
        interval_minutes: Cell::new(5),
        interval_value_label,
        notifications_check,
        _hold_guard: app.hold(),
        _tray_handle: tray_handle,
    });

    install_actions(app, &window);
    connect_window_lifecycle(&window);
    connect_settings_button(&window, &settings_button);
    connect_quit_button(app, &quit_button);
    connect_interval_controls(&ui, &interval_decrease_button, &interval_increase_button);
    connect_controls(&ui, &add_button);
    render_from_store(&ui);
    attach_poll_receiver(&ui, receiver);
    attach_ui_command_receiver(&ui, ui_receiver);
    attach_auto_refresh(&ui);
    APP_STATE.with(|state| {
        *state.borrow_mut() = Some(Rc::clone(&ui));
    });

    start_poll(&ui);
    window.set_content(Some(&toolbar_view));
    if options.start_hidden {
        window.set_opacity(0.0);
        window.present();
        let window = window.clone();
        glib::idle_add_local_once(move || {
            window.set_visible(false);
            window.set_opacity(1.0);
        });
    } else {
        window.present();
    }
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

fn connect_window_lifecycle(window: &adw::ApplicationWindow) {
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });
}

fn connect_settings_button(window: &adw::ApplicationWindow, settings_button: &gtk::Button) {
    let window = window.clone();
    settings_button.connect_clicked(move |_| {
        show_settings_dialog(&window);
    });
}

fn show_settings_dialog(parent: &adw::ApplicationWindow) {
    let dialog = adw::PreferencesDialog::builder()
        .title("Settings")
        .search_enabled(false)
        .build();
    let page = adw::PreferencesPage::builder().title("Settings").build();
    let startup_group = adw::PreferencesGroup::builder().title("Startup").build();
    let autostart_row = adw::ActionRow::builder()
        .title("Start at login")
        .subtitle("Run quietly in the background after you sign in")
        .build();
    let autostart_check = gtk::CheckButton::builder()
        .active(autostart_enabled())
        .valign(gtk::Align::Center)
        .build();
    autostart_row.add_suffix(&autostart_check);
    autostart_row.set_activatable_widget(Some(&autostart_check));

    connect_autostart_check(&autostart_check, &dialog);
    startup_group.add(&autostart_row);
    page.add(&startup_group);
    dialog.add(&page);
    dialog.present(Some(parent));
}

fn connect_autostart_check(check: &gtk::CheckButton, dialog: &adw::PreferencesDialog) {
    let handling_change = Rc::new(Cell::new(false));
    let dialog = dialog.clone();
    check.connect_toggled(move |check| {
        if handling_change.get() {
            return;
        }

        let enabled = check.is_active();
        match set_autostart_enabled(enabled) {
            Ok(()) => {
                let message = if enabled {
                    "Start at login enabled"
                } else {
                    "Start at login disabled"
                };
                dialog.add_toast(adw::Toast::new(message));
            }
            Err(err) => {
                handling_change.set(true);
                check.set_active(!enabled);
                handling_change.set(false);
                dialog.add_toast(adw::Toast::new(&format!(
                    "Could not update login startup: {err}"
                )));
            }
        }
    });
}

fn icon_button(icon: SymbolicIcon, tooltip: &str) -> gtk::Button {
    let symbol = symbolic_icon(icon);
    let button = gtk::Button::builder().tooltip_text(tooltip).build();
    button.set_size_request(36, 34);
    button.set_child(Some(&symbol));
    button
}

fn symbolic_icon(icon: SymbolicIcon) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(16)
        .content_height(16)
        .width_request(16)
        .height_request(16)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(move |area, context, width, height| {
        draw_symbolic_icon(area, context, width, height, icon);
    });
    area
}

fn draw_symbolic_icon(
    area: &gtk::DrawingArea,
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    icon: SymbolicIcon,
) {
    let color = area.color();
    context.set_source_rgba(
        color.red() as f64,
        color.green() as f64,
        color.blue() as f64,
        color.alpha() as f64,
    );
    context.set_line_cap(gtk::cairo::LineCap::Round);
    context.set_line_join(gtk::cairo::LineJoin::Round);
    context.set_line_width(1.7);

    let size = width.min(height) as f64;
    let offset_x = (width as f64 - size) / 2.0;
    let offset_y = (height as f64 - size) / 2.0;
    let _ = context.save();
    context.translate(offset_x, offset_y);
    context.scale(size / 16.0, size / 16.0);

    match icon {
        SymbolicIcon::Add => draw_add_icon(context),
        SymbolicIcon::ExternalLink => draw_external_link_icon(context),
        SymbolicIcon::Minus => draw_minus_icon(context),
        SymbolicIcon::Quit => draw_quit_icon(context),
        SymbolicIcon::Refresh => draw_refresh_icon(context),
        SymbolicIcon::Remove => draw_remove_icon(context),
        SymbolicIcon::Settings => draw_settings_icon(context),
    }

    let _ = context.restore();
}

fn draw_add_icon(context: &gtk::cairo::Context) {
    context.move_to(8.0, 3.5);
    context.line_to(8.0, 12.5);
    context.move_to(3.5, 8.0);
    context.line_to(12.5, 8.0);
    let _ = context.stroke();
}

fn draw_minus_icon(context: &gtk::cairo::Context) {
    context.move_to(3.5, 8.0);
    context.line_to(12.5, 8.0);
    let _ = context.stroke();
}

fn draw_refresh_icon(context: &gtk::cairo::Context) {
    use std::f64::consts::PI;

    context.arc(8.0, 8.0, 5.2, PI * 0.25, PI * 1.85);
    let _ = context.stroke();
    context.move_to(12.6, 4.6);
    context.line_to(12.7, 1.8);
    context.line_to(14.6, 3.8);
    let _ = context.stroke();
}

fn draw_settings_icon(context: &gtk::cairo::Context) {
    use std::f64::consts::PI;

    for tooth in 0..8 {
        let angle = tooth as f64 * PI / 4.0;
        let next_angle = angle + PI / 8.0;
        let outer_x = 8.0 + angle.cos() * 6.0;
        let outer_y = 8.0 + angle.sin() * 6.0;
        let inner_x = 8.0 + next_angle.cos() * 4.4;
        let inner_y = 8.0 + next_angle.sin() * 4.4;

        if tooth == 0 {
            context.move_to(outer_x, outer_y);
        } else {
            context.line_to(outer_x, outer_y);
        }
        context.line_to(inner_x, inner_y);
    }
    context.close_path();
    let _ = context.stroke();

    context.arc(8.0, 8.0, 2.1, 0.0, PI * 2.0);
    let _ = context.stroke();
}

fn draw_quit_icon(context: &gtk::cairo::Context) {
    use std::f64::consts::PI;

    context.move_to(8.0, 2.4);
    context.line_to(8.0, 7.4);
    let _ = context.stroke();
    context.arc(8.0, 9.0, 5.0, PI * 0.78, PI * 2.22);
    let _ = context.stroke();
}

fn draw_remove_icon(context: &gtk::cairo::Context) {
    context.move_to(4.0, 5.2);
    context.line_to(12.0, 5.2);
    context.move_to(6.4, 3.2);
    context.line_to(9.6, 3.2);
    context.move_to(5.4, 6.4);
    context.line_to(5.9, 12.5);
    context.line_to(10.1, 12.5);
    context.line_to(10.6, 6.4);
    context.move_to(7.3, 7.4);
    context.line_to(7.3, 11.1);
    context.move_to(8.7, 7.4);
    context.line_to(8.7, 11.1);
    let _ = context.stroke();
}

fn draw_external_link_icon(context: &gtk::cairo::Context) {
    context.move_to(6.0, 4.3);
    context.line_to(4.2, 4.3);
    context.line_to(4.2, 11.8);
    context.line_to(11.7, 11.8);
    context.line_to(11.7, 10.0);
    context.move_to(8.0, 4.2);
    context.line_to(12.2, 4.2);
    context.line_to(12.2, 8.4);
    context.move_to(12.0, 4.4);
    context.line_to(7.2, 9.2);
    let _ = context.stroke();
}

fn connect_quit_button(app: &adw::Application, quit_button: &gtk::Button) {
    let app = app.clone();
    quit_button.connect_clicked(move |_| {
        app.quit();
    });
}

fn connect_interval_controls(
    ui: &Rc<AppWidgets>,
    decrease_button: &gtk::Button,
    increase_button: &gtk::Button,
) {
    let ui_for_decrease = Rc::clone(ui);
    decrease_button.connect_clicked(move |_| {
        set_interval_minutes(&ui_for_decrease, ui_for_decrease.interval_minutes.get() - 1);
    });

    let ui_for_increase = Rc::clone(ui);
    increase_button.connect_clicked(move |_| {
        set_interval_minutes(&ui_for_increase, ui_for_increase.interval_minutes.get() + 1);
    });
}

fn set_interval_minutes(ui: &Rc<AppWidgets>, value: i32) {
    let value = value.clamp(1, 60);
    ui.interval_minutes.set(value);
    ui.interval_value_label.set_text(&value.to_string());
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

    let ui_for_selection = Rc::clone(ui);
    ui.feed_list.connect_row_selected(move |_, row| {
        let Some(row) = row else {
            return;
        };
        let Ok(feed_id) = row.widget_name().parse::<i64>() else {
            return;
        };
        ui_for_selection.selected_feed_id.set(Some(feed_id));
        render_entries_for_selection(&ui_for_selection);
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

fn attach_ui_command_receiver(ui: &Rc<AppWidgets>, receiver: std::sync::mpsc::Receiver<UiCommand>) {
    let receiver = Rc::new(receiver);
    let ui = Rc::clone(ui);
    glib::timeout_add_seconds_local(1, move || {
        while let Ok(command) = receiver.try_recv() {
            match command {
                UiCommand::ShowWindow => ui.window.present(),
                UiCommand::Refresh => start_poll(&ui),
                UiCommand::Quit => ui.app.quit(),
            }
        }
        glib::ControlFlow::Continue
    });
}

fn attach_auto_refresh(ui: &Rc<AppWidgets>) {
    let ui = Rc::clone(ui);
    glib::timeout_add_seconds_local(30, move || {
        let interval_seconds = (ui.interval_minutes.get().max(1) as i64) * 60;
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
            if ui.selected_feed_id.get() == Some(feed_id) {
                ui.selected_feed_id.set(None);
            }
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
            render_entries_for_selection(ui);
            if ui.notifications_check.is_active() {
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

fn start_tray(sender: Sender<UiCommand>) -> Option<ksni::blocking::Handle<StatusTray>> {
    use ksni::blocking::TrayMethods;

    let tray = StatusTray {
        sender,
        icon_name: tray_icon_name(),
        icon_theme_path: tray_icon_theme_path(),
    };

    match tray.assume_sni_available(true).spawn() {
        Ok(handle) => Some(handle),
        Err(err) => {
            eprintln!("status tray unavailable: {err}");
            None
        }
    }
}

impl StatusTray {
    fn send(&self, command: UiCommand) {
        let _ = self.sender.send(command);
    }
}

impl ksni::Tray for StatusTray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "Status Feed Notifier".into()
    }

    fn icon_name(&self) -> String {
        self.icon_name.clone()
    }

    fn icon_theme_path(&self) -> String {
        self.icon_theme_path.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: self.icon_name(),
            title: self.title(),
            description: "Polling subscribed status feeds".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(UiCommand::ShowWindow);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.send(UiCommand::ShowWindow);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;

        vec![
            StandardItem {
                label: "Open".into(),
                activate: Box::new(|tray: &mut StatusTray| tray.send(UiCommand::ShowWindow)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Refresh now".into(),
                activate: Box::new(|tray: &mut StatusTray| tray.send(UiCommand::Refresh)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut StatusTray| tray.send(UiCommand::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn tray_icon_name() -> String {
    if !tray_icon_theme_path().is_empty() {
        APP_ID.into()
    } else {
        "network-transmit-receive".into()
    }
}

fn tray_icon_theme_path() -> String {
    for path in candidate_icon_dirs() {
        if path.join(format!("{APP_ID}.svg")).is_file() {
            return path.to_string_lossy().to_string();
        }
    }

    String::new()
}

fn candidate_icon_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        dirs.push(exe_dir.join("resources"));
    }

    if let Ok(current_dir) = std::env::current_dir() {
        dirs.push(current_dir.join("packaging"));
    }

    dirs
}

fn render_from_store(ui: &Rc<AppWidgets>) {
    match Store::open(&ui.db_path).and_then(|store| {
        let feeds = store.list_feeds()?;
        Ok(feeds)
    }) {
        Ok(feeds) => {
            render_feeds(ui, &feeds);
            render_entries_for_selection(ui);
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
        ui.selected_feed_id.set(None);
        ui.feed_list.append(&empty_row("No feeds yet"));
        render_entries(ui, &[]);
        return;
    }

    let selected_id = ui
        .selected_feed_id
        .get()
        .filter(|id| feeds.iter().any(|feed| feed.id == *id))
        .unwrap_or(feeds[0].id);
    ui.selected_feed_id.set(Some(selected_id));
    let mut selected_row: Option<gtk::ListBoxRow> = None;

    for feed in feeds {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&feed.id.to_string());
        row.set_activatable(true);
        row.set_selectable(true);

        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        box_.set_margin_top(12);
        box_.set_margin_bottom(12);
        box_.set_margin_start(12);
        box_.set_margin_end(12);

        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
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

        let remove_button = icon_button(SymbolicIcon::Remove, "Remove feed");
        remove_button.set_valign(gtk::Align::Center);
        remove_button.add_css_class("flat");
        let ui_for_remove = Rc::clone(ui);
        let feed_id = feed.id;
        remove_button.connect_clicked(move |_| {
            remove_feed(&ui_for_remove, feed_id);
        });

        box_.append(&text_box);
        box_.append(&remove_button);
        row.set_child(Some(&box_));
        if feed.id == selected_id {
            selected_row = Some(row.clone());
        }
        ui.feed_list.append(&row);
    }

    if let Some(row) = selected_row {
        ui.feed_list.select_row(Some(&row));
    }
}

fn render_entries_for_selection(ui: &Rc<AppWidgets>) {
    match Store::open(&ui.db_path)
        .and_then(|store| store.list_recent_entries_for_feed(ui.selected_feed_id.get(), 80))
    {
        Ok(entries) => render_entries(ui, &entries),
        Err(err) => {
            ui.status_label
                .set_text(&format!("Could not read feed entries: {err}"));
        }
    }
}

fn render_entries(ui: &Rc<AppWidgets>, entries: &[StoredEntry]) {
    clear_list_box(&ui.entry_list);

    if entries.is_empty() {
        let message = if ui.selected_feed_id.get().is_some() {
            "No entries have been stored for this feed yet"
        } else {
            "No feed entries have been stored yet"
        };
        ui.entry_list.append(&empty_row(message));
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

        let open_button = icon_button(SymbolicIcon::ExternalLink, "Open entry");
        open_button.set_valign(gtk::Align::Center);
        open_button.set_sensitive(entry.url.is_some());
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

    fn list_recent_entries_for_feed(
        &self,
        feed_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<StoredEntry>> {
        let mut stmt = match feed_id {
            Some(_) => self.conn.prepare(
                "
                SELECT feed_title, title, url, updated, summary
                FROM entries
                WHERE feed_id = ?1
                ORDER BY created_at DESC
                LIMIT ?2
                ",
            )?,
            None => self.conn.prepare(
                "
                SELECT feed_title, title, url, updated, summary
                FROM entries
                ORDER BY created_at DESC
                LIMIT ?1
                ",
            )?,
        };

        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(StoredEntry {
                feed_title: row.get(0)?,
                title: row.get(1)?,
                url: row.get(2)?,
                updated: row.get(3)?,
                summary: row.get(4)?,
            })
        };

        let rows = match feed_id {
            Some(feed_id) => stmt.query_map(params![feed_id, limit as i64], map_row)?,
            None => stmt.query_map(params![limit as i64], map_row)?,
        };

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
    Ok(PollOutcome {
        feeds,
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

fn autostart_enabled() -> bool {
    autostart_file_path().is_ok_and(|path| path.is_file())
}

fn set_autostart_enabled(enabled: bool) -> Result<()> {
    let path = autostart_file_path()?;
    if enabled {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("autostart path has no parent directory"))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("creating autostart directory {}", parent.display()))?;
        let exec_command = autostart_exec_command()?;
        let icon = app_icon_value();
        fs::write(&path, desktop_file_contents(&exec_command, &icon))
            .with_context(|| format!("writing autostart entry {}", path.display()))?;
    } else {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("removing autostart entry {}", path.display()));
            }
        }
    }

    Ok(())
}

fn autostart_file_path() -> Result<PathBuf> {
    Ok(xdg_config_home()?
        .join("autostart")
        .join(format!("{APP_ID}.desktop")))
}

fn xdg_config_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }

    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".config"))
}

fn autostart_exec_command() -> Result<String> {
    let exe = std::env::current_exe().context("determining current executable path")?;
    Ok(format!("{} --background", desktop_exec_arg(&exe)))
}

fn desktop_exec_arg(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\\' | '$' | '`'))
    {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`");
        format!("\"{escaped}\"")
    } else {
        value.into_owned()
    }
}

fn app_icon_value() -> String {
    candidate_icon_dirs()
        .into_iter()
        .map(|path| path.join(format!("{APP_ID}.svg")))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| APP_ID.into())
}

fn desktop_file_contents(exec_command: &str, icon: &str) -> String {
    format!(
        "\
[Desktop Entry]
Name={DISPLAY_NAME}
Comment={DESKTOP_COMMENT}
Exec={exec_command}
Icon={icon}
Terminal=false
Type=Application
Categories=Network;GTK;
StartupNotify=true
"
    )
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

    #[test]
    fn parses_background_startup_option() {
        match parse_startup_args(["--background".to_string()]) {
            StartupCommand::Run(options) => assert!(options.start_hidden),
            StartupCommand::Exit(_) => panic!("expected app to run"),
        }
    }

    #[test]
    fn rejects_unknown_startup_option() {
        match parse_startup_args(["--bad-option".to_string()]) {
            StartupCommand::Run(_) => panic!("expected argument parsing to fail"),
            StartupCommand::Exit(code) => assert_eq!(code, glib::ExitCode::FAILURE),
        }
    }

    #[test]
    fn quotes_desktop_exec_paths_with_spaces() {
        assert_eq!(
            desktop_exec_arg(Path::new("/tmp/Status Feed/status-feed-notifier")),
            "\"/tmp/Status Feed/status-feed-notifier\""
        );
    }

    #[test]
    fn writes_background_autostart_desktop_entry() {
        let contents =
            desktop_file_contents("/app/status-feed-notifier --background", "/app/icon.svg");
        assert!(contents.contains("Name=Status Feed Notifier\n"));
        assert!(contents.contains("Exec=/app/status-feed-notifier --background\n"));
        assert!(contents.contains("Icon=/app/icon.svg\n"));
    }

    #[test]
    fn lists_entries_for_selected_feed_only() {
        let store = Store {
            conn: Connection::open_in_memory().expect("open in-memory store"),
        };
        store.init().expect("initialize store");
        store.add_feed("https://example.test/a.atom").unwrap();
        store.add_feed("https://example.test/b.atom").unwrap();
        let feeds = store.list_feeds().unwrap();

        store
            .insert_entry(
                &feeds[0],
                "Feed A",
                &FetchedEntry {
                    key: "a-entry".into(),
                    title: "Entry A".into(),
                    url: None,
                    updated: None,
                    summary: "Summary A".into(),
                },
                false,
                1,
            )
            .unwrap();
        store
            .insert_entry(
                &feeds[1],
                "Feed B",
                &FetchedEntry {
                    key: "b-entry".into(),
                    title: "Entry B".into(),
                    url: None,
                    updated: None,
                    summary: "Summary B".into(),
                },
                false,
                2,
            )
            .unwrap();

        let all_entries = store.list_recent_entries_for_feed(None, 10).unwrap();
        assert_eq!(all_entries.len(), 2);

        let feed_a_entries = store
            .list_recent_entries_for_feed(Some(feeds[0].id), 10)
            .unwrap();
        assert_eq!(feed_a_entries.len(), 1);
        assert_eq!(feed_a_entries[0].title, "Entry A");
    }
}
