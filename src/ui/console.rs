//! Pestañas del editor: consola SQL (editor con resaltado, numeración de líneas, autocompletado
//! y resultados) y editor de datos de una tabla (toolbar, filtros WHERE / ORDER BY y grid).

use crate::db::{ColumnInfo, Driver, ObjectKind, SchemaInfo};
use crate::sql;
use crate::ui::icons::{self, Ico};
use crate::ui::results::{self, OutputEntry, ResultView};
use crate::ui::theme::{self, pal, palette};
use crate::ui::{ConnStatusKind, ConnSummary, byte_to_char, char_to_byte, quote_ident};
use crate::worker::ConnId;
use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::{Event, FontId, Key, Modifiers, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResultTab {
    Output,
    Result(usize),
}

/// Qué pestañas de resultados cerrar (la de Output no se cierra nunca).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CloseResults {
    One(usize),
    /// Todas menos esta.
    Others(usize),
    All,
}

impl CloseResults {
    /// Índices (ascendentes) que se cierran de `n` resultados.
    pub fn indices(self, n: usize) -> Vec<usize> {
        match self {
            CloseResults::One(i) => (i < n).then_some(i).into_iter().collect(),
            CloseResults::Others(i) => (0..n).filter(|&j| j != i).collect(),
            CloseResults::All => (0..n).collect(),
        }
    }
}

/// Nuevo índice de la vista `i` tras quitar `removed` (ascendentes); `None` si se quitó.
pub fn remap_index(i: usize, removed: &[usize]) -> Option<usize> {
    if removed.contains(&i) {
        return None;
    }
    Some(i - removed.iter().filter(|&&r| r < i).count())
}

/// Pestaña activa tras quitar `removed`: si se cerró la activa, la que ocupa su sitio (o la
/// última); sin resultados, Output.
pub fn tab_after_close(tab: ResultTab, removed: &[usize], remaining: usize) -> ResultTab {
    let ResultTab::Result(i) = tab else { return tab };
    if remaining == 0 {
        return ResultTab::Output;
    }
    match remap_index(i, removed) {
        Some(j) => ResultTab::Result(j.min(remaining - 1)),
        None => {
            let pos = i - removed.iter().filter(|&&r| r < i).count();
            ResultTab::Result(pos.min(remaining - 1))
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ItemKind {
    Column,
    Table,
    View,
    Schema,
    Keyword,
}

struct CompletionItem {
    text: String,
    kind: ItemKind,
    detail: String,
}

struct Completion {
    /// Posición (en chars) donde empieza el prefijo que se reemplaza.
    start: usize,
    selected: usize,
    items: Vec<CompletionItem>,
    scroll_to_selected: bool,
}

pub struct Console {
    pub id: u64,
    pub title: String,
    pub text: String,
    pub conn: Option<ConnId>,
    /// Base de datos de la pestaña (`None` = la por defecto de la conexión).
    pub db: Option<String>,
    pub running: bool,
    pub run_started: Option<Instant>,
    /// Sentencias a ejecutar cuando termine de conectar.
    pub pending: Option<Vec<String>>,
    pub last_statements: Vec<String>,
    pub results: Vec<ResultView>,
    pub log: Vec<OutputEntry>,
    pub result_tab: ResultTab,
    /// Rango de selección (chars, ordenado) del último frame.
    pub cursor: Option<(usize, usize)>,
    hl_cache: Option<(u64, LayoutJob)>,
    completion: Option<Completion>,
    /// Si es Some, la pestaña es un editor de datos de tabla en vez de una consola.
    pub table: Option<TableTab>,
    /// Tras guardar cambios: la próxima ejecución sustituye solo este resultado.
    pub replace_view: Option<usize>,
    /// Formato elegido en el selector "CSV ⌄" del editor de datos.
    pub export_format: results::ExportFormat,
}

/// Estado del editor de datos de una tabla.
pub struct TableTab {
    pub schema: String,
    pub table: String,
    pub where_clause: String,
    pub order_by: String,
    /// false hasta la primera carga (las pestañas restauradas cargan al activarse).
    pub loaded: bool,
}

/// Acciones que la consola pide a la aplicación.
pub enum ConsoleAction {
    Run,
    RunAll,
    /// Recargar los datos del editor de tabla (con los filtros actuales).
    Reload,
    /// Generar el DDL de la tabla del editor de datos.
    Ddl,
    Connect(ConnId),
    Error(String),
    /// Abrir la previsualización de cambios pendientes del resultado indicado.
    Preview(usize),
    /// Guardar directamente los cambios pendientes del resultado indicado.
    Submit(usize),
    /// Cerrar pestañas de resultados (la app confirma si tienen cambios sin guardar).
    CloseResults(CloseResults),
}

fn edit_action(a: results::EditAction, view: usize) -> ConsoleAction {
    match a {
        results::EditAction::Submit => ConsoleAction::Submit(view),
        results::EditAction::Preview => ConsoleAction::Preview(view),
    }
}

/// Contexto de solo lectura que necesita la consola.
pub struct ConsoleCtx<'a> {
    pub connections: &'a [ConnSummary],
    pub schemas: Option<&'a [SchemaInfo]>,
    pub driver: Driver,
    pub dark: bool,
    pub font_size: f32,
    /// Columnas de la tabla abierta (editor de datos), para marcar claves primarias.
    pub columns: Option<&'a [ColumnInfo]>,
}

impl Console {
    pub fn new(id: u64, title: String, text: String, conn: Option<ConnId>) -> Self {
        Self {
            id,
            title,
            text,
            conn,
            db: None,
            running: false,
            run_started: None,
            pending: None,
            last_statements: Vec::new(),
            results: Vec::new(),
            log: Vec::new(),
            result_tab: ResultTab::Output,
            cursor: None,
            hl_cache: None,
            completion: None,
            table: None,
            replace_view: None,
            export_format: results::ExportFormat::Csv,
        }
    }

    /// Pestaña de editor de datos para `schema.table`.
    pub fn new_table(id: u64, conn: ConnId, db: Option<String>, schema: String, table: String) -> Self {
        let mut c = Self::new(id, table.clone(), String::new(), Some(conn));
        c.db = db;
        c.table = Some(TableTab { schema, table, where_clause: String::new(), order_by: String::new(), loaded: false });
        c
    }

    pub fn is_busy(&self) -> bool {
        self.running || self.pending.is_some()
    }

    /// Resultado cuyos cambios se editan ahora: el último en el editor de datos o el
    /// resultado visible en una consola.
    pub fn edit_view_index(&self) -> Option<usize> {
        if self.table.is_some() {
            return self.results.len().checked_sub(1);
        }
        match self.result_tab {
            ResultTab::Result(i) if i < self.results.len() => Some(i),
            _ => None,
        }
    }

    pub fn has_pending_edits(&self) -> bool {
        self.results.iter().any(|r| r.is_dirty())
    }

    /// true si alguna de las vistas que cerraría `which` tiene cambios sin guardar.
    pub fn closing_dirty(&self, which: CloseResults) -> bool {
        which.indices(self.results.len()).iter().any(|&i| self.results[i].is_dirty())
    }

    /// Cierra pestañas de resultados y reajusta la activa y `replace_view`. Devuelve los
    /// índices quitados (ascendentes) para que la app reajuste los suyos.
    pub fn close_results(&mut self, which: CloseResults) -> Vec<usize> {
        let removed = which.indices(self.results.len());
        for &i in removed.iter().rev() {
            self.results.remove(i);
        }
        if !removed.is_empty() {
            self.result_tab = tab_after_close(self.result_tab, &removed, self.results.len());
            self.replace_view = self.replace_view.and_then(|v| remap_index(v, &removed));
        }
        removed
    }

    pub fn discard_edits(&mut self) {
        for r in &mut self.results {
            r.discard_edits();
        }
    }

    /// (línea, columna) 1-based del cursor del editor.
    pub fn line_col(&self) -> (usize, usize) {
        let c = self.cursor.map(|(_, b)| b).unwrap_or(0);
        let mut line = 1;
        let mut col = 1;
        for ch in self.text.chars().take(c) {
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    pub fn editor_id(&self) -> egui::Id {
        egui::Id::new(("console_editor", self.id))
    }

    /// Texto seleccionado (si lo hay).
    pub fn selection(&self) -> Option<&str> {
        let (a, b) = self.cursor?;
        if a == b {
            return None;
        }
        let s = char_to_byte(&self.text, a);
        let e = char_to_byte(&self.text, b);
        self.text.get(s..e).filter(|t| !t.trim().is_empty())
    }

    pub fn cursor_byte(&self) -> usize {
        let c = self.cursor.map(|(_, b)| b).unwrap_or(self.text.chars().count());
        char_to_byte(&self.text, c)
    }

    /// Mueve el cursor del editor al final y añade texto.
    pub fn append_text(&mut self, ctx: &egui::Context, extra: &str) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = self.text.chars().count();
        self.text.push_str(extra);
        let end = self.text.chars().count();
        self.set_cursor(ctx, start, end);
    }

    fn set_cursor(&mut self, ctx: &egui::Context, a: usize, b: usize) {
        let id = self.editor_id();
        let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
        state.store(ctx, id);
        self.cursor = Some((a.min(b), a.max(b)));
    }

    /// Toolbar + editor de una consola (va dentro de la isla del editor).
    pub fn show_console(&mut self, ui: &mut Ui, cx: &ConsoleCtx<'_>) -> Vec<ConsoleAction> {
        let mut actions = Vec::new();
        self.console_toolbar(ui, cx, &mut actions);
        hline(ui);
        self.editor(ui, cx);
        actions
    }

    /// Contenido de la isla de resultados de una consola.
    pub fn show_results(&mut self, ui: &mut Ui, cx: &ConsoleCtx<'_>) -> Vec<ConsoleAction> {
        let mut actions = Vec::new();
        self.results_ui(ui, cx, &mut actions);
        actions
    }

    /// Editor de datos de una tabla: toolbar, fila de filtros y grid.
    pub fn show_table(&mut self, ui: &mut Ui, cx: &ConsoleCtx<'_>) -> Vec<ConsoleAction> {
        let mut actions = Vec::new();
        let p = pal(ui);
        self.table_toolbar(ui, cx, &mut actions);
        hline(ui);
        self.filter_row(ui, &mut actions);
        hline(ui);
        if let Some(e) = self.log.last().filter(|e| e.is_error && self.result_tab == ResultTab::Output) {
            let msg = e.message.clone();
            egui::Frame::NONE
                .fill(if p.dark { egui::Color32::from_rgb(0x3F, 0x1E, 0x21) } else { egui::Color32::from_rgb(0xFF, 0xF6, 0xF5) })
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        icons::paint(
                            ui.painter(),
                            Rect::from_min_size(ui.cursor().min + vec2(0.0, 1.0), egui::Vec2::splat(16.0)),
                            Ico::Problems,
                            p.error,
                            &p,
                            true,
                        );
                        ui.add_space(22.0);
                        ui.add(egui::Label::new(RichText::new(msg).monospace().color(p.error)).wrap());
                    });
                });
            hline(ui);
        }
        let copy_requested = ui.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy)))
            && ui.memory(|m| m.focused().is_none());
        if let Some(view) = self.results.last_mut() {
            if let Some(err) = results::show_result(ui, view, copy_requested, cx.columns) {
                actions.push(ConsoleAction::Error(err));
            }
        } else if self.is_busy() {
            ui.centered_and_justified(|ui| ui.label(RichText::new("Cargando…").color(p.muted)));
        }
        actions
    }

    fn busy_label(&self, ui: &mut Ui) {
        if self.is_busy() {
            let p = pal(ui);
            ui.add_space(6.0);
            ui.add(egui::Spinner::new().size(14.0).color(p.muted));
            let label = if self.pending.is_some() { "Conectando…" } else { "Ejecutando…" };
            let secs = self.run_started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
            ui.label(RichText::new(format!("{label} {secs:.1} s")).color(p.muted));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Desplegable "Tx: Auto".
    fn tx_dropdown(&self, ui: &mut Ui) {
        let resp = theme::flat_button(ui, None, "Tx: Auto", FontId::proportional(13.0), true, true)
            .on_hover_text("Modo de transacción");
        egui::Popup::menu(&resp).show(|ui| {
            let _ = ui.add(egui::Button::new("✓  Auto").frame(false));
            ui.add_enabled(false, egui::Button::new("    Manual").frame(false));
            ui.separator();
            ui.label(RichText::new("Cada sentencia se confirma al ejecutarse").small().color(pal(ui).muted));
        });
    }

    /// Selector "conexión · base" de la consola (deshabilitado mientras está ocupada).
    fn connection_selector(&mut self, ui: &mut Ui, cx: &ConsoleCtx<'_>, actions: &mut Vec<ConsoleAction>) {
        let busy = self.is_busy();
        let current = self.conn.and_then(|id| cx.connections.iter().find(|c| c.id == id));
        let (ico, label) = match current {
            Some(c) => (Ico::Driver(c.driver), c.label(self.db.as_deref())),
            None => (Ico::Database, "<sin conexión>".to_string()),
        };
        let resp = theme::flat_button(ui, Some(ico), &label, FontId::proportional(13.0), true, !busy)
            .on_hover_text("Data source y base de datos de la consola");
        let before = (self.conn, self.db.clone());
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(240.0);
            if ui.selectable_label(self.conn.is_none(), "<sin conexión>").clicked() {
                self.conn = None;
                self.db = None;
                ui.close();
            }
            for c in cx.connections {
                let this = self.conn == Some(c.id);
                if ui.selectable_label(this && self.db.is_none(), format!("{}  ·  {}", c.name, c.driver.label())).clicked() {
                    self.conn = Some(c.id);
                    self.db = None;
                    ui.close();
                }
                if c.driver == Driver::Sqlite {
                    continue;
                }
                let effective = if this { self.db.clone().or_else(|| c.default_db.clone()) } else { None };
                for d in &c.databases {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        let sel = effective.as_deref() == Some(d.as_str());
                        let text = if c.default_db.as_deref() == Some(d.as_str()) { format!("{d}  (por defecto)") } else { d.clone() };
                        if ui.add(egui::Button::selectable(sel, text)).clicked() {
                            self.conn = Some(c.id);
                            self.db = Some(d.clone());
                            ui.close();
                        }
                    });
                }
            }
        });
        if (self.conn, self.db.clone()) != before {
            // Lo pendiente era para la conexión/base anterior.
            self.pending = None;
            self.run_started = None;
            if let Some(id) = self.conn
                && let Some(c) = cx.connections.iter().find(|c| c.id == id)
                && matches!(c.status, ConnStatusKind::Disconnected | ConnStatusKind::Error)
            {
                actions.push(ConsoleAction::Connect(id));
            }
        }
    }

    fn console_toolbar(&mut self, ui: &mut Ui, cx: &ConsoleCtx<'_>, actions: &mut Vec<ConsoleAction>) {
        let busy = self.is_busy();
        toolbar_row(ui, |ui| {
            theme::split_row(ui, |ui, side| match side {
                theme::Side::Left => {
            if theme::icon_button(ui, Ico::Play, !busy, "Ejecutar sentencia actual o selección (⌘↩)").clicked() {
                actions.push(ConsoleAction::Run);
            }
            if theme::icon_button(ui, Ico::PlayAll, !busy, "Ejecutar todo (⌘⇧↩)").clicked() {
                actions.push(ConsoleAction::RunAll);
            }
            theme::toolbar_separator(ui);
            if theme::icon_button(ui, Ico::History, !self.log.is_empty(), "Ver log de ejecución").clicked() {
                self.result_tab = ResultTab::Output;
            }
            self.tx_dropdown(ui);
            theme::icon_button(ui, Ico::Stop, false, "Cancelar (no disponible)");
            self.busy_label(ui);
                }
                theme::Side::Right => {
                ui.add_space(4.0);
                self.connection_selector(ui, cx, actions);
                }
            });
        });
    }

    fn table_toolbar(&mut self, ui: &mut Ui, cx: &ConsoleCtx<'_>, actions: &mut Vec<ConsoleAction>) {
        let busy = self.is_busy();
        toolbar_row(ui, |ui| {
            theme::split_row(ui, |ui, side| match side {
                theme::Side::Left => {
            if theme::icon_button(ui, Ico::Refresh, !busy, "Recargar página (⌘R)").clicked() {
                actions.push(ConsoleAction::Reload);
            }
            if theme::icon_button(ui, Ico::History, !self.log.is_empty(), "Copiar el log de consultas").clicked() {
                let text = self.log.iter().map(|e| format!("[{}] {} {}", e.time, e.statement, e.message)).collect::<Vec<_>>().join("\n");
                ui.ctx().copy_text(text);
            }
            theme::icon_button(ui, Ico::Stop, false, "Cancelar (no disponible)");
            theme::toolbar_separator(ui);
            let idx = self.results.len().checked_sub(1);
            if let Some(a) = results::edit_buttons(ui, self.results.last_mut(), !busy)
                && let Some(i) = idx
            {
                actions.push(edit_action(a, i));
            }
            if let Some(v) = self.results.last() {
                edit_status_icon(ui, v);
            }
            theme::toolbar_separator(ui);
            self.tx_dropdown(ui);
            theme::toolbar_separator(ui);
            if theme::icon_button(ui, Ico::Ddl, cx.columns.is_some(), "Ver DDL de la tabla").clicked() {
                actions.push(ConsoleAction::Ddl);
            }
            self.busy_label(ui);
                }
                theme::Side::Right => {
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.x = 1.0;
                let has_rows = self.results.last().is_some_and(|r| !r.rs.is_command());
                let fmt = self.export_format;
                let (target, base) = match &self.table {
                    Some(t) => (crate::db::Session::quote_table(cx.driver, &t.schema, &t.table), t.table.clone()),
                    None => ("tabla".to_string(), "resultado".to_string()),
                };
                // ⋮: copiar al portapapeles.
                let more = theme::icon_button(ui, Ico::MoreVert, has_rows, "Copiar al portapapeles");
                egui::Popup::menu(&more).show(|ui| {
                    if ui.button(format!("Copiar todo como {}", fmt.label())).clicked() {
                        if let Some(view) = self.results.last() {
                            ui.ctx().copy_text(results::format_rows(view, fmt, &target));
                        }
                        ui.close();
                    }
                    for other in results::ExportFormat::ALL.into_iter().filter(|f| *f != fmt) {
                        if ui.button(format!("Copiar todo como {}", other.label())).clicked() {
                            if let Some(view) = self.results.last() {
                                ui.ctx().copy_text(results::format_rows(view, other, &target));
                            }
                            ui.close();
                        }
                    }
                });
                // ⤓: exportar directamente a fichero con el formato elegido.
                if theme::icon_button(ui, Ico::Export, has_rows, &format!("Exportar a fichero {}…", fmt.label())).clicked()
                    && let Some(view) = self.results.last()
                    && let Err(e) = results::export_to_file(view, fmt, &target, &base)
                {
                    actions.push(ConsoleAction::Error(format!("No se pudo exportar: {e}")));
                }
                // "CSV ⌄": elegir el formato de extracción.
                let selector = theme::flat_button(ui, None, fmt.label(), FontId::proportional(13.0), true, true)
                    .on_hover_text("Formato de extracción");
                egui::Popup::menu(&selector).show(|ui| {
                    for f in results::ExportFormat::ALL {
                        if ui.add(egui::Button::selectable(f == fmt, f.label())).clicked() {
                            self.export_format = f;
                            ui.close();
                        }
                    }
                });
                }
            });
        });
    }

    /// Fila "WHERE | ORDER BY" del editor de datos.
    fn filter_row(&mut self, ui: &mut Ui, actions: &mut Vec<ConsoleAction>) {
        let p = pal(ui);
        let Some(t) = self.table.as_mut() else { return };
        let id = self.id;
        let h = 32.0;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        let half = (rect.width() / 2.0).round();
        let left = Rect::from_min_max(rect.min, pos2(rect.left() + half, rect.bottom()));
        let right = Rect::from_min_max(pos2(rect.left() + half, rect.top()), rect.max);
        ui.painter().vline(left.right(), rect.y_range().shrink(6.0), Stroke::new(1.0, p.grid));
        let mut submit = false;
        for (r, ico, hint, text, salt) in [
            (left, Ico::Filter, "WHERE", &mut t.where_clause, "where"),
            (right, Ico::Sort, "ORDER BY", &mut t.order_by, "order"),
        ] {
            icons::paint(
                ui.painter(),
                Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 7.0), egui::Vec2::splat(14.0)),
                ico,
                p.icon,
                &p,
                true,
            );
            let edit_rect = Rect::from_min_max(pos2(r.left() + 32.0, r.top() + 4.0), pos2(r.right() - 8.0, r.bottom() - 4.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
            let resp = child.add(
                egui::TextEdit::singleline(text)
                    .id(egui::Id::new((salt, id)))
                    .hint_text(RichText::new(hint).monospace().color(p.disabled))
                    .font(FontId::monospace(13.0))
                    .frame(egui::Frame::NONE)
                    .background_color(egui::Color32::TRANSPARENT)
                    .desired_width(edit_rect.width()),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = true;
            }
        }
        if submit {
            actions.push(ConsoleAction::Reload);
        }
    }

    fn results_ui(&mut self, ui: &mut Ui, cx: &ConsoleCtx<'_>, actions: &mut Vec<ConsoleAction>) {
        let p = pal(ui);
        // Cabecera tipo tool window "Services": título + pestañas de resultados (cerrables).
        let busy = self.is_busy();
        let mut close = None;
        theme::clipped_row(ui, 36.0, |ui| {
            ui.add_space(14.0);
            ui.label(RichText::new("Services").font(theme::bold(13.5)).color(p.text));
            ui.add_space(10.0);
            let lock = match self.result_tab {
                ResultTab::Result(i) => self
                    .results
                    .get(i)
                    .filter(|r| r.edit.is_none() && !r.rs.is_command())
                    .map(|r| r.readonly_reason.clone().unwrap_or_else(|| "Resultado de solo lectura".into())),
                ResultTab::Output => None,
            };
            let tabs_w = (ui.available_width() - if lock.is_some() { 38.0 } else { 8.0 }).max(0.0);
            ui.allocate_ui_with_layout(vec2(tabs_w, 36.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("result_tabs")
                    .max_width(tabs_w)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let out_err = self.log.last().is_some_and(|e| e.is_error);
                            let out_ico = if out_err { Ico::Problems } else { Ico::Console };
                            let has_results = !self.results.is_empty();
                            let (out, _) = theme::tab(ui, self.result_tab == ResultTab::Output, Some(out_ico), "Output", false);
                            if out.clicked() {
                                self.result_tab = ResultTab::Output;
                            }
                            out.context_menu(|ui| {
                                if ui.add_enabled(has_results && !busy, egui::Button::new("Cerrar todos los resultados")).clicked() {
                                    close = Some(CloseResults::All);
                                    ui.close();
                                }
                            });
                            let n = self.results.len();
                            for (i, r) in self.results.iter().enumerate() {
                                let sel = self.result_tab == ResultTab::Result(i);
                                let (resp, closed) = theme::tab(ui, sel, Some(Ico::Table), &r.title, !busy);
                                let resp = resp.on_hover_text(&r.rs.statement);
                                if closed || (resp.middle_clicked() && !busy) {
                                    close = Some(CloseResults::One(i));
                                } else if resp.clicked() {
                                    self.result_tab = ResultTab::Result(i);
                                }
                                resp.context_menu(|ui| {
                                    if ui.add_enabled(!busy, egui::Button::new("Cerrar")).clicked() {
                                        close = Some(CloseResults::One(i));
                                        ui.close();
                                    }
                                    if ui.add_enabled(!busy && n > 1, egui::Button::new("Cerrar otras")).clicked() {
                                        close = Some(CloseResults::Others(i));
                                        ui.close();
                                    }
                                    if ui.add_enabled(!busy, egui::Button::new("Cerrar todas")).clicked() {
                                        close = Some(CloseResults::All);
                                        ui.close();
                                    }
                                });
                            }
                        });
                    });
            });
            if let Some(why) = lock {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), egui::Vec2::splat(14.0)), Ico::Lock, p.muted, &p, true);
                    resp.on_hover_text(format!("Solo lectura: {why}"));
                });
            }
        });
        if let Some(which) = close {
            actions.push(ConsoleAction::CloseResults(which));
        }
        hline(ui);
        let copy_requested = ui.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy)))
            && ui.memory(|m| m.focused().is_none());
        // Mini barra de edición sobre el grid si el resultado es editable.
        if let ResultTab::Result(i) = self.result_tab
            && self.results.get(i).is_some_and(|r| r.edit.is_some())
        {
            let busy = self.is_busy();
            let mut edit = None;
            toolbar_row(ui, |ui| {
                edit = results::edit_buttons(ui, self.results.get_mut(i), !busy);
                if let Some(v) = self.results.get(i) {
                    edit_status_icon(ui, v);
                }
            });
            if let Some(a) = edit {
                actions.push(edit_action(a, i));
            }
            hline(ui);
        }
        match self.result_tab {
            ResultTab::Result(i) if i < self.results.len() => {
                if let Some(err) = results::show_result(ui, &mut self.results[i], copy_requested, cx.columns) {
                    actions.push(ConsoleAction::Error(err));
                }
            }
            _ => {
                egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 8, top: 6, bottom: 4 }).show(ui, |ui| {
                    results::show_output(ui, &self.log);
                });
            }
        }
    }

    fn editor(&mut self, ui: &mut Ui, cx: &ConsoleCtx<'_>) {
        let pal = palette(cx.dark);
        let font = FontId::monospace(cx.font_size);
        let editor_id = self.editor_id();
        let focused = ui.memory(|m| m.has_focus(editor_id));

        // Teclas del autocompletado: se consumen antes de que las vea el TextEdit.
        let mut accept = false;
        if focused {
            if self.completion.is_some() {
                ui.input_mut(|i| {
                    let n = self.completion.as_ref().map(|c| c.items.len()).unwrap_or(0);
                    if let Some(c) = self.completion.as_mut() {
                        if i.consume_key(Modifiers::NONE, Key::ArrowDown) && n > 0 {
                            c.selected = (c.selected + 1) % n;
                            c.scroll_to_selected = true;
                        }
                        if i.consume_key(Modifiers::NONE, Key::ArrowUp) && n > 0 {
                            c.selected = (c.selected + n - 1) % n;
                            c.scroll_to_selected = true;
                        }
                    }
                    if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                        accept = true;
                    }
                    if i.consume_key(Modifiers::NONE, Key::Escape) {
                        self.completion = None;
                    }
                });
            }
            let open_req = ui.input_mut(|i| {
                let before = i.events.len();
                i.events.retain(|e| {
                    !matches!(e, Event::Key { key: Key::Space, pressed: true, modifiers, .. } if modifiers.ctrl)
                });
                // Algunos sistemas generan también el texto " " con Ctrl pulsado.
                let removed = before != i.events.len();
                if removed {
                    i.events.retain(|e| !matches!(e, Event::Text(t) if t == " "));
                }
                removed
            });
            if open_req && let Some((_, c)) = self.cursor {
                self.open_completion(c, cx);
            }
        }
        if accept {
            self.accept_completion(ui.ctx(), cx.driver);
        }

        let caret_line = self.line_col().0;
        // Layouter con caché del LayoutJob resaltado.
        let dark = cx.dark;
        let font_size = cx.font_size;
        let hl_cache = &mut self.hl_cache;
        let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| -> Arc<egui::Galley> {
            let text = buf.as_str();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut h);
            dark.hash(&mut h);
            font_size.to_bits().hash(&mut h);
            let key = h.finish();
            let job = match hl_cache {
                Some((k, job)) if *k == key => job.clone(),
                _ => {
                    let mut job = sql::highlight(text, dark, FontId::monospace(font_size));
                    job.wrap.max_width = f32::INFINITY;
                    *hl_cache = Some((key, job.clone()));
                    job
                }
            };
            ui.fonts_mut(|f| f.layout_job(job))
        };

        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let line_count = self.text.bytes().filter(|&b| b == b'\n').count() + 1;
        let digits = line_count.to_string().len().max(2) as f32;
        let gutter_w = digits * cx.font_size * 0.62 + 22.0;

        let mut typed_dot = false;
        let mut new_cursor = None;
        let mut popup_anchor = None;

        egui::Frame::NONE.fill(pal.editor_bg).show(ui, |ui| {
            let avail = ui.available_size();
            egui::ScrollArea::both()
                .id_salt(("editor_scroll", self.id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let caret_bg = ui.painter().add(egui::Shape::Noop);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let content_h = (line_count as f32 * row_h + 8.0).max(avail.y - 4.0);
                        let (gutter_rect, _) = ui.allocate_exact_size(vec2(gutter_w, content_h), Sense::hover());
                        ui.add_space(14.0);
                        let width = (avail.x - gutter_w - 18.0).max(100.0);
                        let output = egui::TextEdit::multiline(&mut self.text)
                            .id(editor_id)
                            .font(font.clone())
                            .code_editor()
                            .frame(egui::Frame::NONE)
                            .margin(vec2(0.0, 6.0))
                            .background_color(egui::Color32::TRANSPARENT)
                            .desired_width(width)
                            .min_size(vec2(width, content_h))
                            .lock_focus(true)
                            .layouter(&mut layouter)
                            .show(ui);

                        let mut line = 1;
                        let mut start_of_line = true;
                        for row in &output.galley.rows {
                            if start_of_line {
                                let y = output.galley_pos.y + row.pos.y;
                                let on_caret = line == caret_line;
                                if on_caret && focused {
                                    let r = Rect::from_min_max(
                                        pos2(gutter_rect.left(), y),
                                        pos2(gutter_rect.left() + avail.x.max(gutter_w + width), y + row.height()),
                                    );
                                    ui.painter().set(caret_bg, egui::Shape::rect_filled(r, 0.0, pal.caret_row));
                                }
                                ui.painter().text(
                                    egui::pos2(gutter_rect.right() - 4.0, y),
                                    egui::Align2::RIGHT_TOP,
                                    line.to_string(),
                                    font.clone(),
                                    if on_caret { pal.gutter_fg_caret } else { pal.gutter_fg },
                                );
                                line += 1;
                            }
                            start_of_line = row.ends_with_newline;
                        }

                        if let Some(range) = output.cursor_range {
                            let r = range.as_sorted_char_range();
                            new_cursor = Some((r.start.0, r.end.0, range.primary));
                        }
                        if output.response.changed() {
                            typed_dot = ui.input(|i| {
                                i.events.iter().any(|e| matches!(e, Event::Text(t) if t.ends_with('.')))
                            });
                        }
                        if self.completion.is_some()
                            && let Some((_, _, primary)) = new_cursor
                        {
                            let r = output.galley.pos_from_cursor(primary);
                            popup_anchor = Some(output.galley_pos + r.left_bottom().to_vec2() + vec2(0.0, 2.0));
                        }
                    });
                });
        });

        if let Some((a, b, _)) = new_cursor {
            self.cursor = Some((a, b));
        }
        if typed_dot && let Some((_, c)) = self.cursor {
            self.open_completion(c, cx);
        }
        if self.completion.is_some() {
            self.refresh_completion(cx);
        }
        if let Some(anchor) = popup_anchor
            && self.completion.is_some()
            && self.completion_popup(ui, anchor)
        {
            self.accept_completion(ui.ctx(), cx.driver);
            ui.memory_mut(|m| m.request_focus(editor_id));
        }
        let popup_id = egui::Id::new(("completion", self.id));
        let over_popup = ui
            .input(|i| i.pointer.hover_pos())
            .and_then(|p| ui.ctx().layer_id_at(p))
            .is_some_and(|l| l.id == popup_id);
        if !ui.memory(|m| m.has_focus(editor_id)) && !over_popup {
            self.completion = None;
        }
    }

    /// Dibuja el popup; devuelve true si se hizo clic en un elemento.
    fn completion_popup(&mut self, ui: &Ui, anchor: egui::Pos2) -> bool {
        let pal = palette(ui.visuals().dark_mode);
        let Some(comp) = self.completion.as_mut() else { return false };
        let mut clicked = false;
        egui::Area::new(egui::Id::new(("completion", self.id)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).inner_margin(4).show(ui, |ui| {
                    ui.set_min_width(300.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        for (i, item) in comp.items.iter().enumerate() {
                            let sel = i == comp.selected;
                            let ico = match item.kind {
                                ItemKind::Keyword => None,
                                ItemKind::Table => Some(Ico::Table),
                                ItemKind::View => Some(Ico::View),
                                ItemKind::Column => Some(Ico::Column),
                                ItemKind::Schema => Some(Ico::Schema),
                            };
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), 24.0), Sense::click());
                            if sel {
                                ui.painter().rect_filled(rect, 5.0, pal.selection);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, 5.0, pal.row_hover);
                            }
                            let cy = rect.center().y;
                            if let Some(ico) = ico {
                                icons::paint(ui.painter(), Rect::from_min_size(pos2(rect.left() + 6.0, cy - 8.0), egui::Vec2::splat(16.0)), ico, pal.icon, &pal, true);
                            }
                            let font = if item.kind == ItemKind::Keyword { theme::mono_bold(13.0) } else { FontId::monospace(13.0) };
                            let color = if item.kind == ItemKind::Keyword {
                                if pal.dark { egui::Color32::from_rgb(0xCF, 0x8E, 0x6D) } else { egui::Color32::from_rgb(0x00, 0x33, 0xB3) }
                            } else {
                                pal.text
                            };
                            ui.painter().text(pos2(rect.left() + 28.0, cy), egui::Align2::LEFT_CENTER, &item.text, font, color);
                            if !item.detail.is_empty() {
                                ui.painter().text(
                                    pos2(rect.right() - 8.0, cy),
                                    egui::Align2::RIGHT_CENTER,
                                    &item.detail,
                                    FontId::proportional(12.0),
                                    pal.muted,
                                );
                            }
                            if sel && comp.scroll_to_selected {
                                resp.scroll_to_me(None);
                            }
                            if resp.clicked() {
                                comp.selected = i;
                                clicked = true;
                            }
                        }
                    });
                });
            });
        comp.scroll_to_selected = false;
        clicked
    }

    fn open_completion(&mut self, cursor: usize, cx: &ConsoleCtx<'_>) {
        let chars: Vec<char> = self.text.chars().collect();
        let cursor = cursor.min(chars.len());
        let mut start = cursor;
        while start > 0 && is_ident_char(chars[start - 1]) {
            start -= 1;
        }
        self.completion = Some(Completion { start, selected: 0, items: Vec::new(), scroll_to_selected: false });
        self.refresh_completion(cx);
    }

    fn refresh_completion(&mut self, cx: &ConsoleCtx<'_>) {
        let Some((_, cursor)) = self.cursor else {
            self.completion = None;
            return;
        };
        let Some(comp) = self.completion.as_ref() else { return };
        let chars: Vec<char> = self.text.chars().collect();
        let start = comp.start;
        if cursor < start || cursor > chars.len() || chars[start..cursor].iter().any(|c| !is_ident_char(*c)) {
            self.completion = None;
            return;
        }
        let prefix: String = chars[start..cursor].iter().collect();
        let qualifier = if start > 0 && chars[start - 1] == '.' {
            let mut q = start - 1;
            while q > 0 && (is_ident_char(chars[q - 1]) || matches!(chars[q - 1], '"' | '[' | ']' | '`')) {
                q -= 1;
            }
            let raw: String = chars[q..start - 1].iter().collect();
            Some(raw.trim_matches(|c| matches!(c, '"' | '[' | ']' | '`')).to_string())
        } else {
            None
        };
        let items = build_items(&prefix, qualifier.as_deref(), &self.text, cx.schemas.unwrap_or(&[]));
        if items.is_empty() {
            self.completion = None;
            return;
        }
        if let Some(c) = self.completion.as_mut() {
            c.selected = c.selected.min(items.len() - 1);
            c.items = items;
        }
    }

    fn accept_completion(&mut self, ctx: &egui::Context, driver: Driver) {
        let Some(comp) = self.completion.take() else { return };
        let Some(item) = comp.items.get(comp.selected) else { return };
        let Some((_, cursor)) = self.cursor else { return };
        let insert = match item.kind {
            ItemKind::Keyword => item.text.clone(),
            _ => {
                if item.text.chars().all(is_ident_char) && !item.text.starts_with(|c: char| c.is_ascii_digit()) {
                    item.text.clone()
                } else {
                    quote_ident(driver, &item.text)
                }
            }
        };
        let s = char_to_byte(&self.text, comp.start);
        let e = char_to_byte(&self.text, cursor.max(comp.start));
        self.text.replace_range(s..e, &insert);
        let new_pos = byte_to_char(&self.text, s + insert.len());
        self.set_cursor(ctx, new_pos, new_pos);
        self.hl_cache = None;
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn matches_prefix(candidate: &str, prefix: &str) -> bool {
    prefix.is_empty() || candidate.to_lowercase().starts_with(&prefix.to_lowercase())
}

fn build_items(prefix: &str, qualifier: Option<&str>, text: &str, schemas: &[SchemaInfo]) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Vec::new();
    let table_kind = |k: ObjectKind| if k == ObjectKind::Table { ItemKind::Table } else { ItemKind::View };

    if let Some(q) = qualifier {
        // esquema.<tabla>
        for s in schemas.iter().filter(|s| s.name.eq_ignore_ascii_case(q)) {
            for t in &s.tables {
                if matches_prefix(&t.name, prefix) {
                    items.push(CompletionItem { text: t.name.clone(), kind: table_kind(t.kind), detail: s.name.clone() });
                }
            }
        }
        // tabla.<columna> o alias.<columna>
        let table_name = resolve_alias(q, text, schemas).unwrap_or_else(|| q.to_string());
        for s in schemas {
            for t in s.tables.iter().filter(|t| t.name.eq_ignore_ascii_case(&table_name)) {
                for c in &t.columns {
                    if matches_prefix(&c.name, prefix) {
                        items.push(CompletionItem { text: c.name.clone(), kind: ItemKind::Column, detail: c.data_type.clone() });
                    }
                }
            }
        }
    } else {
        let mut seen_cols = std::collections::HashSet::new();
        for s in schemas {
            if schemas.len() > 1 && matches_prefix(&s.name, prefix) {
                items.push(CompletionItem { text: s.name.clone(), kind: ItemKind::Schema, detail: "esquema".into() });
            }
            for t in &s.tables {
                if matches_prefix(&t.name, prefix) {
                    items.push(CompletionItem { text: t.name.clone(), kind: table_kind(t.kind), detail: s.name.clone() });
                }
                for c in &t.columns {
                    if !prefix.is_empty() && matches_prefix(&c.name, prefix) && seen_cols.insert(c.name.to_lowercase()) {
                        items.push(CompletionItem { text: c.name.clone(), kind: ItemKind::Column, detail: t.name.clone() });
                    }
                }
            }
        }
        for kw in sql::KEYWORDS {
            if matches_prefix(kw, prefix) {
                items.push(CompletionItem { text: kw.to_string(), kind: ItemKind::Keyword, detail: String::new() });
            }
        }
    }
    items.sort_by(|a, b| {
        let a_exact = a.text.eq_ignore_ascii_case(prefix);
        let b_exact = b.text.eq_ignore_ascii_case(prefix);
        b_exact.cmp(&a_exact).then(a.kind.cmp(&b.kind)).then_with(|| a.text.to_lowercase().cmp(&b.text.to_lowercase()))
    });
    items.dedup_by(|a, b| a.text == b.text && a.kind == b.kind);
    items.truncate(200);
    items
}

/// Busca `<tabla> [AS] <alias>` en el texto para resolver un alias a su tabla.
fn resolve_alias(alias: &str, text: &str, schemas: &[SchemaInfo]) -> Option<String> {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')' | ';'))
        .filter(|w| !w.is_empty())
        .collect();
    let is_table = |w: &str| -> Option<String> {
        let last = w.rsplit('.').next()?.trim_matches(|c| matches!(c, '"' | '[' | ']' | '`'));
        schemas
            .iter()
            .flat_map(|s| &s.tables)
            .find(|t| t.name.eq_ignore_ascii_case(last))
            .map(|t| t.name.clone())
    };
    for i in 0..words.len() {
        let next = words.get(i + 1).copied();
        let next2 = words.get(i + 2).copied();
        let aliased = next.is_some_and(|n| n.eq_ignore_ascii_case(alias))
            || (next.is_some_and(|n| n.eq_ignore_ascii_case("as")) && next2.is_some_and(|n| n.eq_ignore_ascii_case(alias)));
        if aliased && let Some(t) = is_table(words[i]) {
            return Some(t);
        }
    }
    None
}

/// Contador de cambios pendientes y aviso de tabla sin clave primaria.
fn edit_status_icon(ui: &mut Ui, v: &ResultView) {
    let p = pal(ui);
    let Some(e) = &v.edit else { return };
    if !e.target.has_pk {
        let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        icons::paint(ui.painter(), Rect::from_center_size(rect.center(), egui::Vec2::splat(15.0)), Ico::Problems, p.warn, &p, true);
        resp.on_hover_text(
            "La tabla no tiene clave primaria: cada fila se identifica por todos sus valores \
             originales. Si un cambio no afecta exactamente a una fila, no se guarda nada.",
        );
    }
    let n = e.state.pending_count();
    if n > 0 {
        ui.add_space(4.0);
        let text = if n == 1 { "1 cambio".to_string() } else { format!("{n} cambios") };
        ui.label(RichText::new(text).color(p.accent).size(12.5));
    }
}

/// Línea horizontal fina a todo el ancho (separador interno de isla).
fn hline(ui: &mut Ui) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.island_border));
}

/// Fila de barra de herramientas de 36 px.
fn toolbar_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 36.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(vec2(w, 36.0));
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        add(ui);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ColumnInfo, TableInfo};

    fn schemas() -> Vec<SchemaInfo> {
        vec![SchemaInfo {
            name: "main".into(),
            tables: vec![TableInfo {
                name: "users".into(),
                kind: ObjectKind::Table,
                columns: vec![ColumnInfo { name: "email".into(), data_type: "TEXT".into(), nullable: true, primary_key: false }],
            }],
        }]
    }

    #[test]
    fn alias_resolution() {
        let s = schemas();
        assert_eq!(resolve_alias("u", "SELECT u. FROM users u", &s).as_deref(), Some("users"));
        assert_eq!(resolve_alias("x", "select * from main.users AS x", &s).as_deref(), Some("users"));
        assert!(resolve_alias("z", "select 1", &s).is_none());
    }

    #[test]
    fn column_items_after_dot() {
        let s = schemas();
        let items = build_items("em", Some("users"), "", &s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "email");
    }

    #[test]
    fn indices_a_cerrar() {
        assert_eq!(CloseResults::One(1).indices(3), vec![1]);
        assert!(CloseResults::One(5).indices(3).is_empty());
        assert_eq!(CloseResults::Others(1).indices(3), vec![0, 2]);
        assert_eq!(CloseResults::All.indices(2), vec![0, 1]);
    }

    #[test]
    fn reindexado_tras_cerrar() {
        assert_eq!(remap_index(0, &[1]), Some(0));
        assert_eq!(remap_index(1, &[1]), None);
        assert_eq!(remap_index(3, &[0, 2]), Some(1));
        // Se cierra la activa: pasa a la que ocupa su sitio o a la última.
        assert_eq!(tab_after_close(ResultTab::Result(1), &[1], 2), ResultTab::Result(1));
        assert_eq!(tab_after_close(ResultTab::Result(2), &[2], 2), ResultTab::Result(1));
        assert_eq!(tab_after_close(ResultTab::Result(2), &[0], 2), ResultTab::Result(1));
        assert_eq!(tab_after_close(ResultTab::Result(0), &[0, 1], 0), ResultTab::Output);
        assert_eq!(tab_after_close(ResultTab::Output, &[0], 1), ResultTab::Output);
    }

    #[test]
    fn cerrar_resultados_de_consola() {
        let rs = |stmt: &str| crate::db::ResultSet { statement: stmt.into(), ..Default::default() };
        let mut c = Console::new(1, "c".into(), String::new(), None);
        c.results = (0..4).map(|i| ResultView::new(rs(&format!("select {i}")), i)).collect();
        c.result_tab = ResultTab::Result(2);
        c.replace_view = Some(3);
        assert_eq!(c.close_results(CloseResults::One(0)), vec![0]);
        assert_eq!(c.result_tab, ResultTab::Result(1));
        assert_eq!(c.replace_view, Some(2));
        assert_eq!(c.results[0].rs.statement, "select 1");
        c.close_results(CloseResults::Others(1));
        assert_eq!(c.results.len(), 1);
        assert_eq!(c.results[0].rs.statement, "select 2");
        assert_eq!(c.result_tab, ResultTab::Result(0));
        assert_eq!(c.replace_view, None);
        c.close_results(CloseResults::All);
        assert!(c.results.is_empty());
        assert_eq!(c.result_tab, ResultTab::Output);
    }
}
