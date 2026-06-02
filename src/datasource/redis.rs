//! Redis data source — STUB with its OWN trait.
//!
//! Redis does not fit the rows/columns model, so it deliberately does NOT
//! implement [`DataSource`]. Instead it gets [`KeyBrowser`]: browse keys, then
//! fetch one key's value (which may be a string, list, hash, set, …). When
//! Redis is wired in later, the UI will branch on [`DbKind::Redis`] and use a
//! dedicated key-browser surface rather than the tabular results table.
#![allow(dead_code)] // reserved seam: wired up when Redis support lands.

use anyhow::{bail, Result};

/// A single Redis key as listed in the browser.
#[derive(Debug, Clone)]
pub struct RedisKey {
    pub name: String,
    /// "string" | "list" | "hash" | "set" | "zset" | "stream".
    pub type_name: String,
}

/// The value of one Redis key, rendered for display. Variants mirror Redis'
/// data structures; the UI picks a presentation per variant.
#[derive(Debug, Clone)]
pub enum RedisValue {
    Str(String),
    List(Vec<String>),
    Set(Vec<String>),
    Hash(Vec<(String, String)>),
    /// Sorted set: member + score.
    ZSet(Vec<(String, f64)>),
}

/// Browse a Redis instance. Separate from [`super::DataSource`] on purpose —
/// the key/structure model is too different to share the tabular trait.
#[allow(async_fn_in_trait)]
pub trait KeyBrowser {
    /// Scan keys matching a glob pattern (e.g. `*`, `user:*`).
    async fn scan_keys(&mut self, pattern: &str) -> Result<Vec<RedisKey>>;

    /// Fetch the value of one key.
    async fn get_value(&mut self, key: &str) -> Result<RedisValue>;
}

/// Placeholder Redis source. Construct it to reserve the type; calls fail
/// cleanly until the driver (`redis-rs` / `fred`) is wired in.
pub struct RedisSource {
    pub label: String,
}

impl RedisSource {
    /// Reserved constructor. Does not connect.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl KeyBrowser for RedisSource {
    async fn scan_keys(&mut self, _pattern: &str) -> Result<Vec<RedisKey>> {
        bail!("Redis support is not implemented yet")
    }

    async fn get_value(&mut self, _key: &str) -> Result<RedisValue> {
        bail!("Redis support is not implemented yet")
    }
}
