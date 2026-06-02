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
}

/// The sidebar view.
pub struct Sidebar {
    connection_label: SharedString,
    databases: Vec<String>,
    /// The currently expanded database and its tables, if loaded.
    expanded: Option<String>,
    tables: Vec<TableInfo>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    pub fn new(connection_label: impl Into<SharedString>) -> Self {
        Self {
            connection_label: connection_label.into(),
            databases: Vec::new(),
            expanded: None,
            tables: Vec::new(),
        }
    }

    /// Replace the database list (called after `list_databases` resolves).
    pub fn set_databases(&mut self, databases: Vec<String>) {
        self.databases = databases;
    }

    /// Mark a database expanded and show its tables (after `list_tables`).
    pub fn set_tables(&mut self, database: String, tables: Vec<TableInfo>) {
        self.expanded = Some(database);
        self.tables = tables;
    }

    fn header(&self) -> impl IntoElement {
        div()
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
                    .text_color(rgb(theme::TEXT))
                    .text_size(px(theme::TEXT_SIZE_SM))
                    .child(self.connection_label.clone()),
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
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Build the tree: each database row, with table rows under the expanded one.
        let mut tree = div().id("db-tree").flex().flex_col().overflow_y_scroll();

        for db in self.databases.clone() {
            tree = tree.child(self.db_row(&db, cx));
            if self.expanded.as_deref() == Some(db.as_str()) {
                for t in self.tables.clone() {
                    tree = tree.child(self.table_row(&db, &t, cx));
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
            .child(self.header())
            .child(tree.flex_grow())
    }
}
