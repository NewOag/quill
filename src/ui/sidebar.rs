//! Left sidebar: the database → table tree.
//!
//! Shows the connection label, a list of databases, and (for the expanded
//! database) its tables. Clicking a database expands/collapses it; clicking a
//! table emits [`SidebarEvent::TableSelected`], which the `Workspace`
//! subscribes to and turns into a query.
//!
//! Defined in full here (struct + events + setters + render) before the
//! `Workspace` wires it up.

use gpui::{
    div, prelude::*, px, rgb, Context, EventEmitter, SharedString, Window,
};

use crate::datasource::TableInfo;
use crate::ui::theme;

/// Emitted when the user interacts with the tree. The Workspace handles these.
#[derive(Debug, Clone)]
pub enum SidebarEvent {
    /// User clicked a database name (request to load/expand its tables).
    DatabaseSelected(String),
    /// User clicked a table (request to query it).
    TableSelected { database: String, table: String },
    /// User clicked a Redis key (request to fetch its value).
    KeySelected(String),
    /// User clicked the edit (✎) button on the connection header.
    EditConnection,
    /// User clicked the delete (✕) button on the connection header.
    DeleteConnection,
}

/// The sidebar view.
pub struct Sidebar {
    connection_label: SharedString,
    databases: Vec<String>,
    /// The currently expanded database and its tables, if loaded.
    expanded: Option<String>,
    tables: Vec<TableInfo>,
    /// Redis key list (shown instead of databases when `redis_mode` is true).
    redis_keys: Vec<crate::datasource::redis::RedisKey>,
    redis_mode: bool,
    /// Filter text for the Redis key list.
    redis_filter: String,
    /// Currently highlighted Redis key (for visual selection).
    selected_key: Option<String>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    pub fn new(connection_label: impl Into<SharedString>) -> Self {
        Self {
            connection_label: connection_label.into(),
            databases: Vec::new(),
            expanded: None,
            tables: Vec::new(),
            redis_keys: Vec::new(),
            redis_mode: false,
            redis_filter: String::new(),
            selected_key: None,
        }
    }

    /// Replace the database list (called after `list_databases` resolves).
    pub fn set_databases(&mut self, databases: Vec<String>) {
        self.databases = databases;
    }

    /// Switch to Redis key-browser mode, showing the given keys.
    pub fn set_redis_keys(&mut self, keys: Vec<crate::datasource::redis::RedisKey>) {
        self.redis_mode = true;
        self.redis_keys = keys;
    }

    /// Mark a database expanded and show its tables (after `list_tables`).
    pub fn set_tables(&mut self, database: String, tables: Vec<TableInfo>) {
        self.expanded = Some(database);
        self.tables = tables;
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("conn-header")
            .h(px(theme::HEADER_HEIGHT))
            .px(px(theme::PAD_LG))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_SM))
            .bg(rgb(theme::SURFACE))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            // Connected status dot (green).
            .child(
                div()
                    .text_color(rgb(theme::SUCCESS))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .child(theme::ICON_DOT),
            )
            .child(
                div()
                    .flex_grow()
                    .min_w_0()
                    .overflow_hidden()
                    .truncate()
                    .text_color(rgb(theme::TEXT))
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .child(self.connection_label.clone()),
            )
            // Edit connection button.
            .child(
                div()
                    .id("conn-edit")
                    .px(px(theme::PAD_XS))
                    .rounded(px(theme::RADIUS_SM))
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::ACCENT)))
                    .on_click(cx.listener(|_this, _ev, _window, cx| {
                        cx.emit(SidebarEvent::EditConnection);
                    }))
                    .child("✎"),
            )
            // Delete connection button.
            .child(
                div()
                    .id("conn-delete")
                    .px(px(theme::PAD_XS))
                    .rounded(px(theme::RADIUS_SM))
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::DANGER)))
                    .on_click(cx.listener(|_this, _ev, _window, cx| {
                        cx.emit(SidebarEvent::DeleteConnection);
                    }))
                    .child(theme::ICON_CLOSE),
            )
    }

    /// A leading fixed-width icon cell so labels line up regardless of glyph.
    fn icon_cell(glyph: &'static str, color: u32) -> impl IntoElement {
        div()
            .w(px(16.))
            .flex()
            .justify_center()
            .items_center()
            .text_color(rgb(color))
            .text_size(px(theme::TEXT_SIZE_XS))
            .child(glyph)
    }

    /// One clickable database row.
    fn db_row(&self, name: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let is_expanded = self.expanded.as_deref() == Some(name);
        let chevron = if is_expanded {
            theme::ICON_CHEVRON_OPEN
        } else {
            theme::ICON_CHEVRON
        };
        let name_owned = name.to_string();
        div()
            .id(SharedString::from(format!("db-{name}")))
            .h(px(theme::ROW_HEIGHT))
            .px(px(theme::PAD_SM))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_XS))
            .text_color(rgb(theme::TEXT))
            .text_size(px(theme::TEXT_SIZE))
            .hover(|s| s.bg(rgb(theme::HOVER)))
            .on_click(cx.listener(move |_this, _ev, _window, cx| {
                cx.emit(SidebarEvent::DatabaseSelected(name_owned.clone()));
            }))
            .child(Self::icon_cell(chevron, theme::TEXT_DIM))
            .child(SharedString::from(name.to_string()))
    }

    /// One clickable table row, indented under its database.
    fn table_row(&self, database: &str, t: &TableInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let is_view = t.kind == "view";
        let icon = if is_view { theme::ICON_VIEW } else { theme::ICON_TABLE };
        let icon_color = if is_view { theme::ACCENT_DIM } else { theme::TEXT_DIM };
        let database = database.to_string();
        let table = t.name.clone();
        div()
            .id(SharedString::from(format!("tbl-{}", t.name)))
            .h(px(theme::ROW_HEIGHT))
            .pl(px(theme::PAD_XL))
            .pr(px(theme::PAD_SM))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_XS))
            // 2px left bar that lights up on hover (DataGrip-style affordance).
            .border_l_2()
            .border_color(rgb(theme::BG_PANEL))
            .text_color(rgb(theme::TEXT_DIM))
            .text_size(px(theme::TEXT_SIZE))
            .hover(|s| {
                s.bg(rgb(theme::HOVER))
                    .text_color(rgb(theme::TEXT))
                    .border_color(rgb(theme::ACCENT))
            })
            .on_click(cx.listener(move |_this, _ev, _window, cx| {
                cx.emit(SidebarEvent::TableSelected {
                    database: database.clone(),
                    table: table.clone(),
                });
            }))
            .child(Self::icon_cell(icon, icon_color))
            .child(SharedString::from(t.name.clone()))
    }

    /// One clickable Redis key row.
    fn key_row(&self, key: &crate::datasource::redis::RedisKey, cx: &mut Context<Self>) -> impl IntoElement {
        let name = key.name.clone();
        let is_selected = self.selected_key.as_deref() == Some(&key.name);
        let type_icon = match key.type_name.as_str() {
            "string" => "S", "list" => "L", "hash" => "H",
            "set" => "E", "zset" => "Z", "stream" => "R", _ => "?",
        };
        let type_color = match key.type_name.as_str() {
            "string" => theme::SYN_STRING, "list" => theme::SYN_NUMBER,
            "hash" => theme::SYN_KEYWORD, "set" => theme::ACCENT,
            "zset" => theme::SYN_COMMENT, _ => theme::TEXT_DIM,
        };
        let row = div()
            .id(SharedString::from(format!("rkey-{}", name)))
            .h(px(theme::ROW_HEIGHT))
            .px(px(theme::PAD_SM))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::PAD_XS))
            .border_l_2()
            .border_color(rgb(if is_selected { theme::ACCENT } else { theme::BG_PANEL }))
            .bg(rgb(if is_selected { theme::SELECTED } else { theme::BG_PANEL }))
            .text_color(rgb(if is_selected { theme::TEXT } else { theme::TEXT_DIM }))
            .text_size(px(theme::TEXT_SIZE))
            .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::TEXT)).border_color(rgb(theme::ACCENT)))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.selected_key = Some(name.clone());
                cx.emit(SidebarEvent::KeySelected(name.clone()));
                cx.notify();
            }))
            .child(
                div()
                    .w(px(16.))
                    .flex()
                    .justify_center()
                    .text_color(rgb(type_color))
                    .text_size(px(theme::TEXT_SIZE_XS))
                    .font_family(theme::FONT_MONO)
                    .child(SharedString::from(type_icon.to_string())),
            )
            .child(
                div()
                    .flex_grow()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::FONT_MONO)
                    .child(SharedString::from(key.name.clone())),
            );
        row
    }

    /// Simple inline filter bar for Redis key list.
    fn redis_filter_bar(&self) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(theme::ROW_HEIGHT))
            .px(px(theme::PAD_SM))
            .flex()
            .items_center()
            .bg(rgb(theme::BG_DEEP))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(theme::TEXT_DIM))
            .text_size(px(theme::TEXT_SIZE_XS))
            .child(SharedString::from(format!(
                "{} keys{}",
                self.redis_keys.len(),
                if self.redis_filter.is_empty() { String::new() }
                else { format!(" (filter: {})", self.redis_filter) }
            )))
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tree = div().id("db-tree").flex().flex_col().overflow_y_scroll();

        if self.redis_mode {
            // Redis: flat key list (filtered if redis_filter is set).
            let filter = self.redis_filter.to_lowercase();
            for key in self.redis_keys.clone() {
                if filter.is_empty() || key.name.to_lowercase().contains(&filter) {
                    tree = tree.child(self.key_row(&key, cx));
                }
            }
        } else {
            // SQL: database → table tree.
            for db in self.databases.clone() {
                tree = tree.child(self.db_row(&db, cx));
                if self.expanded.as_deref() == Some(db.as_str()) {
                    for t in self.tables.clone() {
                        tree = tree.child(self.table_row(&db, &t, cx));
                    }
                }
            }
        }

        div()
            .w(px(theme::SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG_PANEL))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(self.header(cx))
            .children(self.redis_mode.then(|| self.redis_filter_bar()))
            .child(tree.flex_grow())
    }
}
