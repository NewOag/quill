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

use gpui::{div, prelude::*, rgb, Context, Entity, EventEmitter, Subscription, Task, Window};
use uuid::Uuid;

use crate::app::db::Db;
use crate::datasource::{Column, QueryResult, QueryState, Row, TableInfo};
use crate::ui::editor::{EditorEvent, QueryEditor};
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::table::DataTable;
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
    editor: Entity<QueryEditor>,
    table: Entity<DataTable>,

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
        let editor = cx.new(|cx| QueryEditor::new(window, cx));
        let table = cx.new(|_| DataTable::new());

        // Own the subscriptions explicitly (instead of detach) to document that
        // they live with this session and die when the tab closes.
        let subs = vec![
            cx.subscribe(&sidebar, Self::on_sidebar_event),
            cx.subscribe(&editor, Self::on_editor_event),
        ];

        let mut this = Self {
            config_id,
            db,
            sidebar,
            editor,
            table,
            _subs: subs,
            _schema_task: None,
            _query_task: None,
        };

        this.editor.update(cx, |e, cx| e.focus(window, cx));
        this.load_databases(cx);
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
            SidebarEvent::DatabaseSelected(db_name) => self.load_tables(db_name.clone(), cx),
            SidebarEvent::TableSelected { database, table } => {
                let sql = select_all_sql(database, table, ROW_LIMIT);
                self.editor.update(cx, |e, cx| e.set_sql(sql.clone(), cx));
                self.run_query(sql, cx);
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
        let preview = sql.chars().take(60).collect::<String>();
        self.table.update(cx, |t, cx| {
            t.set_state(QueryState::Loading(preview));
            cx.notify();
        });

        let Some(db) = self.db.clone() else {
            self.table.update(cx, |t, cx| {
                t.set_state(QueryState::Loaded(sample_result()));
                cx.notify();
            });
            return;
        };

        let handle = db.handle();
        let table = self.table.clone();
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
}

impl Render for Session {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // The session is just its main area: sidebar | (editor over results).
        // The tab bar lives in `Workspace`.
        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(theme::BG_DEEP))
            .child(self.sidebar.clone())
            .child(
                div()
                    .flex_grow()
                    .flex()
                    .flex_col()
                    .child(self.editor.clone())
                    .child(div().flex_grow().child(self.table.clone())),
            )
    }
}

/// Build the auto-query for a table click. Identifiers are backtick-quoted with
/// any embedded backtick doubled, so a table named `` we`ird `` can't break out
/// of the quoting.
fn select_all_sql(database: &str, table: &str, limit: usize) -> String {
    let q = |id: &str| id.replace('`', "``");
    format!("SELECT * FROM `{}`.`{}` LIMIT {limit}", q(database), q(table))
}

// --- sample-mode data (no DB) ---------------------------------------------

fn sample_tables() -> Vec<TableInfo> {
    vec![
        TableInfo { name: "users".into(), kind: "table".into() },
        TableInfo { name: "orders".into(), kind: "table".into() },
        TableInfo { name: "active_users".into(), kind: "view".into() },
    ]
}

fn sample_result() -> QueryResult {
    let columns = vec![
        Column { name: "id".into(), type_name: "INT".into() },
        Column { name: "name".into(), type_name: "VARCHAR".into() },
        Column { name: "email".into(), type_name: "VARCHAR".into() },
        Column { name: "note".into(), type_name: "TEXT".into() },
    ];
    let rows = (0..500)
        .map(|i| Row {
            cells: vec![
                Some(i.to_string()),
                Some(format!("user_{i}")),
                Some(format!("user_{i}@example.com")),
                if i % 7 == 0 { None } else { Some(format!("note {i}")) },
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
