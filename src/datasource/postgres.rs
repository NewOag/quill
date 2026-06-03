//! PostgreSQL data source backed by `tokio-postgres`.
//!
//! A `PostgresSource` opens one connection per query (cheap connection re-use
//! is a follow-up once pooling with `bb8`/`deadpool` is added; for now the
//! single client is kept alive by spawning its `Connection` driver task and
//! holding the `Client` in a `tokio::sync::Mutex`). This matches the style of
//! the MySQL pool: the `Db` layer clones the source and calls async methods from
//! inside tokio tasks.

use anyhow::{Context as _, Result};
use tokio::sync::Mutex;
use std::sync::Arc;
use tokio_postgres::NoTls;

use super::{Column, DataSource, DbKind, QueryResult, Row, SchemaProvider, TableInfo};

/// A PostgreSQL-backed data source.
#[derive(Clone)]
pub struct PostgresSource {
    label: String,
    /// Client shared across clones via Arc+Mutex.
    client: Arc<Mutex<tokio_postgres::Client>>,
}

impl PostgresSource {
    /// Connect to PostgreSQL. Spawns the background driver task on the current
    /// tokio runtime (must be called from within a tokio context, i.e. inside
    /// `handle.spawn(...)`).
    pub async fn connect(url: &str) -> Result<Self> {
        let config: tokio_postgres::Config = url
            .parse()
            .context("invalid PostgreSQL URL")?;

        // Label: user@host:port/db
        let label = format!(
            "{}@{}:{}/{}",
            config.get_user().unwrap_or("?"),
            config
                .get_hosts()
                .first()
                .map(|h| format!("{h:?}"))
                .unwrap_or_else(|| "?".into()),
            config.get_ports().first().copied().unwrap_or(5432),
            config.get_dbname().unwrap_or("")
        );

        let (client, connection) = config
            .connect(NoTls)
            .await
            .context("failed to connect to PostgreSQL")?;

        // The connection object must be driven to completion.
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("quill: postgres connection error: {e}");
            }
        });

        Ok(Self {
            label,
            client: Arc::new(Mutex::new(client)),
        })
    }
}

impl DataSource for PostgresSource {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }

    async fn query(&mut self, sql: &str) -> Result<QueryResult> {
        let client = self.client.lock().await;
        let rows = client.query(sql, &[]).await.context("query failed")?;

        let columns: Vec<Column> = rows
            .first()
            .map(|r| {
                r.columns()
                    .iter()
                    .map(|c| Column {
                        name: c.name().to_string(),
                        type_name: format!("{}", c.type_().name()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let out_rows = rows
            .into_iter()
            .map(|r| {
                let cells = (0..r.len())
                    .map(|i| pg_cell_to_string(&r, i))
                    .collect();
                Row { cells }
            })
            .collect();

        Ok(QueryResult { columns, rows: out_rows })
    }
}

impl SchemaProvider for PostgresSource {
    async fn list_databases(&mut self) -> Result<Vec<String>> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT datname FROM pg_database \
                 WHERE datistemplate = false ORDER BY datname",
                &[],
            )
            .await
            .context("list databases failed")?;
        Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
    }

    async fn list_tables(&mut self, schema: &str) -> Result<Vec<TableInfo>> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT table_name, table_type \
                 FROM information_schema.tables \
                 WHERE table_schema = $1 ORDER BY table_name",
                &[&schema],
            )
            .await
            .context("list tables failed")?;
        Ok(rows
            .iter()
            .map(|r| TableInfo {
                name: r.get::<_, String>(0),
                kind: if r.get::<_, String>(1).eq_ignore_ascii_case("VIEW") {
                    "view".into()
                } else {
                    "table".into()
                },
            })
            .collect())
    }
}

/// Convert a tokio-postgres cell to a display string, trying common types in
/// order. Returns `None` for SQL NULL or unsupported/unrecognized types.
fn pg_cell_to_string(row: &tokio_postgres::Row, idx: usize) -> Option<String> {
    // String / text first (covers varchar, text, char, …).
    if let Ok(v) = row.try_get::<_, Option<String>>(idx) { return v; }
    // Numeric types.
    if let Ok(v) = row.try_get::<_, Option<i64>>(idx)  { return v.map(|x| x.to_string()); }
    if let Ok(v) = row.try_get::<_, Option<i32>>(idx)  { return v.map(|x| x.to_string()); }
    if let Ok(v) = row.try_get::<_, Option<i16>>(idx)  { return v.map(|x| x.to_string()); }
    if let Ok(v) = row.try_get::<_, Option<f64>>(idx)  { return v.map(|x| x.to_string()); }
    if let Ok(v) = row.try_get::<_, Option<f32>>(idx)  { return v.map(|x| x.to_string()); }
    if let Ok(v) = row.try_get::<_, Option<bool>>(idx) { return v.map(|x| x.to_string()); }
    // Bytes (bytea) → show as hex.
    if let Ok(v) = row.try_get::<_, Option<Vec<u8>>>(idx) {
        return v.map(|b| b.iter().map(|x| format!("{x:02x}")).collect());
    }
    // Anything else (JSON, UUID, TIMESTAMP, …) → fall through.
    // tokio-postgres will return an error for unexpected types; we silently
    // return None so the cell shows as NULL rather than crashing.
    None
}
