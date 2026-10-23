//! Background thread with a tokio runtime. The UI sends `Request`s and collects `Response`s
//! with `Worker::poll` every frame; the worker requests a repaint when each task finishes.
//!
//! Each connection can have several databases. SQL Server and SQLite use a single
//! session (with `USE [db]` before each operation on SQL Server); PostgreSQL opens one
//! session per database, because a PG connection cannot switch databases.

use crate::db::{CancelHandle, ConnectionConfig, DatabaseInfo, Driver, ResultSet, SchemaInfo, Session};
use crate::i18n::{tr, trf};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;
use tokio::sync::Mutex;

pub type ConnId = u64;
pub type ConsoleId = u64;

pub enum Request {
    SchemaSearch {
        conn: ConnId,
        request_id: u64,
        request: crate::db::schema_search::SearchRequest,
        cancel: Arc<tokio::sync::Notify>,
    },
    /// Opens (or reopens) the connection, lists its databases and loads the default database's schema.
    Connect(ConnectionConfig),
    /// Only tests the connection and closes it.
    Test(ConnectionConfig),
    Disconnect(ConnId),
    /// Lists the databases again and reloads the default one.
    Refresh(ConnId),
    /// Loads (or reloads) the schema of a specific database.
    LoadDatabase {
        conn: ConnId,
        db: String,
    },
    ObjectInfo {
        conn: ConnId,
        db: String,
        schema: String,
        table: String,
        request_id: u64,
        count_rows: bool,
    },
    ObjectCatalog {
        conn: ConnId,
        db: String,
        schema: Option<String>,
        request_id: u64,
    },
    /// Runs statements in order; stops at the first error.
    /// `db`: database to run them in (`None` = the connection's default database).
    Execute {
        conn: ConnId,
        db: Option<String>,
        console: ConsoleId,
        statements: Vec<String>,
        max_rows: usize,
    },
    /// Saves data editor changes (statements from `edit::change_sql`) in one transaction.
    ApplyChanges {
        conn: ConnId,
        db: Option<String>,
        console: ConsoleId,
        statements: Vec<String>,
    },
    /// Runs DDL outside any console (create or drop a database or table). `db`: as in
    /// `Execute`. `close_db`: database whose own session (PostgreSQL) is closed first, so
    /// it can be dropped.
    /// `transaction`: all statements in one transaction (imports); DDL runs without one.
    RunDdl { conn: ConnId, db: Option<String>, statements: Vec<String>, close_db: Option<String>, transaction: bool },
    /// Cancels what a console's `Execute` is running. Engines without a native cancel
    /// abandon the query and close the session (it reconnects on the next run).
    Cancel { console: ConsoleId },
}

pub enum Response {
    SchemaSearch { request_id: u64, result: Result<crate::db::schema_search::SearchReport, String> },
    /// `databases` in order; the default one has `current = true` and its schema loaded.
    Connected { conn: ConnId, databases: Vec<DatabaseInfo> },
    ConnectFailed { conn: ConnId, error: String },
    /// `Ok` carries the server's product and version when it could be read.
    TestResult { conn: ConnId, result: Result<Option<String>, String> },
    Disconnected(ConnId),
    /// Reply to `Refresh`: same shape as `Connected`.
    Refreshed {
        conn: ConnId,
        databases: Vec<DatabaseInfo>,
    },
    DatabaseLoaded {
        conn: ConnId,
        db: String,
        result: Result<Vec<SchemaInfo>, String>,
    },
    ObjectInfo {
        conn: ConnId,
        request_id: u64,
        result: Result<Box<crate::db::object_info::ObjectInfo>, String>,
    },
    ObjectCatalog {
        conn: ConnId,
        request_id: u64,
        result: Result<Vec<crate::db::object_info::ObjectInfoRow>, String>,
    },
    /// `results` holds what ran before the error (if there was one).
    Executed {
        console: ConsoleId,
        results: Vec<ResultSet>,
        error: Option<String>,
        elapsed: std::time::Duration,
    },
    /// Result of `ApplyChanges`: modified rows or an error (in which case nothing was saved).
    ChangesApplied { console: ConsoleId, result: Result<u64, String> },
    /// Result of `RunDdl`.
    DdlDone { conn: ConnId, result: Result<(), String> },
}

pub struct Worker {
    tx: tokio::sync::mpsc::UnboundedSender<Request>,
    rx: mpsc::Receiver<Response>,
}

impl Worker {
    pub fn start(ctx: egui::Context) -> Self {
        let (req_tx, mut req_rx) = tokio::sync::mpsc::unbounded_channel::<Request>();
        let (resp_tx, resp_rx) = mpsc::channel::<Response>();

        std::thread::Builder::new()
            .name("datacraft-worker".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(async move {
                    let state = Arc::new(State::default());
                    while let Some(req) = req_rx.recv().await {
                        // The epoch is stamped here, in arrival order: a Disconnect after a
                        // Connect invalidates that Connect even if it finishes later.
                        let epoch = state.bump_if_lifecycle(&req);
                        let state = state.clone();
                        let out = resp_tx.clone();
                        let ctx = ctx.clone();
                        tokio::spawn(async move {
                            for resp in handle(req, epoch, &state).await {
                                let _ = out.send(resp);
                            }
                            ctx.request_repaint();
                        });
                    }
                });
            })
            .expect("worker thread");

        Self { tx: req_tx, rx: resp_rx }
    }

    pub fn send(&self, req: Request) {
        let _ = self.tx.send(req);
    }

    pub fn poll(&self) -> Vec<Response> {
        self.rx.try_iter().collect()
    }
}

type SharedSession = Arc<Mutex<Session>>;

/// Open connection: settings (with password, in memory only) and its sessions.
struct Live {
    cfg: ConnectionConfig,
    /// Default database (that of the main session).
    default_db: String,
    /// Key "" = main session. On PostgreSQL, also one session per extra database.
    sessions: HashMap<String, SharedSession>,
}

/// A console's `Execute` in progress. Registered before waiting for the session, so a
/// cancel that arrives meanwhile is not lost.
#[derive(Default)]
struct Running {
    /// Native cancel of the session's running statement (once the session is held).
    native: std::sync::Mutex<Option<CancelHandle>>,
    /// true while a statement of THIS execution is running on the server (a native cancel
    /// outside that window could hit another console's query on a shared session).
    in_flight: std::sync::atomic::AtomicBool,
    /// Abandons the query (engines without native cancel, or if it failed).
    abandon: tokio::sync::Notify,
    cancelled: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct State {
    live: Mutex<HashMap<ConnId, Live>>,
    running: std::sync::Mutex<HashMap<ConsoleId, Arc<Running>>>,
    /// Epoch per connection: changes with every Connect/Disconnect.
    epochs: std::sync::Mutex<HashMap<ConnId, u64>>,
}

impl State {
    fn bump_if_lifecycle(&self, req: &Request) -> u64 {
        let conn = match req {
            Request::Connect(cfg) => cfg.id,
            Request::Disconnect(conn) => *conn,
            _ => return 0,
        };
        let mut epochs = self.epochs.lock().unwrap_or_else(|e| e.into_inner());
        let e = epochs.entry(conn).or_default();
        *e += 1;
        *e
    }

    fn current_epoch(&self, conn: ConnId) -> u64 {
        let epochs = self.epochs.lock().unwrap_or_else(|e| e.into_inner());
        epochs.get(&conn).copied().unwrap_or_default()
    }

    /// Session to operate on `db` with, and the database name to pass to the driver.
    /// On PostgreSQL it opens (and keeps) a new session if the database isn't the default one.
    async fn session_for(&self, conn: ConnId, db: Option<&str>) -> anyhow::Result<(SharedSession, String)> {
        let (cfg, default_db, main, existing) = {
            let live = self.live.lock().await;
            let l = live.get(&conn).ok_or_else(|| anyhow::anyhow!(tr("The connection is not open")))?;
            let target = db.filter(|d| !d.is_empty()).unwrap_or(&l.default_db).to_string();
            let main = l.sessions.get("").cloned().ok_or_else(|| anyhow::anyhow!(tr("The connection is not open")))?;
            let existing = l.sessions.get(&target).cloned();
            (l.cfg.clone(), l.default_db.clone(), main, existing)
        };
        let target = db.filter(|d| !d.is_empty()).unwrap_or(&default_db).to_string();
        if cfg.driver != Driver::Postgres || target == default_db {
            return Ok((main, target));
        }
        if let Some(s) = existing {
            return Ok((s, target));
        }
        let mut extra_cfg = cfg;
        extra_cfg.database = target.clone();
        let epoch = self.current_epoch(conn);
        let session = Arc::new(Mutex::new(Session::connect(&extra_cfg).await?));
        let mut live = self.live.lock().await;
        // If it was disconnected meanwhile, don't keep it (the session closes when dropped).
        if self.current_epoch(conn) == epoch
            && let Some(l) = live.get_mut(&conn)
        {
            let s = l.sessions.entry(target.clone()).or_insert(session);
            return Ok((s.clone(), target));
        }
        Ok((session, target))
    }

    /// Removes the session if it is still the same one (the connection was lost).
    async fn forget(&self, conn: ConnId, session: &SharedSession) {
        let mut live = self.live.lock().await;
        let Some(l) = live.get_mut(&conn) else { return };
        let key = l.sessions.iter().find(|(_, s)| Arc::ptr_eq(s, session)).map(|(k, _)| k.clone());
        match key.as_deref() {
            // Without the main session the whole connection is closed.
            Some("") => {
                live.remove(&conn);
            }
            Some(k) => {
                let k = k.to_string();
                l.sessions.remove(&k);
            }
            None => {}
        }
    }
}

async fn fetch_object_info(
    state: &State,
    conn: ConnId,
    db: &str,
    schema: Option<&str>,
    table: Option<&str>,
    count_rows: bool,
) -> (
    Result<Vec<crate::db::object_info::ObjectInfoRow>, String>,
    bool,
) {
    let generation = state.current_epoch(conn);
    let (session, target) = match state.session_for(conn, Some(db)).await {
        Ok(value) => value,
        Err(e) => return (Err(format!("{e:#}")), false),
    };
    let Some(driver) = state.live.lock().await.get(&conn).map(|l| l.cfg.driver) else {
        return (Err(tr("Connection lost").into()), false);
    };
    let mut s = session.lock().await;
    let cancel = s.cancel_handle();
    let query = async {
        s.use_database(&target).await?;
        match (schema, table) {
            (Some(schema), Some(table)) => s
                .object_info(driver, schema, table, count_rows)
                .await
                .map(|info| {
                    vec![crate::db::object_info::ObjectInfoRow {
                        schema: schema.into(),
                        name: table.into(),
                        info,
                    }]
                }),
            _ => s.object_list_info(driver, schema).await,
        }
    };
    let (result, lost) = match tokio::time::timeout(std::time::Duration::from_secs(15), query).await
    {
        Ok(result) => (result.map_err(|e| format!("{e:#}")), s.is_closed()),
        Err(_) => {
            let cancelled = match cancel {
                Some(h) => h.cancel().await.is_ok(),
                None => false,
            };
            (
                Err(tr("Object information request timed out").into()),
                !cancelled,
            )
        }
    };
    drop(s);
    if lost {
        state.forget(conn, &session).await;
    }
    (result, lost && state.current_epoch(conn) == generation)
}

async fn handle(req: Request, epoch: u64, state: &State) -> Vec<Response> {
    match req {
        Request::SchemaSearch { conn, request_id, request, cancel } => {
            let cfg = state.live.lock().await.get(&conn).map(|l| l.cfg.clone());
            let result = match cfg {
                Some(cfg) => crate::db::schema_search::search(cfg, request, cancel).await.map_err(|e| format!("{e:#}")),
                None => Err(tr("The connection is not open").into()),
            };
            vec![Response::SchemaSearch { request_id, result }]
        }
        Request::Connect(cfg) => {
            let conn = cfg.id;
            match connect_and_load(&cfg).await {
                Ok((session, default_db, databases)) => {
                    let mut live = state.live.lock().await;
                    // Disconnected or reconnected meanwhile: this session is not needed.
                    if state.current_epoch(conn) != epoch {
                        return vec![];
                    }
                    let mut sessions = HashMap::new();
                    sessions.insert(String::new(), Arc::new(Mutex::new(session)));
                    live.insert(conn, Live { cfg, default_db, sessions });
                    vec![Response::Connected { conn, databases }]
                }
                Err(e) if state.current_epoch(conn) == epoch => {
                    vec![Response::ConnectFailed { conn, error: format!("{e:#}") }]
                }
                Err(_) => vec![],
            }
        }
        Request::Test(cfg) => {
            let result = match Session::connect(&cfg).await {
                Ok(mut session) => Ok(session.server_version(cfg.driver).await),
                Err(e) => Err(format!("{e:#}")),
            };
            vec![Response::TestResult { conn: cfg.id, result }]
        }
        Request::Disconnect(conn) => {
            state.live.lock().await.remove(&conn);
            vec![Response::Disconnected(conn)]
        }
        Request::Refresh(conn) => {
            let (session, default_db) = match state.session_for(conn, None).await {
                Ok(s) => s,
                Err(e) => return vec![Response::ConnectFailed { conn, error: format!("{e:#}") }],
            };
            let mut s = session.lock().await;
            match load_databases(&mut s, &default_db).await {
                Ok(databases) => vec![Response::Refreshed { conn, databases }],
                Err(e) => {
                    let lost = s.is_closed();
                    drop(s);
                    if lost {
                        state.forget(conn, &session).await;
                    }
                    vec![Response::ConnectFailed { conn, error: format!("{e:#}") }]
                }
            }
        }
        Request::LoadDatabase { conn, db } => {
            let result = match state.session_for(conn, Some(&db)).await {
                Ok((session, target)) => {
                    let mut s = session.lock().await;
                    s.introspect_database(&target).await.map_err(|e| format!("{e:#}"))
                }
                Err(e) => Err(format!("{e:#}")),
            };
            vec![Response::DatabaseLoaded { conn, db, result }]
        }
        Request::ObjectInfo {
            conn,
            db,
            schema,
            table,
            request_id,
            count_rows,
        } => {
            let (result, lost) =
                fetch_object_info(state, conn, &db, Some(&schema), Some(&table), count_rows).await;
            let result = result.and_then(|rows| {
                rows.into_iter()
                    .next()
                    .map(|row| Box::new(row.info))
                    .ok_or_else(|| tr("Object information is not available").into())
            });
            let mut responses = vec![Response::ObjectInfo {
                conn,
                request_id,
                result,
            }];
            if lost {
                responses.push(Response::ConnectFailed {
                    conn,
                    error: tr("Connection lost").into(),
                });
            }
            responses
        }
        Request::ObjectCatalog {
            conn,
            db,
            schema,
            request_id,
        } => {
            let (result, lost) =
                fetch_object_info(state, conn, &db, schema.as_deref(), None, false).await;
            let mut responses = vec![Response::ObjectCatalog {
                conn,
                request_id,
                result,
            }];
            if lost {
                responses.push(Response::ConnectFailed {
                    conn,
                    error: tr("Connection lost").into(),
                });
            }
            responses
        }
        Request::ApplyChanges { conn, db, console, statements } => {
            let (session, target) = match state.session_for(conn, db.as_deref()).await {
                Ok(s) => s,
                Err(e) => return vec![Response::ChangesApplied { console, result: Err(format!("{e:#}")) }],
            };
            let mut s = session.lock().await;
            let result = match s.use_database(&target).await {
                Ok(()) => s.apply_changes(&statements).await.map_err(|e| format!("{e:#}")),
                Err(e) => Err(format!("{e:#}")),
            };
            let lost = s.is_closed();
            drop(s);
            let mut out = vec![Response::ChangesApplied { console, result }];
            if lost {
                state.forget(conn, &session).await;
                out.push(Response::ConnectFailed { conn, error: tr("Connection lost").into() });
            }
            out
        }
        Request::RunDdl { conn, db, statements, close_db, transaction } => {
            if let Some(name) = close_db.filter(|n| !n.is_empty())
                && let Some(l) = state.live.lock().await.get_mut(&conn)
            {
                l.sessions.remove(&name);
            }
            let (session, target) = match state.session_for(conn, db.as_deref()).await {
                Ok(s) => s,
                Err(e) => return vec![Response::DdlDone { conn, result: Err(format!("{e:#}")) }],
            };
            let mut s = session.lock().await;
            let mut result = s.use_database(&target).await.map_err(|e| format!("{e:#}"));
            if result.is_ok() && transaction {
                result = s.run_transaction(&statements).await.map(|_| ()).map_err(|e| format!("{e:#}"));
            } else if result.is_ok() {
                for stmt in &statements {
                    if let Err(e) = s.execute(stmt, 0).await {
                        result = Err(format!("{e:#}"));
                        break;
                    }
                }
            }
            let lost = s.is_closed();
            drop(s);
            let mut out = vec![Response::DdlDone { conn, result }];
            if lost {
                state.forget(conn, &session).await;
                out.push(Response::ConnectFailed { conn, error: tr("Connection lost").into() });
            }
            out
        }
        Request::Cancel { console } => {
            use std::sync::atomic::Ordering::SeqCst;
            let running = state.running.lock().unwrap_or_else(|e| e.into_inner()).get(&console).cloned();
            if let Some(r) = running {
                r.cancelled.store(true, SeqCst);
                let native = r.native.lock().unwrap_or_else(|e| e.into_inner()).clone();
                // The statement may not have reached the server yet when the first cancel
                // arrives: retry while this same execution is still running a statement.
                let still_running = || {
                    r.in_flight.load(SeqCst)
                        && state.running.lock().unwrap_or_else(|e| e.into_inner()).get(&console).is_some_and(|x| Arc::ptr_eq(x, &r))
                };
                for _ in 0..25 {
                    if !still_running() {
                        break;
                    }
                    match &native {
                        Some(h) if h.cancel().await.is_ok() => {}
                        _ => {
                            r.abandon.notify_one();
                            break;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                // Still running after ~5 s of native cancels: abandon it.
                if still_running() {
                    r.abandon.notify_one();
                }
            }
            vec![]
        }
        Request::Execute { conn, db, console, statements, max_rows } => {
            use std::sync::atomic::Ordering::SeqCst;
            let start = Instant::now();
            let running = Arc::new(Running::default());
            state.running.lock().unwrap_or_else(|e| e.into_inner()).insert(console, running.clone());
            let unregister = |state: &State| {
                let mut map = state.running.lock().unwrap_or_else(|e| e.into_inner());
                if map.get(&console).is_some_and(|r| Arc::ptr_eq(r, &running)) {
                    map.remove(&console);
                }
            };
            let (session, target) = match state.session_for(conn, db.as_deref()).await {
                Ok(s) => s,
                Err(e) => {
                    unregister(state);
                    return vec![Response::Executed {
                        console,
                        results: vec![],
                        error: Some(format!("{e:#}")),
                        elapsed: start.elapsed(),
                    }];
                }
            };
            let mut s = session.lock().await;
            *running.native.lock().unwrap_or_else(|e| e.into_inner()) = s.cancel_handle();
            let statements_tail = statements.last().cloned();
            let mut results = Vec::new();
            let mut error = None;
            let mut abandoned = false;
            if let Err(e) = s.use_database(&target).await {
                error = Some(format!("{e:#}"));
            } else {
                for stmt in statements {
                    // `in_flight` first: either this sees `cancelled` or the canceller sees
                    // `in_flight` (and retries until the statement stops).
                    running.in_flight.store(true, SeqCst);
                    if running.cancelled.load(SeqCst) {
                        running.in_flight.store(false, SeqCst);
                        break;
                    }
                    let result = tokio::select! {
                        r = s.execute(&stmt, max_rows) => r,
                        _ = running.abandon.notified() => {
                            abandoned = true;
                            Err(anyhow::anyhow!(tr("Query cancelled")))
                        }
                    };
                    running.in_flight.store(false, SeqCst);
                    match result {
                        Ok(rs) => results.extend(rs),
                        Err(e) => {
                            error = Some(format!("{e:#}"));
                            break;
                        }
                    }
                    if abandoned {
                        break;
                    }
                }
            }
            // SQL Server explain: `SET SHOWPLAN_TEXT OFF` must run even if the statement failed,
            // or the session would keep returning plans instead of running queries.
            if error.is_some()
                && !abandoned
                && let Some(last) = statements_tail.filter(|s| s.trim().eq_ignore_ascii_case("SET SHOWPLAN_TEXT OFF"))
            {
                let _ = s.execute(&last, 0).await;
            }
            unregister(state);
            if running.cancelled.load(SeqCst) {
                error = Some(tr("Query cancelled").to_string());
            }
            // An abandoned query leaves the session in an unknown state: it is closed.
            let lost = abandoned || s.is_closed();
            drop(s);
            let mut out = vec![Response::Executed { console, results, error: error.clone(), elapsed: start.elapsed() }];
            if abandoned {
                state.forget(conn, &session).await;
                out.push(Response::ConnectFailed { conn, error: tr("Query cancelled; the connection was closed and will reopen on the next run").to_string() });
            } else if lost {
                // Forget the broken session; the UI will reconnect on the next run.
                state.forget(conn, &session).await;
                let detail = error.unwrap_or_default();
                out.push(Response::ConnectFailed { conn, error: trf("Connection lost. {detail}", &[("detail", &detail)]) });
            }
            out
        }
    }
}

async fn connect_and_load(cfg: &ConnectionConfig) -> anyhow::Result<(Session, String, Vec<DatabaseInfo>)> {
    let mut session = Session::connect(cfg).await?;
    let default_db = session.current_database().await?;
    let databases = load_databases(&mut session, &default_db).await?;
    Ok((session, default_db, databases))
}

/// List of databases with the default one's schema already loaded.
async fn load_databases(session: &mut Session, default_db: &str) -> anyhow::Result<Vec<DatabaseInfo>> {
    let mut names = session.list_databases().await?;
    if !names.iter().any(|n| n == default_db) {
        names.push(default_db.to_string());
        names.sort_by_key(|n| n.to_lowercase());
    }
    let schemas = session.introspect_database(default_db).await?;
    let mut schemas = Some(schemas);
    Ok(names
        .into_iter()
        .map(|name| {
            let current = name == default_db;
            DatabaseInfo { schemas: if current { schemas.take() } else { None }, current, name }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wait_for(w: &Worker, pred: impl Fn(&Response) -> bool) -> Response {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            for r in w.poll() {
                if pred(&r) {
                    return r;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for the worker");
    }

    #[test]
    fn object_metadata_is_async_and_exact_counts_are_explicit() {
        let path =
            std::env::temp_dir().join(format!("datacraft-object-info-{}.db", uuid::Uuid::new_v4()));
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("create table notes(id integer); insert into notes values (1),(2);")
            .unwrap();
        let w = Worker::start(egui::Context::default());
        let mut cfg = ConnectionConfig::new(17, Driver::Sqlite);
        cfg.file_path = path.display().to_string();
        cfg.read_only = true;
        w.send(Request::Connect(cfg));
        wait_for(&w, |r| matches!(r, Response::Connected { conn: 17, .. }));
        for (request_id, count_rows) in [(100, false), (101, true)] {
            w.send(Request::ObjectInfo {
                conn: 17,
                db: "main".into(),
                schema: "main".into(),
                table: "notes".into(),
                request_id,
                count_rows,
            });
            let response = wait_for(
                &w,
                |r| matches!(r, Response::ObjectInfo { request_id: id, .. } if *id == request_id),
            );
            let Response::ObjectInfo { conn, result, .. } = response else {
                unreachable!()
            };
            assert_eq!(conn, 17);
            let info = result.unwrap();
            assert_eq!(info.rows, if count_rows { Some(2) } else { None });
            assert_eq!(info.rows_exact, count_rows);
        }
        w.send(Request::ObjectCatalog {
            conn: 17,
            db: "main".into(),
            schema: Some("main".into()),
            request_id: 102,
        });
        let response = wait_for(&w, |r| {
            matches!(
                r,
                Response::ObjectCatalog {
                    request_id: 102,
                    ..
                }
            )
        });
        let Response::ObjectCatalog { result, .. } = response else {
            unreachable!()
        };
        let catalog = result.unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "notes");
        w.send(Request::Disconnect(17));
        wait_for(&w, |r| matches!(r, Response::Disconnected(17)));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cancels_a_running_sqlite_query() {
        let dir = std::env::temp_dir().join(format!("datacraft-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        rusqlite::Connection::open(&path).unwrap().execute_batch("create table t(x)").unwrap();
        let w = Worker::start(egui::Context::default());
        let mut cfg = ConnectionConfig::new(7, Driver::Sqlite);
        cfg.file_path = path.display().to_string();
        w.send(Request::Connect(cfg));
        wait_for(&w, |r| matches!(r, Response::Connected { .. }));
        // Endless recursive CTE: only a cancel stops it.
        let endless = "with recursive c(n) as (select 1 union all select n + 1 from c) select count(*) from c".to_string();
        w.send(Request::Execute { conn: 7, db: None, console: 1, statements: vec![endless], max_rows: 10 });
        std::thread::sleep(Duration::from_millis(300));
        w.send(Request::Cancel { console: 1 });
        let r = wait_for(&w, |r| matches!(r, Response::Executed { console: 1, .. }));
        let Response::Executed { error, .. } = r else { unreachable!() };
        assert_eq!(error.as_deref(), Some(tr("Query cancelled")));
        // The session is still usable.
        w.send(Request::Execute { conn: 7, db: None, console: 2, statements: vec!["select 1".into()], max_rows: 10 });
        let r = wait_for(&w, |r| matches!(r, Response::Executed { console: 2, .. }));
        let Response::Executed { error, results, .. } = r else { unreachable!() };
        assert!(error.is_none() && results.len() == 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn read_only_rejects_writes(cfg: ConnectionConfig, write: &str) {
        let conn = cfg.id;
        let w = Worker::start(egui::Context::default());
        w.send(Request::Connect(cfg));
        wait_for(&w, |r| matches!(r, Response::Connected { .. }));
        w.send(Request::Execute { conn, db: None, console: 3, statements: vec![write.into()], max_rows: 10 });
        let r = wait_for(&w, |r| matches!(r, Response::Executed { console: 3, .. }));
        let Response::Executed { error, .. } = r else { unreachable!() };
        assert!(error.is_some(), "the server accepted a write on a read-only connection");
    }

    #[test]
    fn sqlite_read_only_is_enforced_by_the_engine() {
        let dir = std::env::temp_dir().join(format!("datacraft-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        rusqlite::Connection::open(&path).unwrap().execute_batch("create table t(x); insert into t values (1)").unwrap();
        let mut cfg = ConnectionConfig::new(9, Driver::Sqlite);
        cfg.file_path = path.display().to_string();
        cfg.read_only = true;
        read_only_rejects_writes(cfg, "delete from t");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An import (transactional RunDdl) whose second batch fails leaves no rows behind.
    fn transactional_import_rolls_back(cfg: ConnectionConfig, setup: &str, table: &str) {
        let conn = cfg.id;
        let w = Worker::start(egui::Context::default());
        w.send(Request::Connect(cfg));
        wait_for(&w, |r| matches!(r, Response::Connected { .. }));
        w.send(Request::RunDdl { conn, db: None, statements: vec![setup.into()], close_db: None, transaction: false });
        let Response::DdlDone { result, .. } = wait_for(&w, |r| matches!(r, Response::DdlDone { .. })) else { unreachable!() };
        result.unwrap();
        let statements = vec![format!("insert into {table} values (1), (2)"), format!("insert into {table} values (2)")];
        w.send(Request::RunDdl { conn, db: None, statements, close_db: None, transaction: true });
        let Response::DdlDone { result, .. } = wait_for(&w, |r| matches!(r, Response::DdlDone { .. })) else { unreachable!() };
        assert!(result.is_err());
        w.send(Request::Execute { conn, db: None, console: 4, statements: vec![format!("select count(*) from {table}")], max_rows: 10 });
        let Response::Executed { results, .. } = wait_for(&w, |r| matches!(r, Response::Executed { console: 4, .. })) else { unreachable!() };
        assert_eq!(results[0].rows[0][0].as_deref(), Some("0"));
        w.send(Request::RunDdl { conn, db: None, statements: vec![format!("drop table {table}")], close_db: None, transaction: false });
        wait_for(&w, |r| matches!(r, Response::DdlDone { .. }));
    }

    #[test]
    fn sqlite_import_is_all_or_nothing() {
        let dir = std::env::temp_dir().join(format!("datacraft-tx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        rusqlite::Connection::open(&path).unwrap();
        let mut cfg = ConnectionConfig::new(11, Driver::Sqlite);
        cfg.file_path = path.display().to_string();
        transactional_import_rolls_back(cfg, "create table imp(x integer primary key)", "imp");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore]
    fn mysql_import_is_all_or_nothing() {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let mut cfg = ConnectionConfig::new(12, Driver::MariaDb);
        cfg.host = var("DATACRAFT_MY_HOST");
        cfg.port = var("DATACRAFT_MY_PORT").parse().unwrap_or(3306);
        cfg.user = var("DATACRAFT_MY_USER");
        cfg.password = var("DATACRAFT_MY_PASSWORD");
        cfg.database = var("DATACRAFT_MY_DB");
        transactional_import_rolls_back(cfg, "create table dc_imp(x int primary key)", "dc_imp");
    }

    /// Live, like `cancels_a_running_mysql_query`.
    #[test]
    #[ignore]
    fn mysql_read_only_is_enforced_by_the_server() {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let mut cfg = ConnectionConfig::new(10, Driver::MariaDb);
        cfg.host = var("DATACRAFT_MY_HOST");
        cfg.port = var("DATACRAFT_MY_PORT").parse().unwrap_or(3306);
        cfg.user = var("DATACRAFT_MY_USER");
        cfg.password = var("DATACRAFT_MY_PASSWORD");
        cfg.database = var("DATACRAFT_MY_DB");
        cfg.read_only = true;
        read_only_rejects_writes(cfg, "create table ro_probe(x int)");
    }

    /// Live: `DATACRAFT_MY_HOST/PORT/USER/PASSWORD` (see `db/mysql.rs`).
    #[test]
    #[ignore]
    fn cancels_a_running_mysql_query() {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let mut cfg = ConnectionConfig::new(8, Driver::MariaDb);
        cfg.host = var("DATACRAFT_MY_HOST");
        cfg.port = var("DATACRAFT_MY_PORT").parse().unwrap_or(3306);
        cfg.user = var("DATACRAFT_MY_USER");
        cfg.password = var("DATACRAFT_MY_PASSWORD");
        cfg.database = var("DATACRAFT_MY_DB");
        let w = Worker::start(egui::Context::default());
        w.send(Request::Connect(cfg));
        wait_for(&w, |r| matches!(r, Response::Connected { .. }));
        let start = Instant::now();
        w.send(Request::Execute { conn: 8, db: None, console: 1, statements: vec!["select sleep(30)".into()], max_rows: 10 });
        std::thread::sleep(Duration::from_millis(500));
        w.send(Request::Cancel { console: 1 });
        let r = wait_for(&w, |r| matches!(r, Response::Executed { console: 1, .. }));
        let Response::Executed { error, .. } = r else { unreachable!() };
        assert_eq!(error.as_deref(), Some(tr("Query cancelled")));
        assert!(start.elapsed() < Duration::from_secs(10));
        w.send(Request::Execute { conn: 8, db: None, console: 2, statements: vec!["select 1".into()], max_rows: 10 });
        let r = wait_for(&w, |r| matches!(r, Response::Executed { console: 2, .. }));
        let Response::Executed { error, .. } = r else { unreachable!() };
        assert!(error.is_none(), "{error:?}");
    }
}
