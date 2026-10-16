//! Data access layer: common types and dispatch to each driver.
//!
//! Every driver converts values to text (`Cell`) so the UI doesn't need to
//! know each engine's native types.

pub mod mssql;
pub mod postgres;
pub mod sqlite;

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Driver {
    Postgres,
    Sqlite,
    MsSql,
}

impl Driver {
    pub const ALL: [Driver; 3] = [Driver::Postgres, Driver::MsSql, Driver::Sqlite];

    pub fn label(self) -> &'static str {
        match self {
            Driver::Postgres => "PostgreSQL",
            Driver::Sqlite => "SQLite",
            Driver::MsSql => "SQL Server",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Driver::Postgres => 5432,
            Driver::MsSql => 1433,
            Driver::Sqlite => 0,
        }
    }
}

/// Persisted connection settings. The password is NOT stored here: it goes to the
/// macOS Keychain (see `config.rs`) and is filled into `password` in memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionConfig {
    pub id: u64,
    pub name: String,
    pub driver: Driver,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    #[serde(skip)]
    pub password: String,
    /// File path for SQLite.
    pub file_path: String,
    /// SQL Server: accept self-signed certificates.
    pub trust_cert: bool,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self::new(0, Driver::Postgres)
    }
}

impl ConnectionConfig {
    pub fn new(id: u64, driver: Driver) -> Self {
        Self {
            id,
            name: format!("{} {}", driver.label(), id),
            driver,
            host: "localhost".into(),
            port: driver.default_port(),
            database: match driver {
                Driver::Postgres => "postgres".into(),
                Driver::MsSql => "master".into(),
                Driver::Sqlite => String::new(),
            },
            user: match driver {
                Driver::Postgres => "postgres".into(),
                Driver::MsSql => "sa".into(),
                Driver::Sqlite => String::new(),
            },
            password: String::new(),
            file_path: String::new(),
            trust_cert: true,
        }
    }
}

/// Already formatted cell value. `None` = NULL.
pub type Cell = Option<String>;

/// Result of ONE statement.
#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    /// Statement that produced it (for the title of the results tab).
    pub statement: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    /// Affected rows (DML) if the driver knows them.
    pub affected: Option<u64>,
    /// true if rows were cut off on reaching `max_rows`.
    pub truncated: bool,
    pub elapsed: Duration,
}

impl ResultSet {
    /// Statements without columns (INSERT, CREATE...).
    pub fn is_command(&self) -> bool {
        self.columns.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Table,
    View,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

#[derive(Debug, Clone)]
pub struct TableInfo {
    pub name: String,
    pub kind: ObjectKind,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone)]
pub struct SchemaInfo {
    pub name: String,
    pub tables: Vec<TableInfo>,
}

/// Server database. `schemas` is `None` until it is loaded (when expanded).
#[derive(Debug, Clone)]
pub struct DatabaseInfo {
    pub name: String,
    /// Default database of the connection.
    pub current: bool,
    pub schemas: Option<Vec<SchemaInfo>>,
}

/// Standard error when a change does not affect exactly one row.
pub(crate) fn not_single_row(n: u64, stmt: &str) -> anyhow::Error {
    anyhow::anyhow!(crate::i18n::trf(
        "The statement affected {n} rows (expected 1); nothing was saved:\n{stmt}",
        &[("n", &n), ("stmt", &stmt)]
    ))
}

/// Open session against a specific engine.
pub enum Session {
    Postgres(postgres::PgSession),
    Sqlite(sqlite::SqliteSession),
    MsSql(Box<mssql::MsSession>),
}

impl Session {
    pub async fn connect(cfg: &ConnectionConfig) -> anyhow::Result<Session> {
        Ok(match cfg.driver {
            Driver::Postgres => Session::Postgres(postgres::PgSession::connect(cfg).await?),
            Driver::Sqlite => Session::Sqlite(sqlite::SqliteSession::connect(cfg).await?),
            Driver::MsSql => Session::MsSql(Box::new(mssql::MsSession::connect(cfg).await?)),
        })
    }

    /// Runs ONE statement (or a T-SQL batch) and returns one or more result sets.
    /// Each result set keeps at most `max_rows` rows.
    pub async fn execute(&mut self, sql: &str, max_rows: usize) -> anyhow::Result<Vec<ResultSet>> {
        match self {
            Session::Postgres(s) => s.execute(sql, max_rows).await,
            Session::Sqlite(s) => s.execute(sql, max_rows).await,
            Session::MsSql(s) => s.execute(sql, max_rows).await,
        }
    }

    /// Database the session is connected to when opened.
    pub async fn current_database(&mut self) -> anyhow::Result<String> {
        match self {
            Session::Postgres(s) => s.current_database().await,
            Session::Sqlite(_) => Ok("main".into()),
            Session::MsSql(s) => s.current_database().await,
        }
    }

    /// Databases visible on the server (SQLite: just "main").
    pub async fn list_databases(&mut self) -> anyhow::Result<Vec<String>> {
        match self {
            Session::Postgres(s) => s.list_databases().await,
            Session::Sqlite(_) => Ok(vec!["main".into()]),
            Session::MsSql(s) => s.list_databases().await,
        }
    }

    /// Schema of `db`. On PostgreSQL the session must already be connected to that database.
    pub async fn introspect_database(&mut self, db: &str) -> anyhow::Result<Vec<SchemaInfo>> {
        self.use_database(db).await?;
        self.introspect().await
    }

    /// SQL Server: `USE [db]`. A no-op on the other engines (each session is bound to one database).
    pub async fn use_database(&mut self, db: &str) -> anyhow::Result<()> {
        match self {
            Session::MsSql(s) if !db.is_empty() => s.use_database(db).await,
            _ => Ok(()),
        }
    }

    /// Applies data editor changes in ONE transaction. Each statement must affect exactly
    /// one row; if any fails or affects 0 or several, everything is rolled back.
    /// Returns the total number of modified rows.
    pub async fn apply_changes(&mut self, statements: &[String]) -> anyhow::Result<u64> {
        match self {
            Session::Postgres(s) => s.apply_changes(statements).await,
            Session::Sqlite(s) => s.apply_changes(statements).await,
            Session::MsSql(s) => s.apply_changes(statements).await,
        }
    }

    /// true if the connection has been lost (socket closed, protocol error...).
    pub fn is_closed(&self) -> bool {
        match self {
            Session::Postgres(s) => s.is_closed(),
            Session::Sqlite(_) => false,
            Session::MsSql(s) => s.is_closed(),
        }
    }

    /// Schemas, tables/views and columns (no system objects).
    pub async fn introspect(&mut self) -> anyhow::Result<Vec<SchemaInfo>> {
        match self {
            Session::Postgres(s) => s.introspect().await,
            Session::Sqlite(s) => s.introspect().await,
            Session::MsSql(s) => s.introspect().await,
        }
    }

    /// Qualified name quoted for the dialect, e.g. `"public"."users"` or `[dbo].[users]`.
    pub fn quote_table(driver: Driver, schema: &str, table: &str) -> String {
        match driver {
            Driver::Postgres => format!("\"{}\".\"{}\"", schema.replace('"', "\"\""), table.replace('"', "\"\"")),
            Driver::Sqlite => format!("\"{}\"", table.replace('"', "\"\"")),
            Driver::MsSql => format!("[{}].[{}]", schema.replace(']', "]]"), table.replace(']', "]]")),
        }
    }
}
