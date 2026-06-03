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
    div, prelude::*, px, rgb, uniform_list, Context, EventEmitter, SharedString, Window,
};

use crate::datasource::TableInfo;
use crate::datasource::redis::RedisKey;
use crate::ui::theme;

/// A node in the multi-level Redis key namespace tree.
enum RedisTreeNode {
    Group {
        /// Full path, e.g. `"GATEWAY:CAS_LOGOUT"`.
        path: String,
        /// Just the last segment, e.g. `"CAS_LOGOUT"`.
        label: String,
        count: usize,
        children: Vec<RedisTreeNode>,
    },
    Leaf(RedisKey),
}

/// A pre-order flattened row from the visible tree, used by `uniform_list`
/// (which guarantees vertical scroll regardless of row count).
#[derive(Clone)]
enum FlatRow {
    Group { path: String, label: String, count: usize, depth: usize, is_open: bool },
    Leaf { key: RedisKey, depth: usize },
}

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
    redis_keys: Vec<RedisKey>,
    redis_mode: bool,
    /// Filter text for the Redis key list.
    redis_filter: String,
    /// Currently highlighted Redis key.
    selected_key: Option<String>,
    /// Which namespace groups are expanded (prefix string).
    expanded_groups: std::collections::HashSet<String>,
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
            expanded_groups: std::collections::HashSet::new(),
        }
    }

    /// Replace the database list (called after `list_databases` resolves).
    pub fn set_databases(&mut self, databases: Vec<String>) {
        self.databases = databases;
    }

    /// Switch to Redis key-browser mode, showing the given keys.
    pub fn set_redis_keys(&mut self, keys: Vec<RedisKey>) {
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
    #[allow(dead_code)]
    fn key_row(&self, key: &RedisKey, cx: &mut Context<Self>) -> impl IntoElement {
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

    /// Group a Redis key list by the first `:` namespace segment.
    /// Returns `(prefix, keys_in_group)` pairs; keys with no `:` go into a
    /// `""` (ungrouped) bucket rendered directly without a group header.
    /// Build a multi-level namespace tree from key names.
    /// Each node is (full_path, label, leaf_count, children).
    /// Keys whose remaining suffix after removing `parent_path:` have more `:`
    /// are grouped further. `parent_path` is empty at the root.
    fn build_tree(
        keys: &[RedisKey],
        parent_path: &str,
        filter: &str,
    ) -> Vec<RedisTreeNode> {
        let filter_lc = filter.to_lowercase();
        // Partition into direct leaves and children grouped by next segment.
        let mut children: std::collections::BTreeMap<String, Vec<&RedisKey>> =
            std::collections::BTreeMap::new();
        let mut leaves: Vec<&RedisKey> = Vec::new();
        for key in keys {
            if !filter_lc.is_empty() && !key.name.to_lowercase().contains(&filter_lc) {
                continue;
            }
            let suffix = if parent_path.is_empty() {
                &key.name[..]
            } else {
                key.name.strip_prefix(parent_path)
                    .and_then(|s| s.strip_prefix(':'))
                    .unwrap_or(&key.name)
            };
            if let Some(next_seg) = suffix.find(':').map(|i| &suffix[..i]) {
                let child_path = if parent_path.is_empty() {
                    next_seg.to_string()
                } else {
                    format!("{parent_path}:{next_seg}")
                };
                children.entry(child_path).or_default().push(key);
            } else {
                leaves.push(key);
            }
        }
        let mut nodes: Vec<RedisTreeNode> = children
            .into_iter()
            .map(|(path, child_keys)| {
                let label = path.rsplit(':').next().unwrap_or(&path).to_string();
                let count = child_keys.len();
                let owned: Vec<RedisKey> = child_keys.iter().map(|k| (*k).clone()).collect();
                let sub = Self::build_tree(&owned, &path, filter);
                RedisTreeNode::Group { path, label, count, children: sub }
            })
            .collect();
        for k in leaves {
            nodes.push(RedisTreeNode::Leaf(k.clone()));
        }
        nodes
    }

    /// Flatten the visible tree into pre-order rows for `uniform_list`.
    /// Only recurses into groups that are in `expanded_groups`.
    fn flatten_tree(
        nodes: &[RedisTreeNode],
        depth: usize,
        expanded: &std::collections::HashSet<String>,
        out: &mut Vec<FlatRow>,
    ) {
        for node in nodes {
            match node {
                RedisTreeNode::Group { path, label, count, children } => {
                    let is_open = expanded.contains(path);
                    out.push(FlatRow::Group {
                        path: path.clone(),
                        label: label.clone(),
                        count: *count,
                        depth,
                        is_open,
                    });
                    if is_open {
                        Self::flatten_tree(children, depth + 1, expanded, out);
                    }
                }
                RedisTreeNode::Leaf(key) => {
                    out.push(FlatRow::Leaf { key: key.clone(), depth });
                }
            }
        }
    }

    #[allow(dead_code)]
    /// Render tree nodes at a given indent depth, returning a flat list of
    /// elements to append to the scroll container.
    fn render_tree_nodes(
        &self,
        nodes: &[RedisTreeNode],
        depth: usize,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let indent = depth as f32 * theme::PAD_LG;
        let mut out: Vec<gpui::AnyElement> = Vec::new();
        for node in nodes {
            match node {
                RedisTreeNode::Group { path, label, count, children } => {
                    let is_open = self.expanded_groups.contains(path);
                    let chevron = if is_open { theme::ICON_CHEVRON_OPEN } else { theme::ICON_CHEVRON };
                    let path_owned = path.clone();
                    let row = div()
                        .id(SharedString::from(format!("rgroup-{path}")))
                        .h(px(theme::ROW_HEIGHT))
                        .pl(px(theme::PAD_SM + indent))
                        .pr(px(theme::PAD_SM))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::PAD_XS))
                        .text_color(rgb(theme::TEXT))
                        .text_size(px(theme::TEXT_SIZE))
                        .bg(rgb(theme::SURFACE))
                        .hover(|s| s.bg(rgb(theme::HOVER)))
                        .on_click(cx.listener(move |this, _ev, _w, cx| {
                            if this.expanded_groups.contains(&path_owned) {
                                this.expanded_groups.remove(&path_owned);
                            } else {
                                this.expanded_groups.insert(path_owned.clone());
                            }
                            cx.notify();
                        }))
                        .child(
                            div().w(px(14.)).flex_none()
                                .text_color(rgb(theme::TEXT_DIM))
                                .text_size(px(theme::TEXT_SIZE_XS))
                                .child(chevron),
                        )
                        .child(
                            div().flex_grow().min_w_0().truncate()
                                .font_family(theme::FONT_MONO)
                                .child(SharedString::from(label.clone())),
                        )
                        .child(
                            div().text_size(px(theme::TEXT_SIZE_XS))
                                .text_color(rgb(theme::TEXT_DIM))
                                .child(SharedString::from(format!("{count}"))),
                        );
                    out.push(row.into_any_element());
                    if is_open {
                        out.extend(self.render_tree_nodes(children, depth + 1, cx));
                    }
                }
                RedisTreeNode::Leaf(key) => {
                    out.push(
                        div()
                            .pl(px(indent))
                            .child(self.key_row(key, cx))
                            .into_any_element(),
                    );
                }
            }
        }
        out
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
        let sidebar_outer = div()
            .w(px(theme::SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG_PANEL))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(self.header(cx));

        if self.redis_mode {
            // Redis: flattened tree rows rendered via `uniform_list` so vertical
            // scroll is guaranteed to work regardless of row count.
            let nodes = Self::build_tree(&self.redis_keys, "", &self.redis_filter);
            let mut rows: Vec<FlatRow> = Vec::new();
            Self::flatten_tree(&nodes, 0, &self.expanded_groups, &mut rows);
            let row_count = rows.len();
            let rows = std::sync::Arc::new(rows);
            let entity = cx.entity();

            // Capture selected_key by value so the closure is 'static.
            let selected_key = self.selected_key.clone();
            let list = uniform_list("sidebar-redis", row_count, move |range, _w, _cx| {
                let rows = rows.clone();
                let entity = entity.clone();
                let selected_key = selected_key.clone();
                range.map(|i| {
                    let row = rows[i].clone();
                    match row {
                        FlatRow::Group { path, label, count, depth, is_open } => {
                            let chevron = if is_open { theme::ICON_CHEVRON_OPEN } else { theme::ICON_CHEVRON };
                            let indent = depth as f32 * theme::PAD_LG;
                            let path_owned = path.clone();
                            div()
                                .id(SharedString::from(format!("rg-{i}")))
                                .h(px(theme::ROW_HEIGHT))
                                .pl(px(theme::PAD_SM + indent))
                                .pr(px(theme::PAD_SM))
                                .flex().flex_row().items_center().gap(px(theme::PAD_XS))
                                .text_color(rgb(theme::TEXT))
                                .text_size(px(theme::TEXT_SIZE))
                                .bg(rgb(theme::SURFACE))
                                .hover(|s| s.bg(rgb(theme::HOVER)))
                                .on_click({
                                    let entity = entity.clone();
                                    move |_ev, _w, cx| {
                                        entity.update(cx, |this, cx| {
                                            if this.expanded_groups.contains(&path_owned) {
                                                this.expanded_groups.remove(&path_owned);
                                            } else {
                                                this.expanded_groups.insert(path_owned.clone());
                                            }
                                            cx.notify();
                                        });
                                    }
                                })
                                .child(div().w(px(14.)).flex_none().text_color(rgb(theme::TEXT_DIM)).text_size(px(theme::TEXT_SIZE_XS)).child(chevron))
                                .child(div().flex_grow().min_w_0().truncate().font_family(theme::FONT_MONO).child(SharedString::from(label)))
                                .child(div().text_size(px(theme::TEXT_SIZE_XS)).text_color(rgb(theme::TEXT_DIM)).child(SharedString::from(format!("{count}"))))
                                .into_any_element()
                        }
                        FlatRow::Leaf { key, depth } => {
                            let indent = depth as f32 * theme::PAD_LG;
                            let is_selected_ref = selected_key.as_deref() == Some(&key.name);
                            let key_name = key.name.clone();
                            let type_icon = match key.type_name.as_str() {
                                "string" => "S", "list" => "L", "hash" => "H",
                                "set" => "E", "zset" => "Z", _ => "?",
                            };
                            let type_color = match key.type_name.as_str() {
                                "string" => theme::SYN_STRING, "list" => theme::SYN_NUMBER,
                                "hash" => theme::SYN_KEYWORD, "set" => theme::ACCENT,
                                "zset" => theme::SYN_COMMENT, _ => theme::TEXT_DIM,
                            };
                            div()
                                .id(SharedString::from(format!("rl-{i}")))
                                .h(px(theme::ROW_HEIGHT))
                                .pl(px(theme::PAD_SM + indent))
                                .pr(px(theme::PAD_SM))
                                .flex().flex_row().items_center().gap(px(theme::PAD_XS))
                                .border_l_2()
                                .border_color(rgb(if is_selected_ref { theme::ACCENT } else { theme::BG_PANEL }))
                                .bg(rgb(if is_selected_ref { theme::SELECTED } else { theme::BG_PANEL }))
                                .text_color(rgb(if is_selected_ref { theme::TEXT } else { theme::TEXT_DIM }))
                                .text_size(px(theme::TEXT_SIZE))
                                .hover(|s| s.bg(rgb(theme::HOVER)).text_color(rgb(theme::TEXT)).border_color(rgb(theme::ACCENT)))
                                .on_click({
                                    let entity = entity.clone();
                                    move |_ev, _w, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.selected_key = Some(key_name.clone());
                                            cx.emit(SidebarEvent::KeySelected(key_name.clone()));
                                            cx.notify();
                                        });
                                    }
                                })
                                .child(div().w(px(16.)).flex_none().justify_center().text_color(rgb(type_color)).text_size(px(theme::TEXT_SIZE_XS)).font_family(theme::FONT_MONO).child(type_icon))
                                .child(div().flex_grow().min_w_0().truncate().font_family(theme::FONT_MONO).child(SharedString::from(key.name.clone())))
                                .into_any_element()
                        }
                    }
                }).collect()
            })
            .flex_grow()
            .min_h_0();

            sidebar_outer
                .children(self.redis_mode.then(|| self.redis_filter_bar()))
                .child(list)
        } else {
            // SQL: database → table tree (plain overflow_y_scroll, small number of rows).
            let mut tree = div().id("db-tree").flex().flex_col().overflow_y_scroll().min_h_0();
            for db in self.databases.clone() {
                tree = tree.child(self.db_row(&db, cx));
                if self.expanded.as_deref() == Some(db.as_str()) {
                    for t in self.tables.clone() {
                        tree = tree.child(self.table_row(&db, &t, cx));
                    }
                }
            }
            sidebar_outer.child(tree.flex_grow())
        }
    }
}
