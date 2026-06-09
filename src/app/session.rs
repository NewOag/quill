//! One connection's full working state — a single tab.
//!
//! A `Session` owns its `Db`, its three panes (`Sidebar`, `QueryEditor`,
//! `DataTable`), its subscriptions to those panes, and its in-flight async
//! tasks. It is the single orchestrator for *its* connection: it handles the
//! pane events, launches queries, and pushes results back. Because each tab is
//! its own `Entity<Session>`, "which tab emitted this event" never arises — the
//! handlers are methods on `Session` and `self.db` is unambiguously this tab's.
//!
//! This is the lifted body of the old single-connection `Workspace`; the only
//! semantic change is that there can now be many of them, and the title/tab bar
//! moved up to `Workspace`.
//!
//! ## Async bridge (unchanged)
//! gpui runs on the main thread; `mysql_async` needs a tokio reactor. Per
//! request: set a loading state synchronously, `cx.spawn` onto gpui's executor
//! (storing the `Task` so it isn't cancelled, and so a newer request cancels
//! the older), `handle.spawn` the DB future onto tokio, carry the owned `Send`
//! result back over a `oneshot`, then `entity.update` to re-render.

use gpui::{Context, Entity, EventEmitter, Subscription, Task, Window, div, prelude::*, rgb};
use uuid::Uuid;

use crate::app::db::Db;
use crate::datasource::{Column, QueryResult, QueryState, Row, TableInfo};
use crate::ui::editor::{EditorEvent, QueryEditor};
use crate::ui::redis_view::RedisView;
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::table::{DataTable, TableEvent};
use crate::ui::theme;

/// How many rows a table-click query fetches. Bounded until streaming lands.
const ROW_LIMIT: usize = 500;

/// Events a Session can emit to its owning Workspace.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// User clicked the edit-connection button in the sidebar header.
    EditConnection,
    /// User clicked the delete-connection button in the sidebar header.
    DeleteConnection,
}

impl EventEmitter<SessionEvent> for Session {}

pub struct Session {
    /// Links back to the saved `ConnectionConfig` (tab label, persistence).
    pub config_id: Uuid,
    /// `Some` for a real connection; `None` only in sample mode.
    db: Option<Db>,
    sidebar: Entity<Sidebar>,
    /// SQL mode panes (None for Redis connections).
    editor: Option<Entity<QueryEditor>>,
    table: Option<Entity<DataTable>>,
    /// Redis mode view (None for SQL connections).
    redis_view: Option<Entity<RedisView>>,

    /// Database and table name from the last sidebar table-click query.
    /// Set only for table-click queries; cleared on manual SQL edits.
    current_database: Option<String>,
    current_table: Option<String>,

    /// Subscriptions to the panes; held so they live as long as this session.
    _subs: Vec<Subscription>,
    /// In-flight async tasks, held so they aren't cancelled on drop. A newer
    /// request overwrites (and cancels) the previous one.
    _schema_task: Option<Task<()>>,
    _query_task: Option<Task<()>>,
}

impl Session {
    /// Build a session. `db` is `None` for sample mode. Kicks off loading the
    /// database list immediately.
    pub fn new(
        config_id: Uuid,
        db: Option<Db>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let label = db
            .as_ref()
            .map(|d| d.label().to_string())
            .unwrap_or_else(|| "sample (no DB)".to_string());

        let sidebar = cx.new(|_| Sidebar::new(label));
        let is_redis = db.as_ref().map(|d| !d.is_sql()).unwrap_or(false);

        // Build SQL or Redis panes depending on the connection type.
        let (editor, table, redis_view) = if is_redis {
            let rv = cx.new(|cx| RedisView::new(db.clone(), window, cx));
            (None, None, Some(rv))
        } else {
            let e = cx.new(|cx| QueryEditor::new(window, cx));
            let t = cx.new(|cx| DataTable::new(cx));
            (Some(e), Some(t), None)
        };

        let mut subs = vec![cx.subscribe(&sidebar, Self::on_sidebar_event)];
        if let Some(ed) = &editor {
            subs.push(cx.subscribe(ed, Self::on_editor_event));
        }
        if let Some(t) = &table {
            subs.push(cx.subscribe(t, Self::on_table_event));
        }

        let mut this = Self {
            config_id,
            db,
            sidebar,
            editor,
            table,
            redis_view,
            current_database: None,
            current_table: None,
            _subs: subs,
            _schema_task: None,
            _query_task: None,
        };

        // Focus SQL editor immediately.
        if let Some(ed) = &this.editor {
            ed.update(cx, |e, cx| e.focus(window, cx));
        }

        if is_redis {
            this.load_redis_keys(cx);
        } else {
            this.load_databases(cx);
        }
        this
    }

    // --- event handlers ----------------------------------------------------

    fn on_sidebar_event(
        &mut self,
        _sidebar: Entity<Sidebar>,
        event: &SidebarEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarEvent::DatabaseSelected(db_name) => {
                // Navigating to a different database — the current table context is stale.
                self.current_database = None;
                self.current_table = None;
                if let Some(t) = &self.table {
                    t.update(cx, |tbl, _| tbl.set_editable(false));
                }
                self.load_tables(db_name.clone(), cx);
            }
            SidebarEvent::TableSelected { database, table } => {
                // Record context so run_cell_update can build UPDATE SQL.
                self.current_database = Some(database.clone());
                self.current_table = Some(table.clone());
                let sql = select_all_sql(database, table, ROW_LIMIT);
                if let Some(ed) = &self.editor {
                    ed.update(cx, |e, cx| e.set_sql(sql.clone(), cx));
                }
                self.run_query(sql, cx);
                // Mark the table as editable now that context is set.
                if let Some(t) = &self.table {
                    t.update(cx, |tbl, _| tbl.set_editable(true));
                }
            }
            SidebarEvent::KeySelected(key) => {
                if let Some(rv) = &self.redis_view {
                    rv.update(cx, |v, cx| v.load_key(key.clone(), cx));
                }
            }
            SidebarEvent::EditConnection => cx.emit(SessionEvent::EditConnection),
            SidebarEvent::DeleteConnection => cx.emit(SessionEvent::DeleteConnection),
        }
    }

    fn on_editor_event(
        &mut self,
        _editor: Entity<QueryEditor>,
        event: &EditorEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Run(sql) => {
                if !sql.trim().is_empty() {
                    // Manual SQL — lose the table context so inline editing is disabled.
                    self.current_database = None;
                    self.current_table = None;
                    if let Some(t) = &self.table {
                        t.update(cx, |tbl, _| tbl.set_editable(false));
                    }
                    self.run_query(sql.clone(), cx);
                }
            }
        }
    }

    // --- async orchestration ----------------------------------------------

    fn load_databases(&mut self, cx: &mut Context<Self>) {
        let Some(db) = self.db.clone() else {
            self.sidebar.update(cx, |s, cx| {
                s.set_databases(vec!["sample_db".into()]);
                cx.notify();
            });
            return;
        };

        let handle = db.handle();
        let sidebar = self.sidebar.clone();
        self._schema_task = Some(cx.spawn(async move |_weak, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            handle.spawn(async move {
                let _ = tx.send(db.list_databases().await);
            });
            let result = rx.await.unwrap_or_else(|_| Ok(vec![]));
            if let Ok(dbs) = result {
                let _ = sidebar.update(cx, |s, cx| {
                    s.set_databases(dbs);
                    cx.notify();
                });
            }
        }));
    }

    fn load_tables(&mut self, database: String, cx: &mut Context<Self>) {
        let Some(db) = self.db.clone() else {
            self.sidebar.update(cx, |s, cx| {
                s.set_tables(database.clone(), sample_tables());
                cx.notify();
            });
            return;
        };

        let handle = db.handle();
        let sidebar = self.sidebar.clone();
        self._schema_task = Some(cx.spawn(async move |_weak, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            let db_for_task = database.clone();
            handle.spawn(async move {
                let _ = tx.send(db.list_tables(&db_for_task).await);
            });
            if let Ok(Ok(tables)) = rx.await {
                let _ = sidebar.update(cx, |s, cx| {
                    s.set_tables(database, tables);
                    cx.notify();
                });
            }
        }));
    }

    fn run_query(&mut self, sql: String, cx: &mut Context<Self>) {
        let Some(table_entity) = self.table.clone() else {
            return;
        };
        let preview = sql.chars().take(60).collect::<String>();
        table_entity.update(cx, |t, cx| {
            t.set_state(QueryState::Loading(preview));
            cx.notify();
        });

        let Some(db) = self.db.clone() else {
            table_entity.update(cx, |t, cx| {
                t.set_state(QueryState::Loaded(sample_result()));
                cx.notify();
            });
            return;
        };

        let handle = db.handle();
        let table = table_entity;
        self._query_task = Some(cx.spawn(async move |_weak, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            let sql_for_log = sql.clone();
            handle.spawn(async move {
                let started = std::time::Instant::now();
                let result = db.query(&sql).await;
                // Diagnostics to stderr so a hung/slow/failed query is visible
                // in the terminal rather than an indefinite "running…".
                match &result {
                    Ok(r) => eprintln!(
                        "quill: query OK in {:?} — {} rows × {} cols [{}]",
                        started.elapsed(),
                        r.row_count(),
                        r.col_count(),
                        sql_for_log
                    ),
                    Err(e) => eprintln!(
                        "quill: query ERR in {:?}: {e:#} [{}]",
                        started.elapsed(),
                        sql_for_log
                    ),
                }
                let _ = tx.send(result);
            });
            let state = match rx.await {
                Ok(Ok(result)) => QueryState::Loaded(result),
                Ok(Err(e)) => QueryState::Error(format!("{e:#}")),
                Err(_) => QueryState::Error("query task dropped".into()),
            };
            let _ = table.update(cx, |t, cx| {
                t.set_state(state);
                cx.notify();
            });
        }));
    }

    // --- inline cell edit ---------------------------------------------------

    fn on_table_event(
        &mut self,
        _table: Entity<DataTable>,
        event: &TableEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TableEvent::UpdateCell {
                orig_row,
                col,
                new_value,
            } => {
                self.run_cell_update(*orig_row, *col, new_value.clone(), cx);
            }
        }
    }

    fn run_cell_update(
        &mut self,
        orig_row: usize,
        col: usize,
        new_value: String,
        cx: &mut Context<Self>,
    ) {
        let (db_name, table_name) = match (&self.current_database, &self.current_table) {
            (Some(d), Some(t)) => (d.clone(), t.clone()),
            _ => return,
        };

        let result = match self.table.as_ref().and_then(|t| t.read(cx).loaded_result()) {
            Some(r) => r,
            None => return,
        };

        let sql = build_update_sql(&db_name, &table_name, orig_row, col, &new_value, &result);
        if sql.is_empty() {
            return;
        }

        // Show loading state, run the UPDATE, then re-run the SELECT to refresh.
        let Some(table_entity) = self.table.clone() else {
            return;
        };
        let Some(db) = self.db.clone() else { return };
        let handle = db.handle();

        // The refresh query is the current SELECT * (re-run with same db/table).
        let refresh_sql = select_all_sql(&db_name, &table_name, ROW_LIMIT);

        table_entity.update(cx, |t, cx| {
            t.set_state(QueryState::Loading("updating…".into()));
            cx.notify();
        });

        let table = table_entity;
        self._query_task = Some(cx.spawn(async move |_weak, cx| {
            // Execute the UPDATE.
            let (tx, rx) = futures::channel::oneshot::channel::<anyhow::Result<()>>();
            let sql_for_task = sql.clone();
            let db2 = db.clone();
            handle.spawn(async move {
                let _ = tx.send(db2.execute(&sql_for_task).await);
            });

            match rx.await {
                Ok(Ok(())) => {
                    // Re-fetch the table so the edit is visible.
                    let (tx2, rx2) = futures::channel::oneshot::channel();
                    handle.spawn(async move {
                        let _ = tx2.send(db.query(&refresh_sql).await);
                    });
                    match rx2.await {
                        Ok(Ok(result)) => {
                            // Keep editable after refresh.
                            let _ = table.update(cx, |t, cx| {
                                t.set_state(QueryState::Loaded(result));
                                t.set_editable(true);
                                cx.notify();
                            });
                        }
                        Ok(Err(e)) => {
                            let _ = table.update(cx, |t, cx| {
                                t.set_state(QueryState::Error(format!("{e:#}")));
                                cx.notify();
                            });
                        }
                        Err(_) => {
                            let _ = table.update(cx, |t, cx| {
                                t.set_state(QueryState::Error("refresh task dropped".into()));
                                cx.notify();
                            });
                        }
                    };
                }
                Ok(Err(e)) => {
                    let _ = table.update(cx, |t, cx| {
                        t.set_state(QueryState::Error(format!("UPDATE failed: {e:#}")));
                        cx.notify();
                    });
                }
                Err(_) => {
                    let _ = table.update(cx, |t, cx| {
                        t.set_state(QueryState::Error("update task dropped".into()));
                        cx.notify();
                    });
                }
            }
        }));
    }

    // --- Redis-specific async methods ---------------------------------------

    fn load_redis_keys(&mut self, cx: &mut Context<Self>) {
        let Some(db) = self.db.clone() else { return };
        let handle = db.handle();
        let sidebar = self.sidebar.clone();
        self._schema_task = Some(cx.spawn(async move |_weak, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            handle.spawn(async move {
                let _ = tx.send(db.redis_scan("*", 500).await);
            });
            if let Ok(Ok(keys)) = rx.await {
                let _ = sidebar.update(cx, |s, cx| {
                    s.set_redis_keys(keys);
                    cx.notify();
                });
            }
        }));
    }
}

impl Render for Session {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let main = if let Some(rv) = &self.redis_view {
            // Redis: dedicated key-value view (no SQL editor).
            div().flex_grow().child(rv.clone())
        } else {
            // SQL: editor + results table.
            div()
                .flex_grow()
                .flex()
                .flex_col()
                .children(self.editor.clone())
                .children(
                    self.table
                        .as_ref()
                        .map(|t| div().flex_grow().child(t.clone())),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(theme::BG_DEEP))
            .child(self.sidebar.clone())
            .child(main)
    }
}

/// Build the auto-query for a table click. Identifiers are backtick-quoted with
/// any embedded backtick doubled, so a table named `` we`ird `` can't break out
/// of the quoting.
fn select_all_sql(database: &str, table: &str, limit: usize) -> String {
    let q = |id: &str| id.replace('`', "``");
    format!(
        "SELECT * FROM `{}`.`{}` LIMIT {limit}",
        q(database),
        q(table)
    )
}

/// Escape a string value for use in a SQL literal (single-quote doubling).
fn sql_escape(v: &str) -> String {
    v.replace('\'', "''")
}

/// Build an `UPDATE` statement that sets `col` to `new_value` in `orig_row`.
/// Uses all non-NULL column values as WHERE conditions + `LIMIT 1` for safety.
/// Returns an empty string if the row or column index is out of bounds.
fn build_update_sql(
    database: &str,
    table: &str,
    orig_row: usize,
    col: usize,
    new_value: &str,
    result: &QueryResult,
) -> String {
    let row = match result.rows.get(orig_row) {
        Some(r) => r,
        None => return String::new(),
    };
    let set_col = match result.columns.get(col) {
        Some(c) => &c.name,
        None => return String::new(),
    };

    let q = |id: &str| id.replace('`', "``");
    let set_clause = format!("`{}` = '{}'", q(set_col), sql_escape(new_value));

    // WHERE: every non-NULL column value (including the one being updated, using
    // its *original* value so the row is still identifiable).
    let where_parts: Vec<String> = result
        .columns
        .iter()
        .zip(row.cells.iter())
        .filter_map(|(c, cell)| {
            cell.as_ref()
                .map(|v| format!("`{}` = '{}'", q(&c.name), sql_escape(v)))
        })
        .collect();

    if where_parts.is_empty() {
        // All-NULL row — can't safely identify it.
        return String::new();
    }

    format!(
        "UPDATE `{}`.`{}` SET {} WHERE {} LIMIT 1",
        q(database),
        q(table),
        set_clause,
        where_parts.join(" AND "),
    )
}

// --- sample-mode data (no DB) ---------------------------------------------

fn sample_tables() -> Vec<TableInfo> {
    vec![
        TableInfo {
            name: "users".into(),
            kind: "table".into(),
        },
        TableInfo {
            name: "orders".into(),
            kind: "table".into(),
        },
        TableInfo {
            name: "active_users".into(),
            kind: "view".into(),
        },
    ]
}

fn sample_result() -> QueryResult {
    let columns = vec![
        Column {
            name: "id".into(),
            type_name: "INT".into(),
        },
        Column {
            name: "name".into(),
            type_name: "VARCHAR".into(),
        },
        Column {
            name: "email".into(),
            type_name: "VARCHAR".into(),
        },
        Column {
            name: "note".into(),
            type_name: "TEXT".into(),
        },
    ];
    let rows = (0..500)
        .map(|i| Row {
            cells: vec![
                Some(i.to_string()),
                Some(format!("user_{i}")),
                Some(format!("user_{i}@example.com")),
                if i % 7 == 0 {
                    None
                } else {
                    Some(format!("note {i}"))
                },
            ],
        })
        .collect();
    QueryResult { columns, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_all_sql_quotes_identifiers() {
        assert_eq!(
            select_all_sql("shop", "users", 500),
            "SELECT * FROM `shop`.`users` LIMIT 500"
        );
    }

    #[test]
    fn select_all_sql_escapes_backticks() {
        assert_eq!(
            select_all_sql("db", "we`ird", 10),
            "SELECT * FROM `db`.`we``ird` LIMIT 10"
        );
    }

    #[test]
    fn sample_result_shape_is_consistent() {
        let r = sample_result();
        assert_eq!(r.col_count(), 4);
        assert_eq!(r.row_count(), 500);
        assert!(r.rows.iter().all(|row| row.cells.len() == r.col_count()));
        assert!(r.rows[0].cells[3].is_none());
        assert!(r.rows[1].cells[3].is_some());
    }
}
