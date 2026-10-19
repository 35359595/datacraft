//! Data access layer: common types and dispatch to each driver.
//!
//! Every driver converts values to text (`Cell`) so the UI doesn't need to
//! know each engine's native types.

pub mod mssql;
pub mod mysql;
pub mod oracle;
pub mod postgres;
pub mod sqlite;

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Driver {
    Postgres,
    Sqlite,
    MsSql,
    MySql,
    MariaDb,
    Oracle,
}

impl Driver {
    pub const ALL: [Driver; 6] = [Driver::Postgres, Driver::MySql, Driver::MariaDb, Driver::MsSql, Driver::Oracle, Driver::Sqlite];

    pub fn label(self) -> &'static str {
        match self {
            Driver::Postgres => "PostgreSQL",
            Driver::Sqlite => "SQLite",
            Driver::MsSql => "SQL Server",
            Driver::MySql => "MySQL",
            Driver::MariaDb => "MariaDB",
            Driver::Oracle => "Oracle",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Driver::Postgres => 5432,
            Driver::MsSql => 1433,
            Driver::MySql | Driver::MariaDb => 3306,
            Driver::Oracle => 1521,
            Driver::Sqlite => 0,
        }
    }

    /// Engines where each database has a single schema with its own name (SQLite: "main"),
    /// shown flattened in the explorer.
    pub fn flat_schema(self) -> bool {
        matches!(self, Driver::Sqlite | Driver::MySql | Driver::MariaDb)
    }

    /// Engines whose server has several databases that can be created and dropped from the app.
    pub fn has_databases(self) -> bool {
        !matches!(self, Driver::Sqlite | Driver::Oracle)
    }

    /// Identifier quoted for the dialect.
    pub fn quote_ident(self, name: &str) -> String {
        match self {
            Driver::MsSql => format!("[{}]", name.replace(']', "]]")),
            Driver::MySql | Driver::MariaDb => format!("`{}`", name.replace('`', "``")),
            Driver::Postgres | Driver::Sqlite | Driver::Oracle => format!("\"{}\"", name.replace('"', "\"\"")),
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
    /// DataGrip's "Read-only": the data editor can't save and consoles only run queries.
    pub read_only: bool,
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
                // Oracle: service name of the pluggable database.
                Driver::Oracle => "FREEPDB1".into(),
                Driver::Sqlite | Driver::MySql | Driver::MariaDb => String::new(),
            },
            user: match driver {
                Driver::Postgres => "postgres".into(),
                Driver::MsSql => "sa".into(),
                Driver::MySql | Driver::MariaDb => "root".into(),
                Driver::Oracle => "system".into(),
                Driver::Sqlite => String::new(),
            },
            password: String::new(),
            file_path: String::new(),
            trust_cert: true,
            read_only: false,
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

/// Cancels the query a session is running, from outside that session.
#[derive(Clone)]
pub enum CancelHandle {
    Postgres(tokio_postgres::CancelToken),
    Sqlite(std::sync::Arc<rusqlite::InterruptHandle>),
    /// MySQL / MariaDB: `KILL QUERY <thread id>` from a second connection.
    MySql(Box<mysql_async::OptsBuilder>, u32),
    /// Oracle: `break_execution` on the same connection.
    Oracle(std::sync::Arc<::oracle::Connection>),
}

impl CancelHandle {
    pub async fn cancel(&self) -> anyhow::Result<()> {
        match self {
            CancelHandle::Postgres(t) => t.cancel_query(tokio_postgres::NoTls).await.map_err(anyhow::Error::from),
            CancelHandle::Sqlite(h) => {
                h.interrupt();
                Ok(())
            }
            CancelHandle::Oracle(conn) => {
                let conn = std::sync::Arc::clone(conn);
                tokio::task::spawn_blocking(move || conn.break_execution()).await??;
                Ok(())
            }
            CancelHandle::MySql(opts, id) => {
                use mysql_async::prelude::Queryable;
                let mut conn = mysql_async::Conn::new((**opts).clone()).await?;
                conn.query_drop(format!("KILL QUERY {id}")).await?;
                conn.disconnect().await?;
                Ok(())
            }
        }
    }
}

/// Open session against a specific engine.
pub enum Session {
    Postgres(postgres::PgSession),
    Sqlite(sqlite::SqliteSession),
    MsSql(Box<mssql::MsSession>),
    /// MySQL and MariaDB.
    MySql(Box<mysql::MySession>),
    Oracle(Box<oracle::OraSession>),
}

impl Session {
    pub async fn connect(cfg: &ConnectionConfig) -> anyhow::Result<Session> {
        let mut session = Self::open(cfg).await?;
        // Read-only data sources are also enforced by the server where the engine allows it
        // (SQLite opens the file read-only; SQL Server and Oracle rely on the app's checks).
        if cfg.read_only {
            let set = match cfg.driver {
                Driver::Postgres => Some("SET default_transaction_read_only = on"),
                Driver::MySql | Driver::MariaDb => Some("SET SESSION TRANSACTION READ ONLY"),
                _ => None,
            };
            if let Some(set) = set {
                session.execute(set, 0).await?;
            }
        }
        Ok(session)
    }

    async fn open(cfg: &ConnectionConfig) -> anyhow::Result<Session> {
        Ok(match cfg.driver {
            Driver::Postgres => Session::Postgres(postgres::PgSession::connect(cfg).await?),
            Driver::Sqlite => Session::Sqlite(sqlite::SqliteSession::connect(cfg).await?),
            Driver::MsSql => Session::MsSql(Box::new(mssql::MsSession::connect(cfg).await?)),
            Driver::MySql | Driver::MariaDb => Session::MySql(Box::new(mysql::MySession::connect(cfg).await?)),
            Driver::Oracle => Session::Oracle(Box::new(oracle::OraSession::connect(cfg).await?)),
        })
    }

    /// Runs ONE statement (or a T-SQL batch) and returns one or more result sets.
    /// Each result set keeps at most `max_rows` rows.
    pub async fn execute(&mut self, sql: &str, max_rows: usize) -> anyhow::Result<Vec<ResultSet>> {
        match self {
            Session::Postgres(s) => s.execute(sql, max_rows).await,
            Session::Sqlite(s) => s.execute(sql, max_rows).await,
            Session::MsSql(s) => s.execute(sql, max_rows).await,
            Session::MySql(s) => s.execute(sql, max_rows).await,
            Session::Oracle(s) => s.execute(sql, max_rows).await,
        }
    }

    /// Database the session is connected to when opened.
    pub async fn current_database(&mut self) -> anyhow::Result<String> {
        match self {
            Session::Postgres(s) => s.current_database().await,
            Session::Sqlite(_) => Ok("main".into()),
            Session::MsSql(s) => s.current_database().await,
            Session::MySql(s) => s.current_database().await,
            Session::Oracle(s) => s.current_database().await,
        }
    }

    /// Databases visible on the server (SQLite: just "main").
    pub async fn list_databases(&mut self) -> anyhow::Result<Vec<String>> {
        match self {
            Session::Postgres(s) => s.list_databases().await,
            Session::Sqlite(_) => Ok(vec!["main".into()]),
            Session::MsSql(s) => s.list_databases().await,
            Session::MySql(s) => s.list_databases().await,
            Session::Oracle(s) => s.list_databases().await,
        }
    }

    /// Schema of `db`. On PostgreSQL the session must already be connected to that database.
    pub async fn introspect_database(&mut self, db: &str) -> anyhow::Result<Vec<SchemaInfo>> {
        self.use_database(db).await?;
        self.introspect().await
    }

    /// SQL Server and MySQL: `USE db`. A no-op on the other engines (each session is bound to one database).
    pub async fn use_database(&mut self, db: &str) -> anyhow::Result<()> {
        match self {
            Session::MsSql(s) if !db.is_empty() => s.use_database(db).await,
            Session::MySql(s) if !db.is_empty() => s.use_database(db).await,
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
            Session::MySql(s) => s.apply_changes(statements).await,
            Session::Oracle(s) => s.apply_changes(statements).await,
        }
    }

    /// Native way to cancel this session's running query, if the engine has one. Without it
    /// the worker abandons the query and closes the session.
    pub fn cancel_handle(&self) -> Option<CancelHandle> {
        match self {
            Session::Postgres(s) => Some(CancelHandle::Postgres(s.cancel_token())),
            Session::Sqlite(s) => Some(CancelHandle::Sqlite(s.interrupt_handle())),
            Session::MySql(s) => {
                let (opts, id) = s.cancel_target();
                Some(CancelHandle::MySql(Box::new(opts), id))
            }
            Session::Oracle(s) => Some(CancelHandle::Oracle(s.break_handle())),
            _ => None,
        }
    }

    /// true if the connection has been lost (socket closed, protocol error...).
    pub fn is_closed(&self) -> bool {
        match self {
            Session::Postgres(s) => s.is_closed(),
            Session::Sqlite(_) => false,
            Session::MsSql(s) => s.is_closed(),
            Session::MySql(s) => s.is_closed(),
            Session::Oracle(s) => s.is_closed(),
        }
    }

    /// Schemas, tables/views and columns (no system objects).
    pub async fn introspect(&mut self) -> anyhow::Result<Vec<SchemaInfo>> {
        match self {
            Session::Postgres(s) => s.introspect().await,
            Session::Sqlite(s) => s.introspect().await,
            Session::MsSql(s) => s.introspect().await,
            Session::MySql(s) => s.introspect().await,
            Session::Oracle(s) => s.introspect().await,
        }
    }

    /// Qualified name quoted for the dialect, e.g. `"public"."users"` or `[dbo].[users]`.
    pub fn quote_table(driver: Driver, schema: &str, table: &str) -> String {
        match driver {
            Driver::Postgres => format!("\"{}\".\"{}\"", schema.replace('"', "\"\""), table.replace('"', "\"\"")),
            Driver::Sqlite => format!("\"{}\"", table.replace('"', "\"\"")),
            Driver::MsSql => format!("[{}].[{}]", schema.replace(']', "]]"), table.replace(']', "]]")),
            // MySQL: the schema is the database.
            Driver::MySql | Driver::MariaDb | Driver::Oracle => format!("{}.{}", driver.quote_ident(schema), driver.quote_ident(table)),
        }
    }
}
