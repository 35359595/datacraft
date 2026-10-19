//! Oracle driver (rust-oracle over ODPI-C; needs the Oracle Instant Client at runtime).
//! The connection has one database (the service); its schemas are the users that own objects.

use super::{ConnectionConfig, ResultSet, SchemaInfo};

pub struct OraSession {}

impl OraSession {
    pub async fn connect(_cfg: &ConnectionConfig) -> anyhow::Result<Self> {
        anyhow::bail!("Oracle is not implemented yet")
    }

    pub async fn execute(&mut self, _sql: &str, _max_rows: usize) -> anyhow::Result<Vec<ResultSet>> {
        anyhow::bail!("Oracle is not implemented yet")
    }

    pub async fn current_database(&mut self) -> anyhow::Result<String> {
        anyhow::bail!("Oracle is not implemented yet")
    }

    pub async fn list_databases(&mut self) -> anyhow::Result<Vec<String>> {
        anyhow::bail!("Oracle is not implemented yet")
    }

    pub async fn apply_changes(&mut self, _statements: &[String]) -> anyhow::Result<u64> {
        anyhow::bail!("Oracle is not implemented yet")
    }

    pub fn is_closed(&self) -> bool {
        false
    }

    pub async fn introspect(&mut self) -> anyhow::Result<Vec<SchemaInfo>> {
        anyhow::bail!("Oracle is not implemented yet")
    }
}
