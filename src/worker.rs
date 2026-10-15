//! Hilo de fondo con runtime tokio. La UI envía `Request`s y recoge `Response`s
//! con `Worker::poll` en cada frame; el worker pide repintado al terminar cada tarea.
//!
//! Cada conexión puede tener varias bases de datos. SQL Server y SQLite usan una única
//! sesión (con `USE [db]` antes de cada operación en SQL Server); PostgreSQL abre una
//! sesión por base, porque una conexión de PG no puede cambiar de base.

use crate::db::{ConnectionConfig, DatabaseInfo, Driver, ResultSet, SchemaInfo, Session};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;
use tokio::sync::Mutex;

pub type ConnId = u64;
pub type ConsoleId = u64;

pub enum Request {
    /// Abre (o reabre) la conexión, lista sus bases y carga el esquema de la base por defecto.
    Connect(ConnectionConfig),
    /// Solo prueba la conexión y la cierra.
    Test(ConnectionConfig),
    Disconnect(ConnId),
    /// Vuelve a listar las bases y recarga la base por defecto.
    Refresh(ConnId),
    /// Carga (o recarga) el esquema de una base concreta.
    LoadDatabase { conn: ConnId, db: String },
    /// Ejecuta sentencias en orden; se detiene en el primer error.
    /// `db`: base en la que ejecutar (`None` = la base por defecto de la conexión).
    Execute {
        conn: ConnId,
        db: Option<String>,
        console: ConsoleId,
        statements: Vec<String>,
        max_rows: usize,
    },
    /// Guarda cambios del editor de datos (sentencias de `edit::change_sql`) en una transacción.
    ApplyChanges {
        conn: ConnId,
        db: Option<String>,
        console: ConsoleId,
        statements: Vec<String>,
    },
}

pub enum Response {
    /// `databases` en orden; la por defecto lleva `current = true` y su esquema cargado.
    Connected { conn: ConnId, databases: Vec<DatabaseInfo> },
    ConnectFailed { conn: ConnId, error: String },
    TestResult { conn: ConnId, result: Result<(), String> },
    Disconnected(ConnId),
    /// Respuesta a `Refresh`: misma forma que `Connected`.
    Refreshed { conn: ConnId, databases: Vec<DatabaseInfo> },
    DatabaseLoaded { conn: ConnId, db: String, result: Result<Vec<SchemaInfo>, String> },
    /// `results` contiene lo ejecutado antes del error (si lo hubo).
    Executed {
        console: ConsoleId,
        results: Vec<ResultSet>,
        error: Option<String>,
        elapsed: std::time::Duration,
    },
    /// Resultado de `ApplyChanges`: filas modificadas o error (y entonces no se guardó nada).
    ChangesApplied { console: ConsoleId, result: Result<u64, String> },
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
                        // La generación se sella aquí, en orden de llegada: un Disconnect
                        // posterior a un Connect invalida ese Connect aunque termine después.
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

/// Conexión abierta: configuración (con contraseña, solo en memoria) y sus sesiones.
struct Live {
    cfg: ConnectionConfig,
    /// Base por defecto (la de la sesión principal).
    default_db: String,
    /// Clave "" = sesión principal. En PostgreSQL, además, una sesión por base extra.
    sessions: HashMap<String, SharedSession>,
}

#[derive(Default)]
struct State {
    live: Mutex<HashMap<ConnId, Live>>,
    /// Generación por conexión: cambia con cada Connect/Disconnect.
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

    /// Sesión con la que operar sobre `db` y el nombre de base a pasar al driver.
    /// En PostgreSQL abre (y guarda) una sesión nueva si la base no es la por defecto.
    async fn session_for(&self, conn: ConnId, db: Option<&str>) -> anyhow::Result<(SharedSession, String)> {
        let (cfg, default_db, main, existing) = {
            let live = self.live.lock().await;
            let l = live.get(&conn).ok_or_else(|| anyhow::anyhow!("La conexión no está abierta"))?;
            let target = db.filter(|d| !d.is_empty()).unwrap_or(&l.default_db).to_string();
            let main = l.sessions.get("").cloned().ok_or_else(|| anyhow::anyhow!("La conexión no está abierta"))?;
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
        // Si mientras tanto se desconectó, no se guarda (la sesión se cierra al soltarla).
        if self.current_epoch(conn) == epoch
            && let Some(l) = live.get_mut(&conn)
        {
            let s = l.sessions.entry(target.clone()).or_insert(session);
            return Ok((s.clone(), target));
        }
        Ok((session, target))
    }

    /// Quita la sesión si sigue siendo la misma (se perdió la conexión).
    async fn forget(&self, conn: ConnId, session: &SharedSession) {
        let mut live = self.live.lock().await;
        let Some(l) = live.get_mut(&conn) else { return };
        let key = l.sessions.iter().find(|(_, s)| Arc::ptr_eq(s, session)).map(|(k, _)| k.clone());
        match key.as_deref() {
            // Sin sesión principal la conexión entera queda cerrada.
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
                    // Desconectada o reconectada mientras tanto: esta sesión sobra.
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
                out.push(Response::ConnectFailed { conn, error: "Conexión perdida".into() });
            }
            out
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
            let mut results = Vec::new();
            let mut error = None;
            if let Err(e) = s.use_database(&target).await {
                error = Some(format!("{e:#}"));
            } else {
                for stmt in statements {
                    match s.execute(&stmt, max_rows).await {
                        Ok(rs) => results.extend(rs),
                        Err(e) => {
                            error = Some(format!("{e:#}"));
                            break;
                        }
                    }
                }
            }
            let lost = s.is_closed();
            drop(s);
            let mut out = vec![Response::Executed { console, results, error: error.clone(), elapsed: start.elapsed() }];
            if lost {
                // Se olvida la sesión rota; la UI reconectará en la siguiente ejecución.
                state.forget(conn, &session).await;
                let detail = error.unwrap_or_default();
                out.push(Response::ConnectFailed { conn, error: format!("Conexión perdida. {detail}") });
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

/// Lista de bases con el esquema de la por defecto ya cargado.
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
