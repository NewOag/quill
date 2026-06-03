//! Redis data source backed by `redis-rs` with the async tokio driver.
//!
//! Redis does not fit the rows/columns model — it uses its own [`KeyBrowser`]
//! trait. The `Db` layer exposes a thin Redis API (`scan_keys`, `get_value`)
//! that the UI consumes without ever depending on redis-rs types directly.

use anyhow::{Context as _, Result};
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;

/// A single Redis key as listed in the browser.
#[derive(Debug, Clone)]
pub struct RedisKey {
    pub name: String,
    /// "string" | "list" | "hash" | "set" | "zset" | "stream" | "?".
    pub type_name: String,
}

/// The value of one Redis key serialised as a display string.
/// All variants are flattened to `String` so the UI can pass them through the
/// generic `QueryResult` / detail-panel pipeline without special casing.
#[derive(Debug, Clone)]
pub struct RedisValue {
    pub key: String,
    pub type_name: String,
    /// The value, already formatted for display (JSON-like for collections).
    pub display: String,
}

/// Browse a Redis instance.
#[allow(async_fn_in_trait)]
pub trait KeyBrowser {
    /// Scan keys matching a glob pattern (e.g. `*`, `user:*`), up to `count`
    /// results.
    async fn scan_keys(&mut self, pattern: &str, count: usize) -> Result<Vec<RedisKey>>;

    /// Fetch the value of one key and format it for display.
    async fn get_value(&mut self, key: &str) -> Result<RedisValue>;
}

/// A live Redis connection.
#[derive(Clone)]
pub struct RedisSource {
    pub label: String,
    conn: MultiplexedConnection,
}

impl RedisSource {
    /// Connect to Redis. Must be called from within a tokio runtime context.
    pub async fn connect(url: &str) -> Result<Self> {
        let client =
            redis::Client::open(url).context("invalid Redis URL")?;
        let conn = client
            .get_multiplexed_tokio_connection()
            .await
            .context("failed to connect to Redis")?;
        // Label: extract host from URL.
        let label = url
            .trim_start_matches("redis://")
            .split('/')
            .next()
            .unwrap_or(url)
            .to_string();
        Ok(Self { label, conn })
    }
}

impl KeyBrowser for RedisSource {
    async fn scan_keys(&mut self, pattern: &str, count: usize) -> Result<Vec<RedisKey>> {
        // SCAN with MATCH — collect into a Vec first, then drop the iter so
        // we can borrow self.conn again for the TYPE pipeline.
        let keys: Vec<String> = {
            let mut acc: Vec<String> = Vec::new();
            let mut iter: redis::AsyncIter<String> = self
                .conn
                .scan_match(pattern)
                .await
                .context("SCAN failed")?;
            while let Some(k) = iter.next_item().await {
                acc.push(k);
                if acc.len() >= count {
                    break;
                }
            }
            acc
        };

        // Fetch type for each key via a pipeline for efficiency.
        let mut pipe = redis::pipe();
        for k in &keys {
            pipe.cmd("TYPE").arg(k);
        }
        let types: Vec<String> = pipe
            .query_async(&mut self.conn)
            .await
            .unwrap_or_else(|_| vec!["?".into(); keys.len()]);

        Ok(keys
            .into_iter()
            .zip(types)
            .map(|(name, type_name)| RedisKey { name, type_name })
            .collect())
    }

    async fn get_value(&mut self, key: &str) -> Result<RedisValue> {
        let type_name: String = redis::cmd("TYPE")
            .arg(key)
            .query_async(&mut self.conn)
            .await
            .unwrap_or_else(|_| "?".into());

        let display = match type_name.as_str() {
            "string" => {
                let v: Option<String> = self.conn.get(key).await.ok();
                v.unwrap_or_else(|| "(nil)".into())
            }
            "list" => {
                let v: Vec<String> = self.conn.lrange(key, 0, 99).await.unwrap_or_default();
                format_list(&v)
            }
            "set" => {
                let v: Vec<String> = self.conn.smembers(key).await.unwrap_or_default();
                format_list(&v)
            }
            "hash" => {
                let v: Vec<(String, String)> =
                    self.conn.hgetall(key).await.unwrap_or_default();
                format_hash(&v)
            }
            "zset" => {
                let v: Vec<(String, f64)> = self
                    .conn
                    .zrange_withscores(key, 0isize, 99isize)
                    .await
                    .unwrap_or_default();
                format_zset(&v)
            }
            _ => format!("(type={type_name}, cannot display)"),
        };

        Ok(RedisValue {
            key: key.to_string(),
            type_name,
            display,
        })
    }
}

fn format_list(items: &[String]) -> String {
    let entries: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(i, v)| format!("{i}: {v}"))
        .collect();
    entries.join("\n")
}

fn format_hash(pairs: &[(String, String)]) -> String {
    let entries: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    entries.join("\n")
}

fn format_zset(pairs: &[(String, f64)]) -> String {
    let entries: Vec<String> = pairs
        .iter()
        .map(|(m, s)| format!("{s:.4}  {m}"))
        .collect();
    entries.join("\n")
}
