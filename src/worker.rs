//! Hilo de fondo con runtime tokio. La UI envía `Request`s y recoge `Response`s
//! con `Worker::poll` en cada frame; el worker pide repintado al terminar cada tarea.

use crate::db::{ConnectionConfig, ResultSet, SchemaInfo, Session};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;
use tokio::sync::Mutex;

pub type ConnId = u64;
pub type ConsoleId = u64;

pub enum Request {
    /// Abre (o reabre) la sesión de una conexión y carga su esquema.
    Connect(ConnectionConfig),
    /// Solo prueba la conexión y la cierra.
    Test(ConnectionConfig),
    Disconnect(ConnId),
    /// Recarga el árbol de esquema.
    Refresh(ConnId),
    /// Ejecuta sentencias en orden; se detiene en el primer error.
    Execute {
        conn: ConnId,
        console: ConsoleId,
        statements: Vec<String>,
        max_rows: usize,
    },
}

pub enum Response {
    Connected { conn: ConnId, schemas: Vec<SchemaInfo> },
    ConnectFailed { conn: ConnId, error: String },
    TestResult { conn: ConnId, result: Result<(), String> },
    Disconnected(ConnId),
    Schema { conn: ConnId, schemas: Vec<SchemaInfo> },
    /// `results` contiene lo ejecutado antes del error (si lo hubo).
    Executed {
        console: ConsoleId,
        results: Vec<ResultSet>,
        error: Option<String>,
        elapsed: std::time::Duration,
    },
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

#[derive(Default)]
struct State {
    sessions: Mutex<HashMap<ConnId, Arc<Mutex<Session>>>>,
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

    async fn session(&self, conn: ConnId) -> Option<Arc<Mutex<Session>>> {
        self.sessions.lock().await.get(&conn).cloned()
    }
}

async fn handle(req: Request, epoch: u64, state: &State) -> Vec<Response> {
    match req {
        Request::Connect(cfg) => {
            let conn = cfg.id;
            match connect_and_load(&cfg).await {
                Ok((session, schemas)) => {
                    let mut sessions = state.sessions.lock().await;
                    // Desconectada o reconectada mientras tanto: esta sesión sobra.
                    if state.current_epoch(conn) != epoch {
                        return vec![];
                    }
                    sessions.insert(conn, Arc::new(Mutex::new(session)));
                    vec![Response::Connected { conn, schemas }]
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
            state.sessions.lock().await.remove(&conn);
            vec![Response::Disconnected(conn)]
        }
        Request::Refresh(conn) => {
            let Some(session) = state.session(conn).await else {
                return vec![Response::ConnectFailed { conn, error: "No conectado".into() }];
            };
            let mut s = session.lock().await;
            match s.introspect().await {
                Ok(schemas) => vec![Response::Schema { conn, schemas }],
                Err(e) => vec![Response::ConnectFailed { conn, error: format!("{e:#}") }],
            }
        }
        Request::Execute { conn, console, statements, max_rows } => {
            let start = Instant::now();
            let Some(session) = state.session(conn).await else {
                return vec![Response::Executed {
                    console,
                    results: vec![],
                    error: Some("La conexión no está abierta".into()),
                    elapsed: start.elapsed(),
                }];
            };
            let mut s = session.lock().await;
            let mut results = Vec::new();
            let mut error = None;
            for stmt in statements {
                match s.execute(&stmt, max_rows).await {
                    Ok(rs) => results.extend(rs),
                    Err(e) => {
                        error = Some(format!("{e:#}"));
                        break;
                    }
                }
            }
            let lost = s.is_closed();
            drop(s);
            let mut out = vec![Response::Executed { console, results, error: error.clone(), elapsed: start.elapsed() }];
            if lost {
                // Se olvida la sesión rota; la UI reconectará en la siguiente ejecución.
                let mut sessions = state.sessions.lock().await;
                if sessions.get(&conn).is_some_and(|cur| Arc::ptr_eq(cur, &session)) {
                    sessions.remove(&conn);
                }
                let detail = error.unwrap_or_default();
                out.push(Response::ConnectFailed { conn, error: format!("Conexión perdida. {detail}") });
            }
            out
        }
    }
}

async fn connect_and_load(cfg: &ConnectionConfig) -> anyhow::Result<(Session, Vec<SchemaInfo>)> {
    let mut session = Session::connect(cfg).await?;
    let schemas = session.introspect().await?;
    Ok((session, schemas))
}
