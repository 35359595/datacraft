//! Read-only, cancellable value search on a dedicated session.

use super::{ConnectionConfig, Driver, ObjectKind, ResultSet, Session, TableInfo};
use crate::i18n::tr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

pub const PREVIEW_ROWS: usize = 100;

#[derive(Clone)]
pub struct SearchRequest {
    pub db: String,
    pub schema: Option<String>,
    pub value: String,
    pub exact: bool,
    pub case_sensitive: bool,
}

pub struct TableMatch {
    pub schema: String,
    pub table: String,
    pub result: ResultSet,
}

#[derive(Default)]
pub struct SearchReport {
    pub matches: Vec<TableMatch>,
    pub errors: Vec<String>,
    pub scanned: usize,
    pub total: usize,
    pub stopped: Option<String>,
}

fn searchable_type(data_type: &str) -> bool {
    let t = data_type.to_ascii_lowercase();
    ![
        "binary",
        "blob",
        "bytea",
        "image",
        "raw",
        "geometry",
        "geography",
        "bfile",
    ]
    .iter()
    .any(|word| t.contains(word))
}

fn search_sql(
    driver: Driver,
    schema: &str,
    table: &TableInfo,
    request: &SearchRequest,
) -> Option<String> {
    // LIKE metacharacters are literal input. SQL Server also treats '[' specially.
    let mut value = request.value.clone();
    if !request.exact && driver != Driver::Sqlite {
        value = value
            .replace('!', "!!")
            .replace('%', "!%")
            .replace('_', "!_");
        if driver == Driver::MsSql {
            value = value.replace('[', "![");
        }
        value = format!("%{value}%");
    }
    let value = crate::edit::literal(driver, &Some(value));
    let value = if request.case_sensitive {
        value
    } else {
        format!("LOWER({value})")
    };
    let predicates: Vec<_> = table
        .columns
        .iter()
        .filter(|c| {
            searchable_type(&c.data_type)
                && !(driver == Driver::MsSql
                    && matches!(
                        c.data_type.to_ascii_lowercase().as_str(),
                        "timestamp" | "rowversion"
                    ))
        })
        .map(|c| {
            let column = driver.quote_ident(&c.name);
            let text = match driver {
                Driver::MsSql => format!("CONVERT(nvarchar(max), {column})"),
                Driver::MySql | Driver::MariaDb => format!("CAST({column} AS CHAR)"),
                Driver::Oracle => format!("CAST({column} AS VARCHAR2(4000))"),
                Driver::Snowflake => format!("TO_VARCHAR({column})"),
                Driver::Postgres | Driver::Sqlite => format!("CAST({column} AS TEXT)"),
            };
            let text = if request.case_sensitive {
                text
            } else {
                format!("LOWER({text})")
            };
            let text = match driver {
                Driver::MsSql => format!("{text} COLLATE Latin1_General_100_BIN2"),
                Driver::MySql | Driver::MariaDb => format!("BINARY {text}"),
                Driver::Sqlite => format!("{text} COLLATE BINARY"),
                _ => text,
            };
            if driver == Driver::Sqlite && !request.exact {
                format!("INSTR({text}, {value}) > 0")
            } else if request.exact {
                format!("{text} = {value}")
            } else {
                format!("{text} LIKE {value} ESCAPE '!' ")
            }
        })
        .collect();
    if predicates.is_empty() {
        return None;
    }
    let name = Session::quote_table(driver, schema, &table.name);
    let predicate = predicates.join(" OR ");
    Some(match driver {
        Driver::MsSql => format!("SELECT TOP ({PREVIEW_ROWS}) * FROM {name} WHERE {predicate}"),
        Driver::Oracle => {
            format!("SELECT * FROM {name} WHERE ({predicate}) AND ROWNUM <= {PREVIEW_ROWS}")
        }
        _ => format!("SELECT * FROM {name} WHERE {predicate} LIMIT {PREVIEW_ROWS}"),
    })
}

pub async fn search(
    mut cfg: ConnectionConfig,
    request: SearchRequest,
    cancel: Arc<Notify>,
) -> anyhow::Result<SearchReport> {
    // A separate session keeps cancellation and database switching out of editor transactions.
    cfg.database.clone_from(&request.db);
    cfg.read_only = true;
    let setup = async {
        let mut session = Session::connect(&cfg).await?;
        let schemas = session.introspect_database(&request.db).await?;
        anyhow::Ok((session, schemas))
    };
    let (mut session, schemas) = tokio::select! {
        _ = cancel.notified() => return Ok(SearchReport { stopped: Some(tr("Search cancelled").into()), ..Default::default() }),
        result = tokio::time::timeout(Duration::from_secs(30), setup) => result.map_err(|_| anyhow::anyhow!(tr("Schema search timed out")))??,
    };
    let tables: Vec<_> = schemas
        .into_iter()
        .filter(|s| request.schema.as_ref().is_none_or(|name| *name == s.name))
        .flat_map(|s| {
            s.tables
                .into_iter()
                .filter(|t| t.kind == ObjectKind::Table)
                .map(move |t| (s.name.clone(), t))
        })
        .collect();
    let mut report = SearchReport {
        total: tables.len(),
        ..Default::default()
    };
    for (schema, table) in tables {
        let Some(sql) = search_sql(cfg.driver, &schema, &table, &request) else {
            report.scanned += 1;
            continue;
        };
        let native_cancel = session.cancel_handle();
        let outcome = tokio::select! {
            _ = cancel.notified() => Err(tr("Search cancelled")),
            result = tokio::time::timeout(Duration::from_secs(15), session.execute(&sql, PREVIEW_ROWS)) => {
                result.map_err(|_| tr("Schema search timed out"))
            }
        };
        let results = match outcome {
            Ok(results) => results,
            Err(reason) => {
                if let Some(handle) = native_cancel {
                    let _ = tokio::time::timeout(Duration::from_secs(2), handle.cancel()).await;
                }
                report.stopped = Some(reason.into());
                break;
            }
        };
        report.scanned += 1;
        match results {
            Ok(results) => {
                for result in results.into_iter().filter(|r| !r.rows.is_empty()) {
                    report.matches.push(TableMatch {
                        schema: schema.clone(),
                        table: table.name.clone(),
                        result,
                    });
                }
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("{schema}.{}: {error:#}", table.name));
                if session.is_closed() {
                    report.stopped = Some(tr("Connection lost").into());
                    break;
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ColumnInfo;

    fn request(value: &str) -> SearchRequest {
        SearchRequest {
            db: "main".into(),
            schema: None,
            value: value.into(),
            exact: false,
            case_sensitive: false,
        }
    }

    #[test]
    fn dialects_quote_identifiers_and_literal_search_terms() {
        let table = TableInfo {
            name: "a\"`]b".into(),
            kind: ObjectKind::Table,
            columns: vec![
                ColumnInfo {
                    name: "name".into(),
                    data_type: "text".into(),
                    nullable: true,
                    primary_key: false,
                },
                ColumnInfo {
                    name: "bytes".into(),
                    data_type: "blob".into(),
                    nullable: true,
                    primary_key: false,
                },
            ],
        };
        for driver in [
            Driver::Postgres,
            Driver::Sqlite,
            Driver::MySql,
            Driver::MariaDb,
            Driver::MsSql,
            Driver::Oracle,
            Driver::Snowflake,
        ] {
            let sql = search_sql(driver, "schema", &table, &request("O'Reilly_50%!\\")).unwrap();
            assert!(
                sql.contains(if driver == Driver::Sqlite {
                    "O''Reilly_50%!"
                } else {
                    "O''Reilly!_50!%!!"
                }),
                "{sql}"
            );
            assert!(!sql.contains("bytes"));
            assert!(sql.contains(&Session::quote_table(driver, "schema", &table.name)));
        }
    }

    #[tokio::test]
    async fn sqlite_searches_every_table_and_treats_wildcards_literally() {
        let path =
            std::env::temp_dir().join(format!("datacraft-search-{}.db", uuid::Uuid::new_v4()));
        {
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch("CREATE TABLE a (id INTEGER, note TEXT); CREATE TABLE b (note TEXT); CREATE TABLE c (note TEXT); INSERT INTO a VALUES (42, 'O''Reilly_50%'); INSERT INTO b VALUES ('O''ReillyX50x'), ('other'); INSERT INTO c VALUES ('O''Reilly_50%');").unwrap();
        }
        let mut cfg = ConnectionConfig::new(1, Driver::Sqlite);
        cfg.file_path = path.to_string_lossy().into_owned();
        let report = search(cfg.clone(), request("reilly_50%"), Arc::new(Notify::new()))
            .await
            .unwrap();
        assert_eq!(report.scanned, 3);
        assert_eq!(report.matches.len(), 2);
        assert!(report.errors.is_empty());
        let report = search(
            cfg.clone(),
            SearchRequest {
                exact: true,
                ..request("42")
            },
            Arc::new(Notify::new()),
        )
        .await
        .unwrap();
        assert_eq!(report.matches.len(), 1);
        let report = search(
            cfg.clone(),
            SearchRequest {
                case_sensitive: true,
                ..request("REILLY")
            },
            Arc::new(Notify::new()),
        )
        .await
        .unwrap();
        assert!(report.matches.is_empty());
        let report = search(
            cfg.clone(),
            request("'; DROP TABLE a; --"),
            Arc::new(Notify::new()),
        )
        .await
        .unwrap();
        assert_eq!(report.scanned, 3);
        assert!(report.matches.is_empty());
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM a", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(db);
        let cancel = Arc::new(Notify::new());
        cancel.notify_one();
        let report = search(cfg, request("a"), cancel).await.unwrap();
        assert!(report.stopped.is_some());
        std::fs::remove_file(path).unwrap();
    }
}
