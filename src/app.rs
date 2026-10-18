//! Main application state and UI composition (DataGrip "Islands" style).
//!
//! Debug environment variable: `DATACRAFT_DEMO=<path.db>` starts with a temporary SQLite
//! connection (not saved to `connections.json`) to that file, connects it, opens the data
//! editor of its first table and a console with a sample query. In that mode the UI state
//! is persisted to a temporary directory (see `main.rs`).
//! `DATACRAFT_DEMO_DARK=1` forces Islands Dark, `DATACRAFT_DEMO_DIALOG=1` opens the new
//! connection dialog and `DATACRAFT_DEMO_TAB=console` leaves the console active.
//! `DATACRAFT_DEMO_ZOOM=<percent>` sets the IDE zoom (e.g. 125).
//! `DATACRAFT_DEMO_EDIT=cell|changes|preview` simulates in memory, on the data editor of the
//! first table, a cell being edited, several pending changes (modified cell, NULL, new row
//! and a row marked for deletion) or those changes with the preview window open.
//! They are never sent to the database.
//! `DATACRAFT_DEMO_EXPLORER_W=<pt>` fixes the Database Explorer width (150–700, not
//! resizable), `DATACRAFT_DEMO_WINDOW=<width>x<height>` resizes the window at startup
//! (may go below the normal minimum) and `DATACRAFT_DEMO_MULTI=1` uses a console with
//! several queries, run with "Run all" (one result tab each).
//! `DATACRAFT_DEMO_FILTER=<column>` opens that column's funnel with its first value unchecked.
//! `DATACRAFT_DEMO_LANG=en|es` forces the interface language.
//! `DATACRAFT_DEMO_CREATE=table` opens the "New Table" dialog with a few sample columns;
//! `table-submit` also creates that table in the demo database.

use crate::config;
use crate::db::{ColumnInfo, ConnectionConfig, DatabaseInfo, Driver, SchemaInfo, Session, TableInfo};
use crate::edit;
use crate::i18n::{self, Lang, tr, trf};
use crate::sql;
use crate::ui::connection_dialog::{ConnectionDialog, DialogResult, TestState};
use crate::ui::console::{self, CloseResults, Console, ConsoleAction, ConsoleCtx, ResultTab};
use crate::ui::create_dialog::{self, CreateDialog, CreateResult, CreateTarget};
use crate::ui::editing::{self, RowRef};
use crate::ui::explorer::{self, ExplorerAction, ExplorerState, TableRef};
use crate::ui::icons::{self, Ico};
use crate::ui::results::{self, OutputEntry, ResultView};
use crate::ui::theme::{self, palette};
use crate::ui::{ConnStatusKind, ConnSummary, quote_ident_if_needed};
use crate::worker::{ConnId, Request, Response, Worker};
use egui::{FontId, Key, Margin, Modifiers, Rect, RichText, Sense, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

const STORAGE_KEY: &str = "datacraft_ui_state";
const DEFAULT_FONT_SIZE: f32 = 13.0;

/// IDE zoom steps.
pub const ZOOM_STEPS: [f32; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// Next zoom step above `z`.
pub fn zoom_in(z: f32) -> f32 {
    ZOOM_STEPS.iter().copied().find(|s| *s > z + 0.001).unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
}

/// Previous zoom step below `z`.
pub fn zoom_out(z: f32) -> f32 {
    ZOOM_STEPS.iter().rev().copied().find(|s| *s < z - 0.001).unwrap_or(ZOOM_STEPS[0])
}

/// Valid zoom (within 50%–200%; non-finite values go back to 100%).
pub fn clamp_zoom(z: f32) -> f32 {
    if z.is_finite() { z.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]) } else { 1.0 }
}

pub fn zoom_label(z: f32) -> String {
    format!("{:.0}%", z * 100.0)
}

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
            ConnStatus::Disconnected => tr("disconnected"),
            ConnStatus::Connecting => tr("connecting…"),
            ConnStatus::Connected => tr("connected"),
            ConnStatus::Error(_) => tr("error"),
        }
    }
}

pub struct ConnState {
    pub cfg: ConnectionConfig,
    pub status: ConnStatus,
    /// Server databases (empty if not connected).
    pub databases: Vec<DatabaseInfo>,
    /// Databases whose schema is being loaded.
    pub loading: HashSet<String>,
    /// Last error loading each database.
    pub db_errors: HashMap<String, String>,
}

impl ConnState {
    pub fn new(cfg: ConnectionConfig) -> Self {
        Self { cfg, status: ConnStatus::Disconnected, databases: Vec::new(), loading: HashSet::new(), db_errors: HashMap::new() }
    }

    fn clear(&mut self) {
        self.databases.clear();
        self.loading.clear();
        self.db_errors.clear();
    }

    /// The connection's default database (if connected).
    pub fn default_db(&self) -> Option<&str> {
        self.databases.iter().find(|d| d.current).map(|d| d.name.as_str())
    }

    /// Actual name of `db` (`None` = the default one).
    pub fn resolve_db<'a>(&'a self, db: Option<&'a str>) -> Option<&'a str> {
        db.filter(|d| !d.is_empty()).or_else(|| self.default_db())
    }

    pub fn database(&self, db: Option<&str>) -> Option<&DatabaseInfo> {
        let name = self.resolve_db(db)?;
        self.databases.iter().find(|d| d.name == name)
    }

    /// Schemas of `db` if already loaded.
    pub fn schemas(&self, db: Option<&str>) -> Option<&[SchemaInfo]> {
        self.database(db)?.schemas.as_deref()
    }

    fn summary(&self) -> ConnSummary {
        ConnSummary {
            id: self.cfg.id,
            name: self.cfg.name.clone(),
            driver: self.cfg.driver,
            status: self.status.kind(),
            databases: self.databases.iter().map(|d| d.name.clone()).collect(),
            default_db: self.default_db().map(str::to_string),
        }
    }
}

/// Action awaiting confirmation because it would discard unsaved changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Discard {
    Close(u64),
    CloseAll,
    Reload(u64),
    Run(u64, bool),
    /// Close a console's result tabs.
    CloseResults(u64, CloseResults),
}

/// "Preview pending changes" window.
struct PreviewState {
    console: u64,
    view: usize,
    statements: Vec<String>,
    error: Option<String>,
    applying: bool,
    /// false on a direct submit (⇧ / ⌘S): the window is not shown.
    visible: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DemoEdit {
    Cell,
    Changes,
    Preview,
}

#[derive(Serialize, Deserialize)]
struct PersistedConsole {
    title: String,
    text: String,
    conn: Option<u64>,
    /// (schema, table) if the tab is a data editor.
    #[serde(default)]
    table: Option<(String, String)>,
    /// The tab's database (`None` = the connection's default).
    #[serde(default)]
    db: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Persisted {
    /// Islands Dark theme (Islands Light by default). Replaces the old `dark` (Darcula).
    islands_dark: bool,
    font_size: f32,
    max_rows: usize,
    consoles: Vec<PersistedConsole>,
    active: usize,
    console_counter: u64,
    show_explorer: bool,
    show_results: bool,
    /// IDE zoom (1.0 = 100%).
    zoom: f32,
    /// Interface language (`None` = the system one).
    lang: Option<Lang>,
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
            zoom: 1.0,
            lang: None,
        }
    }
}

pub struct DataCraftApp {
    worker: Worker,
    conns: Vec<ConnState>,
    /// Editor tabs: consoles and data editors.
    consoles: Vec<Console>,
    active: usize,
    next_console_id: u64,
    console_counter: u64,
    result_salt: u64,
    explorer: ExplorerState,
    dialog: Option<ConnectionDialog>,
    confirm_delete: Option<ConnId>,
    /// "New Database" / "New Table" dialog.
    create: Option<CreateDialog>,
    renaming: Option<(u64, String)>,
    show_about: bool,
    show_settings: bool,
    /// Native macOS menu bar (None if it could not be installed).
    native_menu: Option<crate::native_menu::NativeMenu>,
    show_explorer: bool,
    show_results: bool,
    dark: bool,
    applied_dark: Option<bool>,
    font_size: f32,
    max_rows: usize,
    status_msg: Option<(String, bool)>,
    /// Last connection id handed out (so ids are monotonic).
    last_conn_id: u64,
    /// Demo mode: open the first table's data editor on connect.
    demo_pending: bool,
    /// Demo mode: simulated changes to apply when the data editor loads.
    demo_edit: Option<DemoEdit>,
    /// IDE zoom and the last one applied to the context.
    zoom: f32,
    applied_zoom: Option<f32>,
    preview: Option<PreviewState>,
    confirm_discard: Option<Discard>,
    /// Demo mode: fixed Database Explorer width (`DATACRAFT_DEMO_EXPLORER_W`).
    demo_explorer_w: Option<f32>,
    /// Demo mode: window size to apply on the first frame (`DATACRAFT_DEMO_WINDOW`).
    demo_window: Option<egui::Vec2>,
    /// Language chosen by the user (`None` = follow the system).
    lang: Option<Lang>,
}

impl DataCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let demo = std::env::var("DATACRAFT_DEMO").ok().filter(|s| !s.is_empty());
        let persisted: Persisted = if demo.is_some() {
            Persisted::default()
        } else {
            cc.storage.and_then(|s| eframe::get_value(s, STORAGE_KEY)).unwrap_or_default()
        };
        // Before any text is created (native menu, first console...).
        // Demo mode can force the language: DATACRAFT_DEMO_LANG=en|es.
        let demo_lang = demo.as_ref().and(std::env::var("DATACRAFT_DEMO_LANG").ok()).and_then(|l| match l.as_str() {
            "es" => Some(Lang::Es),
            "en" => Some(Lang::En),
            _ => None,
        });
        i18n::set_lang(demo_lang.or(persisted.lang).unwrap_or_else(Lang::system));
        let conns: Vec<ConnState> = if demo.is_some() {
            Vec::new()
        } else {
            config::load_connections()
                .into_iter()
                .map(ConnState::new)
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
            create: None,
            renaming: None,
            show_about: false,
            show_settings: false,
            native_menu: crate::native_menu::NativeMenu::install(&cc.egui_ctx),
            show_explorer: persisted.show_explorer,
            show_results: persisted.show_results,
            dark: persisted.islands_dark,
            applied_dark: None,
            font_size: persisted.font_size.clamp(8.0, 32.0),
            max_rows: persisted.max_rows.max(1),
            status_msg: None,
            last_conn_id,
            demo_pending: false,
            demo_edit: None,
            zoom: clamp_zoom(persisted.zoom),
            applied_zoom: None,
            preview: None,
            confirm_discard: None,
            demo_explorer_w: None,
            demo_window: None,
            lang: persisted.lang,
        };
        for pc in persisted.consoles {
            let conn = pc.conn.filter(|id| app.conns.iter().any(|c| c.cfg.id == *id));
            let id = app.alloc_console_id();
            match (pc.table, conn) {
                (Some((schema, table)), Some(conn)) => app.consoles.push(Console::new_table(id, conn, pc.db, schema, table)),
                (Some(_), None) => {}
                (None, _) => {
                    let mut c = Console::new(id, pc.title, pc.text, conn);
                    c.db = pc.db.filter(|_| conn.is_some());
                    app.consoles.push(c);
                }
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
        // Keyboard zoom is handled by the app (⌘= / ⌘- / ⌘0) so it is not applied twice.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        cc.egui_ctx.set_zoom_factor(app.zoom);
        app.applied_zoom = Some(app.zoom);
        app
    }

    /// Demo mode (`DATACRAFT_DEMO`): temporary SQLite connection, console and data editor.
    fn start_demo(&mut self, path: &str) {
        let id = self.next_conn_id();
        let mut cfg = ConnectionConfig::new(id, Driver::Sqlite);
        cfg.name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.into());
        cfg.file_path = path.to_string();
        self.conns.push(ConnState::new(cfg));
        self.explorer.expanded.insert(format!("c:{id}"));
        self.explorer.selected = Some(format!("c:{id}"));
        self.dark = std::env::var("DATACRAFT_DEMO_DARK").is_ok_and(|v| v == "1");
        if std::env::var("DATACRAFT_DEMO_DIALOG").is_ok_and(|v| v == "1") {
            self.open_new_connection_dialog();
        }
        if let Some(z) = std::env::var("DATACRAFT_DEMO_ZOOM").ok().and_then(|v| v.trim().trim_end_matches('%').parse::<f32>().ok()) {
            self.zoom = clamp_zoom(z / 100.0);
        }
        self.demo_explorer_w = std::env::var("DATACRAFT_DEMO_EXPLORER_W").ok().and_then(|v| v.trim().parse::<f32>().ok()).map(|w| w.clamp(150.0, 700.0));
        self.demo_window = std::env::var("DATACRAFT_DEMO_WINDOW").ok().and_then(|v| {
            let (w, h) = v.trim().split_once('x')?;
            Some(vec2(w.trim().parse::<f32>().ok()?.max(400.0), h.trim().parse::<f32>().ok()?.max(300.0)))
        });
        self.demo_edit = match std::env::var("DATACRAFT_DEMO_EDIT").as_deref() {
            Ok("cell") => Some(DemoEdit::Cell),
            Ok("changes") => Some(DemoEdit::Changes),
            Ok("preview") => Some(DemoEdit::Preview),
            _ => None,
        };
        let mut text = String::from("SELECT o.id, u.name, o.total\nFROM orders o\nJOIN users u ON u.id = o.user_id\nWHERE o.total > 10\nORDER BY o.total DESC;\n");
        if std::env::var("DATACRAFT_DEMO_MULTI").is_ok_and(|v| v == "1") {
            text.push_str("\nSELECT * FROM users;\n\nSELECT count(*) AS n FROM orders;\n");
        }
        let idx = self.new_console(Some(id), None, text);
        self.run_console(idx, true);
        // The first table's data editor opens when the schema arrives.
        self.demo_pending = true;
    }

    fn alloc_console_id(&mut self) -> u64 {
        let id = self.next_console_id;
        self.next_console_id += 1;
        id
    }

    /// Creates a console and activates it. Returns its index.
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
            self.error(trf("Could not save the connections: {error}", &[("error", &format!("{e:#}"))]));
        }
    }

    /// Known table of a database (for primary keys, DDL and editing).
    fn table_info(&self, conn: ConnId, db: Option<&str>, schema: &str, table: &str) -> Option<&TableInfo> {
        self.conn(conn)?.schemas(db)?.iter().find(|s| s.name == schema)?.tables.iter().find(|t| t.name == table)
    }

    fn table_columns(&self, conn: ConnId, db: Option<&str>, schema: &str, table: &str) -> Option<&[ColumnInfo]> {
        self.table_info(conn, db, schema, table).map(|t| t.columns.as_slice())
    }

    /// Asks the worker for a database's schema.
    fn load_database(&mut self, conn: ConnId, db: String) {
        let Some(c) = self.conn_mut(conn) else { return };
        if !matches!(c.status, ConnStatus::Connected) || c.loading.contains(&db) {
            return;
        }
        c.loading.insert(db.clone());
        c.db_errors.remove(&db);
        self.worker.send(Request::LoadDatabase { conn, db });
    }

    /// Loads the database if not loaded yet (autocomplete and data editors).
    fn ensure_db_loaded(&mut self, conn: ConnId, db: Option<&str>) {
        let Some(c) = self.conn(conn) else { return };
        if !matches!(c.status, ConnStatus::Connected) {
            return;
        }
        let Some(name) = c.resolve_db(db).map(str::to_string) else { return };
        let needs = c.databases.iter().any(|d| d.name == name && d.schemas.is_none())
            && !c.loading.contains(&name)
            && !c.db_errors.contains_key(&name);
        if needs {
            self.load_database(conn, name);
        }
    }

    // ---------------------------------------------------------------- connection

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
            c.clear();
            self.worker.send(Request::Disconnect(id));
        }
        // Whatever was waiting for this connection will no longer run.
        for console in self.consoles.iter_mut().filter(|c| c.conn == Some(id)) {
            if console.pending.take().is_some() {
                console.run_started = None;
            }
        }
    }

    fn handle_responses(&mut self) {
        for resp in self.worker.poll() {
            match resp {
                Response::Connected { conn, databases } => {
                    // Stale response: the connection was disconnected, edited or deleted while connecting.
                    if !self.conn(conn).is_some_and(|c| c.status == ConnStatus::Connecting) {
                        self.worker.send(Request::Disconnect(conn));
                        continue;
                    }
                    let mut default_db = None;
                    if let Some(c) = self.conn_mut(conn) {
                        c.status = ConnStatus::Connected;
                        c.clear();
                        c.databases = databases;
                        default_db = c.default_db().map(str::to_string);
                    }
                    if let Some(db) = &default_db {
                        self.explorer.expanded.insert(explorer::db_key(conn, db));
                    }
                    let max_rows = self.max_rows;
                    for console in self.consoles.iter_mut().filter(|c| c.conn == Some(conn)) {
                        if let Some(statements) = console.pending.take() {
                            console.running = true;
                            let db = console.db.clone();
                            self.worker.send(Request::Execute { conn, db, console: console.id, statements, max_rows });
                        }
                    }
                    self.recheck_edit_targets(conn);
                    if self.demo_pending {
                        self.demo_pending = false;
                        let first = self.conn(conn).and_then(|c| {
                            let db = c.default_db()?.to_string();
                            c.schemas(None)?
                                .iter()
                                .find_map(|s| s.tables.first().map(|t| (db.clone(), s.name.clone(), t.name.clone())))
                        });
                        if let Some((db, schema, table)) = first {
                            let tref = TableRef { conn, db, schema, table };
                            self.explorer.expanded.insert(explorer::table_key(&tref));
                            self.explorer.selected = Some(explorer::table_key(&tref));
                            self.explorer.selected_table = Some(tref.clone());
                            self.open_table(&tref);
                        }
                        if std::env::var("DATACRAFT_DEMO_TAB").is_ok_and(|v| v == "console") {
                            self.active = 0;
                        }
                        let demo_create = std::env::var("DATACRAFT_DEMO_CREATE").unwrap_or_default();
                        if demo_create.starts_with("table")
                            && let Some(db) = self.conn(conn).and_then(|c| c.default_db().map(str::to_string))
                        {
                            self.open_new_table(conn, db.clone(), None);
                            if let Some(d) = self.create.as_mut() {
                                d.name = "customers".into();
                                d.columns.extend(create_dialog::demo_columns());
                                if demo_create == "table-submit" {
                                    d.running = true;
                                    let statements = vec![create_dialog::create_table_sql(d.driver, "main", &d.name, &d.columns)];
                                    self.worker.send(Request::RunDdl { conn, db: Some(db), statements });
                                }
                            }
                        }
                    }
                }
                Response::ConnectFailed { conn, error } => {
                    let was_connected = self.conn(conn).is_some_and(|c| c.status == ConnStatus::Connected);
                    if let Some(c) = self.conn_mut(conn) {
                        // Also if it was connected: the session is lost and the next run
                        // will reconnect.
                        c.status = ConnStatus::Error(error.clone());
                    }
                    if was_connected {
                        self.error(trf("Connection lost: {error}", &[("error", &error)]));
                    }
                    for console in self.consoles.iter_mut().filter(|c| c.conn == Some(conn)) {
                        if console.pending.take().is_some() {
                            console.run_started = None;
                            console.log.push(OutputEntry {
                                time: now(),
                                statement: String::new(),
                                message: trf("Could not connect: {error}", &[("error", &error)]),
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
                        c.clear();
                    }
                }
                Response::Refreshed { conn, databases } => self.refreshed(conn, databases),
                Response::DatabaseLoaded { conn, db, result } => {
                    if let Some(c) = self.conn_mut(conn) {
                        c.loading.remove(&db);
                        match result {
                            Ok(schemas) => {
                                c.db_errors.remove(&db);
                                if let Some(d) = c.databases.iter_mut().find(|d| d.name == db) {
                                    d.schemas = Some(schemas);
                                }
                            }
                            Err(e) => {
                                c.db_errors.insert(db.clone(), e);
                            }
                        }
                    }
                    self.recheck_edit_targets(conn);
                }
                Response::Executed { console, results, error, elapsed } => {
                    self.result_salt += 1;
                    let salt = self.result_salt;
                    if let Some(c) = self.console_by_id(console) {
                        apply_execution(c, results, error, elapsed, salt);
                    }
                    self.attach_edit_targets(console);
                    self.apply_demo_edit(console);
                }
                Response::ChangesApplied { console, result } => self.changes_applied(console, result),
                Response::DdlDone { conn, result } => self.ddl_done(conn, result),
            }
        }
    }

    /// New database list after a refresh: already loaded schemas are kept while they are
    /// requested again.
    fn refreshed(&mut self, conn: ConnId, mut databases: Vec<DatabaseInfo>) {
        let Some(c) = self.conn_mut(conn) else { return };
        let mut reload = Vec::new();
        for d in &mut databases {
            if d.schemas.is_some() {
                continue;
            }
            if let Some(old) = c.databases.iter_mut().find(|o| o.name == d.name)
                && let Some(schemas) = old.schemas.take()
            {
                d.schemas = Some(schemas);
                reload.push(d.name.clone());
            }
        }
        c.databases = databases;
        c.db_errors.clear();
        let names: HashSet<String> = c.databases.iter().map(|d| d.name.clone()).collect();
        c.loading.retain(|d| names.contains(d));
        for db in &reload {
            c.loading.remove(db);
        }
        for db in reload {
            self.load_database(conn, db);
        }
        self.recheck_edit_targets(conn);
        self.info(tr("Schema refreshed"));
    }

    /// Decides again whether a connection's read-only results are editable
    /// (e.g. when its database schema arrives).
    fn recheck_edit_targets(&mut self, conn: ConnId) {
        let ids: Vec<u64> = self.consoles.iter().filter(|c| c.conn == Some(conn)).map(|c| c.id).collect();
        for id in ids {
            if let Some(c) = self.console_by_id(id) {
                for v in c.results.iter_mut().filter(|v| v.edit.is_none()) {
                    v.edit_checked = false;
                }
            }
            self.attach_edit_targets(id);
        }
    }

    /// Decides which of a tab's new results can be edited.
    fn attach_edit_targets(&mut self, console: u64) {
        let Some(idx) = self.consoles.iter().position(|c| c.id == console) else { return };
        let c = &self.consoles[idx];
        let mut targets = Vec::new();
        for (i, v) in c.results.iter().enumerate() {
            if v.edit_checked || v.rs.is_command() {
                continue;
            }
            let target = match (c.conn, &c.table) {
                (None, _) => Err(tr("No connection").to_string()),
                (Some(conn), Some(t)) => match self.table_info(conn, c.db.as_deref(), &t.schema, &t.table) {
                    Some(info) => editing::target_for(&v.rs.columns, &t.schema, info.kind, &info.name, &info.columns),
                    None => Err(tr("Table structure not loaded yet").to_string()),
                },
                (Some(conn), None) => match self.conn(conn).and_then(|cs| cs.schemas(c.db.as_deref())) {
                    Some(schemas) => editing::console_target(&v.rs.statement, &v.rs.columns, schemas),
                    None => Err(tr("Database schema not loaded yet").to_string()),
                },
            };
            targets.push((i, target));
        }
        let c = &mut self.consoles[idx];
        for (i, t) in targets {
            if let Some(v) = c.results.get_mut(i) {
                v.set_edit_target(t);
            }
        }
    }

    /// Demo mode: simulated changes (never saved) on the first data editor.
    fn apply_demo_edit(&mut self, console: u64) {
        let Some(kind) = self.demo_edit else { return };
        let Some(idx) = self.consoles.iter().position(|c| c.id == console && c.table.is_some()) else { return };
        let Some(v) = self.consoles[idx].results.last_mut() else { return };
        if v.edit.is_none() {
            return;
        }
        self.demo_edit = None;
        self.active = idx;
        let n_cols = v.rs.columns.len();
        let last = n_cols.saturating_sub(1);
        if kind == DemoEdit::Cell {
            v.begin_edit(1, last);
            return;
        }
        if let Some(e) = v.edit.as_mut() {
            let t = e.target.clone();
            e.state.set_value(&v.rs, RowRef::Existing(0), last, Some("edited".into()));
            e.state.set_value(&v.rs, RowRef::Existing(2), 2.min(last), None);
            e.state.set_value(&v.rs, RowRef::Existing(4), last, Some("new note".into()));
            let n = e.state.add_row(n_cols);
            e.state.set_value(&v.rs, RowRef::New(n), last, Some("new row".into()));
            e.state.delete_rows(&v.rs, &t, &[RowRef::Existing(3)]);
        }
        if kind == DemoEdit::Preview {
            let vi = self.consoles[idx].results.len() - 1;
            self.open_preview(idx, vi);
        }
    }

    // ---------------------------------------------------------------- data editing

    /// Opens the preview window with the SQL of the pending changes.
    fn open_preview(&mut self, idx: usize, view: usize) {
        self.prepare_changes(idx, view, true);
    }

    /// Saves the pending changes without showing the SQL (that's what Preview is for).
    fn submit_now(&mut self, idx: usize, view: usize) {
        self.prepare_changes(idx, view, false);
        match self.preview.as_ref() {
            Some(pv) if !pv.visible => match pv.error.clone() {
                Some(err) => {
                    self.preview = None;
                    self.error(err);
                }
                None => {
                    self.submit_preview();
                    // If it could not be sent (no connection, busy tab), report it and discard.
                    if let Some(pv) = self.preview.as_ref().filter(|p| !p.visible && !p.applying) {
                        let err = pv.error.clone().unwrap_or_default();
                        self.preview = None;
                        self.error(err);
                    }
                }
            },
            _ => {}
        }
    }

    fn prepare_changes(&mut self, idx: usize, view: usize, visible: bool) {
        let Some(c) = self.consoles.get_mut(idx) else { return };
        let Some(v) = c.results.get_mut(view) else { return };
        v.finish_inline_edit();
        let Some(e) = v.edit.as_ref() else { return };
        if !e.state.is_dirty() {
            return;
        }
        let driver = c.conn.and_then(|id| self.conns.iter().find(|x| x.cfg.id == id)).map(|x| x.cfg.driver).unwrap_or(Driver::Postgres);
        let (statements, error) = match e.state.build_changes(&v.rs, &e.target) {
            Ok(changes) => (changes.iter().map(|ch| edit::change_sql(driver, &e.target.schema, &e.target.table, ch)).collect(), None),
            Err(err) => (Vec::new(), Some(err)),
        };
        self.preview = Some(PreviewState { console: c.id, view, statements, error, applying: false, visible });
    }

    /// Sends the preview's changes in a transaction.
    fn submit_preview(&mut self) {
        let Some(pv) = self.preview.as_ref() else { return };
        if pv.applying || pv.statements.is_empty() {
            return;
        }
        let (console, statements) = (pv.console, pv.statements.clone());
        let Some(c) = self.consoles.iter().find(|c| c.id == console) else {
            self.preview = None;
            return;
        };
        let (conn, db, busy) = (c.conn, c.db.clone(), c.is_busy());
        let connected = conn.and_then(|id| self.conn(id)).is_some_and(|c| matches!(c.status, ConnStatus::Connected));
        let Some(pv) = self.preview.as_mut() else { return };
        match conn {
            _ if busy => pv.error = Some(tr("The tab is running another operation").into()),
            Some(conn) if connected => {
                pv.applying = true;
                pv.error = None;
                if let Some(c) = self.console_by_id(console) {
                    c.running = true;
                    c.run_started = Some(Instant::now());
                }
                self.worker.send(Request::ApplyChanges { conn, db, console, statements });
            }
            _ => pv.error = Some(tr("The connection is not open; connect it and try again").into()),
        }
    }

    fn changes_applied(&mut self, console: u64, result: Result<u64, String>) {
        let preview_view = self.preview.as_ref().filter(|p| p.console == console).map(|p| p.view);
        let Some(idx) = self.consoles.iter().position(|c| c.id == console) else {
            self.preview = None;
            return;
        };
        let c = &mut self.consoles[idx];
        c.running = false;
        c.run_started = None;
        match result {
            Ok(n) => {
                if preview_view.is_some() {
                    self.preview = None;
                }
                let msg = if n == 1 { tr("1 row modified").to_string() } else { trf("{n} rows modified", &[("n", &n)]) };
                c.log.push(OutputEntry { time: now(), statement: String::new(), message: msg.clone(), is_error: false });
                let vi = preview_view.or_else(|| c.edit_view_index());
                if let Some(v) = vi.and_then(|i| c.results.get_mut(i)) {
                    v.discard_edits();
                }
                self.info(msg);
                // Reloads that tab's data.
                let c = &mut self.consoles[idx];
                if c.table.is_some() {
                    self.reload_table(idx);
                } else if let Some(i) = vi
                    && let Some(stmt) = c.results.get(i).map(|v| v.rs.statement.clone())
                {
                    c.replace_view = Some(i);
                    self.run_statements(idx, vec![stmt]);
                }
            }
            Err(e) => {
                c.log.push(OutputEntry {
                    time: now(),
                    statement: String::new(),
                    message: trf("Changes were not saved: {error}", &[("error", &e)]),
                    is_error: true,
                });
                match self.preview.as_mut().filter(|p| p.console == console && p.visible) {
                    Some(pv) => {
                        pv.applying = false;
                        pv.error = Some(e);
                    }
                    None => {
                        if self.preview.as_ref().is_some_and(|p| p.console == console) {
                            self.preview = None;
                        }
                        self.error(trf("Changes were not saved: {error}", &[("error", &e)]));
                    }
                }
            }
        }
    }

    /// Closes a console's result tabs (asks for confirmation if they have changes).
    fn close_results(&mut self, idx: usize, which: CloseResults) {
        let Some(c) = self.consoles.get(idx).filter(|c| c.table.is_none() && !c.is_busy()) else { return };
        if self.guard(Discard::CloseResults(c.id, which)) {
            self.close_results_now(idx, which);
        }
    }

    fn close_results_now(&mut self, idx: usize, which: CloseResults) {
        let Some(c) = self.consoles.get_mut(idx).filter(|c| c.table.is_none()) else { return };
        let id = c.id;
        let removed = c.close_results(which);
        // The changes preview keeps the index of its view.
        if let Some(pv) = self.preview.as_mut().filter(|p| p.console == id) {
            match console::remap_index(pv.view, &removed) {
                Some(v) => pv.view = v,
                None => self.preview = None,
            }
        }
    }

    /// true if the action can go ahead now; with unsaved changes, asks for confirmation.
    fn guard(&mut self, action: Discard) -> bool {
        let dirty = match action {
            Discard::CloseAll => self.consoles.iter().any(|c| c.has_pending_edits()),
            Discard::Close(id) | Discard::Reload(id) | Discard::Run(id, _) => {
                self.consoles.iter().any(|c| c.id == id && c.has_pending_edits())
            }
            Discard::CloseResults(id, which) => self.consoles.iter().any(|c| c.id == id && c.closing_dirty(which)),
        };
        if dirty {
            self.confirm_discard = Some(action);
        }
        !dirty
    }

    /// Runs a confirmed action, discarding the pending changes.
    fn discard_and_do(&mut self, action: Discard) {
        match action {
            Discard::CloseAll => {
                self.consoles.clear();
                self.active = 0;
            }
            Discard::Close(id) => {
                if let Some(i) = self.consoles.iter().position(|c| c.id == id) {
                    self.close_console_now(i);
                }
            }
            Discard::CloseResults(id, which) => {
                if let Some(i) = self.consoles.iter().position(|c| c.id == id) {
                    self.close_results_now(i, which);
                }
            }
            Discard::Reload(id) | Discard::Run(id, _) => {
                let Some(i) = self.consoles.iter().position(|c| c.id == id) else { return };
                self.consoles[i].discard_edits();
                match action {
                    Discard::Run(_, all) => self.run_console(i, all),
                    _ => self.reload_table(i),
                }
            }
        }
    }

    // ---------------------------------------------------------------- execution

    fn run_console(&mut self, idx: usize, all: bool) {
        let Some(console) = self.consoles.get(idx) else { return };
        let id = console.id;
        if console.is_busy() {
            return;
        }
        if console.table.is_some() {
            if self.guard(Discard::Reload(id)) {
                self.reload_table(idx);
            }
            return;
        }
        if !self.guard(Discard::Run(id, all)) {
            return;
        }
        let Some(console) = self.consoles.get(idx) else { return };
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

    /// Sends statements of tab `idx`, connecting first if needed.
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
            console.log.push(OutputEntry { time: now(), statement: String::new(), message: tr("Nothing to run").into(), is_error: true });
            console.result_tab = ResultTab::Output;
            return;
        }
        let (Some(conn), Some(status)) = (conn_id, status) else {
            console.log.push(OutputEntry {
                time: now(),
                statement: String::new(),
                message: tr("Select a connection for the console").into(),
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
                let db = console.db.clone();
                self.worker.send(Request::Execute { conn, db, console: id, statements, max_rows });
            }
            ConnStatus::Connecting => console.pending = Some(statements),
            ConnStatus::Disconnected | ConnStatus::Error(_) => {
                console.pending = Some(statements);
                self.connect(conn);
            }
        }
    }

    /// Opens (or activates) a table's data editor.
    fn open_table(&mut self, t: &TableRef) {
        let default_db = self.conn(t.conn).and_then(|c| c.default_db()).map(str::to_string);
        if let Some(i) = self.consoles.iter().position(|c| {
            c.conn == Some(t.conn)
                && c.db.as_ref().or(default_db.as_ref()) == Some(&t.db)
                && c.table.as_ref().is_some_and(|x| x.schema == t.schema && x.table == t.table)
        }) {
            self.active = i;
            // With pending changes it is only activated (reloading would discard them).
            if !self.consoles[i].has_pending_edits() && !self.consoles[i].is_busy() {
                self.reload_table(i);
            }
            return;
        }
        let id = self.alloc_console_id();
        self.consoles.push(Console::new_table(id, t.conn, Some(t.db.clone()), t.schema.clone(), t.table.clone()));
        self.active = self.consoles.len() - 1;
        self.reload_table(self.active);
    }

    /// Reloads the data editor with its filters, limiting rows in the SQL itself.
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
        let Some(columns) = self.table_columns(t.conn, Some(&t.db), &t.schema, &t.table) else {
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
        let default_db = c.default_db().map(str::to_string);
        let target = self
            .consoles
            .get(self.active)
            .filter(|c| {
                c.conn == Some(t.conn) && c.table.is_none() && c.db.as_ref().or(default_db.as_ref()) == Some(&t.db)
            })
            .map(|_| self.active);
        match target {
            Some(i) => self.consoles[i].append_text(ctx, &text),
            None => {
                let title = (kind == Generate::Ddl).then(|| format!("{}.sql", t.table));
                let i = self.new_console(Some(t.conn), title, text);
                self.consoles[i].db = Some(t.db.clone());
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
                        cfg.name = trf("{name} (copy)", &[("name", &cfg.name)]);
                        config::store_password(cfg.id, &cfg.password);
                        self.conns.push(ConnState::new(cfg));
                        self.persist_connections();
                    }
                }
                ExplorerAction::Delete(id) => self.confirm_delete = Some(id),
                ExplorerAction::LoadDatabase(id, db) => self.load_database(id, db),
                ExplorerAction::NewDatabase(id) => {
                    if let Some(c) = self.conn(id).filter(|c| c.status == ConnStatus::Connected) {
                        self.create = Some(CreateDialog::database(id, c.cfg.driver));
                    }
                }
                ExplorerAction::NewTable { conn, db, schema } => self.open_new_table(conn, db, schema),
                ExplorerAction::NewConsole(id, db) => {
                    let i = self.new_console(Some(id), None, String::new());
                    self.consoles[i].db = db;
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

    fn open_new_table(&mut self, conn: ConnId, db: String, schema: Option<String>) {
        let Some(c) = self.conn(conn).filter(|c| c.status == ConnStatus::Connected) else { return };
        let driver = c.cfg.driver;
        let schemas: Vec<String> = c.schemas(Some(&db)).unwrap_or_default().iter().map(|s| s.name.clone()).collect();
        let default = create_dialog::default_schema(driver).to_string();
        let schema = schema.or_else(|| schemas.contains(&default).then_some(default.clone())).or_else(|| schemas.first().cloned()).unwrap_or(default);
        self.ensure_db_loaded(conn, Some(&db));
        self.create = Some(CreateDialog::table(conn, driver, db, schema));
    }

    /// Reply to the "New Database" / "New Table" dialog's statements.
    fn ddl_done(&mut self, conn: ConnId, result: Result<(), String>) {
        let Some(d) = self.create.as_mut().filter(|d| d.conn == conn && d.running) else {
            if let Err(e) = result {
                self.error(e);
            }
            return;
        };
        if let Err(e) = result {
            d.running = false;
            d.error = Some(e);
            return;
        }
        let Some(d) = self.create.take() else { return };
        let name = d.name.trim().to_string();
        match d.target {
            CreateTarget::Database => {
                self.explorer.expanded.insert(format!("c:{conn}"));
                self.explorer.selected = Some(explorer::db_key(conn, &name));
                self.worker.send(Request::Refresh(conn));
            }
            CreateTarget::Table { db, schema } => {
                let schema = if d.driver == Driver::Sqlite { "main".to_string() } else { schema.trim().to_string() };
                let tref = TableRef { conn, db: db.clone(), schema: schema.clone(), table: name.clone() };
                self.explorer.expanded.insert(format!("c:{conn}"));
                self.explorer.expanded.insert(explorer::db_key(conn, &db));
                self.explorer.expanded.insert(format!("s:{conn}:{db}:{schema}"));
                self.explorer.selected = Some(explorer::table_key(&tref));
                self.explorer.selected_table = Some(tref);
                // The reload brings the new table; if a load is already in progress it may
                // miss it, so wait for it and ask again.
                if let Some(c) = self.conn_mut(conn) {
                    c.loading.remove(&db);
                }
                self.load_database(conn, db);
                self.info(trf("Table “{name}” created", &[("name", &name)]));
            }
        }
    }

    /// Monotonic ids: a deleted connection's id is never reused (its keychain password or a
    /// late worker response could be associated with the new one).
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
            self.conns.push(ConnState::new(cfg));
            if let Some(c) = self.consoles.get_mut(self.active)
                && c.conn.is_none()
            {
                c.conn = Some(id);
            }
        }
        self.persist_connections();
        self.info(tr("Connection saved"));
    }

    fn delete_connection(&mut self, id: ConnId) {
        self.worker.send(Request::Disconnect(id));
        self.conns.retain(|c| c.cfg.id != id);
        config::delete_password(id);
        for c in self.consoles.iter_mut().filter(|c| c.conn == Some(id)) {
            c.conn = None;
            c.db = None;
            c.pending = None;
            c.run_started = None;
        }
        // That connection's data editors no longer make sense.
        let active_id = self.consoles.get(self.active).map(|c| c.id);
        self.consoles.retain(|c| !(c.table.is_some() && c.conn.is_none()));
        self.active = active_id
            .and_then(|id| self.consoles.iter().position(|c| c.id == id))
            .unwrap_or(0)
            .min(self.consoles.len().saturating_sub(1));
        self.persist_connections();
    }

    // ---------------------------------------------------------------- files

    fn open_sql_file(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title(tr("Open SQL script")).add_filter("SQL", &["sql"]).pick_file() else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
                let (conn, db) = self.consoles.get(self.active).map(|c| (c.conn, c.db.clone())).unwrap_or_default();
                let i = self.new_console(conn, title, text);
                self.consoles[i].db = db;
            }
            Err(e) => self.error(trf("Could not open {path}: {error}", &[("path", &path.display()), ("error", &e)])),
        }
    }

    fn save_console_as(&mut self) {
        let Some(console) = self.consoles.get(self.active).filter(|c| c.table.is_none()) else { return };
        let Some(path) = rfd::FileDialog::new()
            .set_title(tr("Save console as"))
            .add_filter("SQL", &["sql"])
            .set_file_name(format!("{}.sql", console.title))
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, &console.text) {
            Ok(()) => self.info(trf("Saved to {path}", &[("path", &path.display())])),
            Err(e) => self.error(trf("Could not save: {error}", &[("error", &e)])),
        }
    }

    fn close_console(&mut self, idx: usize) {
        if let Some(id) = self.consoles.get(idx).map(|c| c.id)
            && self.guard(Discard::Close(id))
        {
            self.close_console_now(idx);
        }
    }

    fn close_console_now(&mut self, idx: usize) {
        if idx < self.consoles.len() {
            self.consoles.remove(idx);
            if self.active > idx || self.active >= self.consoles.len() {
                self.active = self.active.saturating_sub(1);
            }
        }
    }

    fn new_console_here(&mut self) {
        let (conn, db) = match self.consoles.get(self.active).filter(|c| c.conn.is_some()) {
            Some(c) => (c.conn, c.db.clone()),
            None => (self.conns.first().map(|c| c.cfg.id), None),
        };
        let i = self.new_console(conn, None, String::new());
        self.consoles[i].db = db;
    }

    // ---------------------------------------------------------------- UI

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // IDE zoom (⌘= / ⌘+ / ⌘- / ⌘0) and editor font (⌥⌘= / ⌥⌘-). The ⌥ variants go
        // first: `consume_key` ignores an extra ⌥.
        let (font_up, font_down, zoom_up, zoom_down, zoom_reset) = ctx.input_mut(|i| {
            let alt = Modifiers::COMMAND | Modifiers::ALT;
            let font_up = i.consume_key(alt, Key::Equals) || i.consume_key(alt, Key::Plus);
            let font_down = i.consume_key(alt, Key::Minus);
            let zoom_up = i.consume_key(Modifiers::COMMAND, Key::Equals) || i.consume_key(Modifiers::COMMAND, Key::Plus);
            let zoom_down = i.consume_key(Modifiers::COMMAND, Key::Minus);
            let zoom_reset = i.consume_key(Modifiers::COMMAND, Key::Num0);
            (font_up, font_down, zoom_up, zoom_down, zoom_reset)
        });
        if font_up {
            self.font_size = (self.font_size + 1.0).min(32.0);
        }
        if font_down {
            self.font_size = (self.font_size - 1.0).max(8.0);
        }
        if zoom_up {
            self.zoom = zoom_in(self.zoom);
        }
        if zoom_down {
            self.zoom = zoom_out(self.zoom);
        }
        if zoom_reset {
            self.zoom = 1.0;
        }
        if self.modal_open() {
            return;
        }
        let (run_all, run, new_console, reload, quit, save) = ctx.input_mut(|i| {
            let run_all = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
            let run = i.consume_key(Modifiers::COMMAND, Key::Enter);
            let new_console = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::N);
            let reload = i.consume_key(Modifiers::COMMAND, Key::R);
            let quit = i.consume_key(Modifiers::COMMAND, Key::Q);
            let save = i.consume_key(Modifiers::COMMAND, Key::S);
            (run_all, run, new_console, reload, quit, save)
        });
        if run_all {
            self.run_console(self.active, true);
        } else if run {
            self.run_console(self.active, false);
        }
        if reload {
            self.reload_active();
        }
        if save {
            self.save_active_changes();
        }
        if new_console {
            self.new_console_here();
        }
        if quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// A modal dialog is open: shortcuts and run commands are ignored.
    fn modal_open(&self) -> bool {
        self.dialog.is_some()
            || self.confirm_delete.is_some()
            || self.create.is_some()
            || self.show_about
            || self.show_settings
            || self.preview.as_ref().is_some_and(|p| p.visible)
            || self.confirm_discard.is_some()
    }

    fn reload_active(&mut self) {
        if let Some(c) = self.consoles.get(self.active).filter(|c| c.table.is_some() && !c.is_busy())
            && self.guard(Discard::Reload(c.id))
        {
            self.reload_table(self.active);
        }
    }

    fn save_active_changes(&mut self) {
        if let Some(c) = self.consoles.get(self.active)
            && let Some(vi) = c.edit_view_index()
            && !c.is_busy()
        {
            self.submit_now(self.active, vi);
        }
    }

    /// Native macOS menu commands. The menu takes their accelerators (⌘↩, ⌘S, ⌘=...), so the
    /// equivalent shortcuts in `shortcuts` only act if the menu was not installed.
    fn menu_commands(&mut self, ctx: &egui::Context) {
        use crate::native_menu::MenuCommand as M;
        let Some(menu) = self.native_menu.as_ref() else { return };
        let cmds = menu.poll();
        for cmd in cmds {
            let modal = self.modal_open();
            match cmd {
                M::About => self.show_about = true,
                M::Settings => self.show_settings = true,
                M::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                M::NewConnection if !modal => self.open_new_connection_dialog(),
                M::NewConsole if !modal => self.new_console_here(),
                M::OpenSql if !modal => self.open_sql_file(),
                M::SaveSqlAs if !modal => self.save_console_as(),
                M::CloseTab if !modal && !self.consoles.is_empty() => self.close_console(self.active),
                M::Find => {
                    self.show_explorer = true;
                    self.explorer.show_filter = true;
                }
                M::ToggleExplorer => self.show_explorer = !self.show_explorer,
                M::ToggleResults => self.show_results = !self.show_results,
                M::ThemeLight => self.dark = false,
                M::ThemeDark => self.dark = true,
                M::ZoomIn => self.zoom = zoom_in(self.zoom),
                M::ZoomOut => self.zoom = zoom_out(self.zoom),
                M::ZoomReset => self.zoom = 1.0,
                M::FontBigger => self.font_size = (self.font_size + 1.0).min(32.0),
                M::FontSmaller => self.font_size = (self.font_size - 1.0).max(8.0),
                M::FontReset => self.font_size = DEFAULT_FONT_SIZE,
                M::Run if !modal => self.run_console(self.active, false),
                M::RunAll if !modal => self.run_console(self.active, true),
                M::SaveChanges if !modal => self.save_active_changes(),
                M::Reload if !modal => self.reload_active(),
                // No help website: shows the app info window.
                M::Help => self.show_about = true,
                _ => {}
            }
        }
        if let Some(menu) = self.native_menu.as_ref() {
            menu.sync(self.dark, self.show_explorer, self.show_results);
        }
    }

    /// Main application menu (under the "DataCraft ⌄" project widget).
    fn app_menu(&mut self, ui: &mut Ui) {
        ui.set_min_width(220.0);
        ui.menu_button(tr("File"), |ui| {
            if ui.button(tr("New data source…")).clicked() {
                self.open_new_connection_dialog();
                ui.close();
            }
            if ui.add(egui::Button::new(tr("New console")).shortcut_text("⇧⌘N")).clicked() {
                self.new_console_here();
                ui.close();
            }
            ui.separator();
            if ui.button(tr("Open .sql…")).clicked() {
                ui.close();
                self.open_sql_file();
            }
            let can_save = self.consoles.get(self.active).is_some_and(|c| c.table.is_none());
            if ui.add_enabled(can_save, egui::Button::new(tr("Save console as .sql…"))).clicked() {
                ui.close();
                self.save_console_as();
            }
            ui.separator();
            if ui.add(egui::Button::new(tr("Quit")).shortcut_text("⌘Q")).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button(tr("View"), |ui| self.view_menu(ui));
        ui.menu_button(tr("Help"), |ui| {
            if ui.button(tr("About DataCraft")).clicked() {
                self.show_about = true;
                ui.close();
            }
        });
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        let p = palette(self.dark);
        ui.label(RichText::new(tr("Theme")).small().color(p.muted));
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
        ui.checkbox(&mut self.show_results, tr("Services (results)"));
        ui.separator();
        ui.menu_button(trf("IDE zoom ({zoom})", &[("zoom", &zoom_label(self.zoom))]), |ui| self.zoom_menu(ui));
        ui.menu_button("Language / Idioma", |ui| {
            for lang in Lang::ALL {
                if ui.radio(i18n::lang() == lang, lang.native_name()).clicked() {
                    self.set_language(lang);
                    ui.close();
                }
            }
        });
        ui.separator();
        ui.label(RichText::new(trf("Editor font: {size} pt", &[("size", &format!("{:.0}", self.font_size))])).small().color(p.muted));
        if ui.add(egui::Button::new(tr("Increase")).shortcut_text("⌥⌘+")).clicked() {
            self.font_size = (self.font_size + 1.0).min(32.0);
        }
        if ui.add(egui::Button::new(tr("Decrease")).shortcut_text("⌥⌘-")).clicked() {
            self.font_size = (self.font_size - 1.0).max(8.0);
        }
        if ui.button(tr("Reset")).clicked() {
            self.font_size = DEFAULT_FONT_SIZE;
        }
    }

    /// Changes the interface language, remembers it and relabels the native menu.
    fn set_language(&mut self, lang: Lang) {
        i18n::set_lang(lang);
        self.lang = Some(lang);
        if let Some(menu) = self.native_menu.as_ref() {
            menu.relabel();
        }
    }

    /// "IDE zoom" submenu: zoom in, zoom out, reset and fixed steps.
    fn zoom_menu(&mut self, ui: &mut Ui) {
        ui.set_min_width(180.0);
        if ui.add_enabled(self.zoom < ZOOM_STEPS[ZOOM_STEPS.len() - 1], egui::Button::new(tr("Zoom In")).shortcut_text("⌘+")).clicked() {
            self.zoom = zoom_in(self.zoom);
        }
        if ui.add_enabled(self.zoom > ZOOM_STEPS[0], egui::Button::new(tr("Zoom Out")).shortcut_text("⌘-")).clicked() {
            self.zoom = zoom_out(self.zoom);
        }
        if ui.add(egui::Button::new(tr("Reset (100%)")).shortcut_text("⌘0")).clicked() {
            self.zoom = 1.0;
            ui.close();
        }
        ui.separator();
        for z in ZOOM_STEPS {
            if ui.radio((self.zoom - z).abs() < 0.001, zoom_label(z)).clicked() {
                self.zoom = z;
                ui.close();
            }
        }
    }

    /// Top bar unified with the title (main toolbar).
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
            theme::split_row(ui, |ui, side| match side {
                theme::Side::Left => {
            ui.spacing_mut().item_spacing.x = 4.0;
            // Room for the traffic lights: 80 screen pt, whatever the IDE zoom.
            let zoom = ui.ctx().zoom_factor().max(0.1);
            ui.add_space(if fullscreen { 10.0 } else { 80.0 / zoom });

            // Project widget: purple square with the initial + name + chevron.
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

            // Active tab's connection selector.
            let current = self.consoles.get(self.active).and_then(|c| c.conn).and_then(|id| self.conn(id));
            let active_db = self.consoles.get(self.active).and_then(|c| c.db.clone());
            let (ico, label) = match current {
                Some(c) => (Ico::Driver(c.cfg.driver), c.summary().label(active_db.as_deref())),
                None => (Ico::Database, tr("No connection").into()),
            };
            let busy = self.consoles.get(self.active).is_some_and(|c| c.is_busy() || c.table.is_some());
            let resp = theme::flat_button(ui, Some(ico), &label, FontId::proportional(13.0), true, true)
                .on_hover_text(tr("Data source of the active tab"));
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
                if ui.button(tr("New data source…")).clicked() {
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

                }
                theme::Side::Right => {
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                let gear = theme::icon_button_sized(ui, Ico::Gear, true, tr("Settings"), 30.0);
                egui::Popup::menu(&gear).show(|ui| {
                    self.view_menu(ui);
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(tr("Max rows:"));
                        ui.add(egui::DragValue::new(&mut self.max_rows).range(1..=1_000_000).speed(10));
                    });
                });
                if theme::icon_button_sized(ui, Ico::Search, true, tr("Search tables"), 30.0).clicked() {
                    self.show_explorer = true;
                    self.explorer.show_filter = !self.explorer.show_filter;
                }
                }
            });
        });
    }

    /// Changes the active console's connection (if it is not busy).
    fn set_active_connection(&mut self, id: ConnId) {
        let needs_connect = self.conn(id).is_some_and(|c| matches!(c.status, ConnStatus::Disconnected | ConnStatus::Error(_)));
        if let Some(c) = self.consoles.get_mut(self.active) {
            if c.is_busy() || c.table.is_some() || c.conn == Some(id) {
                return;
            }
            c.conn = Some(id);
            c.db = None;
            c.pending = None;
            c.run_started = None;
        } else {
            self.new_console(Some(id), None, String::new());
        }
        if needs_connect {
            self.connect(id);
        }
    }

    /// Left tool window stripe.
    fn left_stripe(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let mut top = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Center)));
        top.spacing_mut().item_spacing.y = 4.0;
        top.add_space(2.0);
        if stripe_button(&mut top, Ico::Database, self.show_explorer, "Database Explorer").clicked() {
            self.show_explorer = !self.show_explorer;
        }
        let more = stripe_button(&mut top, Ico::More, false, tr("More tool windows"));
        egui::Popup::menu(&more).show(|ui| {
            ui.checkbox(&mut self.show_explorer, "Database Explorer");
            ui.checkbox(&mut self.show_results, "Services");
        });

        let mut bottom = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::bottom_up(egui::Align::Center)));
        bottom.spacing_mut().item_spacing.y = 4.0;
        bottom.add_space(4.0);
        let has_error = self.status_msg.as_ref().is_some_and(|(_, e)| *e);
        let problems = stripe_button(&mut bottom, Ico::Problems, false, tr("Problems"));
        if problems.clicked()
            && let Some((msg, _)) = &self.status_msg
        {
            ui.ctx().copy_text(msg.clone());
        }
        if has_error {
            let c = problems.rect.right_top() + vec2(-9.0, 9.0);
            bottom.painter().circle_filled(c, 3.0, palette(self.dark).error);
        }
        if stripe_button(&mut bottom, Ico::Terminal, false, tr("New console (⇧⌘N)")).clicked() {
            self.new_console_here();
        }
        if stripe_button(&mut bottom, Ico::Services, self.show_results, tr("Services (results)")).clicked() {
            self.show_results = !self.show_results;
        }
    }

    /// Right stripe (notifications and structure).
    fn right_stripe(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let mut top = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Center)));
        top.spacing_mut().item_spacing.y = 4.0;
        top.add_space(2.0);
        let msg = self.status_msg.clone();
        let bell = stripe_button(&mut top, Ico::Bell, false, tr("Notifications"));
        egui::Popup::menu(&bell).show(|ui| {
            ui.set_min_width(260.0);
            match &msg {
                Some((m, err)) => {
                    let color = if *err { palette(self.dark).error } else { palette(self.dark).text };
                    ui.add(egui::Label::new(RichText::new(m).color(color)).wrap());
                    if ui.button(tr("Dismiss")).clicked() {
                        self.status_msg = None;
                        ui.close();
                    }
                }
                None => {
                    ui.label(RichText::new(tr("No notifications")).color(palette(self.dark).muted));
                }
            }
        });
        if msg.is_some() {
            let c = bell.rect.right_top() + vec2(-9.0, 9.0);
            top.painter().circle_filled(c, 3.0, palette(self.dark).accent);
        }
        let structure = stripe_button(&mut top, Ico::Structure, false, tr("Tab structure"));
        egui::Popup::menu(&structure).show(|ui| {
            let cols = self.consoles.get(self.active).and_then(|c| {
                let t = c.table.as_ref()?;
                self.table_columns(c.conn?, c.db.as_deref(), &t.schema, &t.table)
            });
            match cols {
                Some(cols) => {
                    for c in cols {
                        ui.label(format!("{}  {}", c.name, c.data_type.to_lowercase()));
                    }
                }
                None => {
                    ui.label(RichText::new(tr("No structure")).color(palette(self.dark).muted));
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let p = palette(self.dark);
        ui.horizontal_centered(|ui| {
            theme::split_row(ui, |ui, side| match side {
                theme::Side::Left => {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(12.0);
            // Active tab's breadcrumbs.
            let console = self.consoles.get(self.active);
            let conn = console.and_then(|c| c.conn).and_then(|id| self.conn(id));
            let mut crumbs: Vec<(Option<Ico>, String)> = vec![(None, "Database".into())];
            if let Some(c) = conn {
                crumbs.push((Some(Ico::Driver(c.cfg.driver)), c.cfg.name.clone()));
                if let Some(db) = c.resolve_db(console.and_then(|x| x.db.as_deref())) {
                    crumbs.push((Some(Ico::Database), db.to_string()));
                }
            }
            if let Some(c) = console {
                match &c.table {
                    Some(t) => {
                        // SQLite: the "main" schema is the same as the database (the explorer omits it too).
                        if conn.is_none_or(|x| x.cfg.driver != Driver::Sqlite) {
                            crumbs.push((Some(Ico::Schema), t.schema.clone()));
                        }
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

                }
                theme::Side::Right => {
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), r, Ico::Lock, p.text_muted, &p, true);
                ui.add_space(6.0);
                if (self.zoom - 1.0).abs() > 0.001 {
                    let resp = theme::flat_button(ui, None, &zoom_label(self.zoom), FontId::proportional(12.5), false, true)
                        .on_hover_text(tr("IDE zoom (⌘0 to reset)"));
                    egui::Popup::menu(&resp).show(|ui| self.zoom_menu(ui));
                    ui.add_space(4.0);
                }
                ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
                ui.add(
                    egui::DragValue::new(&mut self.max_rows)
                        .range(1..=1_000_000)
                        .speed(10)
                        .prefix("max rows: "),
                )
                .on_hover_text(tr("Maximum rows per query (drag or double-click to edit)"));
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
                            trf("{n} rows · {time}", &[("n", &r.row_count()), ("time", &results::format_duration(r.rs.elapsed))])
                        };
                        ui.label(RichText::new(text).color(p.text_muted).size(12.5));
                    }
                }
                if let Some((msg, is_err)) = &self.status_msg {
                    ui.add_space(8.0);
                    let color = if *is_err { p.error } else { p.text_muted };
                    // The notice cannot take the whole bar: a third at most.
                    let max_w = (ui.available_width() / 3.0).max(80.0);
                    let resp = ui
                        .scope(|ui| {
                            ui.set_max_width(max_w);
                            ui.add(egui::Label::new(RichText::new(msg).color(color).size(12.5)).truncate().sense(Sense::click()))
                        })
                        .inner;
                    if resp.on_hover_text(tr("Click to dismiss")).clicked() {
                        self.status_msg = None;
                    }
                }
                }
            });
        });
    }

    /// Editor tabs (at the top of the editor island).
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
                            let title = if busy {
                                format!("{} …", c.title)
                            } else if c.has_pending_edits() {
                                format!("{} *", c.title)
                            } else {
                                c.title.clone()
                            };
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
                                if ui.add_enabled(c.table.is_none(), egui::Button::new(tr("Rename"))).clicked() {
                                    start_rename = Some((c.id, c.title.clone()));
                                    ui.close();
                                }
                                if ui.button(tr("Close")).clicked() {
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
                let more = theme::icon_button(ui, Ico::MoreVert, true, tr("Tabs"));
                egui::Popup::menu(&more).show(|ui| {
                    if ui.add(egui::Button::new(tr("New console")).shortcut_text("⇧⌘N")).clicked() {
                        new = true;
                        ui.close();
                    }
                    if ui.add_enabled(!self.consoles.is_empty(), egui::Button::new(tr("Close all"))).clicked() {
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
        if close_all && self.guard(Discard::CloseAll) {
            self.consoles.clear();
            self.active = 0;
        }
        if new {
            self.new_console_here();
        }
    }

    fn central(&mut self, ui: &mut Ui) {
        let p = palette(self.dark);
        // Deferred loading of restored data editors.
        if let Some(c) = self.consoles.get(self.active)
            && c.table.as_ref().is_some_and(|t| !t.loaded)
            && !c.is_busy()
        {
            self.reload_table(self.active);
        }
        // Schema of the tab's database (autocomplete, keys and editing).
        if let Some(c) = self.consoles.get(self.active)
            && let Some(conn) = c.conn
        {
            let db = c.db.clone();
            self.ensure_db_loaded(conn, db.as_deref());
        }
        let is_console = self.consoles.get(self.active).is_some_and(|c| c.table.is_none());
        let mut actions = Vec::new();
        let conn_list = self.conn_list();

        if is_console && self.show_results {
            // The editor (tabs + toolbar + a few lines) always keeps its room: the results
            // panel cannot grow past what is left, even with short windows.
            let avail = ui.available_height();
            let max_results = (avail - 170.0).max(avail * 0.45).max(60.0);
            let min_results = 120.0_f32.min(max_results);
            egui::Panel::bottom(egui::Id::new("results_island"))
                .resizable(true)
                .default_size(300.0_f32.min(max_results))
                .size_range(min_results..=max_results)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.inner_margin(Margin { top: 2 * theme::ISLAND_GAP, ..Margin::ZERO }))
                .show(ui, |ui| {
                    theme::fixed_island(ui, &p, |ui| {
                        let idx = self.active;
                        let console = &self.consoles[idx];
                        let conn = console.conn.and_then(|id| self.conns.iter().find(|c| c.cfg.id == id));
                        let cx = ConsoleCtx {
                            connections: &conn_list,
                            schemas: conn.and_then(|c| c.schemas(console.db.as_deref())),
                            driver: conn.map(|c| c.cfg.driver).unwrap_or(Driver::Postgres),
                            dark: self.dark,
                            font_size: self.font_size,
                            columns: None,
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
                    ui.label(RichText::new(tr("No open tabs")).color(p.muted));
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr("New console  ⇧⌘N")).color(p.muted));
                    ui.add_space(8.0);
                    if theme::primary_button(ui, tr("New console")).clicked() {
                        self.new_console_here();
                    }
                });
                return;
            }
            let idx = self.active.min(self.consoles.len() - 1);
            self.active = idx;
            let console = &self.consoles[idx];
            let conn = console.conn.and_then(|id| self.conns.iter().find(|c| c.cfg.id == id));
            let schemas = conn.and_then(|c| c.schemas(console.db.as_deref()));
            let columns = console.table.as_ref().and_then(|t| {
                schemas?
                    .iter()
                    .find(|s| s.name == t.schema)?
                    .tables
                    .iter()
                    .find(|x| x.name == t.table)
                    .map(|t| t.columns.as_slice())
            });
            let cx = ConsoleCtx {
                connections: &conn_list,
                schemas,
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
                ConsoleAction::Reload => {
                    if let Some(id) = self.consoles.get(idx).map(|c| c.id)
                        && self.guard(Discard::Reload(id))
                    {
                        self.reload_table(idx);
                    }
                }
                ConsoleAction::Ddl => {
                    if let Some(c) = self.consoles.get(idx)
                        && let (Some(conn), Some(t)) = (c.conn, c.table.as_ref())
                        && let Some(db) = self.conn(conn).and_then(|x| x.resolve_db(c.db.as_deref()))
                    {
                        let tref = TableRef { conn, db: db.to_string(), schema: t.schema.clone(), table: t.table.clone() };
                        self.generate(ui.ctx(), &tref, Generate::Ddl);
                    }
                }
                ConsoleAction::Connect(id) => self.connect(id),
                ConsoleAction::Error(e) => self.error(e),
                ConsoleAction::Preview(view) => self.open_preview(idx, view),
                ConsoleAction::Submit(view) => self.submit_now(idx, view),
                ConsoleAction::CloseResults(which) => self.close_results(idx, which),
            }
        }
    }

    fn conn_list(&self) -> Vec<ConnSummary> {
        self.conns.iter().map(ConnState::summary).collect()
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

        if let Some(d) = self.create.as_ref() {
            let schemas: Vec<String> = match &d.target {
                CreateTarget::Table { db, .. } => {
                    self.conn(d.conn).and_then(|c| c.schemas(Some(db))).unwrap_or_default().iter().map(|s| s.name.clone()).collect()
                }
                CreateTarget::Database => vec![],
            };
            if let Some(d) = self.create.as_mut() {
                match d.show(ctx, &schemas) {
                    Some(CreateResult::Create(statements)) => {
                        d.running = true;
                        d.error = None;
                        let db = match &d.target {
                            CreateTarget::Table { db, .. } => Some(db.clone()),
                            CreateTarget::Database => None,
                        };
                        let conn = d.conn;
                        self.worker.send(Request::RunDdl { conn, db, statements });
                    }
                    Some(CreateResult::Cancel) => self.create = None,
                    None => {}
                }
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
                ui.label(RichText::new(tr("Delete data source")).font(theme::bold(15.0)));
                ui.add_space(8.0);
                ui.label(trf("Delete “{name}”? This action cannot be undone.", &[("name", &name)]));
                ui.add_space(16.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::primary_button(ui, tr("Delete")).clicked() {
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

        if let Some(action) = self.confirm_discard {
            let mut decision = None;
            egui::Modal::new(egui::Id::new("confirm_discard")).frame(frame).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(tr("Unsaved changes")).font(theme::bold(15.0)));
                ui.add_space(8.0);
                ui.label(match action {
                    Discard::Close(_) | Discard::CloseAll => tr("Closing the tab will discard the pending grid changes."),
                    Discard::Reload(_) => tr("Reloading the data will discard the pending grid changes."),
                    Discard::CloseResults(..) => tr("Closing the result tab will discard the pending grid changes."),
                    Discard::Run(..) => tr("Running again will discard the pending grid changes."),
                });
                ui.add_space(16.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::primary_button(ui, tr("Discard")).clicked() {
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
                    self.confirm_discard = None;
                    self.discard_and_do(action);
                }
                Some(false) => self.confirm_discard = None,
                None => {}
            }
        }

        self.preview_dialog(ctx, frame);

        if self.show_about {
            let resp = egui::Modal::new(egui::Id::new("about")).frame(frame).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
                    ui.painter().rect_filled(r, 10.0, p.project_badge);
                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "D", theme::bold(24.0), egui::Color32::WHITE);
                    ui.add_space(8.0);
                    ui.label(RichText::new("DataCraft").font(theme::bold(17.0)));
                    ui.label(RichText::new(trf("Version {version}", &[("version", &env!("CARGO_PKG_VERSION"))])).color(p.muted));
                    ui.add_space(8.0);
                    ui.label(tr("Lightweight SQL client for PostgreSQL, SQL Server and SQLite."));
                    ui.label(RichText::new(tr("Made with Rust, egui and tokio")).small().color(p.muted));
                    ui.add_space(14.0);
                    theme::primary_button(ui, "Cerrar").clicked()
                })
                .inner
            });
            if resp.inner || resp.should_close() {
                self.show_about = false;
            }
        }
        if self.show_settings {
            let resp = egui::Modal::new(egui::Id::new("settings")).frame(frame).show(ctx, |ui| {
                ui.set_width(300.0);
                ui.label(RichText::new(tr("Settings")).font(theme::bold(15.0)));
                ui.add_space(8.0);
                self.view_menu(ui);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(tr("Max rows per query:"));
                    ui.add(egui::DragValue::new(&mut self.max_rows).range(1..=1_000_000).speed(10));
                });
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| theme::primary_button(ui, "Cerrar").clicked())
                    .inner
            });
            if resp.inner || resp.should_close() {
                self.show_settings = false;
            }
        }
    }
}

impl DataCraftApp {
    /// "Preview pending changes" modal window.
    fn preview_dialog(&mut self, ctx: &egui::Context, frame: egui::Frame) {
        let Some(pv) = self.preview.as_ref().filter(|p| p.visible) else { return };
        let p = palette(self.dark);
        let n = pv.statements.len();
        let applying = pv.applying;
        let error = pv.error.clone();
        let title = self
            .consoles
            .iter()
            .find(|c| c.id == pv.console)
            .and_then(|c| c.results.get(pv.view))
            .and_then(|v| v.edit.as_ref())
            .map(|e| format!("{}.{}", e.target.schema, e.target.table))
            .unwrap_or_default();
        let sql_text = pv.statements.iter().map(|s| format!("{s};")).collect::<Vec<_>>().join("\n");
        let dark = self.dark;
        let mut submit = false;
        let mut cancel = false;
        let resp = egui::Modal::new(egui::Id::new("preview_changes")).frame(frame).show(ctx, |ui| {
            let screen = ctx.content_rect();
            let w = (screen.width() * 0.6).clamp(360.0, 860.0);
            ui.set_width(w);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Preview pending changes").font(theme::bold(15.0)));
                ui.add_space(8.0);
                let count = if n == 1 { tr("1 change").to_string() } else { trf("{n} changes", &[("n", &n)]) };
                ui.label(RichText::new(format!("{count} · {title}")).color(p.muted));
            });
            ui.add_space(10.0);
            egui::Frame::NONE
                .fill(p.editor_bg)
                .stroke(egui::Stroke::new(1.0, p.control_border))
                .corner_radius(6)
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    let max_h = (screen.height() * 0.5).clamp(120.0, 520.0);
                    egui::ScrollArea::both().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _w: f32| {
                            let mut job = sql::highlight(buf.as_str(), dark, FontId::monospace(12.5));
                            job.wrap.max_width = f32::INFINITY;
                            ui.fonts_mut(|f| f.layout_job(job))
                        };
                        let mut text = sql_text.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .id(egui::Id::new("preview_sql"))
                                .code_editor()
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY)
                                .desired_rows(4)
                                .layouter(&mut layouter),
                        );
                    });
                });
            if let Some(e) = &error {
                ui.add_space(8.0);
                ui.add(egui::Label::new(RichText::new(e).monospace().color(p.error)).wrap());
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::secondary_button(ui, tr("Copy SQL")).clicked() {
                    ui.ctx().copy_text(sql_text.clone());
                }
                if applying {
                    ui.add_space(8.0);
                    ui.add(egui::Spinner::new().size(14.0).color(p.muted));
                    ui.label(RichText::new(tr("Saving…")).color(p.muted));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.add_enabled_ui(!applying && n > 0, |ui| {
                        if theme::primary_button(ui, "Submit").clicked() {
                            submit = true;
                        }
                    });
                    ui.add_enabled_ui(!applying, |ui| {
                        if theme::secondary_button(ui, tr("Cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        });
        if !applying && (resp.should_close() || ctx.input(|i| i.key_pressed(Key::Escape))) {
            cancel = true;
        }
        if cancel {
            self.preview = None;
        } else if submit {
            self.submit_preview();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Generate {
    Select,
    Insert,
    Ddl,
}

/// Data editor query with filters and the row limit in the SQL itself.
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

/// Approximate DDL from the known column information.
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

/// Tool window stripe button (37x40 in DataGrip; here 30x30 within a 40 wide stripe).
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

/// Puts a run's result into the console (tabs + log).
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
            trf("{message} in {time}", &[("message", &results::affected_message(rs)), ("time", &results::format_duration(rs.elapsed))])
        } else {
            let mut m = trf("{n} rows retrieved in {time}", &[("n", &rs.rows.len()), ("time", &results::format_duration(rs.elapsed))]);
            if rs.truncated {
                m.push_str(tr(" (limited to max_rows)"));
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
            message: trf("{n} statements executed in {time}", &[("n", &c.last_statements.len()), ("time", &results::format_duration(elapsed))]),
            is_error: false,
        });
    }
    if c.log.len() > 1000 {
        let extra = c.log.len() - 1000;
        c.log.drain(..extra);
    }

    // Reload after saving changes: replaces only the edited result.
    let replace = c.replace_view.take();
    if let Some(i) = replace
        && error.is_none()
        && i < c.results.len()
        && results_in.len() == 1
        && let Some(rs) = results_in.first().cloned()
    {
        let title = std::mem::take(&mut c.results[i].title);
        c.results[i] = ResultView::new(rs, salt * 1000);
        if !title.is_empty() {
            c.results[i].title = title;
        }
        c.result_tab = ResultTab::Result(i);
        return;
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
        // First of all: the menu's edit commands are injected as events of this frame.
        self.menu_commands(ctx);
        self.handle_responses();
        if self.applied_zoom != Some(self.zoom) {
            ctx.set_zoom_factor(self.zoom);
            self.applied_zoom = Some(self.zoom);
        }
        if self.applied_dark != Some(self.dark) {
            theme::apply(ctx, self.dark);
            self.applied_dark = Some(self.dark);
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(size) = self.demo_window.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(400.0, 300.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        self.shortcuts(&ctx);
        let p = palette(self.dark);
        let bar = egui::Frame::NONE.fill(p.main_bg);

        // With zoom < 100% the bar doesn't go below 32 screen pt (macOS traffic lights).
        let title_h = 40.0_f32.max(32.0 / ctx.zoom_factor().max(0.1));
        egui::Panel::top("title_bar")
            .exact_size(title_h)
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
            let mut panel = egui::Panel::left("explorer")
                .resizable(true)
                .default_size(300.0)
                .size_range(150.0..=700.0)
                .show_separator_line(false)
                .frame(bar.inner_margin(Margin { right: theme::ISLAND_GAP, ..Margin::ZERO }));
            if let Some(w) = self.demo_explorer_w {
                panel = panel.exact_size(w);
            }
            let explorer_actions = panel
                .show(ui, |ui| theme::fixed_island(ui, &p, |ui| explorer::show(ui, &mut self.explorer, &self.conns)))
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
                    db: c.db.clone(),
                })
                .collect(),
            active: self.active,
            console_counter: self.console_counter,
            show_explorer: self.show_explorer,
            show_results: self.show_results,
            zoom: self.zoom,
            lang: self.lang,
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
    fn zoom_steps() {
        assert_eq!(zoom_in(1.0), 1.1);
        assert_eq!(zoom_out(1.0), 0.9);
        assert_eq!(zoom_in(2.0), 2.0);
        assert_eq!(zoom_out(0.5), 0.5);
        // From an intermediate value it goes to the nearest step in that direction.
        assert_eq!(zoom_in(1.2), 1.25);
        assert_eq!(zoom_out(1.2), 1.1);
        assert_eq!(clamp_zoom(5.0), 2.0);
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
        assert_eq!(zoom_label(1.25), "125%");
        let mut z = 0.5;
        for _ in 0..ZOOM_STEPS.len() {
            z = zoom_in(z);
        }
        assert_eq!(z, 2.0);
    }

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
