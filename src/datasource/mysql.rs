//! MySQL implementation of [`DataSource`] and [`SchemaProvider`], backed by
//! `mysql_async`.
//!
//! Connections are managed through a `mysql_async::Pool`, which is cheap to
//! clone (it's `Arc`-backed). That matters for the async bridge in
//! [`crate::app::db`]: each query clones the pool into a tokio task, grabs a
//! connection, runs, and returns it to the pool — so the UI never holds a
//! single borrowed connection across the thread boundary.
#![allow(dead_code)] // some DataSource methods are reached via free fns for now.

use anyhow::{Context as _, Result};
use mysql_async::prelude::*;
use mysql_async::{Opts, Pool, Row as MyRow};

use super::{Column, DataSource, DbKind, QueryResult, Row, SchemaProvider, TableInfo};

/// A MySQL-backed data source over a connection pool.
#[derive(Clone)]
pub struct MysqlSource {
    label: String,
    pool: Pool,
}

impl MysqlSource {
    /// Build a pool from a standard MySQL URL, e.g.
    /// `mysql://user:pass@127.0.0.1:3306/dbname`. Does not eagerly connect; the
    /// first query establishes a connection (and surfaces connection errors).
    pub fn new(url: &str) -> Result<Self> {
        let opts = Opts::from_url(url).context("invalid MySQL URL")?;
        let label = format!(
            "{}@{}:{}/{}",
            opts.user().unwrap_or("?"),
            opts.ip_or_hostname(),
            opts.tcp_port(),
            opts.db_name().unwrap_or("")
        );
        let pool = Pool::new(opts);
        Ok(Self { label, pool })
    }

    /// Borrow the underlying pool (used by the async bridge to run queries on a
    /// tokio task without holding `&mut self`).
    pub fn pool(&self) -> Pool {
        self.pool.clone()
    }
}

impl DataSource for MysqlSource {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn kind(&self) -> DbKind {
        DbKind::Mysql
    }

    async fn query(&mut self, sql: &str) -> Result<QueryResult> {
        run_query(&self.pool, sql).await
    }
}

impl SchemaProvider for MysqlSource {
    async fn list_databases(&mut self) -> Result<Vec<String>> {
        let mut conn = self.pool.get_conn().await.context("get connection")?;
        let dbs: Vec<String> = conn
            .query("SHOW DATABASES")
            .await
            .context("SHOW DATABASES failed")?;
        Ok(dbs)
    }

    async fn list_tables(&mut self, database: &str) -> Result<Vec<TableInfo>> {
        let mut conn = self.pool.get_conn().await.context("get connection")?;
        // (table_name, table_type) so we can distinguish views from tables.
        let sql = format!(
            "SELECT TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = '{}' ORDER BY TABLE_NAME",
            database.replace('\'', "''")
        );
        let rows: Vec<(String, String)> = conn.query(sql).await.context("list tables failed")?;
        Ok(rows
            .into_iter()
            .map(|(name, ty)| TableInfo {
                name,
                kind: if ty.eq_ignore_ascii_case("VIEW") {
                    "view".to_string()
                } else {
                    "table".to_string()
                },
            })
            .collect())
    }
}

/// Run a query against a pool and map the result into our owned, `Send` types.
/// Free function (not a method) so the async bridge can call it with just a
/// cloned `Pool`, no `&mut self`.
pub async fn run_query(pool: &Pool, sql: &str) -> Result<QueryResult> {
    let mut conn = pool.get_conn().await.context("get connection")?;
    let rows: Vec<MyRow> = conn.query(sql).await.context("query execution failed")?;

    let columns: Vec<Column> = rows
        .first()
        .map(|r| {
            r.columns_ref()
                .iter()
                .map(|c| Column {
                    name: c.name_str().to_string(),
                    type_name: short_type_name(c.column_type()),
                })
                .collect()
        })
        .unwrap_or_default();

    let out_rows = rows
        .into_iter()
        .map(|r| {
            let cells = (0..r.columns_ref().len())
                .map(|i| cell_to_string(&r, i))
                .collect();
            Row { cells }
        })
        .collect();

    Ok(QueryResult {
        columns,
        rows: out_rows,
    })
}

/// Execute a statement that produces no result set (UPDATE, INSERT, DELETE).
/// Free function like `run_query` so the async bridge can call it without `&mut self`.
pub async fn run_execute(pool: &Pool, sql: &str) -> Result<()> {
    let mut conn = pool.get_conn().await.context("get connection")?;
    conn.exec_drop(sql, ()).await.context("execute failed")?;
    Ok(())
}

/// Map a driver column type to a short, familiar SQL-ish name for the header
/// (e.g. `MYSQL_TYPE_VAR_STRING` → `VARCHAR`), instead of the verbose debug
/// form. Unknown types fall back to a trimmed debug string.
fn short_type_name(ty: mysql_async::consts::ColumnType) -> String {
    use mysql_async::consts::ColumnType::*;
    let s = match ty {
        MYSQL_TYPE_TINY => "TINYINT",
        MYSQL_TYPE_SHORT => "SMALLINT",
        MYSQL_TYPE_INT24 => "MEDIUMINT",
        MYSQL_TYPE_LONG => "INT",
        MYSQL_TYPE_LONGLONG => "BIGINT",
        MYSQL_TYPE_FLOAT => "FLOAT",
        MYSQL_TYPE_DOUBLE => "DOUBLE",
        MYSQL_TYPE_NEWDECIMAL | MYSQL_TYPE_DECIMAL => "DECIMAL",
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING => "VARCHAR",
        MYSQL_TYPE_STRING => "CHAR",
        MYSQL_TYPE_BLOB => "TEXT/BLOB",
        MYSQL_TYPE_TINY_BLOB => "TINYTEXT",
        MYSQL_TYPE_MEDIUM_BLOB => "MEDIUMTEXT",
        MYSQL_TYPE_LONG_BLOB => "LONGTEXT",
        MYSQL_TYPE_JSON => "JSON",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "DATE",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "TIME",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "DATETIME",
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "TIMESTAMP",
        MYSQL_TYPE_YEAR => "YEAR",
        MYSQL_TYPE_BIT => "BIT",
        MYSQL_TYPE_ENUM => "ENUM",
        MYSQL_TYPE_SET => "SET",
        MYSQL_TYPE_GEOMETRY => "GEOMETRY",
        MYSQL_TYPE_NULL => "NULL",
        // Fall back: strip the verbose "MYSQL_TYPE_" prefix from the debug name.
        other => {
            return format!("{other:?}")
                .trim_start_matches("MYSQL_TYPE_")
                .to_string();
        }
    };
    s.to_string()
}

/// Convert a single cell to its display string, preserving NULL as `None`.
fn cell_to_string(row: &MyRow, idx: usize) -> Option<String> {
    use mysql_async::Value;
    match row.as_ref(idx) {
        Some(Value::NULL) | None => None,
        Some(Value::Bytes(b)) => Some(String::from_utf8_lossy(b).into_owned()),
        Some(Value::Int(n)) => Some(n.to_string()),
        Some(Value::UInt(n)) => Some(n.to_string()),
        Some(Value::Float(f)) => Some(f.to_string()),
        Some(Value::Double(d)) => Some(d.to_string()),
        Some(v @ Value::Date(..)) | Some(v @ Value::Time(..)) => Some(format!("{:?}", v)),
    }
}
