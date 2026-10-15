//! Capa de acceso a datos: tipos comunes y despacho a cada driver.
//!
//! Todos los drivers convierten los valores a texto (`Cell`) para que la UI
//! no tenga que conocer los tipos nativos de cada motor.

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

/// Configuración persistida de una conexión. La contraseña NO se guarda aquí,
/// va al llavero de macOS (ver `config.rs`) y se rellena en `password` en memoria.
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
    /// Ruta del fichero para SQLite.
    pub file_path: String,
    /// SQL Server: aceptar certificados autofirmados.
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

/// Valor de una celda ya formateado. `None` = NULL.
pub type Cell = Option<String>;

/// Resultado de UNA sentencia.
#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    /// Sentencia que lo produjo (para el título de la pestaña de resultados).
    pub statement: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    /// Filas afectadas (DML) si el driver lo sabe.
    pub affected: Option<u64>,
    /// true si se cortaron filas por llegar a `max_rows`.
    pub truncated: bool,
    pub elapsed: Duration,
}

impl ResultSet {
    /// Sentencias sin columnas (INSERT, CREATE...).
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

/// Base de datos del servidor. `schemas` es `None` hasta que se carga (al expandirla).
#[derive(Debug, Clone)]
pub struct DatabaseInfo {
    pub name: String,
    /// Base por defecto de la conexión.
    pub current: bool,
    pub schemas: Option<Vec<SchemaInfo>>,
}

/// Error estándar cuando un cambio no afecta exactamente a una fila.
pub(crate) fn not_single_row(n: u64, stmt: &str) -> anyhow::Error {
    anyhow::anyhow!("La sentencia afectó a {n} filas (se esperaba 1); no se ha guardado nada:\n{stmt}")
}

/// Sesión abierta contra un motor concreto.
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

    /// Ejecuta UNA sentencia (o un lote T-SQL) y devuelve uno o varios result sets.
    /// Cada result set guarda como mucho `max_rows` filas.
    pub async fn execute(&mut self, sql: &str, max_rows: usize) -> anyhow::Result<Vec<ResultSet>> {
        match self {
            Session::Postgres(s) => s.execute(sql, max_rows).await,
            Session::Sqlite(s) => s.execute(sql, max_rows).await,
            Session::MsSql(s) => s.execute(sql, max_rows).await,
        }
    }

    /// Base a la que está conectada la sesión al abrirla.
    pub async fn current_database(&mut self) -> anyhow::Result<String> {
        match self {
            Session::Postgres(s) => s.current_database().await,
            Session::Sqlite(_) => Ok("main".into()),
            Session::MsSql(s) => s.current_database().await,
        }
    }

    /// Bases visibles en el servidor (SQLite: solo "main").
    pub async fn list_databases(&mut self) -> anyhow::Result<Vec<String>> {
        match self {
            Session::Postgres(s) => s.list_databases().await,
            Session::Sqlite(_) => Ok(vec!["main".into()]),
            Session::MsSql(s) => s.list_databases().await,
        }
    }

    /// Esquema de `db`. En PostgreSQL la sesión ya debe estar conectada a esa base.
    pub async fn introspect_database(&mut self, db: &str) -> anyhow::Result<Vec<SchemaInfo>> {
        self.use_database(db).await?;
        self.introspect().await
    }

    /// SQL Server: `USE [db]`. En los demás motores no hace nada (cada sesión es de una base).
    pub async fn use_database(&mut self, db: &str) -> anyhow::Result<()> {
        match self {
            Session::MsSql(s) if !db.is_empty() => s.use_database(db).await,
            _ => Ok(()),
        }
    }

    /// Aplica cambios del editor de datos en UNA transacción. Cada sentencia debe afectar
    /// exactamente a una fila; si alguna falla o afecta a 0 o varias, se deshace todo.
    /// Devuelve el total de filas modificadas.
    pub async fn apply_changes(&mut self, statements: &[String]) -> anyhow::Result<u64> {
        match self {
            Session::Postgres(s) => s.apply_changes(statements).await,
            Session::Sqlite(s) => s.apply_changes(statements).await,
            Session::MsSql(s) => s.apply_changes(statements).await,
        }
    }

    /// true si la conexión se ha perdido (socket cerrado, error de protocolo...).
    pub fn is_closed(&self) -> bool {
        match self {
            Session::Postgres(s) => s.is_closed(),
            Session::Sqlite(_) => false,
            Session::MsSql(s) => s.is_closed(),
        }
    }

    /// Esquemas, tablas/vistas y columnas (sin objetos de sistema).
    pub async fn introspect(&mut self) -> anyhow::Result<Vec<SchemaInfo>> {
        match self {
            Session::Postgres(s) => s.introspect().await,
            Session::Sqlite(s) => s.introspect().await,
            Session::MsSql(s) => s.introspect().await,
        }
    }

    /// Nombre cualificado y entrecomillado según el dialecto, p.ej. `"public"."users"` o `[dbo].[users]`.
    pub fn quote_table(driver: Driver, schema: &str, table: &str) -> String {
        match driver {
            Driver::Postgres => format!("\"{}\".\"{}\"", schema.replace('"', "\"\""), table.replace('"', "\"\"")),
            Driver::Sqlite => format!("\"{}\"", table.replace('"', "\"\"")),
            Driver::MsSql => format!("[{}].[{}]", schema.replace(']', "]]"), table.replace(']', "]]")),
        }
    }
}
