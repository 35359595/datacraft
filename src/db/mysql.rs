//! MySQL and MariaDB driver (mysql_async). One session per connection; `USE db` switches
//! databases. In the explorer each database has a single schema with its own name.

use super::{ConnectionConfig, ResultSet, SchemaInfo};

pub struct MySession {}

impl MySession {
    pub async fn connect(_cfg: &ConnectionConfig) -> anyhow::Result<Self> {
        anyhow::bail!("MySQL is not implemented yet")
    }

    pub async fn execute(&mut self, _sql: &str, _max_rows: usize) -> anyhow::Result<Vec<ResultSet>> {
        anyhow::bail!("MySQL is not implemented yet")
    }

    pub async fn current_database(&mut self) -> anyhow::Result<String> {
        anyhow::bail!("MySQL is not implemented yet")
    }

    pub async fn list_databases(&mut self) -> anyhow::Result<Vec<String>> {
        anyhow::bail!("MySQL is not implemented yet")
    }

    pub async fn use_database(&mut self, _db: &str) -> anyhow::Result<()> {
        anyhow::bail!("MySQL is not implemented yet")
    }

    pub async fn apply_changes(&mut self, _statements: &[String]) -> anyhow::Result<u64> {
        anyhow::bail!("MySQL is not implemented yet")
    }

    pub fn is_closed(&self) -> bool {
        false
    }

    pub async fn introspect(&mut self) -> anyhow::Result<Vec<SchemaInfo>> {
        anyhow::bail!("MySQL is not implemented yet")
    }
}
