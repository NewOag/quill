//! Saved connections and their persistence.
//!
//! A [`ConnectionConfig`] is the discrete-field description of one database
//! connection (host, port, user, …). We store the *parts*, not a URL string,
//! so the connection form can edit them field-by-field; the mysql_async URL is
//! built on demand via [`ConnectionConfig::mysql_url`] (with percent-encoding so
//! a password containing `@`, `:`, `/`, or `#` can't corrupt the URL).
//!
//! [`ConnectionStore`] is the whole persisted set, saved as pretty JSON under
//! the platform config dir (`dirs::config_dir()/quill/connections.json`; on
//! macOS that's `~/Library/Application Support/quill/`). Loading never panics:
//! a missing or unparseable file yields an empty store.
//!
//! Password storage is **plaintext JSON** for now (see the field doc). On Unix
//! the file is written with `0600` perms as a cheap mitigation. A system
//! keychain is a planned follow-up that won't change this type's shape.

use std::fs;
use std::path::PathBuf;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::datasource::DbKind;

/// One saved connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Stable id, used as the tab key and for `last_open`.
    pub id: Uuid,
    /// User-facing label (tab title).
    pub name: String,
    pub kind: DbKind,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// NOTE: stored in plaintext for now. See module docs.
    pub password: String,
    /// May be empty (connect without selecting a default database).
    pub database: String,
}

impl ConnectionConfig {
    /// Create a config with a fresh id.
    pub fn new(
        name: String,
        kind: DbKind,
        host: String,
        port: u16,
        user: String,
        password: String,
        database: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            kind,
            host,
            port,
            user,
            password,
            database,
        }
    }

    /// Build a `mysql_async`-compatible URL from the parts. User, password and
    /// database are percent-encoded so reserved characters are safe.
    pub fn mysql_url(&self) -> String {
        let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
        format!(
            "mysql://{}:{}@{}:{}/{}",
            enc(&self.user),
            enc(&self.password),
            self.host,
            self.port,
            enc(&self.database),
        )
    }

    /// Build a `redis-rs`-compatible URL.
    pub fn redis_url(&self) -> String {
        let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
        if self.password.is_empty() {
            format!("redis://{}:{}/{}", self.host, self.port, enc(&self.database))
        } else {
            format!("redis://:{}@{}:{}/{}", enc(&self.password), self.host, self.port, enc(&self.database))
        }
    }

    /// Build a `tokio-postgres`-compatible connection string.
    pub fn postgres_url(&self) -> String {
        let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
        format!(
            "postgresql://{}:{}@{}:{}/{}",
            enc(&self.user),
            enc(&self.password),
            self.host,
            self.port,
            enc(&self.database),
        )
    }
}

/// The full set of saved connections plus which one was last open.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionStore {
    pub connections: Vec<ConnectionConfig>,
    /// The connection open when the app last closed, to auto-reopen.
    pub last_open: Option<Uuid>,
}

impl ConnectionStore {
    /// Path to the config file, or `None` if no config dir is available.
    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("quill").join("connections.json"))
    }

    /// Load the store. A missing or unparseable file yields an empty store —
    /// never panics.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("quill: ignoring unreadable config ({e}); starting empty");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Persist the store. Errors are logged, never fatal. The file is written
    /// `0600` on Unix.
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            if let Err(e) = fs::create_dir_all(dir) {
                eprintln!("quill: could not create config dir: {e}");
                return;
            }
        }
        let json = match serde_json::to_string_pretty(self) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("quill: could not serialize config: {e}");
                return;
            }
        };
        if let Err(e) = fs::write(&path, json) {
            eprintln!("quill: could not write config: {e}");
            return;
        }
        restrict_permissions(&path);
    }

    /// Find a connection by id.
    pub fn find(&self, id: Uuid) -> Option<&ConnectionConfig> {
        self.connections.iter().find(|c| c.id == id)
    }

    /// Replace a saved connection in-place (edit). No-op if id not found.
    pub fn update(&mut self, id: Uuid, new_config: ConnectionConfig) {
        if let Some(existing) = self.connections.iter_mut().find(|c| c.id == id) {
            *existing = new_config;
        }
    }

    /// Remove a saved connection. Clears `last_open` if it pointed here.
    pub fn remove(&mut self, id: Uuid) {
        self.connections.retain(|c| c.id != id);
        if self.last_open == Some(id) {
            self.last_open = None;
        }
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        eprintln!("quill: could not set config perms: {e}");
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ConnectionConfig {
        ConnectionConfig::new(
            "local".into(),
            DbKind::Mysql,
            "127.0.0.1".into(),
            3306,
            "root".into(),
            "p@ss:w/rd#1".into(),
            "demo".into(),
        )
    }

    #[test]
    fn mysql_url_percent_encodes_reserved_chars() {
        let url = sample().mysql_url();
        // The reserved chars in the password must be escaped, not literal.
        assert!(url.starts_with("mysql://root:"));
        assert!(url.contains("@127.0.0.1:3306/demo"));
        assert!(!url.contains("p@ss:w/rd#1"));
        assert!(url.contains("p%40ss%3Aw%2Frd%231"));
    }

    #[test]
    fn store_roundtrips_through_json() {
        let store = ConnectionStore {
            connections: vec![sample()],
            last_open: Some(sample().id),
        };
        let json = serde_json::to_string(&store).unwrap();
        let back: ConnectionStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.connections.len(), 1);
        assert_eq!(back.connections[0].name, "local");
        assert_eq!(back.connections[0].port, 3306);
        assert_eq!(back.connections[0].kind, DbKind::Mysql);
    }

    #[test]
    fn dbkind_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&DbKind::Mysql).unwrap(), "\"mysql\"");
    }
}
