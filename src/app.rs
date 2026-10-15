//! Estado principal de la aplicación y composición de la interfaz (estilo DataGrip "Islands").
//!
//! Variable de entorno de depuración: `DATACRAFT_DEMO=<ruta.db>` arranca con una conexión
//! SQLite temporal (no se guarda en `connections.json`) a ese fichero, la conecta, abre el
//! editor de datos de su primera tabla y una consola con una consulta de ejemplo. En ese modo
//! el estado de la UI se persiste en un directorio temporal (ver `main.rs`).
//! `DATACRAFT_DEMO_DARK=1` fuerza Islands Dark y `DATACRAFT_DEMO_DIALOG=1` abre el diálogo
//! de nueva conexión y `DATACRAFT_DEMO_TAB=console` deja activa la consola.

use crate::config;
use crate::db::{ColumnInfo, ConnectionConfig, Driver, SchemaInfo, Session};
use crate::sql;
use crate::ui::connection_dialog::{ConnectionDialog, DialogResult, TestState};
use crate::ui::console::{Console, ConsoleAction, ConsoleCtx, ResultTab};
use crate::ui::explorer::{self, ExplorerAction, ExplorerState, TableRef};
use crate::ui::icons::{self, Ico};
use crate::ui::results::{self, OutputEntry, ResultView};
use crate::ui::theme::{self, palette};
use crate::ui::{ConnStatusKind, quote_ident_if_needed};
use crate::worker::{ConnId, Request, Response, Worker};
use egui::{FontId, Key, Margin, Modifiers, Rect, RichText, Sense, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use std::time::Instant;

const STORAGE_KEY: &str = "datacraft_ui_state";
const DEFAULT_FONT_SIZE: f32 = 13.0;

#[derive(Clone, Debug, PartialEq)]
pub enum ConnStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

impl ConnStatus {
    fn kind(&self) -> ConnStatusKind {
        match self {
            ConnStatus::Disconnected => ConnStatusKind::Disconnected,
            ConnStatus::Connecting => ConnStatusKind::Connecting,
            ConnStatus::Connected => ConnStatusKind::Connected,
            ConnStatus::Error(_) => ConnStatusKind::Error,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            ConnStatus::Disconnected => "desconectado",
            ConnStatus::Connecting => "conectando…",
            ConnStatus::Connected => "conectado",
            ConnStatus::Error(_) => "error",
        }
    }
}

pub struct ConnState {
    pub cfg: ConnectionConfig,
    pub status: ConnStatus,
    pub schemas: Vec<SchemaInfo>,
}

#[derive(Serialize, Deserialize)]
struct PersistedConsole {
    title: String,
    text: String,
    conn: Option<u64>,
    /// (esquema, tabla) si la pestaña es un editor de datos.
    #[serde(default)]
    table: Option<(String, String)>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Persisted {
    /// Tema Islands Dark (por defecto Islands Light). Sustituye al antiguo `dark` (Darcula).
    islands_dark: bool,
    font_size: f32,
    max_rows: usize,
    consoles: Vec<PersistedConsole>,
    active: usize,
    console_counter: u64,
    show_explorer: bool,
    show_results: bool,
}

impl Default for Persisted {
    fn default() -> Self {
        Self {
            islands_dark: false,
            font_size: DEFAULT_FONT_SIZE,
            max_rows: 1000,
            consoles: Vec::new(),
            active: 0,
            console_counter: 0,
            show_explorer: true,
            show_results: true,
        }
    }
}

pub struct DataCraftApp {
    worker: Worker,
    conns: Vec<ConnState>,
    /// Pestañas del editor: consolas y editores de datos.
    consoles: Vec<Console>,
    active: usize,
    next_console_id: u64,
    console_counter: u64,
    result_salt: u64,
    explorer: ExplorerState,
    dialog: Option<ConnectionDialog>,
    confirm_delete: Option<ConnId>,
    renaming: Option<(u64, String)>,
    show_about: bool,
    show_explorer: bool,
    show_results: bool,
    dark: bool,
    applied_dark: Option<bool>,
    font_size: f32,
    max_rows: usize,
    status_msg: Option<(String, bool)>,
    /// Último id de conexión entregado (para que los ids sean monótonos).
    last_conn_id: u64,
    /// Modo demo: abrir el editor de datos de la primera tabla al conectar.
    demo_pending: bool,
}

impl DataCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let demo = std::env::var("DATACRAFT_DEMO").ok().filter(|s| !s.is_empty());
        let persisted: Persisted = if demo.is_some() {
            Persisted::default()
        } else {
            cc.storage.and_then(|s| eframe::get_value(s, STORAGE_KEY)).unwrap_or_default()
        };
        let conns: Vec<ConnState> = if demo.is_some() {
            Vec::new()
        } else {
            config::load_connections()
                .into_iter()
                .map(|cfg| ConnState { cfg, status: ConnStatus::Disconnected, schemas: Vec::new() })
                .collect()
        };
        let last_conn_id = conns.iter().map(|c| c.cfg.id).max().unwrap_or(0);

        theme::install_fonts(&cc.egui_ctx);
        let mut app = Self {
            worker: Worker::start(cc.egui_ctx.clone()),
            conns,
            consoles: Vec::new(),
            active: 0,
            next_console_id: 1,
            console_counter: persisted.console_counter,
            result_salt: 0,
            explorer: ExplorerState::default(),
            dialog: None,
            confirm_delete: None,
            renaming: None,
            show_about: false,
            show_explorer: persisted.show_explorer,
            show_results: persisted.show_results,
            dark: persisted.islands_dark,
            applied_dark: None,
            font_size: persisted.font_size.clamp(8.0, 32.0),
            max_rows: persisted.max_rows.max(1),
            status_msg: None,
            last_conn_id,
            demo_pending: false,
        };
        for pc in persisted.consoles {
            let conn = pc.conn.filter(|id| app.conns.iter().any(|c| c.cfg.id == *id));
            let id = app.alloc_console_id();
            match (pc.table, conn) {
                (Some((schema, table)), Some(conn)) => app.consoles.push(Console::new_table(id, conn, schema, table)),
                (Some(_), None) => {}
                (None, _) => app.consoles.push(Console::new(id, pc.title, pc.text, conn)),
            }
        }
        if app.consoles.is_empty() && demo.is_none() {
            let conn = app.conns.first().map(|c| c.cfg.id);
            app.new_console(conn, None, String::new());
        }
        app.active = persisted.active.min(app.consoles.len().saturating_sub(1));
        if let Some(path) = demo {
            app.start_demo(&path);
        }
        theme::apply(&cc.egui_ctx, app.dark);
        app.applied_dark = Some(app.dark);
        app
    }

    /// Modo demo (`DATACRAFT_DEMO`): conexión SQLite temporal, consola y editor de datos.
    fn start_demo(&mut self, path: &str) {
        let id = self.next_conn_id();
        let mut cfg = ConnectionConfig::new(id, Driver::Sqlite);
        cfg.name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.into());
        cfg.file_path = path.to_string();
        self.conns.push(ConnState { cfg, status: ConnStatus::Disconnected, schemas: Vec::new() });
        self.explorer.expanded.insert(format!("c:{id}"));
        self.explorer.selected = Some(format!("c:{id}"));
        self.dark = std::env::var("DATACRAFT_DEMO_DARK").is_ok_and(|v| v == "1");
        if std::env::var("DATACRAFT_DEMO_DIALOG").is_ok_and(|v| v == "1") {
            self.open_new_connection_dialog();
        }
        let idx = self.new_console(Some(id), None, "SELECT o.id, u.name, o.total\nFROM orders o\nJOIN users u ON u.id = o.user_id\nWHERE o.total > 10\nORDER BY o.total DESC;\n".into());
        self.run_console(idx, true);
        // El editor de datos de la primera tabla se abre al llegar el esquema.
        self.demo_pending = true;
    }

    fn alloc_console_id(&mut self) -> u64 {
        let id = self.next_console_id;
        self.next_console_id += 1;
        id
    }

    /// Crea una consola y la activa. Devuelve su índice.
    fn new_console(&mut self, conn: Option<ConnId>, title: Option<String>, text: String) -> usize {
        let id = self.alloc_console_id();
        let title = title.unwrap_or_else(|| {
            self.console_counter += 1;
            format!("console_{}", self.console_counter)
        });
        self.consoles.push(Console::new(id, title, text, conn));
        self.active = self.consoles.len() - 1;
        self.active
    }

    fn conn(&self, id: ConnId) -> Option<&ConnState> {
        self.conns.iter().find(|c| c.cfg.id == id)
    }

    fn conn_mut(&mut self, id: ConnId) -> Option<&mut ConnState> {
        self.conns.iter_mut().find(|c| c.cfg.id == id)
    }

    fn console_by_id(&mut self, id: u64) -> Option<&mut Console> {
        self.consoles.iter_mut().find(|c| c.id == id)
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.status_msg = Some((msg.into(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.status_msg = Some((msg.into(), true));
    }

    fn persist_connections(&mut self) {
        let list: Vec<ConnectionConfig> = self.conns.iter().map(|c| c.cfg.clone()).collect();
        if let Err(e) = config::save_connections(&list) {
            self.error(format!("No se pudieron guardar las conexiones: {e:#}"));
        }
    }

    /// Columnas conocidas de una tabla (para claves primarias y DDL).
    fn table_columns(&self, conn: ConnId, schema: &str, table: &str) -> Option<&[ColumnInfo]> {
        self.conn(conn)?
            .schemas
            .iter()
            .find(|s| s.name == schema)?
            .tables
            .iter()
            .find(|t| t.name == table)
            .map(|t| t.columns.as_slice())
    }

    // ---------------------------------------------------------------- conexión

    fn connect(&mut self, id: ConnId) {
        if let Some(c) = self.conn_mut(id) {
            if matches!(c.status, ConnStatus::Connecting) {
                return;
            }
            c.status = ConnStatus::Connecting;
            let cfg = c.cfg.clone();
            self.worker.send(Request::Connect(cfg));
        }
    }

    fn disconnect(&mut self, id: ConnId) {
        if let Some(c) = self.conn_mut(id) {
            c.status = ConnStatus::Disconnected;
            c.schemas.clear();
            self.worker.send(Request::Disconnect(id));
        }
        // Lo que esperaba a esta conexión ya no se va a ejecutar.
        for console in self.consoles.iter_mut().filter(|c| c.conn == Some(id)) {
            if console.pending.take().is_some() {
                console.run_started = None;
            }
        }
    }

    fn handle_responses(&mut self) {
        for resp in self.worker.poll() {
            match resp {
                Response::Connected { conn, schemas } => {
                    // Respuesta obsoleta: la conexión se desconectó, editó o borró mientras conectaba.
                    if !self.conn(conn).is_some_and(|c| c.status == ConnStatus::Connecting) {
                        self.worker.send(Request::Disconnect(conn));
                        continue;
                    }
                    if let Some(c) = self.conn_mut(conn) {
                        c.status = ConnStatus::Connected;
                        c.schemas = schemas;
                    }
                    let max_rows = self.max_rows;
                    for console in self.consoles.iter_mut().filter(|c| c.conn == Some(conn)) {
                        if let Some(statements) = console.pending.take() {
                            console.running = true;
                            self.worker.send(Request::Execute { conn, console: console.id, statements, max_rows });
                        }
                    }
                    if self.demo_pending {
                        self.demo_pending = false;
                        let first = self.conn(conn).and_then(|c| {
                            c.schemas.iter().find_map(|s| s.tables.first().map(|t| (s.name.clone(), t.name.clone())))
                        });
                        if let Some((schema, table)) = first {
                            self.explorer.expanded.insert(format!("t:{conn}:{schema}:{table}"));
                            self.explorer.selected = Some(format!("t:{conn}:{schema}:{table}"));
                            self.explorer.selected_table = Some(TableRef { conn, schema: schema.clone(), table: table.clone() });
                            self.open_table(&TableRef { conn, schema, table });
                        }
                        if std::env::var("DATACRAFT_DEMO_TAB").is_ok_and(|v| v == "console") {
                            self.active = 0;
                        }
                    }
                }
                Response::ConnectFailed { conn, error } => {
                    let was_connected = self.conn(conn).is_some_and(|c| c.status == ConnStatus::Connected);
                    if let Some(c) = self.conn_mut(conn) {
                        // También si estaba conectada: la sesión se ha perdido y la próxima
                        // ejecución volverá a conectar.
                        c.status = ConnStatus::Error(error.clone());
                    }
                    if was_connected {
                        self.error(format!("Conexión perdida: {error}"));
                    }
                    for console in self.consoles.iter_mut().filter(|c| c.conn == Some(conn)) {
                        if console.pending.take().is_some() {
                            console.run_started = None;
                            console.log.push(OutputEntry {
                                time: now(),
                                statement: String::new(),
                                message: format!("No se pudo conectar: {error}"),
                                is_error: true,
                            });
                            console.result_tab = ResultTab::Output;
                        }
                    }
                }
                Response::TestResult { conn, result } => {
                    if let Some(d) = self.dialog.as_mut()
                        && d.cfg.id == conn
                    {
                        d.test = TestState::Done(result);
                    }
                }
                Response::Disconnected(conn) => {
                    if let Some(c) = self.conn_mut(conn)
                        && c.status != ConnStatus::Connecting
                    {
                        c.status = ConnStatus::Disconnected;
                        c.schemas.clear();
                    }
                }
                Response::Schema { conn, schemas } => {
                    if let Some(c) = self.conn_mut(conn) {
                        c.schemas = schemas;
                    }
                    self.info("Esquema actualizado");
                }
                Response::Executed { console, results, error, elapsed } => {
                    self.result_salt += 1;
                    let salt = self.result_salt;
                    if let Some(c) = self.console_by_id(console) {
                        apply_execution(c, results, error, elapsed, salt);
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- ejecución

    fn run_console(&mut self, idx: usize, all: bool) {
        let Some(console) = self.consoles.get(idx) else { return };
        if console.table.is_some() {
            self.reload_table(idx);
            return;
        }
        let conn_id = console.conn;
        let driver = conn_id.and_then(|id| self.conn(id)).map(|c| c.cfg.driver).unwrap_or(Driver::Postgres);
        let statements: Vec<String> = if all {
            sql::split_batches(&console.text, driver).into_iter().map(|s| s.text).collect()
        } else if let Some(sel) = console.selection() {
            sql::split_batches(sel, driver).into_iter().map(|s| s.text).collect()
        } else {
            sql::statement_at(&console.text, console.cursor_byte(), driver).into_iter().map(|s| s.text).collect()
        };
        self.run_statements(idx, statements);
    }

    /// Envía sentencias de la pestaña `idx`, conectando antes si hace falta.
    fn run_statements(&mut self, idx: usize, statements: Vec<String>) {
        let Some(console) = self.consoles.get(idx) else { return };
        if console.is_busy() {
            return;
        }
        let conn_id = console.conn;
        let status = conn_id.and_then(|id| self.conn(id)).map(|c| c.status.clone());
        let max_rows = self.max_rows;

        let Some(console) = self.consoles.get_mut(idx) else { return };
        if statements.is_empty() {
            console.log.push(OutputEntry { time: now(), statement: String::new(), message: "Nada que ejecutar".into(), is_error: true });
            console.result_tab = ResultTab::Output;
            return;
        }
        let (Some(conn), Some(status)) = (conn_id, status) else {
            console.log.push(OutputEntry {
                time: now(),
                statement: String::new(),
                message: "Selecciona una conexión para la consola".into(),
                is_error: true,
            });
            console.result_tab = ResultTab::Output;
            return;
        };
        console.last_statements = statements.clone();
        console.run_started = Some(Instant::now());
        match status {
            ConnStatus::Connected => {
                console.running = true;
                let id = console.id;
                self.worker.send(Request::Execute { conn, console: id, statements, max_rows });
            }
            ConnStatus::Connecting => console.pending = Some(statements),
            ConnStatus::Disconnected | ConnStatus::Error(_) => {
                console.pending = Some(statements);
                self.connect(conn);
            }
        }
    }

    /// Abre (o activa) el editor de datos de una tabla.
    fn open_table(&mut self, t: &TableRef) {
        if let Some(i) = self.consoles.iter().position(|c| {
            c.conn == Some(t.conn) && c.table.as_ref().is_some_and(|x| x.schema == t.schema && x.table == t.table)
        }) {
            self.active = i;
            self.reload_table(i);
            return;
        }
        let id = self.alloc_console_id();
        self.consoles.push(Console::new_table(id, t.conn, t.schema.clone(), t.table.clone()));
        self.active = self.consoles.len() - 1;
        self.reload_table(self.active);
    }

    /// Recarga el editor de datos con sus filtros, limitando las filas en el propio SQL.
    fn reload_table(&mut self, idx: usize) {
        let Some(c) = self.consoles.get(idx) else { return };
        let (Some(conn), Some(t)) = (c.conn, c.table.as_ref()) else { return };
        let Some(driver) = self.conn(conn).map(|c| c.cfg.driver) else { return };
        let sql = table_query(driver, &t.schema, &t.table, &t.where_clause, &t.order_by, self.max_rows);
        if let Some(t) = self.consoles.get_mut(idx).and_then(|c| c.table.as_mut()) {
            t.loaded = true;
        }
        self.run_statements(idx, vec![sql]);
    }

    fn generate(&mut self, ctx: &egui::Context, t: &TableRef, kind: Generate) {
        let Some(c) = self.conn(t.conn) else { return };
        let driver = c.cfg.driver;
        let Some(columns) = self.table_columns(t.conn, &t.schema, &t.table) else {
            return;
        };
        let cols: Vec<String> = columns.iter().map(|c| quote_ident_if_needed(driver, &c.name)).collect();
        let table = Session::quote_table(driver, &t.schema, &t.table);
        let text = match kind {
            Generate::Insert => {
                let values: Vec<&str> = cols.iter().map(|_| "NULL").collect();
                format!("INSERT INTO {table} ({})\nVALUES ({});", cols.join(", "), values.join(", "))
            }
            Generate::Select if cols.is_empty() => format!("SELECT *\nFROM {table};"),
            Generate::Select => format!("SELECT {}\nFROM {table};", cols.join(", ")),
            Generate::Ddl => ddl(driver, &table, columns),
        };
        let target = self
            .consoles
            .get(self.active)
            .filter(|c| c.conn == Some(t.conn) && c.table.is_none())
            .map(|_| self.active);
        match target {
            Some(i) => self.consoles[i].append_text(ctx, &text),
            None => {
                let title = (kind == Generate::Ddl).then(|| format!("{}.sql", t.table));
                self.new_console(Some(t.conn), title, text);
            }
        }
    }

    fn handle_explorer(&mut self, ctx: &egui::Context, actions: Vec<ExplorerAction>) {
        for a in actions {
            match a {
                ExplorerAction::NewConnection => self.open_new_connection_dialog(),
                ExplorerAction::Connect(id) => self.connect(id),
                ExplorerAction::Disconnect(id) => self.disconnect(id),
                ExplorerAction::Refresh(id) => self.worker.send(Request::Refresh(id)),
                ExplorerAction::Edit(id) => {
                    if let Some(c) = self.conn(id) {
                        self.dialog = Some(ConnectionDialog::edit(c.cfg.clone()));
                    }
                }
                ExplorerAction::Duplicate(id) => {
                    if let Some(c) = self.conn(id) {
                        let mut cfg = c.cfg.clone();
                        cfg.id = self.next_conn_id();
                        cfg.name = format!("{} (copia)", cfg.name);
                        config::store_password(cfg.id, &cfg.password);
                        self.conns.push(ConnState { cfg, status: ConnStatus::Disconnected, schemas: Vec::new() });
                        self.persist_connections();
                    }
                }
                ExplorerAction::Delete(id) => self.confirm_delete = Some(id),
                ExplorerAction::NewConsole(id) => {
                    self.new_console(Some(id), None, String::new());
                    if self.conn(id).is_some_and(|c| matches!(c.status, ConnStatus::Disconnected | ConnStatus::Error(_))) {
                        self.connect(id);
                    }
                }
                ExplorerAction::OpenTable(t) => self.open_table(&t),
                ExplorerAction::GenerateSelect(t) => self.generate(ctx, &t, Generate::Select),
                ExplorerAction::GenerateInsert(t) => self.generate(ctx, &t, Generate::Insert),
                ExplorerAction::GenerateDdl(t) => self.generate(ctx, &t, Generate::Ddl),
                ExplorerAction::Hide => self.show_explorer = false,
            }
        }
    }

    /// Ids monótonos: nunca se reutiliza el id de una conexión borrada (su contraseña en el
    /// llavero o una respuesta tardía del worker podrían asociarse a la nueva).
    fn next_conn_id(&mut self) -> u64 {
        let max = self.conns.iter().map(|c| c.cfg.id).max().unwrap_or(0).max(self.last_conn_id);
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let id = (max + 1).max(millis);
        self.last_conn_id = id;
        id
    }

    fn open_new_connection_dialog(&mut self) {
        let id = self.next_conn_id();
        self.dialog = Some(ConnectionDialog::new_connection(id));
    }

    fn save_connection(&mut self, cfg: ConnectionConfig) {
        config::store_password(cfg.id, &cfg.password);
        let id = cfg.id;
        if let Some(existing) = self.conn_mut(id) {
            existing.cfg = cfg;
            if existing.status != ConnStatus::Disconnected {
                self.disconnect(id);
            }
        } else {
            self.conns.push(ConnState { cfg, status: ConnStatus::Disconnected, schemas: Vec::new() });
            if let Some(c) = self.consoles.get_mut(self.active)
                && c.conn.is_none()
            {
                c.conn = Some(id);
            }
        }
        self.persist_connections();
        self.info("Conexión guardada");
    }

    fn delete_connection(&mut self, id: ConnId) {
        self.worker.send(Request::Disconnect(id));
        self.conns.retain(|c| c.cfg.id != id);
        config::delete_password(id);
        for c in self.consoles.iter_mut().filter(|c| c.conn == Some(id)) {
            c.conn = None;
            c.pending = None;
            c.run_started = None;
        }
        // Los editores de datos de esa conexión ya no tienen sentido.
        let active_id = self.consoles.get(self.active).map(|c| c.id);
        self.consoles.retain(|c| !(c.table.is_some() && c.conn.is_none()));
        self.active = active_id
            .and_then(|id| self.consoles.iter().position(|c| c.id == id))
            .unwrap_or(0)
            .min(self.consoles.len().saturating_sub(1));
        self.persist_connections();
    }

    // ---------------------------------------------------------------- ficheros

    fn open_sql_file(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title("Abrir script SQL").add_filter("SQL", &["sql"]).pick_file() else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
                let conn = self.consoles.get(self.active).and_then(|c| c.conn);
                self.new_console(conn, title, text);
            }
            Err(e) => self.error(format!("No se pudo abrir {}: {e}", path.display())),
        }
    }

    fn save_console_as(&mut self) {
        let Some(console) = self.consoles.get(self.active).filter(|c| c.table.is_none()) else { return };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Guardar consola como")
            .add_filter("SQL", &["sql"])
            .set_file_name(format!("{}.sql", console.title))
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, &console.text) {
            Ok(()) => self.info(format!("Guardado en {}", path.display())),
            Err(e) => self.error(format!("No se pudo guardar: {e}")),
        }
    }

    fn close_console(&mut self, idx: usize) {
        if idx < self.consoles.len() {
            self.consoles.remove(idx);
            if self.active > idx || self.active >= self.consoles.len() {
                self.active = self.active.saturating_sub(1);
            }
        }
    }

    fn new_console_here(&mut self) {
        let conn = self.consoles.get(self.active).and_then(|c| c.conn).or_else(|| self.conns.first().map(|c| c.cfg.id));
        self.new_console(conn, None, String::new());
    }

    // ---------------------------------------------------------------- UI

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.confirm_delete.is_some() || self.show_about {
            return;
        }
        let (run_all, run, new_console, bigger, smaller, reload, quit) = ctx.input_mut(|i| {
            let run_all = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
            let run = i.consume_key(Modifiers::COMMAND, Key::Enter);
            let new_console = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::N);
            let bigger = i.consume_key(Modifiers::COMMAND, Key::Equals) || i.consume_key(Modifiers::COMMAND, Key::Plus);
            let smaller = i.consume_key(Modifiers::COMMAND, Key::Minus);
            let reload = i.consume_key(Modifiers::COMMAND, Key::R);
            let quit = i.consume_key(Modifiers::COMMAND, Key::Q);
            (run_all, run, new_console, bigger, smaller, reload, quit)
        });
        if run_all {
            self.run_console(self.active, true);
        } else if run {
            self.run_console(self.active, false);
        }
        if reload && self.consoles.get(self.active).is_some_and(|c| c.table.is_some()) {
            self.reload_table(self.active);
        }
        if new_console {
            self.new_console_here();
        }
        if bigger {
            self.font_size = (self.font_size + 1.0).min(32.0);
        }
        if smaller {
            self.font_size = (self.font_size - 1.0).max(8.0);
        }
        if quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Menú principal de la aplicación (bajo el widget de proyecto "DataCraft ⌄").
    fn app_menu(&mut self, ui: &mut Ui) {
        ui.set_min_width(220.0);
        ui.menu_button("Archivo", |ui| {
            if ui.button("Nuevo data source…").clicked() {
                self.open_new_connection_dialog();
                ui.close();
            }
            if ui.add(egui::Button::new("Nueva consola").shortcut_text("⇧⌘N")).clicked() {
                self.new_console_here();
                ui.close();
            }
            ui.separator();
            if ui.button("Abrir .sql…").clicked() {
                ui.close();
                self.open_sql_file();
            }
            let can_save = self.consoles.get(self.active).is_some_and(|c| c.table.is_none());
            if ui.add_enabled(can_save, egui::Button::new("Guardar consola como .sql…")).clicked() {
                ui.close();
                self.save_console_as();
            }
            ui.separator();
            if ui.add(egui::Button::new("Salir").shortcut_text("⌘Q")).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Ver", |ui| self.view_menu(ui));
        ui.menu_button("Ayuda", |ui| {
            if ui.button("Acerca de DataCraft").clicked() {
                self.show_about = true;
                ui.close();
            }
        });
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        let p = palette(self.dark);
        ui.label(RichText::new("Tema").small().color(p.muted));
        if ui.radio(!self.dark, "Islands Light").clicked() {
            self.dark = false;
            ui.close();
        }
        if ui.radio(self.dark, "Islands Dark").clicked() {
            self.dark = true;
            ui.close();
        }
        ui.separator();
        ui.checkbox(&mut self.show_explorer, "Database Explorer");
        ui.checkbox(&mut self.show_results, "Services (resultados)");
        ui.separator();
        ui.label(RichText::new(format!("Fuente del editor: {:.0} pt", self.font_size)).small().color(p.muted));
        if ui.add(egui::Button::new("Aumentar").shortcut_text("⌘+")).clicked() {
            self.font_size = (self.font_size + 1.0).min(32.0);
        }
        if ui.add(egui::Button::new("Reducir").shortcut_text("⌘-")).clicked() {
            self.font_size = (self.font_size - 1.0).max(8.0);
        }
        if ui.button("Restablecer").clicked() {
            self.font_size = DEFAULT_FONT_SIZE;
        }
    }

    /// Barra superior unificada con el título (toolbar principal).
    fn title_bar(&mut self, ui: &mut Ui) {
        let p = palette(self.dark);
        let rect = ui.max_rect();
        let bg = ui.interact(rect, egui::Id::new("title_bar_drag"), Sense::click_and_drag());
        if bg.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if bg.double_clicked() {
            let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space(if fullscreen { 10.0 } else { 80.0 });

            // Widget de proyecto: cuadrado morado con la inicial + nombre + chevron.
            let font = theme::bold(13.0);
            let galley = ui.painter().layout_no_wrap("DataCraft".into(), font, p.text);
            let w = 8.0 + 20.0 + 8.0 + galley.size().x + 4.0 + 14.0 + 8.0;
            let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
            if resp.hovered() || resp.is_pointer_button_down_on() {
                ui.painter().rect_filled(r, 6.0, if resp.is_pointer_button_down_on() { p.pressed } else { p.hover });
            }
            let sq = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 10.0), vec2(20.0, 20.0));
            ui.painter().rect_filled(sq, 5.0, p.project_badge);
            ui.painter().text(sq.center(), egui::Align2::CENTER_CENTER, "D", theme::bold(11.5), egui::Color32::WHITE);
            ui.painter().galley(pos2(sq.right() + 8.0, r.center().y - galley.size().y / 2.0), galley.clone(), p.text);
            icons::paint(
                ui.painter(),
                Rect::from_min_size(pos2(sq.right() + 8.0 + galley.size().x + 4.0, r.center().y - 7.0), vec2(14.0, 14.0)),
                Ico::ChevronDown,
                p.muted,
                &p,
                true,
            );
            egui::Popup::menu(&resp).show(|ui| self.app_menu(ui));

            // Selector de la conexión de la pestaña activa.
            let current = self.consoles.get(self.active).and_then(|c| c.conn).and_then(|id| self.conn(id));
            let (ico, label) = match current {
                Some(c) => (Ico::Driver(c.cfg.driver), c.cfg.name.clone()),
                None => (Ico::Database, "Sin conexión".into()),
            };
            let busy = self.consoles.get(self.active).is_some_and(|c| c.is_busy() || c.table.is_some());
            let resp = theme::flat_button(ui, Some(ico), &label, FontId::proportional(13.0), true, true)
                .on_hover_text("Data source de la pestaña activa");
            let mut chosen = None;
            let mut new_ds = false;
            egui::Popup::menu(&resp).show(|ui| {
                ui.set_min_width(220.0);
                for c in &self.conns {
                    let sel = current.is_some_and(|x| x.cfg.id == c.cfg.id);
                    if ui
                        .add_enabled(!busy || sel, egui::Button::selectable(sel, format!("{}  ·  {}", c.cfg.name, c.cfg.driver.label())))
                        .clicked()
                    {
                        chosen = Some(c.cfg.id);
                        ui.close();
                    }
                }
                if !self.conns.is_empty() {
                    ui.separator();
                }
                if ui.button("Nuevo data source…").clicked() {
                    new_ds = true;
                    ui.close();
                }
            });
            if new_ds {
                self.open_new_connection_dialog();
            }
            if let Some(id) = chosen {
                self.set_active_connection(id);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                let gear = theme::icon_button_sized(ui, Ico::Gear, true, "Ajustes", 30.0);
                egui::Popup::menu(&gear).show(|ui| {
                    self.view_menu(ui);
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Máx. filas:");
                        ui.add(egui::DragValue::new(&mut self.max_rows).range(1..=1_000_000).speed(10));
                    });
                });
                if theme::icon_button_sized(ui, Ico::Search, true, "Buscar tablas", 30.0).clicked() {
                    self.show_explorer = true;
                    self.explorer.show_filter = !self.explorer.show_filter;
                }
            });
        });
    }

    /// Cambia la conexión de la consola activa (si no está ocupada).
    fn set_active_connection(&mut self, id: ConnId) {
        let needs_connect = self.conn(id).is_some_and(|c| matches!(c.status, ConnStatus::Disconnected | ConnStatus::Error(_)));
        if let Some(c) = self.consoles.get_mut(self.active) {
            if c.is_busy() || c.table.is_some() || c.conn == Some(id) {
                return;
            }
            c.conn = Some(id);
            c.pending = None;
            c.run_started = None;
        } else {
            self.new_console(Some(id), None, String::new());
        }
        if needs_connect {
            self.connect(id);
        }
    }

    /// Franja izquierda de tool windows.
    fn left_stripe(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let mut top = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Center)));
        top.spacing_mut().item_spacing.y = 4.0;
        top.add_space(2.0);
        if stripe_button(&mut top, Ico::Database, self.show_explorer, "Database Explorer").clicked() {
            self.show_explorer = !self.show_explorer;
        }
        let more = stripe_button(&mut top, Ico::More, false, "Más tool windows");
        egui::Popup::menu(&more).show(|ui| {
            ui.checkbox(&mut self.show_explorer, "Database Explorer");
            ui.checkbox(&mut self.show_results, "Services");
        });

        let mut bottom = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::bottom_up(egui::Align::Center)));
        bottom.spacing_mut().item_spacing.y = 4.0;
        bottom.add_space(4.0);
        let has_error = self.status_msg.as_ref().is_some_and(|(_, e)| *e);
        let problems = stripe_button(&mut bottom, Ico::Problems, false, "Problemas");
        if problems.clicked() {
            if let Some((msg, _)) = &self.status_msg {
                ui.ctx().copy_text(msg.clone());
            }
        }
        if has_error {
            let c = problems.rect.right_top() + vec2(-9.0, 9.0);
            bottom.painter().circle_filled(c, 3.0, palette(self.dark).error);
        }
        if stripe_button(&mut bottom, Ico::Terminal, false, "Nueva consola (⇧⌘N)").clicked() {
            self.new_console_here();
        }
        if stripe_button(&mut bottom, Ico::Services, self.show_results, "Services (resultados)").clicked() {
            self.show_results = !self.show_results;
        }
    }

    /// Franja derecha (notificaciones y estructura).
    fn right_stripe(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let mut top = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Center)));
        top.spacing_mut().item_spacing.y = 4.0;
        top.add_space(2.0);
        let msg = self.status_msg.clone();
        let bell = stripe_button(&mut top, Ico::Bell, false, "Notificaciones");
        egui::Popup::menu(&bell).show(|ui| {
            ui.set_min_width(260.0);
            match &msg {
                Some((m, err)) => {
                    let color = if *err { palette(self.dark).error } else { palette(self.dark).text };
                    ui.add(egui::Label::new(RichText::new(m).color(color)).wrap());
                    if ui.button("Descartar").clicked() {
                        self.status_msg = None;
                        ui.close();
                    }
                }
                None => {
                    ui.label(RichText::new("Sin notificaciones").color(palette(self.dark).muted));
                }
            }
        });
        if msg.is_some() {
            let c = bell.rect.right_top() + vec2(-9.0, 9.0);
            top.painter().circle_filled(c, 3.0, palette(self.dark).accent);
        }
        let structure = stripe_button(&mut top, Ico::Structure, false, "Estructura de la pestaña");
        egui::Popup::menu(&structure).show(|ui| {
            let cols = self.consoles.get(self.active).and_then(|c| {
                let t = c.table.as_ref()?;
                self.table_columns(c.conn?, &t.schema, &t.table)
            });
            match cols {
                Some(cols) => {
                    for c in cols {
                        ui.label(format!("{}  {}", c.name, c.data_type.to_lowercase()));
                    }
                }
                None => {
                    ui.label(RichText::new("Sin estructura").color(palette(self.dark).muted));
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let p = palette(self.dark);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(12.0);
            // Breadcrumbs de la pestaña activa.
            let console = self.consoles.get(self.active);
            let conn = console.and_then(|c| c.conn).and_then(|id| self.conn(id));
            let mut crumbs: Vec<(Option<Ico>, String)> = vec![(None, "Database".into())];
            if let Some(c) = conn {
                crumbs.push((Some(Ico::Driver(c.cfg.driver)), c.cfg.name.clone()));
            }
            if let Some(c) = console {
                match &c.table {
                    Some(t) => {
                        crumbs.push((Some(Ico::Schema), t.schema.clone()));
                        crumbs.push((Some(Ico::Folder), "tables".into()));
                        crumbs.push((Some(Ico::Table), t.table.clone()));
                    }
                    None => crumbs.push((Some(Ico::Console), c.title.clone())),
                }
            }
            let n = crumbs.len();
            for (i, (ico, text)) in crumbs.into_iter().enumerate() {
                breadcrumb(ui, ico, &text);
                if i + 1 < n {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    icons::paint(ui.painter(), r.shrink(1.0), Ico::ChevronRight, p.muted, &p, true);
                }
            }
            if let Some(c) = conn
                && !matches!(c.status, ConnStatus::Connected)
            {
                ui.add_space(8.0);
                let color = match c.status {
                    ConnStatus::Error(_) => p.error,
                    ConnStatus::Connecting => p.warn,
                    _ => p.muted,
                };
                ui.label(RichText::new(c.status.label()).color(color).size(12.0));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), r, Ico::Lock, p.text_muted, &p, true);
                ui.add_space(6.0);
                ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
                ui.add(
                    egui::DragValue::new(&mut self.max_rows)
                        .range(1..=1_000_000)
                        .speed(10)
                        .prefix("max rows: "),
                )
                .on_hover_text("Máximo de filas por consulta (arrastra o haz doble clic para editar)");
                if let Some(c) = self.consoles.get(self.active) {
                    if c.table.is_none() {
                        let (l, col) = c.line_col();
                        ui.add_space(8.0);
                        ui.label(RichText::new(format!("{l}:{col}")).color(p.text_muted).size(12.5));
                    }
                    if let Some(r) = c.results.last() {
                        ui.add_space(8.0);
                        let text = if r.rs.is_command() {
                            results::affected_message(&r.rs)
                        } else {
                            format!("{} filas · {}", r.row_count(), results::format_duration(r.rs.elapsed))
                        };
                        ui.label(RichText::new(text).color(p.text_muted).size(12.5));
                    }
                }
                if let Some((msg, is_err)) = &self.status_msg {
                    ui.add_space(8.0);
                    let color = if *is_err { p.error } else { p.text_muted };
                    let resp = ui.add(egui::Label::new(RichText::new(msg).color(color).size(12.5)).truncate().sense(Sense::click()));
                    if resp.on_hover_text("Clic para descartar").clicked() {
                        self.status_msg = None;
                    }
                }
            });
        });
    }

    /// Pestañas del editor (arriba de la isla del editor).
    fn editor_tabs(&mut self, ui: &mut Ui) {
        let mut close = None;
        let mut activate = None;
        let mut start_rename = None;
        let mut new = false;
        let mut close_all = false;
        let w = ui.available_width();
        ui.allocate_ui_with_layout(vec2(w, 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_size(vec2(w, 40.0));
            ui.add_space(6.0);
            let scroll_w = (w - 46.0).max(60.0);
            ui.allocate_ui_with_layout(vec2(scroll_w, 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                egui::ScrollArea::horizontal().id_salt("editor_tabs").max_width(scroll_w).show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (i, c) in self.consoles.iter().enumerate() {
                            if let Some((id, name)) = self.renaming.as_mut()
                                && *id == c.id
                            {
                                let resp = ui.add(egui::TextEdit::singleline(name).desired_width(140.0).margin(vec2(6.0, 4.0)));
                                resp.request_focus();
                                let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                                if esc {
                                    self.renaming = None;
                                } else if enter || resp.lost_focus() {
                                    let name = name.trim().to_string();
                                    let id = *id;
                                    self.renaming = None;
                                    if !name.is_empty() {
                                        ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("rename_result"), (id, name)));
                                    }
                                }
                                continue;
                            }
                            let ico = if c.table.is_some() { Ico::Table } else { Ico::Console };
                            let busy = c.is_busy();
                            let title = if busy { format!("{} …", c.title) } else { c.title.clone() };
                            let (resp, closed) = theme::tab(ui, i == self.active, Some(ico), &title, true);
                            if closed {
                                close = Some(i);
                            } else if resp.clicked() {
                                activate = Some(i);
                            }
                            if resp.double_clicked() && c.table.is_none() {
                                start_rename = Some((c.id, c.title.clone()));
                            }
                            if resp.middle_clicked() {
                                close = Some(i);
                            }
                            resp.context_menu(|ui| {
                                if ui.add_enabled(c.table.is_none(), egui::Button::new("Renombrar")).clicked() {
                                    start_rename = Some((c.id, c.title.clone()));
                                    ui.close();
                                }
                                if ui.button("Cerrar").clicked() {
                                    close = Some(i);
                                    ui.close();
                                }
                            });
                        }
                    });
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                let more = theme::icon_button(ui, Ico::MoreVert, true, "Pestañas");
                egui::Popup::menu(&more).show(|ui| {
                    if ui.add(egui::Button::new("Nueva consola").shortcut_text("⇧⌘N")).clicked() {
                        new = true;
                        ui.close();
                    }
                    if ui.add_enabled(!self.consoles.is_empty(), egui::Button::new("Cerrar todas")).clicked() {
                        close_all = true;
                        ui.close();
                    }
                    if !self.consoles.is_empty() {
                        ui.separator();
                        for (i, c) in self.consoles.iter().enumerate() {
                            if ui.selectable_label(i == self.active, &c.title).clicked() {
                                activate = Some(i);
                                ui.close();
                            }
                        }
                    }
                });
            });
        });
        if let Some((id, name)) = ui.memory_mut(|m| m.data.remove_temp::<(u64, String)>(egui::Id::new("rename_result")))
            && let Some(c) = self.console_by_id(id)
        {
            c.title = name;
        }
        if let Some(i) = activate {
            self.active = i;
        }
        if start_rename.is_some() {
            self.renaming = start_rename;
        }
        if let Some(i) = close {
            self.close_console(i);
        }
        if close_all {
            self.consoles.clear();
            self.active = 0;
        }
        if new {
            self.new_console_here();
        }
    }

    fn central(&mut self, ui: &mut Ui) {
        let p = palette(self.dark);
        // Carga diferida de editores de datos restaurados.
        if let Some(c) = self.consoles.get(self.active)
            && c.table.as_ref().is_some_and(|t| !t.loaded)
            && !c.is_busy()
        {
            self.reload_table(self.active);
        }
        let is_console = self.consoles.get(self.active).is_some_and(|c| c.table.is_none());
        let mut actions = Vec::new();

        if is_console && self.show_results {
            egui::Panel::bottom(egui::Id::new("results_island"))
                .resizable(true)
                .default_size(300.0)
                .size_range(120.0..=2000.0)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.inner_margin(Margin { top: 2 * theme::ISLAND_GAP, ..Margin::ZERO }))
                .show(ui, |ui| {
                    theme::island_frame(&p).show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        let idx = self.active;
                        let (conn_list, cx_columns) = (self.conn_list(), None);
                        let conn = self.consoles[idx].conn.and_then(|id| self.conns.iter().find(|c| c.cfg.id == id));
                        let cx = ConsoleCtx {
                            connections: &conn_list,
                            schemas: conn.map(|c| c.schemas.as_slice()),
                            driver: conn.map(|c| c.cfg.driver).unwrap_or(Driver::Postgres),
                            dark: self.dark,
                            font_size: self.font_size,
                            columns: cx_columns,
                        };
                        actions.extend(self.consoles[idx].show_results(ui, &cx));
                    });
                });
        }

        theme::island_frame(&p).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            self.editor_tabs(ui);
            separator(ui, p.island_border);
            if self.consoles.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.label(RichText::new("No hay pestañas abiertas").color(p.muted));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Nueva consola  ⇧⌘N").color(p.muted));
                    ui.add_space(8.0);
                    if theme::primary_button(ui, "Nueva consola").clicked() {
                        self.new_console_here();
                    }
                });
                return;
            }
            let idx = self.active.min(self.consoles.len() - 1);
            self.active = idx;
            let conn_list = self.conn_list();
            let conn_id = self.consoles[idx].conn;
            let conn = conn_id.and_then(|id| self.conns.iter().find(|c| c.cfg.id == id));
            let columns = match (&self.consoles[idx].table, conn) {
                (Some(t), Some(c)) => c
                    .schemas
                    .iter()
                    .find(|s| s.name == t.schema)
                    .and_then(|s| s.tables.iter().find(|x| x.name == t.table))
                    .map(|t| t.columns.as_slice()),
                _ => None,
            };
            let cx = ConsoleCtx {
                connections: &conn_list,
                schemas: conn.map(|c| c.schemas.as_slice()),
                driver: conn.map(|c| c.cfg.driver).unwrap_or(Driver::Postgres),
                dark: self.dark,
                font_size: self.font_size,
                columns,
            };
            let console = &mut self.consoles[idx];
            let out = if console.table.is_some() { console.show_table(ui, &cx) } else { console.show_console(ui, &cx) };
            actions.extend(out);
        });

        let idx = self.active;
        for a in actions {
            match a {
                ConsoleAction::Run => self.run_console(idx, false),
                ConsoleAction::RunAll => self.run_console(idx, true),
                ConsoleAction::Reload => self.reload_table(idx),
                ConsoleAction::Ddl => {
                    if let Some(c) = self.consoles.get(idx)
                        && let (Some(conn), Some(t)) = (c.conn, c.table.as_ref())
                    {
                        let tref = TableRef { conn, schema: t.schema.clone(), table: t.table.clone() };
                        self.generate(ui.ctx(), &tref, Generate::Ddl);
                    }
                }
                ConsoleAction::Connect(id) => self.connect(id),
                ConsoleAction::Error(e) => self.error(e),
            }
        }
    }

    fn conn_list(&self) -> Vec<(ConnId, String, Driver, ConnStatusKind)> {
        self.conns.iter().map(|c| (c.cfg.id, c.cfg.name.clone(), c.cfg.driver, c.status.kind())).collect()
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(dialog) = self.dialog.as_mut() {
            match dialog.show(ctx) {
                Some(DialogResult::Save(cfg)) => {
                    self.dialog = None;
                    self.save_connection(cfg);
                }
                Some(DialogResult::Test(cfg)) => self.worker.send(Request::Test(cfg)),
                Some(DialogResult::Cancel) => self.dialog = None,
                None => {}
            }
        }

        let p = palette(self.dark);
        let frame = egui::Frame::popup(&ctx.global_style())
            .fill(p.popup_bg)
            .corner_radius(12)
            .inner_margin(Margin::same(20))
            .stroke(egui::Stroke::new(1.0, p.popup_border));

        if let Some(id) = self.confirm_delete {
            let name = self.conn(id).map(|c| c.cfg.name.clone()).unwrap_or_default();
            let mut decision = None;
            egui::Modal::new(egui::Id::new("confirm_delete")).frame(frame).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new("Eliminar data source").font(theme::bold(15.0)));
                ui.add_space(8.0);
                ui.label(format!("¿Eliminar «{name}»? Esta acción no se puede deshacer."));
                ui.add_space(16.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::primary_button(ui, "Eliminar").clicked() {
                        decision = Some(true);
                    }
                    if theme::secondary_button(ui, "Cancelar").clicked() {
                        decision = Some(false);
                    }
                });
            });
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                decision = Some(false);
            }
            match decision {
                Some(true) => {
                    self.confirm_delete = None;
                    self.delete_connection(id);
                }
                Some(false) => self.confirm_delete = None,
                None => {}
            }
        }

        if self.show_about {
            let resp = egui::Modal::new(egui::Id::new("about")).frame(frame).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
                    ui.painter().rect_filled(r, 10.0, p.project_badge);
                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "D", theme::bold(24.0), egui::Color32::WHITE);
                    ui.add_space(8.0);
                    ui.label(RichText::new("DataCraft").font(theme::bold(17.0)));
                    ui.label(RichText::new(format!("Versión {}", env!("CARGO_PKG_VERSION"))).color(p.muted));
                    ui.add_space(8.0);
                    ui.label("Cliente SQL ligero para PostgreSQL, SQL Server y SQLite.");
                    ui.label(RichText::new("Hecho con Rust, egui y tokio").small().color(p.muted));
                    ui.add_space(14.0);
                    theme::primary_button(ui, "Cerrar").clicked()
                })
                .inner
            });
            if resp.inner || resp.should_close() {
                self.show_about = false;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Generate {
    Select,
    Insert,
    Ddl,
}

/// Consulta del editor de datos con filtros y límite de filas en el propio SQL.
pub fn table_query(driver: Driver, schema: &str, table: &str, where_clause: &str, order_by: &str, max_rows: usize) -> String {
    let t = Session::quote_table(driver, schema, table);
    let mut sql = match driver {
        Driver::MsSql => format!("SELECT TOP ({max_rows}) * FROM {t}"),
        Driver::Postgres | Driver::Sqlite => format!("SELECT * FROM {t}"),
    };
    let w = where_clause.trim().trim_end_matches(';');
    if !w.is_empty() {
        sql.push_str(&format!(" WHERE {w}"));
    }
    let o = order_by.trim().trim_end_matches(';');
    if !o.is_empty() {
        sql.push_str(&format!(" ORDER BY {o}"));
    }
    if driver != Driver::MsSql {
        sql.push_str(&format!(" LIMIT {max_rows}"));
    }
    sql
}

/// DDL aproximado a partir de la información de columnas conocida.
fn ddl(driver: Driver, table: &str, columns: &[ColumnInfo]) -> String {
    let mut lines: Vec<String> = columns
        .iter()
        .map(|c| {
            let mut l = format!("    {} {}", quote_ident_if_needed(driver, &c.name), c.data_type.to_lowercase());
            if !c.nullable {
                l.push_str(" not null");
            }
            l
        })
        .collect();
    let pk: Vec<String> =
        columns.iter().filter(|c| c.primary_key).map(|c| quote_ident_if_needed(driver, &c.name)).collect();
    if !pk.is_empty() {
        lines.push(format!("    primary key ({})", pk.join(", ")));
    }
    format!("create table {table}\n(\n{}\n);", lines.join(",\n"))
}

/// Botón de la franja de tool windows (37x40 en DataGrip; aquí 30x30 dentro de 40 de ancho).
fn stripe_button(ui: &mut Ui, ico: Ico, active: bool, tooltip: &str) -> egui::Response {
    let p = theme::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    if active {
        ui.painter().rect_filled(rect, 7.0, p.stripe_selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 7.0, p.hover);
    }
    let color = if active { p.text } else { p.icon };
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), ico, color, &p, true);
    resp.on_hover_text(tooltip)
}

fn breadcrumb(ui: &mut Ui, ico: Option<Ico>, text: &str) {
    let p = theme::pal(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(12.5), p.text_muted);
    let w = 6.0 + if ico.is_some() { 20.0 } else { 0.0 } + galley.size().x + 6.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    if resp.hovered() {
        ui.painter().rect_filled(r, 5.0, p.hover);
    }
    let mut x = r.left() + 6.0;
    if let Some(ico) = ico {
        icons::paint(ui.painter(), Rect::from_min_size(pos2(x, r.center().y - 7.0), vec2(14.0, 14.0)), ico, p.icon, &p, true);
        x += 20.0;
    }
    let color = if resp.hovered() { p.text } else { p.text_muted };
    ui.painter().galley(pos2(x, r.center().y - galley.size().y / 2.0), galley, color);
}

fn separator(ui: &mut Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, color));
}

fn now() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Vuelca el resultado de una ejecución en la consola (pestañas + log).
fn apply_execution(
    c: &mut Console,
    results_in: Vec<crate::db::ResultSet>,
    error: Option<String>,
    elapsed: std::time::Duration,
    salt: u64,
) {
    c.running = false;
    c.run_started = None;
    let time = now();
    for rs in &results_in {
        let message = if rs.is_command() {
            format!("{} en {}", results::affected_message(rs), results::format_duration(rs.elapsed))
        } else {
            let mut m = format!("{} filas recuperadas en {}", rs.rows.len(), results::format_duration(rs.elapsed));
            if rs.truncated {
                m.push_str(" (limitado a max_rows)");
            }
            m
        };
        c.log.push(OutputEntry { time: time.clone(), statement: rs.statement.clone(), message, is_error: false });
    }
    if let Some(err) = &error {
        let failed = c
            .last_statements
            .iter()
            .find(|s| !results_in.iter().any(|r| r.statement.trim() == s.trim()))
            .cloned()
            .unwrap_or_default();
        c.log.push(OutputEntry { time: time.clone(), statement: failed, message: err.clone(), is_error: true });
    } else if c.last_statements.len() > 1 {
        c.log.push(OutputEntry {
            time,
            statement: String::new(),
            message: format!("{} sentencias ejecutadas en {}", c.last_statements.len(), results::format_duration(elapsed)),
            is_error: false,
        });
    }
    if c.log.len() > 1000 {
        let extra = c.log.len() - 1000;
        c.log.drain(..extra);
    }

    let has_queries = results_in.iter().any(|r| !r.is_command());
    let mut views: Vec<ResultView> = Vec::new();
    let last_command = results_in.iter().rev().find(|r| r.is_command()).cloned();
    for (i, rs) in results_in.into_iter().enumerate() {
        if !rs.is_command() {
            views.push(ResultView::new(rs, salt * 1000 + i as u64));
        }
    }
    if !has_queries && let Some(rs) = last_command {
        views.push(ResultView::new(rs, salt * 1000));
    }
    if !views.is_empty() || error.is_none() {
        c.results = views;
    }
    c.result_tab = if error.is_some() || c.results.is_empty() {
        ResultTab::Output
    } else {
        ResultTab::Result(c.results.len() - 1)
    };
}

impl eframe::App for DataCraftApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_responses();
        if self.applied_dark != Some(self.dark) {
            theme::apply(ctx, self.dark);
            self.applied_dark = Some(self.dark);
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        let p = palette(self.dark);
        let bar = egui::Frame::NONE.fill(p.main_bg);

        egui::Panel::top("title_bar")
            .exact_size(40.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(bar)
            .show(ui, |ui| self.title_bar(ui));
        egui::Panel::bottom("status_bar")
            .exact_size(30.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(bar)
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("stripe_left")
            .exact_size(40.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(bar)
            .show(ui, |ui| self.left_stripe(ui));
        egui::Panel::right("stripe_right")
            .exact_size(40.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(bar)
            .show(ui, |ui| self.right_stripe(ui));
        if self.show_explorer {
            let explorer_actions = egui::Panel::left("explorer")
                .resizable(true)
                .default_size(300.0)
                .size_range(200.0..=700.0)
                .show_separator_line(false)
                .frame(bar.inner_margin(Margin { right: theme::ISLAND_GAP, ..Margin::ZERO }))
                .show(ui, |ui| {
                    theme::island_frame(&p)
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            explorer::show(ui, &mut self.explorer, &self.conns)
                        })
                        .inner
                })
                .inner;
            self.handle_explorer(&ctx, explorer_actions);
        }
        let left = if self.show_explorer { theme::ISLAND_GAP } else { 0 };
        egui::CentralPanel::default()
            .frame(bar.inner_margin(Margin { left, ..Margin::ZERO }))
            .show(ui, |ui| self.central(ui));
        self.dialogs(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let state = Persisted {
            islands_dark: self.dark,
            font_size: self.font_size,
            max_rows: self.max_rows,
            consoles: self
                .consoles
                .iter()
                .map(|c| PersistedConsole {
                    title: c.title.clone(),
                    text: c.text.clone(),
                    conn: c.conn,
                    table: c.table.as_ref().map(|t| (t.schema.clone(), t.table.clone())),
                })
                .collect(),
            active: self.active,
            console_counter: self.console_counter,
            show_explorer: self.show_explorer,
            show_results: self.show_results,
        };
        eframe::set_value(storage, STORAGE_KEY, &state);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_query_limits_rows_per_dialect() {
        assert_eq!(table_query(Driver::Sqlite, "main", "users", "", "", 50), "SELECT * FROM \"users\" LIMIT 50");
        assert_eq!(
            table_query(Driver::Postgres, "public", "t", "id > 3;", " name desc ", 10),
            "SELECT * FROM \"public\".\"t\" WHERE id > 3 ORDER BY name desc LIMIT 10"
        );
        assert_eq!(
            table_query(Driver::MsSql, "dbo", "t", "x = 1", "", 5),
            "SELECT TOP (5) * FROM [dbo].[t] WHERE x = 1"
        );
    }
}
