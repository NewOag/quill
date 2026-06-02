//! Quill — a fast, native database query tool built on gpui.
//!
//! Architecture (three layers, GUI-agnostic at the bottom):
//! - `datasource` — drivers + plain data types (MySQL real; PG/Redis reserved).
//! - `app`        — the `Db` async bridge and the `Workspace` root view.
//! - `ui`         — reusable gpui components (sidebar, editor, results table).
//!
//! Connections are managed in-app and persisted to
//! `dirs::config_dir()/quill/connections.json`. As a convenience, setting
//! `QUILL_MYSQL_URL` (e.g. `mysql://root:pass@127.0.0.1:3306/test`) auto-opens
//! that connection on startup without saving it.
//!
//! Threading: a multi-thread tokio runtime is created in `main` and lives for
//! the whole process (it outlives gpui's `run`, which blocks until exit). Its
//! `Handle` is moved into the `Db`; gpui keeps its own main-thread executor.
//! See `app::workspace` for how the two async worlds are bridged per query.

mod app;
mod datasource;
mod ui;

use gpui::{
    px, size, App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions,
};

use app::config::{ConnectionConfig, ConnectionStore};
use app::workspace::Workspace;
use datasource::DbKind;
use ui::text_input::{
    Backspace, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectLeft,
    SelectRight, Submit,
};

/// Bind the text-input actions, scoped to the `"QuillInput"` key context so
/// they only fire while the SQL input is focused.
fn bind_input_keys(cx: &mut App) {
    let ctx = Some("QuillInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("enter", Submit, ctx),
    ]);
}

/// Parse `QUILL_MYSQL_URL` into an ephemeral connection config, if set and
/// valid. Used as a startup convenience; not persisted.
fn ephemeral_from_env() -> Option<ConnectionConfig> {
    let url = std::env::var("QUILL_MYSQL_URL").ok()?;
    let opts = mysql_async::Opts::from_url(&url)
        .map_err(|e| eprintln!("quill: ignoring bad QUILL_MYSQL_URL: {e}"))
        .ok()?;
    Some(ConnectionConfig::new(
        "env".into(),
        DbKind::Mysql,
        opts.ip_or_hostname().to_string(),
        opts.tcp_port(),
        opts.user().unwrap_or("").to_string(),
        opts.pass().unwrap_or("").to_string(),
        opts.db_name().unwrap_or("").to_string(),
    ))
}

fn main() {
    // 1. Tokio runtime — built before gpui, owned for the whole process.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    let handle = runtime.handle().clone();

    // 2. Load persisted connections (empty store on first run / parse error).
    let store = ConnectionStore::load();
    let ephemeral = ephemeral_from_env();

    // 3. Start gpui. `run` blocks until the app exits, so `runtime` stays alive.
    Application::new().run(move |cx: &mut App| {
        bind_input_keys(cx);
        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        let handle = handle.clone();
        let store = store.clone();
        let ephemeral = ephemeral.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(move |cx| {
                    let mut ws = Workspace::new(handle, store, window, cx);
                    if let Some(config) = ephemeral {
                        ws.open_ephemeral(config, window, cx);
                    }
                    ws
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });

    // Keep the runtime owned until here so it isn't dropped while gpui runs.
    drop(runtime);
}
