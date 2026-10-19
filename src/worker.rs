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
    /// Opens (or reopens) the connection, lists its databases and loads the default database's schema.
    Connect(ConnectionConfig),
    /// Only tests the connection and closes it.
    Test(ConnectionConfig),
    Disconnect(ConnId),
    /// Lists the databases again and reloads the default one.
    Refresh(ConnId),
    /// Loads (or reloads) the schema of a specific database.
    LoadDatabase { conn: ConnId, db: String },
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
    RunDdl { conn: ConnId, db: Option<String>, statements: Vec<String>, close_db: Option<String> },
    /// Cancels what a console's `Execute` is running. Engines without a native cancel
    /// abandon the query and close the session (it reconnects on the next run).
    Cancel { console: ConsoleId },
}

pub enum Response {
    /// `databases` in order; the default one has `current = true` and its schema loaded.
    Connected { conn: ConnId, databases: Vec<DatabaseInfo> },
    ConnectFailed { conn: ConnId, error: String },
    TestResult { conn: ConnId, result: Result<(), String> },
    Disconnected(ConnId),
    /// Reply to `Refresh`: same shape as `Connected`.
    Refreshed { conn: ConnId, databases: Vec<DatabaseInfo> },
    DatabaseLoaded { conn: ConnId, db: String, result: Result<Vec<SchemaInfo>, String> },
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

/// A console's `Execute` in progress.
struct Running {
    native: Option<CancelHandle>,
    /// Abandons the query (engines without `native`).
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

async fn handle(req: Request, epoch: u64, state: &State) -> Vec<Response> {
    match req {
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
            let result = Session::connect(&cfg).await.map(|_| ()).map_err(|e| format!("{e:#}"));
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
        Request::RunDdl { conn, db, statements, close_db } => {
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
            if result.is_ok() {
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
            let running = state.running.lock().unwrap_or_else(|e| e.into_inner()).get(&console).cloned();
            if let Some(r) = running {
                r.cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                match &r.native {
                    Some(h) => {
                        if h.cancel().await.is_err() {
                            r.abandon.notify_one();
                        }
                    }
                    None => r.abandon.notify_one(),
                }
            }
            vec![]
        }
        Request::Execute { conn, db, console, statements, max_rows } => {
            let start = Instant::now();
            let (session, target) = match state.session_for(conn, db.as_deref()).await {
                Ok(s) => s,
                Err(e) => {
                    return vec![Response::Executed {
                        console,
                        results: vec![],
                        error: Some(format!("{e:#}")),
                        elapsed: start.elapsed(),
                    }];
                }
            };
            let mut s = session.lock().await;
            let running = Arc::new(Running {
                native: s.cancel_handle(),
                abandon: tokio::sync::Notify::new(),
                cancelled: std::sync::atomic::AtomicBool::new(false),
            });
            state.running.lock().unwrap_or_else(|e| e.into_inner()).insert(console, running.clone());
            let mut results = Vec::new();
            let mut error = None;
            let mut abandoned = false;
            if let Err(e) = s.use_database(&target).await {
                error = Some(format!("{e:#}"));
            } else {
                for stmt in statements {
                    let result = if running.native.is_some() {
                        s.execute(&stmt, max_rows).await
                    } else {
                        tokio::select! {
                            r = s.execute(&stmt, max_rows) => r,
                            _ = running.abandon.notified() => {
                                abandoned = true;
                                Err(anyhow::anyhow!(tr("Query cancelled")))
                            }
                        }
                    };
                    match result {
                        Ok(rs) => results.extend(rs),
                        Err(e) => {
                            error = Some(format!("{e:#}"));
                            break;
                        }
                    }
                    if running.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                }
            }
            state.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&console);
            if running.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
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
}
