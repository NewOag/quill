//! Quill data-source layer.
//!
//! This layer is intentionally GUI-agnostic: it knows nothing about gpui.
//! The UI depends only on the plain data types defined here ([`QueryResult`],
//! [`Column`], [`Row`], [`DbKind`], …) and on the traits — never on a concrete
//! driver like `mysql_async` directly.
//!
//! Design note: we deliberately keep [`DataSource`] *thin*. SQL databases and
//! Redis have fundamentally different data models (rows/columns vs.
//! keys/structures), so we don't try to flatten them into one universal shape.
//! For the MySQL/PostgreSQL world, a tabular [`QueryResult`] plus the
//! [`SchemaProvider`] trait is the right abstraction. Redis gets its OWN trait
//! ([`redis::KeyBrowser`]) rather than being forced onto [`DataSource`].

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub mod mysql;
pub mod postgres;
pub mod redis;

/// Which database engine a connection targets. The enum is the single place
/// the rest of the app branches on when behaviour must differ per engine
/// (icon, default port, dialect quirks).
///
/// `Serialize`/`Deserialize` are derived so it can live inside a persisted
/// `ConnectionConfig`; serde here is harmless to the GUI-agnostic layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // Postgres/Redis variants are reserved seams for now.
pub enum DbKind {
    Mysql,
    Postgres,
    Redis,
}

#[allow(dead_code)] // label()/is_sql() are used once the UI branches per engine.
impl DbKind {
    /// Short display label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            DbKind::Mysql => "MySQL",
            DbKind::Postgres => "PostgreSQL",
            DbKind::Redis => "Redis",
        }
    }

    /// Whether this engine speaks the tabular SQL model (rows/columns) and so
    /// can be driven through [`DataSource`] / [`SchemaProvider`]. Redis cannot.
    pub fn is_sql(self) -> bool {
        matches!(self, DbKind::Mysql | DbKind::Postgres)
    }
}

/// One column in a tabular result set.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    /// Driver-reported type name (e.g. "INT", "VARCHAR"). Kept as a string so
    /// the UI can display it without depending on driver-specific enums.
    pub type_name: String,
}

/// One row: cell values rendered as display strings, aligned with the column
/// order in the owning [`QueryResult`]. `None` represents SQL NULL, so the UI
/// can distinguish it from an empty string.
#[derive(Debug, Clone)]
pub struct Row {
    pub cells: Vec<Option<String>>,
}

/// A tabular result from a query (the SQL-family shape).
#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

impl QueryResult {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn col_count(&self) -> usize {
        self.columns.len()
    }
}

/// A database object that can be listed in the sidebar tree (a table or view).
#[derive(Debug, Clone)]
pub struct TableInfo {
    pub name: String,
    /// "table" | "view" — kept as a string for display flexibility.
    pub kind: String,
}

/// Error string carried back across the async boundary. We use an owned
/// `String` (not a borrowed driver error) so it is `Send + 'static` and can be
/// moved from the tokio task into the gpui view. See [`crate::app::db`].
pub type QueryError = String;

/// The lifecycle of a single query/fetch, held by the UI so it can render a
/// spinner, the result, or an error. `Send + 'static` throughout.
#[derive(Debug, Clone, Default)]
pub enum QueryState {
    /// Nothing requested yet.
    #[default]
    Idle,
    /// A query is in flight; the string is a short description (e.g. the SQL).
    Loading(String),
    /// A result is ready.
    Loaded(QueryResult),
    /// The last query failed.
    Error(QueryError),
}

/// A live connection that can answer tabular queries (the SQL-family shape).
///
/// Kept minimal on purpose — connection lifecycle + querying. Schema
/// introspection lives in the separate [`SchemaProvider`] trait so a driver can
/// implement them independently.
#[allow(async_fn_in_trait)]
pub trait DataSource {
    /// A human-readable label for this source, shown in the UI title bar.
    fn label(&self) -> String;

    /// The engine kind, so the UI can branch on it. (Reserved: used once the
    /// UI renders per-engine affordances.)
    #[allow(dead_code)]
    fn kind(&self) -> DbKind;

    /// Run a query and return the full tabular result.
    ///
    /// Note: this loads the whole result into memory. Streaming / paginated
    /// fetch is a planned follow-up; for now callers should bound their SQL
    /// (e.g. `LIMIT 500`). (Reserved: the app currently queries via
    /// [`crate::app::db::Db::query`]; this trait method anchors the contract.)
    #[allow(dead_code)]
    async fn query(&mut self, sql: &str) -> Result<QueryResult>;
}

/// Schema introspection for SQL-family engines: enumerate databases and the
/// tables within one. Separate from [`DataSource`] so the sidebar can depend on
/// just this slice.
#[allow(async_fn_in_trait)]
pub trait SchemaProvider {
    /// List the databases/schemas visible to this connection.
    async fn list_databases(&mut self) -> Result<Vec<String>>;

    /// List the tables/views inside the given database.
    async fn list_tables(&mut self, database: &str) -> Result<Vec<TableInfo>>;
}
