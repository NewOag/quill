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
use crate::datasource::{DataSource, DbKind, QueryResult, SchemaProvider, TableInfo};

/// The active connection, one variant per engine. Only MySQL connects this
/// round; the others are reserved so `DbKind` stays a real branch end-to-end.
#[derive(Clone)]
#[allow(dead_code)] // Postgres/Redis variants are reserved seams for now.
enum Conn {
    Mysql(MysqlSource),
    /// Reserved — not wired yet.
    Postgres,
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
            Conn::Postgres => bail!("PostgreSQL not implemented yet"),
            Conn::Redis => bail!("Redis has no databases in the SQL sense"),
        }
    }

    /// List tables in a database.
    pub async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        match &self.conn {
            Conn::Mysql(src) => {
                let mut src = src.clone();
                src.list_tables(database).await
            }
            Conn::Postgres => bail!("PostgreSQL not implemented yet"),
            Conn::Redis => bail!("Redis has no tables"),
        }
    }

    /// Run an arbitrary SQL query.
    pub async fn query(&self, sql: &str) -> Result<QueryResult> {
        match &self.conn {
            Conn::Mysql(src) => mysql::run_query(&src.pool(), sql).await,
            Conn::Postgres => bail!("PostgreSQL not implemented yet"),
            Conn::Redis => bail!("Redis does not run SQL"),
        }
    }
}
