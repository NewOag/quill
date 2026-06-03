//! The async bridge between gpui (main-thread executor) and the database
//! drivers (which need a tokio reactor).
//!
//! `Db` owns a tokio `Handle` plus the active connection. It exposes plain
//! `async fn`s that run entirely in the tokio world and return our own
//! `Send + 'static` types ([`QueryResult`], `Vec<TableInfo>`, …). The gpui side
//! (see [`crate::app::workspace`]) is responsible for *spawning* these onto
//! tokio via `handle.spawn(...)` and marshalling the result back with a
//! `oneshot` channel + `WeakEntity::update`.
//!
//! Keeping the tokio-specific objects (pools, connections) strictly inside this
//! module means the UI never accidentally awaits a driver future on gpui's
//! executor — the #1 footgun in mixing the two async worlds.

use anyhow::{bail, Result};
use tokio::runtime::Handle;

use crate::datasource::mysql::{self, MysqlSource};
use crate::datasource::postgres::PostgresSource;
use crate::datasource::{DataSource, DbKind, QueryResult, SchemaProvider, TableInfo};

/// The active connection, one variant per engine.
#[derive(Clone)]
#[allow(dead_code)] // Redis variant is a reserved seam.
enum Conn {
    Mysql(MysqlSource),
    Postgres(PostgresSource),
    /// Reserved — not wired yet.
    Redis,
}

/// A handle to the active database, cloneable and `Send` so it can be captured
/// into tokio tasks. Cloning shares the underlying connection pool.
#[derive(Clone)]
pub struct Db {
    handle: Handle,
    conn: Conn,
    label: String,
    /// Reserved: the UI will branch on this once it renders per-engine affordances.
    #[allow(dead_code)]
    kind: DbKind,
}

impl Db {
    /// Build a MySQL-backed `Db` from a URL. Does not eagerly connect — the
    /// first query/listing establishes the connection and surfaces errors.
    pub fn mysql(handle: Handle, url: &str) -> Result<Self> {
        let source = MysqlSource::new(url)?;
        let label = source.label();
        Ok(Self {
            handle,
            conn: Conn::Mysql(source),
            label,
            kind: DbKind::Mysql,
        })
    }

    /// Build a PostgreSQL-backed `Db`. Connects eagerly (tokio-postgres requires
    /// an async connect; this blocks the caller via `handle.block_on`).
    pub fn postgres(handle: Handle, url: &str) -> Result<Self> {
        let url = url.to_string();
        let source = handle.block_on(PostgresSource::connect(&url))?;
        let label = source.label();
        Ok(Self {
            handle,
            conn: Conn::Postgres(source),
            label,
            kind: DbKind::Postgres,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    #[allow(dead_code)] // reserved: read once the UI branches per engine.
    pub fn kind(&self) -> DbKind {
        self.kind
    }

    /// The tokio handle, for the Workspace to spawn the futures below.
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// List databases. Async fn living in the tokio world.
    pub async fn list_databases(&self) -> Result<Vec<String>> {
        match &self.conn {
            Conn::Mysql(src) => {
                let mut src = src.clone();
                src.list_databases().await
            }
            Conn::Postgres(src) => {
                let mut src = src.clone();
                src.list_databases().await
            }
            Conn::Redis => bail!("Redis has no databases in the SQL sense"),
        }
    }

    /// List tables in a database/schema.
    pub async fn list_tables(&self, schema: &str) -> Result<Vec<TableInfo>> {
        match &self.conn {
            Conn::Mysql(src) => {
                let mut src = src.clone();
                src.list_tables(schema).await
            }
            Conn::Postgres(src) => {
                let mut src = src.clone();
                src.list_tables(schema).await
            }
            Conn::Redis => bail!("Redis has no tables"),
        }
    }

    /// Run an arbitrary SQL query.
    pub async fn query(&self, sql: &str) -> Result<QueryResult> {
        match &self.conn {
            Conn::Mysql(src) => mysql::run_query(&src.pool(), sql).await,
            Conn::Postgres(src) => {
                let mut src = src.clone();
                src.query(sql).await
            }
            Conn::Redis => bail!("Redis does not run SQL"),
        }
    }
}
