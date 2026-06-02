//! PostgreSQL data source — STUB.
//!
//! The abstraction seam is reserved here so that wiring PostgreSQL later is a
//! matter of filling in these methods (likely with `tokio-postgres` or `sqlx`)
//! without touching the UI. Nothing connects yet; every method reports that it
//! is not implemented. This keeps `DbKind::Postgres` a real, type-checked
//! branch throughout the app.
#![allow(dead_code)] // reserved seam: wired up when PostgreSQL support lands.

use anyhow::{bail, Result};

use super::{DataSource, DbKind, QueryResult, SchemaProvider, TableInfo};

/// Placeholder PostgreSQL source. Construct it to reserve the type; calls fail
/// cleanly until the driver is wired in.
pub struct PostgresSource {
    label: String,
}

impl PostgresSource {
    /// Reserved constructor. Does not connect.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl DataSource for PostgresSource {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }

    async fn query(&mut self, _sql: &str) -> Result<QueryResult> {
        bail!("PostgreSQL support is not implemented yet")
    }
}

impl SchemaProvider for PostgresSource {
    async fn list_databases(&mut self) -> Result<Vec<String>> {
        bail!("PostgreSQL support is not implemented yet")
    }

    async fn list_tables(&mut self, _database: &str) -> Result<Vec<TableInfo>> {
        bail!("PostgreSQL support is not implemented yet")
    }
}
