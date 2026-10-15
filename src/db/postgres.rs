//! Driver PostgreSQL sobre `tokio-postgres` (sin TLS).
//!
//! Se usa el protocolo simple: los valores llegan ya como texto y admite
//! varias sentencias separadas por `;` en una sola llamada.

use std::time::{Duration, Instant};

use anyhow::anyhow;
use futures_util::TryStreamExt;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use super::{ColumnInfo, ConnectionConfig, ObjectKind, ResultSet, SchemaInfo, TableInfo};

pub struct PgSession {
    client: Client,
}

/// Traduce un error de tokio-postgres a un mensaje legible (mensaje + detalle + pista).
fn pg_error(e: tokio_postgres::Error) -> anyhow::Error {
    match e.as_db_error() {
        Some(db) => {
            let mut msg = format!("{}: {}", db.severity(), db.message());
            if let Some(detail) = db.detail() {
                msg.push_str(&format!("\nDetalle: {detail}"));
            }
            if let Some(hint) = db.hint() {
                msg.push_str(&format!("\nSugerencia: {hint}"));
            }
            anyhow!(msg)
        }
        None => anyhow!(e.to_string()),
    }
}

impl PgSession {
    pub async fn connect(cfg: &ConnectionConfig) -> anyhow::Result<Self> {
        let mut pg = tokio_postgres::Config::new();
        pg.host(if cfg.host.trim().is_empty() { "localhost" } else { cfg.host.trim() })
            .port(if cfg.port == 0 { 5432 } else { cfg.port })
            .user(&cfg.user)
            .password(&cfg.password)
            .application_name("DataCraft")
            .connect_timeout(Duration::from_secs(10));
        if !cfg.database.trim().is_empty() {
            pg.dbname(cfg.database.trim());
        }

        let (client, connection) = pg.connect(NoTls).await.map_err(pg_error)?;
        // La conexión es la que realmente lee/escribe el socket; debe vivir en su propia tarea.
        tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(Self { client })
    }

    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    pub async fn execute(&mut self, sql: &str, max_rows: usize) -> anyhow::Result<Vec<ResultSet>> {
        let statement = sql.trim().to_string();
        let mut stream = std::pin::pin!(self.client.simple_query_raw(sql).await.map_err(pg_error)?);

        let mut results = Vec::new();
        // Result set en curso (abierto por un RowDescription).
        let mut current: Option<ResultSet> = None;
        let mut start = Instant::now();

        while let Some(msg) = stream.try_next().await.map_err(pg_error)? {
            match msg {
                SimpleQueryMessage::RowDescription(cols) => {
                    current = Some(ResultSet {
                        statement: statement.clone(),
                        columns: cols.iter().map(|c| c.name().to_string()).collect(),
                        ..Default::default()
                    });
                }
                SimpleQueryMessage::Row(row) => {
                    let Some(rs) = current.as_mut() else { continue };
                    if rs.rows.len() >= max_rows {
                        rs.truncated = true;
                        continue;
                    }
                    let cells = (0..row.len())
                        .map(|i| row.try_get(i).ok().flatten().map(str::to_string))
                        .collect();
                    rs.rows.push(cells);
                }
                SimpleQueryMessage::CommandComplete(n) => {
                    let mut rs = match current.take() {
                        Some(rs) => rs,
                        None => ResultSet {
                            statement: statement.clone(),
                            affected: Some(n),
                            ..Default::default()
                        },
                    };
                    rs.elapsed = start.elapsed();
                    results.push(rs);
                    start = Instant::now();
                }
                _ => {}
            }
        }
        // Por si el servidor no envió CommandComplete tras la descripción.
        if let Some(mut rs) = current {
            rs.elapsed = start.elapsed();
            results.push(rs);
        }
        Ok(results)
    }

    pub async fn introspect(&mut self) -> anyhow::Result<Vec<SchemaInfo>> {
        const SQL: &str = r#"
            SELECT t.table_schema, t.table_name, t.table_type,
                   c.column_name, c.data_type, c.udt_name, c.is_nullable,
                   CASE WHEN pk.column_name IS NULL THEN 'N' ELSE 'Y' END AS is_pk
            FROM information_schema.tables t
            LEFT JOIN information_schema.columns c
                   ON c.table_schema = t.table_schema AND c.table_name = t.table_name
            LEFT JOIN (
                SELECT kcu.table_schema, kcu.table_name, kcu.column_name
                FROM information_schema.table_constraints tc
                JOIN information_schema.key_column_usage kcu
                  ON kcu.constraint_schema = tc.constraint_schema
                 AND kcu.constraint_name = tc.constraint_name
                 AND kcu.table_name = tc.table_name
                WHERE tc.constraint_type = 'PRIMARY KEY'
            ) pk ON pk.table_schema = c.table_schema AND pk.table_name = c.table_name
                AND pk.column_name = c.column_name
            WHERE t.table_schema NOT IN ('pg_catalog', 'information_schema')
              AND t.table_schema NOT LIKE 'pg\_toast%'
              AND t.table_schema NOT LIKE 'pg\_temp%'
            ORDER BY t.table_schema, t.table_name, c.ordinal_position
        "#;

        let mut schemas: Vec<SchemaInfo> = Vec::new();
        let messages = self.client.simple_query(SQL).await.map_err(pg_error)?;
        for msg in messages {
            let SimpleQueryMessage::Row(row) = msg else { continue };
            let get = |i: usize| row.try_get(i).ok().flatten().unwrap_or_default();
            let (schema, table, ttype) = (get(0), get(1), get(2));

            if schemas.last().is_none_or(|s| s.name != schema) {
                schemas.push(SchemaInfo { name: schema.to_string(), tables: Vec::new() });
            }
            let Some(s) = schemas.last_mut() else { continue };
            if s.tables.last().is_none_or(|t| t.name != table) {
                s.tables.push(TableInfo {
                    name: table.to_string(),
                    kind: if ttype == "VIEW" { ObjectKind::View } else { ObjectKind::Table },
                    columns: Vec::new(),
                });
            }
            let Some(t) = s.tables.last_mut() else { continue };

            // Tabla sin columnas visibles: el LEFT JOIN deja la columna a NULL.
            let Ok(Some(column)) = row.try_get(3) else { continue };
            let data_type = match get(4) {
                // Para arrays y tipos de usuario information_schema da poco detalle.
                "ARRAY" => format!("{}[]", get(5).trim_start_matches('_')),
                "USER-DEFINED" => get(5).to_string(),
                other => other.to_string(),
            };
            t.columns.push(ColumnInfo {
                name: column.to_string(),
                data_type,
                nullable: get(6) == "YES",
                primary_key: get(7) == "Y",
            });
        }
        Ok(schemas)
    }
}

#[cfg(test)]
mod tests {
    //! Tests contra un PostgreSQL real. Se ejecutan con `cargo test -- --ignored` y las variables:
    //! DATACRAFT_PG_HOST (obligatoria; sin ella se omite), DATACRAFT_PG_PORT (5432), DATACRAFT_PG_USER (postgres),
    //! DATACRAFT_PG_PASSWORD, DATACRAFT_PG_DB (postgres).

    use super::*;
    use crate::db::Driver;

    fn env(name: &str, default: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| default.to_string())
    }

    async fn session() -> Option<PgSession> {
        let Ok(host) = std::env::var("DATACRAFT_PG_HOST") else {
            eprintln!("DATACRAFT_PG_HOST no definida: se omite el test de PostgreSQL");
            return None;
        };
        let mut cfg = ConnectionConfig::new(1, Driver::Postgres);
        cfg.host = host;
        cfg.port = env("DATACRAFT_PG_PORT", "5432").parse().expect("puerto");
        cfg.user = env("DATACRAFT_PG_USER", "postgres");
        cfg.password = env("DATACRAFT_PG_PASSWORD", "");
        cfg.database = env("DATACRAFT_PG_DB", "postgres");
        Some(PgSession::connect(&cfg).await.expect("conexión"))
    }

    #[tokio::test]
    #[ignore]
    async fn tipos_variados() {
        let Some(mut s) = session().await else { return };
        let rs = s
            .execute(
                "SELECT 42::int AS i, 12.50::numeric(10,2) AS n, 'hola'::text AS t, true AS b, \
                 DATE '2026-10-16' AS d, '6f1c1b1e-0000-4000-8000-000000000001'::uuid AS u, NULL::int AS z",
                100,
            )
            .await
            .unwrap();
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].columns, ["i", "n", "t", "b", "d", "u", "z"]);
        let row: Vec<Option<&str>> = rs[0].rows[0].iter().map(|c| c.as_deref()).collect();
        assert_eq!(
            row,
            [
                Some("42"),
                Some("12.50"),
                Some("hola"),
                Some("t"),
                Some("2026-10-16"),
                Some("6f1c1b1e-0000-4000-8000-000000000001"),
                None
            ]
        );
    }

    #[tokio::test]
    #[ignore]
    async fn ddl_dml_varias_sentencias_e_introspeccion() {
        let Some(mut s) = session().await else { return };
        s.execute("DROP SCHEMA IF EXISTS dc_test CASCADE; CREATE SCHEMA dc_test", 10).await.unwrap();
        let rs = s
            .execute(
                "CREATE TABLE dc_test.items (id int PRIMARY KEY, name text NOT NULL, price numeric);\
                 INSERT INTO dc_test.items VALUES (1,'a',1.5),(2,'b',NULL),(3,'c',3);\
                 SELECT * FROM dc_test.items ORDER BY id;\
                 SELECT * FROM dc_test.items WHERE false;\
                 UPDATE dc_test.items SET price = 0",
                2,
            )
            .await
            .unwrap();
        assert_eq!(rs.len(), 5);
        assert!(rs[0].is_command());
        assert_eq!(rs[1].affected, Some(3));
        assert_eq!(rs[2].rows.len(), 2);
        assert!(rs[2].truncated);
        assert_eq!(rs[2].rows[1][2], None);
        assert_eq!(rs[3].columns.len(), 3);
        assert!(rs[3].rows.is_empty());
        assert_eq!(rs[4].affected, Some(3));

        s.execute("CREATE VIEW dc_test.v AS SELECT id FROM dc_test.items", 10).await.unwrap();
        let schemas = s.introspect().await.unwrap();
        let sch = schemas.iter().find(|s| s.name == "dc_test").expect("esquema");
        assert_eq!(sch.tables.len(), 2);
        let items = &sch.tables[0];
        assert_eq!(items.name, "items");
        assert_eq!(items.kind, ObjectKind::Table);
        assert_eq!(items.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "name", "price"]);
        assert!(items.columns[0].primary_key && !items.columns[0].nullable);
        assert!(!items.columns[1].primary_key && !items.columns[1].nullable);
        assert!(items.columns[2].nullable);
        assert_eq!(sch.tables[1].kind, ObjectKind::View);
        assert!(!schemas.iter().any(|s| s.name == "pg_catalog" || s.name == "information_schema"));

        let err = s.execute("SELECT * FROM dc_test.no_existe", 10).await.unwrap_err();
        assert!(err.to_string().contains("no_existe"), "{err}");
        s.execute("DROP SCHEMA dc_test CASCADE", 10).await.unwrap();
    }
}
