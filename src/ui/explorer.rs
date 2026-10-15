//! Panel "Database Explorer": árbol de conexiones, bases de datos, esquemas, tablas y columnas
//! (estilo Islands).

use crate::app::{ConnState, ConnStatus};
use crate::db::{DatabaseInfo, Driver, ObjectKind, SchemaInfo, TableInfo};
use crate::ui::icons::{self, Ico};
use crate::ui::theme::{self, pal};
use crate::worker::ConnId;
use egui::{FontId, Rect, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};
use std::collections::HashSet;

#[derive(Default)]
pub struct ExplorerState {
    pub filter: String,
    pub show_filter: bool,
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    /// Tabla seleccionada (para los botones de la barra de herramientas).
    pub selected_table: Option<TableRef>,
    /// true si el último clic fue dentro del panel (selección "con foco" en azul).
    pub focused: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TableRef {
    pub conn: ConnId,
    pub db: String,
    pub schema: String,
    pub table: String,
}

pub enum ExplorerAction {
    NewConnection,
    Connect(ConnId),
    Disconnect(ConnId),
    Refresh(ConnId),
    Edit(ConnId),
    Duplicate(ConnId),
    Delete(ConnId),
    /// Nueva consola en la conexión (y la base, si se abre desde una base).
    NewConsole(ConnId, Option<String>),
    /// Cargar el esquema de una base al expandirla.
    LoadDatabase(ConnId, String),
    OpenTable(TableRef),
    GenerateSelect(TableRef),
    GenerateInsert(TableRef),
    GenerateDdl(TableRef),
    Hide,
}

const ROW_H: f32 = 26.0;
const INDENT: f32 = 20.0;

struct RowOut {
    resp: egui::Response,
    toggled: bool,
}

struct RowSpec<'a> {
    key: &'a str,
    depth: usize,
    expandable: bool,
    expanded: bool,
    icon: Ico,
    label: &'a str,
    /// Badge redondeado gris tras el nombre (p.ej. número de tablas).
    badge: Option<&'a str>,
    /// Texto secundario en gris (contadores, tipos).
    detail: Option<(&'a str, egui::Color32)>,
    status: Option<egui::Color32>,
    label_color: Option<egui::Color32>,
}

fn tree_row(ui: &mut Ui, state: &mut ExplorerState, spec: RowSpec<'_>) -> RowOut {
    let p = pal(ui);
    let width = ui.available_width().max(160.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        let mut toggled = false;
        if resp.double_clicked() && spec.expandable {
            toggled = true;
        }
        return RowOut { resp, toggled };
    }
    let selected = state.selected.as_deref() == Some(spec.key);
    let bg_rect = Rect::from_min_max(pos2(rect.left() + 6.0, rect.top() + 1.0), pos2(rect.right() - 6.0, rect.bottom() - 1.0));
    if selected {
        let fill = if state.focused { p.selection } else { p.selection_inactive };
        ui.painter().rect_filled(bg_rect, 5.0, fill);
    } else if resp.hovered() {
        ui.painter().rect_filled(bg_rect, 5.0, p.row_hover);
    }
    let x0 = rect.left() + 12.0 + spec.depth as f32 * INDENT;
    let cy = rect.center().y;
    let chevron_rect = Rect::from_min_size(pos2(x0, cy - 8.0), Vec2::splat(16.0));
    let mut toggled = false;
    if spec.expandable {
        let ico = if spec.expanded { Ico::ChevronDown } else { Ico::ChevronRight };
        icons::paint(ui.painter(), chevron_rect.shrink(1.0), ico, p.muted, &p, true);
        if resp.clicked()
            && let Some(pt) = resp.interact_pointer_pos()
            && pt.x < chevron_rect.right() + 2.0
        {
            toggled = true;
        }
    }
    let icon_rect = Rect::from_min_size(pos2(x0 + 20.0, cy - 8.0), Vec2::splat(16.0));
    icons::paint(ui.painter(), icon_rect, spec.icon, p.icon, &p, true);
    if let Some(c) = spec.status {
        let center = pos2(icon_rect.right() - 1.0, icon_rect.bottom() - 2.0);
        ui.painter().circle_filled(center, 3.6, p.island_bg);
        ui.painter().circle_filled(center, 2.6, c);
    }
    let painter = ui.painter_at(rect);
    let text_color = spec.label_color.unwrap_or(p.text);
    let label_rect = painter.text(
        pos2(icon_rect.right() + 6.0, cy),
        egui::Align2::LEFT_CENTER,
        spec.label,
        FontId::proportional(13.0),
        text_color,
    );
    let mut x = label_rect.right() + 6.0;
    if let Some(b) = spec.badge {
        let r = theme::paint_badge(ui, pos2(x, cy), b);
        x = r.right() + 6.0;
    }
    if let Some((d, color)) = spec.detail {
        painter.text(pos2(x, cy), egui::Align2::LEFT_CENTER, d, FontId::proportional(12.0), color);
    }
    if resp.clicked() || resp.secondary_clicked() {
        state.selected = Some(spec.key.to_string());
    }
    if resp.double_clicked() && spec.expandable {
        toggled = true;
    }
    RowOut { resp, toggled }
}

fn toggle(state: &mut ExplorerState, key: &str, open: bool) {
    if open {
        state.expanded.remove(key);
    } else {
        state.expanded.insert(key.to_string());
    }
}

fn table_matches(t: &TableInfo, f: &str) -> bool {
    f.is_empty() || t.name.to_lowercase().contains(f)
}

/// Base de la fila seleccionada (si es una base o algo dentro de ella).
fn selected_db(state: &ExplorerState, conns: &[ConnState]) -> Option<String> {
    let key = state.selected.as_deref()?;
    let (kind, rest) = key.split_once(':')?;
    if kind == "c" {
        return None;
    }
    let conn = conn_of_key(key)?;
    let c = conns.iter().find(|c| c.cfg.id == conn)?;
    let after = rest.split_once(':')?.1;
    // La base puede contener ':'; se busca la más larga que encaje.
    c.databases
        .iter()
        .filter(|d| after == d.name || after.starts_with(&format!("{}:", d.name)))
        .max_by_key(|d| d.name.len())
        .map(|d| d.name.clone())
}

/// Conexión de la fila seleccionada (si la hay).
fn selected_conn(state: &ExplorerState) -> Option<ConnId> {
    state.selected.as_deref().and_then(conn_of_key)
}

/// Cabecera de la tool window + barra de iconos.
fn header(ui: &mut Ui, state: &mut ExplorerState, conns: &[ConnState], actions: &mut Vec<ExplorerAction>) {
    let p = pal(ui);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(RichText::new("Database Explorer").font(theme::bold(13.5)).color(p.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            if theme::icon_button(ui, Ico::Minus, true, "Ocultar").clicked() {
                actions.push(ExplorerAction::Hide);
            }
            let more = theme::icon_button(ui, Ico::MoreVert, true, "Opciones");
            egui::Popup::menu(&more).show(|ui| {
                if ui.button("Nueva conexión…").clicked() {
                    actions.push(ExplorerAction::NewConnection);
                    ui.close();
                }
                if ui.button("Expandir conexiones").clicked() {
                    for c in conns {
                        state.expanded.insert(format!("c:{}", c.cfg.id));
                    }
                    ui.close();
                }
                if ui.button("Contraer todo").clicked() {
                    state.expanded.clear();
                    ui.close();
                }
            });
        });
    });
    ui.add_space(2.0);

    let sel_conn = selected_conn(state);
    let sel_state = sel_conn.and_then(|id| conns.iter().find(|c| c.cfg.id == id));
    let connected = sel_state.is_some_and(|c| matches!(c.status, ConnStatus::Connected));
    let active = sel_state.is_some_and(|c| matches!(c.status, ConnStatus::Connected | ConnStatus::Connecting));
    let table = state.selected_table.clone().filter(|t| Some(t.conn) == sel_conn);

    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.x = 1.0;
        if theme::icon_button(ui, Ico::Plus, true, "Nueva conexión").clicked() {
            actions.push(ExplorerAction::NewConnection);
        }
        if theme::icon_button(ui, Ico::Properties, sel_conn.is_some(), "Propiedades del data source").clicked()
            && let Some(id) = sel_conn
        {
            actions.push(ExplorerAction::Edit(id));
        }
        theme::toolbar_separator(ui);
        if theme::icon_button(ui, Ico::Refresh, !conns.is_empty(), "Refrescar").clicked() {
            for c in conns {
                if matches!(c.status, ConnStatus::Connected) && sel_conn.is_none_or(|id| id == c.cfg.id) {
                    actions.push(ExplorerAction::Refresh(c.cfg.id));
                }
            }
        }
        if theme::icon_button(ui, Ico::Stop, active, "Desconectar").clicked()
            && let Some(id) = sel_conn
        {
            actions.push(ExplorerAction::Disconnect(id));
        }
        theme::toolbar_separator(ui);
        if theme::icon_button(ui, Ico::JumpToConsole, sel_conn.is_some(), "Ir a consola").clicked()
            && let Some(id) = sel_conn
        {
            actions.push(ExplorerAction::NewConsole(id, selected_db(state, conns)));
        }
        if theme::icon_button(ui, Ico::Table, table.is_some(), "Editar datos de la tabla").clicked()
            && let Some(t) = table.clone()
        {
            actions.push(ExplorerAction::OpenTable(t));
        }
        if theme::icon_button(ui, Ico::Ddl, table.is_some() && connected, "Generar DDL").clicked()
            && let Some(t) = table.clone()
        {
            actions.push(ExplorerAction::GenerateDdl(t));
        }
        theme::toolbar_separator(ui);
        let eye = theme::icon_button(ui, Ico::Eye, true, "Filtrar tablas");
        if state.show_filter {
            ui.painter().rect_stroke(eye.rect, 6.0, Stroke::new(1.0, p.control_border), egui::StrokeKind::Inside);
        }
        if eye.clicked() {
            state.show_filter = !state.show_filter;
            if !state.show_filter {
                state.filter.clear();
            }
        }
    });
    if state.show_filter {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let w = ui.available_width() - 12.0;
            ui.add(
                egui::TextEdit::singleline(&mut state.filter)
                    .hint_text("Filtrar tablas…")
                    .desired_width(w)
                    .margin(vec2(8.0, 4.0)),
            );
        });
    }
    ui.add_space(4.0);
}

pub fn show(ui: &mut Ui, state: &mut ExplorerState, conns: &[ConnState]) -> Vec<ExplorerAction> {
    let mut actions = Vec::new();
    let p = pal(ui);
    let panel_rect = ui.max_rect();
    if ui.input(|i| i.pointer.any_pressed())
        && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
    {
        state.focused = panel_rect.contains(pos);
    }

    header(ui, state, conns, &mut actions);

    let filter = state.filter.trim().to_lowercase();

    egui::ScrollArea::both().id_salt("explorer_tree").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if conns.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Crea un data source con Alt+Insert").color(p.muted));
                ui.add_space(6.0);
                if theme::primary_button(ui, "Nueva conexión…").clicked() {
                    actions.push(ExplorerAction::NewConnection);
                }
            });
        }
        for c in conns {
            connection_rows(ui, state, &mut actions, c, &filter);
        }
        ui.add_space(8.0);
    });
    actions
}

fn db_has_match(d: &DatabaseInfo, filter: &str) -> bool {
    d.schemas.as_ref().is_some_and(|ss| ss.iter().any(|s| s.tables.iter().any(|t| table_matches(t, filter))))
}

fn connection_rows(ui: &mut Ui, state: &mut ExplorerState, actions: &mut Vec<ExplorerAction>, c: &ConnState, filter: &str) {
    let p = pal(ui);
    let filtering = !filter.is_empty();
    let id = c.cfg.id;
    let name_match = filtering && c.cfg.name.to_lowercase().contains(filter);
    let any_table = c.databases.iter().any(|d| db_has_match(d, filter));
    if filtering && !name_match && !any_table {
        return;
    }
    let ckey = format!("c:{id}");
    let open = state.expanded.contains(&ckey) || (filtering && any_table);
    let (dot, detail) = match &c.status {
        ConnStatus::Disconnected => (None, None),
        ConnStatus::Connecting => (Some(p.warn), Some(("conectando…", p.muted))),
        ConnStatus::Connected => (Some(p.ok), None),
        ConnStatus::Error(_) => (Some(p.error), Some(("error", p.error))),
    };
    let badge = (!c.databases.is_empty()).then(|| c.databases.len().to_string());
    let row = tree_row(
        ui,
        state,
        RowSpec {
            key: &ckey,
            depth: 0,
            expandable: true,
            expanded: open,
            icon: Ico::Driver(c.cfg.driver),
            label: &c.cfg.name,
            badge: badge.as_deref(),
            detail,
            status: dot,
            label_color: None,
        },
    );
    let mut resp = row.resp;
    if let ConnStatus::Error(e) = &c.status {
        resp = resp.on_hover_text(RichText::new(e).color(p.error));
    } else {
        resp = resp.on_hover_text(connection_summary(c));
    }
    if row.toggled {
        toggle(state, &ckey, open);
        if !open && matches!(c.status, ConnStatus::Disconnected | ConnStatus::Error(_)) {
            actions.push(ExplorerAction::Connect(id));
        }
    }
    resp.context_menu(|ui| {
        let connected = matches!(c.status, ConnStatus::Connected);
        if ui.add_enabled(!connected, egui::Button::new("Conectar")).clicked() {
            actions.push(ExplorerAction::Connect(id));
            state.expanded.insert(ckey.clone());
            ui.close();
        }
        if ui.add_enabled(!matches!(c.status, ConnStatus::Disconnected), egui::Button::new("Desconectar")).clicked() {
            actions.push(ExplorerAction::Disconnect(id));
            ui.close();
        }
        if ui.add_enabled(connected, egui::Button::new("Refrescar")).clicked() {
            actions.push(ExplorerAction::Refresh(id));
            ui.close();
        }
        ui.separator();
        if ui.button("Nueva consola").clicked() {
            actions.push(ExplorerAction::NewConsole(id, None));
            ui.close();
        }
        ui.separator();
        if ui.button("Propiedades…").clicked() {
            actions.push(ExplorerAction::Edit(id));
            ui.close();
        }
        if ui.button("Duplicar").clicked() {
            actions.push(ExplorerAction::Duplicate(id));
            ui.close();
        }
        if ui.button(RichText::new("Eliminar…").color(p.error)).clicked() {
            actions.push(ExplorerAction::Delete(id));
            ui.close();
        }
    });
    if !open {
        return;
    }
    if let ConnStatus::Error(e) = &c.status {
        error_line(ui, 1, e);
    }
    for d in &c.databases {
        database_rows(ui, state, actions, c, d, filter, name_match);
    }
}

/// Mensaje de error en rojo bajo un nodo.
fn error_line(ui: &mut Ui, depth: usize, msg: &str) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.add_space(12.0 + depth as f32 * INDENT + 20.0);
        ui.add(egui::Label::new(RichText::new(msg).small().color(p.error)).wrap());
    });
}

#[allow(clippy::too_many_arguments)]
fn database_rows(
    ui: &mut Ui,
    state: &mut ExplorerState,
    actions: &mut Vec<ExplorerAction>,
    c: &ConnState,
    d: &DatabaseInfo,
    filter: &str,
    name_match: bool,
) {
    let p = pal(ui);
    let filtering = !filter.is_empty();
    let id = c.cfg.id;
    let has_match = db_has_match(d, filter);
    if filtering && !has_match && !name_match {
        return;
    }
    let dkey = format!("d:{id}:{}", d.name);
    let open = state.expanded.contains(&dkey) || (filtering && has_match);
    let loading = c.loading.contains(&d.name);
    let sqlite = c.cfg.driver == Driver::Sqlite;
    // SQLite: un único esquema "main" igual que la base; se muestran sus tablas directamente.
    let flat = sqlite && d.schemas.as_ref().is_some_and(|ss| ss.len() == 1);
    let badge = d.schemas.as_ref().map(|ss| {
        if flat { ss.iter().map(|s| s.tables.len()).sum::<usize>().to_string() } else { ss.len().to_string() }
    });
    let detail = if loading {
        Some(("cargando…", p.muted))
    } else if d.current && !sqlite {
        Some(("por defecto", p.muted))
    } else {
        None
    };
    let row = tree_row(
        ui,
        state,
        RowSpec {
            key: &dkey,
            depth: 1,
            expandable: true,
            expanded: open,
            icon: Ico::Database,
            label: &d.name,
            badge: badge.as_deref(),
            detail,
            status: None,
            label_color: None,
        },
    );
    if row.toggled {
        toggle(state, &dkey, open);
        if !open && d.schemas.is_none() && !loading && matches!(c.status, ConnStatus::Connected) {
            actions.push(ExplorerAction::LoadDatabase(id, d.name.clone()));
        }
    }
    let db_name = d.name.clone();
    row.resp.context_menu(|ui| {
        if ui.button("Nueva consola").clicked() {
            actions.push(ExplorerAction::NewConsole(id, Some(db_name.clone())));
            ui.close();
        }
        let connected = matches!(c.status, ConnStatus::Connected);
        if ui.add_enabled(connected && !loading, egui::Button::new("Recargar esquema")).clicked() {
            actions.push(ExplorerAction::LoadDatabase(id, db_name.clone()));
            ui.close();
        }
        if ui.button("Copiar nombre").clicked() {
            ui.ctx().copy_text(db_name.clone());
            ui.close();
        }
    });
    if !open {
        return;
    }
    if let Some(e) = c.db_errors.get(&d.name) {
        error_line(ui, 2, e);
    }
    let Some(schemas) = &d.schemas else {
        if loading {
            ui.horizontal(|ui| {
                ui.add_space(12.0 + 2.0 * INDENT + 20.0);
                ui.add(egui::Spinner::new().size(12.0).color(p.muted));
                ui.label(RichText::new("cargando…").small().color(p.muted));
            });
        }
        return;
    };
    if flat {
        if let Some(s) = schemas.first() {
            object_groups(ui, state, actions, id, &d.name, s, filter, 2);
        }
        return;
    }
    for s in schemas {
        schema_rows(ui, state, actions, id, &d.name, schemas, s, filter, name_match);
    }
}

#[allow(clippy::too_many_arguments)]
fn schema_rows(
    ui: &mut Ui,
    state: &mut ExplorerState,
    actions: &mut Vec<ExplorerAction>,
    conn: ConnId,
    db: &str,
    all: &[SchemaInfo],
    s: &SchemaInfo,
    filter: &str,
    name_match: bool,
) {
    let filtering = !filter.is_empty();
    let skey = format!("s:{conn}:{db}:{}", s.name);
    let s_any = s.tables.iter().any(|t| table_matches(t, filter));
    if filtering && !s_any && !name_match {
        return;
    }
    let s_open = state.expanded.contains(&skey)
        || (filtering && s_any)
        || (all.len() == 1 && !state.expanded.contains(&format!("{skey}:closed")));
    let n = s.tables.len().to_string();
    let row = tree_row(
        ui,
        state,
        RowSpec {
            key: &skey,
            depth: 2,
            expandable: true,
            expanded: s_open,
            icon: Ico::Schema,
            label: &s.name,
            badge: None,
            detail: (!s.tables.is_empty()).then_some((n.as_str(), pal(ui).muted)),
            status: None,
            label_color: None,
        },
    );
    if row.toggled {
        if all.len() == 1 {
            let closed_key = format!("{skey}:closed");
            if s_open {
                state.expanded.insert(closed_key);
                state.expanded.remove(&skey);
            } else {
                state.expanded.remove(&closed_key);
            }
        } else {
            toggle(state, &skey, s_open);
        }
    }
    if s_open {
        object_groups(ui, state, actions, conn, db, s, filter, 3);
    }
}

/// Carpetas "tables" / "views" de un esquema y sus objetos.
#[allow(clippy::too_many_arguments)]
fn object_groups(
    ui: &mut Ui,
    state: &mut ExplorerState,
    actions: &mut Vec<ExplorerAction>,
    conn: ConnId,
    db: &str,
    s: &SchemaInfo,
    filter: &str,
    depth: usize,
) {
    let p = pal(ui);
    let filtering = !filter.is_empty();
    for (kind, label, gk) in [(ObjectKind::Table, "tables", "t"), (ObjectKind::View, "views", "v")] {
        let tables: Vec<&TableInfo> = s.tables.iter().filter(|t| t.kind == kind && table_matches(t, filter)).collect();
        if tables.is_empty() {
            continue;
        }
        let gkey = format!("g:{conn}:{db}:{}:{gk}", s.name);
        let g_open = !state.expanded.contains(&format!("{gkey}:closed")) || filtering;
        let n = tables.len().to_string();
        let row = tree_row(
            ui,
            state,
            RowSpec {
                key: &gkey,
                depth,
                expandable: true,
                expanded: g_open,
                icon: Ico::Folder,
                label,
                badge: None,
                detail: Some((&n, p.muted)),
                status: None,
                label_color: None,
            },
        );
        if row.toggled {
            let closed_key = format!("{gkey}:closed");
            if g_open {
                state.expanded.insert(closed_key);
            } else {
                state.expanded.remove(&closed_key);
            }
        }
        if !g_open {
            continue;
        }
        for t in tables {
            let tref = TableRef { conn, db: db.to_string(), schema: s.name.clone(), table: t.name.clone() };
            table_rows(ui, state, actions, tref, t, depth + 1);
        }
    }
}

fn table_rows(ui: &mut Ui, state: &mut ExplorerState, actions: &mut Vec<ExplorerAction>, tref: TableRef, t: &TableInfo, depth: usize) {
    let p = pal(ui);
    let tkey = table_key(&tref);
    let open = state.expanded.contains(&tkey);
    let icon = if t.kind == ObjectKind::Table { Ico::Table } else { Ico::View };
    let row = tree_row(
        ui,
        state,
        RowSpec {
            key: &tkey,
            depth,
            expandable: !t.columns.is_empty(),
            expanded: open,
            icon,
            label: &t.name,
            badge: None,
            detail: None,
            status: None,
            label_color: None,
        },
    );
    if row.resp.clicked() || row.resp.secondary_clicked() {
        state.selected_table = Some(tref.clone());
    }
    if row.resp.double_clicked() {
        actions.push(ExplorerAction::OpenTable(tref.clone()));
    } else if row.toggled {
        toggle(state, &tkey, open);
    }
    row.resp.context_menu(|ui| {
        if ui.button("Editar datos").clicked() {
            actions.push(ExplorerAction::OpenTable(tref.clone()));
            ui.close();
        }
        if ui.button("Copiar nombre").clicked() {
            ui.ctx().copy_text(t.name.clone());
            ui.close();
        }
        ui.separator();
        if ui.button("Generar SELECT").clicked() {
            actions.push(ExplorerAction::GenerateSelect(tref.clone()));
            ui.close();
        }
        if ui.button("Generar INSERT").clicked() {
            actions.push(ExplorerAction::GenerateInsert(tref.clone()));
            ui.close();
        }
        if ui.button("Generar DDL").clicked() {
            actions.push(ExplorerAction::GenerateDdl(tref.clone()));
            ui.close();
        }
    });
    if !open {
        return;
    }
    for col in &t.columns {
        let ckey = format!("{tkey}:{}", col.name);
        let mut detail = col.data_type.to_lowercase();
        if !col.nullable {
            detail.push_str(" not null");
        }
        let row = tree_row(
            ui,
            state,
            RowSpec {
                key: &ckey,
                depth: depth + 1,
                expandable: false,
                expanded: false,
                icon: if col.primary_key { Ico::Key } else { Ico::Column },
                label: &col.name,
                badge: None,
                detail: Some((&detail, p.muted)),
                status: None,
                label_color: None,
            },
        );
        row.resp.context_menu(|ui| {
            if ui.button("Copiar nombre").clicked() {
                ui.ctx().copy_text(col.name.clone());
                ui.close();
            }
        });
    }
}

/// Clave del árbol de una tabla (para expandirla o seleccionarla desde fuera).
pub fn table_key(t: &TableRef) -> String {
    format!("t:{}:{}:{}:{}", t.conn, t.db, t.schema, t.table)
}

/// Clave del árbol de una base.
pub fn db_key(conn: ConnId, db: &str) -> String {
    format!("d:{conn}:{db}")
}

fn conn_of_key(key: &str) -> Option<ConnId> {
    let rest = key.split_once(':')?.1;
    rest.split(':').next()?.parse().ok()
}

fn connection_summary(c: &ConnState) -> String {
    let cfg = &c.cfg;
    match cfg.driver {
        crate::db::Driver::Sqlite => format!("SQLite: {}", cfg.file_path),
        d => format!("{}: {}@{}:{}/{}", d.label(), cfg.user, cfg.host, cfg.port, cfg.database),
    }
}
