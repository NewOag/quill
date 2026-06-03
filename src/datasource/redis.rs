//! Redis data source backed by `redis-rs` with the async tokio driver.
//!
//! The data model here is deliberately richer than the SQL layer — Redis values
//! are typed (string/list/hash/set/zset) and are returned as structured
//! [`RedisValue`] variants so the UI can render each differently.

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

/// Structured value of one Redis key.
#[derive(Debug, Clone)]
pub enum RedisValue {
    Str(String),
    List(Vec<String>),
    Set(Vec<String>),
    Hash(Vec<(String, String)>),
    /// Sorted set: (member, score) pairs, ordered by score.
    ZSet(Vec<(String, f64)>),
    /// Unknown or unsupported type.
    Unknown(String),
}

/// Full detail for one Redis key, including TTL.
#[derive(Debug, Clone)]
pub struct RedisKeyDetail {
    pub key: String,
    pub type_name: String,
    /// Remaining TTL in seconds. -1 = no expiry, -2 = key not found.
    pub ttl: i64,
    pub value: RedisValue,
}

impl RedisKeyDetail {
    /// A short human-readable TTL string for the UI.
    pub fn ttl_label(&self) -> String {
        match self.ttl {
            -2 => "not found".into(),
            -1 => "no expiry".into(),
            s if s < 60 => format!("{s}s"),
            s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
            s => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        }
    }
}

/// Browse a Redis instance.
#[allow(async_fn_in_trait)]
pub trait KeyBrowser {
    /// Scan keys matching a glob pattern, up to `count` results.
    async fn scan_keys(&mut self, pattern: &str, count: usize) -> Result<Vec<RedisKey>>;

    /// Fetch the typed value + TTL for one key.
    async fn get_value(&mut self, key: &str) -> Result<RedisKeyDetail>;

    /// Execute an arbitrary Redis command line (space-separated tokens) and
    /// return the response as a display string.
    async fn exec_cmd(&mut self, cmd_line: &str) -> Result<String>;
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
        let client = redis::Client::open(url).context("invalid Redis URL")?;
        let conn = client
            .get_multiplexed_tokio_connection()
            .await
            .context("failed to connect to Redis")?;
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

        // Fetch types via pipeline.
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

    async fn get_value(&mut self, key: &str) -> Result<RedisKeyDetail> {
        let type_name: String = redis::cmd("TYPE")
            .arg(key)
            .query_async(&mut self.conn)
            .await
            .unwrap_or_else(|_| "?".into());

        let ttl: i64 = redis::cmd("TTL")
            .arg(key)
            .query_async(&mut self.conn)
            .await
            .unwrap_or(-1);

        let value = match type_name.as_str() {
            "string" => {
                let v: Option<String> = self.conn.get(key).await.ok().flatten();
                RedisValue::Str(v.unwrap_or_else(|| "(nil)".into()))
            }
            "list" => {
                let v: Vec<String> = self.conn.lrange(key, 0, 499).await.unwrap_or_default();
                RedisValue::List(v)
            }
            "set" => {
                let v: Vec<String> = self.conn.smembers(key).await.unwrap_or_default();
                RedisValue::Set(v)
            }
            "hash" => {
                let v: Vec<(String, String)> =
                    self.conn.hgetall(key).await.unwrap_or_default();
                RedisValue::Hash(v)
            }
            "zset" => {
                let v: Vec<(String, f64)> = self
                    .conn
                    .zrange_withscores(key, 0isize, 499isize)
                    .await
                    .unwrap_or_default();
                RedisValue::ZSet(v)
            }
            other => RedisValue::Unknown(format!("unsupported type: {other}")),
        };

        Ok(RedisKeyDetail {
            key: key.to_string(),
            type_name,
            ttl,
            value,
        })
    }

    async fn exec_cmd(&mut self, cmd_line: &str) -> Result<String> {
        let tokens: Vec<&str> = cmd_line.split_whitespace().collect();
        if tokens.is_empty() {
            return Ok(String::new());
        }
        let mut cmd = redis::cmd(tokens[0]);
        for arg in &tokens[1..] {
            cmd.arg(arg);
        }
        let value: redis::Value = cmd
            .query_async(&mut self.conn)
            .await
            .context("command failed")?;
        Ok(redis_value_to_string(&value))
    }
}

/// Render a redis::Value as a human-readable string (redis 0.25 variants).
fn redis_value_to_string(v: &redis::Value) -> String {
    match v {
        redis::Value::Nil => "(nil)".into(),
        redis::Value::Int(n) => format!("(integer) {n}"),
        redis::Value::Data(b) => String::from_utf8_lossy(b).into_owned(),
        redis::Value::Status(s) => s.clone(),
        redis::Value::Okay => "OK".into(),
        redis::Value::Bulk(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| format!("{}) {}", i + 1, redis_value_to_string(v)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
