//! Panel de resultados: grid de datos (TableBuilder virtualizado) con edición en línea,
//! log de salida y exportación CSV.

use crate::db::{Cell, ColumnInfo, ResultSet};
use crate::ui::editing::{EditState, EditTarget, RowRef};
use crate::ui::icons::{self, Ico};
use crate::ui::theme::{self, pal};
use egui::text::{CCursor, CCursorRange};
use egui::{Color32, FontId, Key, Modifiers, Rect, RichText, Sense, Stroke, Ui, pos2};
use egui_extras::{Column, TableBuilder};
use std::cmp::Ordering;
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// Editor en línea abierto sobre una celda.
struct InlineEdit {
    row: RowRef,
    col: usize,
    buffer: String,
    /// Pedir el foco (y seleccionar todo) en el próximo frame.
    focus: bool,
}

/// Estado de edición de un resultado editable.
pub struct EditSession {
    pub target: EditTarget,
    pub state: EditState,
    editing: Option<InlineEdit>,
}

/// Un result set con estado de vista (orden, selección y edición).
pub struct ResultView {
    pub rs: ResultSet,
    pub title: String,
    /// Permutación de filas según el orden actual.
    order: Vec<usize>,
    sort: Option<(usize, SortDir)>,
    /// Celda seleccionada (fila visual, columna).
    selected: Option<(usize, usize)>,
    /// Filas visuales seleccionadas (para eliminar varias).
    selected_rows: BTreeSet<usize>,
    anchor: Option<usize>,
    scroll_to: Option<usize>,
    /// Sal para el id de la tabla (cambia en cada ejecución para reiniciar anchos).
    salt: u64,
    /// Some si el resultado se puede editar.
    pub edit: Option<EditSession>,
    /// Motivo por el que es de solo lectura (tooltip).
    pub readonly_reason: Option<String>,
    /// true cuando la app ya decidió si es editable.
    pub edit_checked: bool,
}

impl ResultView {
    pub fn new(rs: ResultSet, salt: u64) -> Self {
        let title = statement_title(&rs.statement);
        let order = (0..rs.rows.len()).collect();
        Self {
            rs,
            title,
            order,
            sort: None,
            selected: None,
            selected_rows: BTreeSet::new(),
            anchor: None,
            scroll_to: None,
            salt,
            edit: None,
            readonly_reason: None,
            edit_checked: false,
        }
    }

    pub fn set_edit_target(&mut self, target: Result<EditTarget, String>) {
        self.edit_checked = true;
        match target {
            Ok(target) => {
                self.edit = Some(EditSession { target, state: EditState::default(), editing: None });
                self.readonly_reason = None;
            }
            Err(reason) => {
                self.edit = None;
                self.readonly_reason = Some(reason);
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.edit.as_ref().is_some_and(|e| e.state.is_dirty())
    }

    pub fn can_undo(&self) -> bool {
        self.edit.as_ref().is_some_and(|e| e.state.can_undo())
    }

    /// Descarta cambios pendientes y edición en curso.
    pub fn discard_edits(&mut self) {
        if let Some(e) = self.edit.as_mut() {
            e.state.clear();
            e.editing = None;
        }
        self.clamp_selection();
    }

    pub fn undo(&mut self) {
        if let Some(e) = self.edit.as_mut() {
            e.editing = None;
            e.state.undo();
        }
        self.clamp_selection();
    }

    pub fn revert_all(&mut self) {
        if let Some(e) = self.edit.as_mut() {
            e.editing = None;
            e.state.revert_all();
        }
        self.clamp_selection();
    }

    /// Filas visibles: las originales (en el orden actual) y después las nuevas.
    fn n_view_rows(&self) -> usize {
        self.order.len() + self.edit.as_ref().map_or(0, |e| e.state.inserted.len())
    }

    fn row_ref(&self, view_row: usize) -> Option<RowRef> {
        match self.order.get(view_row) {
            Some(&r) => Some(RowRef::Existing(r)),
            None => {
                let i = view_row - self.order.len();
                (i < self.edit.as_ref().map_or(0, |e| e.state.inserted.len())).then_some(RowRef::New(i))
            }
        }
    }

    fn clamp_selection(&mut self) {
        let n = self.n_view_rows();
        self.selected_rows.retain(|&r| r < n);
        if self.selected.is_some_and(|(r, _)| r >= n) {
            self.selected = None;
        }
    }

    /// Añade una fila nueva al final y abre el editor en su primera columna editable.
    pub fn add_row(&mut self) {
        let n_cols = self.rs.columns.len();
        let base = self.order.len();
        let Some(e) = self.edit.as_mut() else { return };
        let i = e.state.add_row(n_cols);
        let vr = base + i;
        let col = (0..n_cols).find(|&c| e.state.cell_editable(&self.rs, &e.target, RowRef::New(i), c)).unwrap_or(0);
        e.editing = Some(InlineEdit { row: RowRef::New(i), col, buffer: String::new(), focus: true });
        self.selected = Some((vr, col));
        self.selected_rows = BTreeSet::from([vr]);
        self.anchor = Some(vr);
        self.scroll_to = Some(vr);
    }

    /// Filas seleccionadas como referencias a filas originales/nuevas.
    fn selected_refs(&self) -> Vec<RowRef> {
        let mut rows: BTreeSet<usize> = self.selected_rows.clone();
        if let Some((r, _)) = self.selected {
            rows.insert(r);
        }
        rows.into_iter().filter_map(|r| self.row_ref(r)).collect()
    }

    pub fn has_selection(&self) -> bool {
        self.selected.is_some() || !self.selected_rows.is_empty()
    }

    /// Marca para borrar las filas seleccionadas (las nuevas se quitan directamente).
    pub fn delete_selected(&mut self) {
        let refs = self.selected_refs();
        let Some(e) = self.edit.as_mut() else { return };
        e.editing = None;
        if e.state.delete_rows(&self.rs, &e.target, &refs) && refs.iter().any(|r| matches!(r, RowRef::New(_))) {
            self.selected = None;
            self.selected_rows.clear();
        }
        self.clamp_selection();
    }

    /// Abre el editor en línea sobre la celda visual `(vr, col)` si es editable.
    fn start_edit(&mut self, vr: usize, col: usize) -> bool {
        let Some(row) = self.row_ref(vr) else { return false };
        let Some(e) = self.edit.as_mut() else { return false };
        if !e.state.cell_editable(&self.rs, &e.target, row, col) {
            return false;
        }
        let buffer = e.state.current(&self.rs, row, col).flatten().unwrap_or_default();
        e.editing = Some(InlineEdit { row, col, buffer, focus: true });
        self.selected = Some((vr, col));
        true
    }

    /// Confirma el editor en línea. `step` mueve a la celda editable siguiente/anterior.
    fn commit_edit(&mut self, step: i32) {
        let Some(e) = self.edit.as_mut() else { return };
        let Some(ed) = e.editing.take() else { return };
        let current = e.state.current(&self.rs, ed.row, ed.col);
        let unchanged_empty = ed.buffer.is_empty() && matches!(current, None | Some(None));
        if !unchanged_empty {
            e.state.set_value(&self.rs, ed.row, ed.col, Some(ed.buffer));
        }
        if step != 0 {
            let n = self.rs.columns.len() as i32;
            let mut c = ed.col as i32 + step;
            while (0..n).contains(&c) {
                if e.state.cell_editable(&self.rs, &e.target, ed.row, c as usize) {
                    let vr = self.view_row_of(ed.row);
                    if let Some(vr) = vr {
                        self.start_edit(vr, c as usize);
                    }
                    return;
                }
                c += step;
            }
        }
    }

    fn view_row_of(&self, row: RowRef) -> Option<usize> {
        match row {
            RowRef::Existing(r) => self.order.iter().position(|&x| x == r),
            RowRef::New(i) => Some(self.order.len() + i),
        }
    }

    fn toggle_sort(&mut self, col: usize) {
        self.sort = match self.sort {
            Some((c, SortDir::Asc)) if c == col => Some((col, SortDir::Desc)),
            Some((c, SortDir::Desc)) if c == col => None,
            _ => Some((col, SortDir::Asc)),
        };
        self.order = (0..self.rs.rows.len()).collect();
        if let Some((c, dir)) = self.sort {
            let rows = &self.rs.rows;
            self.order.sort_by(|&a, &b| {
                let ord = compare_cells(rows[a].get(c), rows[b].get(c));
                if dir == SortDir::Desc { ord.reverse() } else { ord }
            });
        }
    }

    /// Valor mostrado (con cambios pendientes). `None` = celda nueva sin tocar.
    fn display_value(&self, row: RowRef, col: usize) -> Option<Cell> {
        match &self.edit {
            Some(e) => e.state.current(&self.rs, row, col),
            None => match row {
                RowRef::Existing(r) => self.rs.rows.get(r).and_then(|x| x.get(col)).cloned(),
                RowRef::New(_) => None,
            },
        }
    }

    fn row_tsv(&self, view_row: usize) -> String {
        let Some(row) = self.row_ref(view_row) else { return String::new() };
        let vals: Vec<Cell> = (0..self.rs.columns.len()).map(|c| self.display_value(row, c).flatten()).collect();
        row_to_tsv(&vals)
    }

    pub fn row_count(&self) -> usize {
        self.rs.rows.len()
    }

    /// Abre el editor en línea en la celda visual `(vr, col)` (modo demo y atajos).
    pub fn begin_edit(&mut self, vr: usize, col: usize) -> bool {
        self.start_edit(vr, col)
    }

    /// Confirma el editor en línea si hay uno abierto (antes de previsualizar o guardar).
    pub fn finish_inline_edit(&mut self) {
        self.commit_edit(0);
    }
}

fn row_to_tsv(row: &[Cell]) -> String {
    row.iter()
        .map(|c| c.as_deref().unwrap_or("").replace(['\t', '\n'], " "))
        .collect::<Vec<_>>()
        .join("\t")
}

fn compare_cells(a: Option<&Cell>, b: Option<&Cell>) -> Ordering {
    let a = a.and_then(|c| c.as_deref());
    let b = b.and_then(|c| c.as_deref());
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => match (x.trim().parse::<f64>(), y.trim().parse::<f64>()) {
            (Ok(nx), Ok(ny)) => nx.partial_cmp(&ny).unwrap_or(Ordering::Equal),
            _ => x.cmp(y),
        },
    }
}

/// Título corto a partir de la sentencia.
pub fn statement_title(sql: &str) -> String {
    let one_line = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&one_line, 28)
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    } else {
        s.to_string()
    }
}

/// Entrada del log de ejecución.
pub struct OutputEntry {
    pub time: String,
    pub statement: String,
    pub message: String,
    pub is_error: bool,
}

pub fn affected_message(rs: &ResultSet) -> String {
    match rs.affected {
        Some(1) => "1 fila afectada".into(),
        Some(n) => format!("{n} filas afectadas"),
        None => "OK".into(),
    }
}

pub fn format_duration(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 { format!("{ms:.0} ms") } else { format!("{:.2} s", ms / 1000.0) }
}

/// Escapa un valor para CSV (RFC 4180).
pub fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn to_csv(view: &ResultView) -> String {
    let mut out = String::new();
    out.push_str(&view.rs.columns.iter().map(|c| csv_escape(c)).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    for &r in &view.order {
        if let Some(row) = view.rs.rows.get(r) {
            let line: Vec<String> = row.iter().map(|c| c.as_deref().map(csv_escape).unwrap_or_default()).collect();
            out.push_str(&line.join(","));
            out.push_str("\r\n");
        }
    }
    out
}

pub fn export_csv(view: &ResultView) -> Result<bool, String> {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Exportar a CSV")
        .add_filter("CSV", &["csv"])
        .set_file_name("resultado.csv")
        .save_file()
    else {
        return Ok(false);
    };
    std::fs::write(&path, to_csv(view)).map(|_| true).map_err(|e| e.to_string())
}

/// Alto de fila y de cabecera del grid.
const ROW_H: f32 = 24.0;
const HEADER_H: f32 = 30.0;

fn is_numeric(v: &str) -> bool {
    let t = v.trim();
    !t.is_empty() && t.len() < 40 && t.parse::<f64>().is_ok()
}

/// Lo que el grid pide hacer tras dibujarse (se aplica fuera del TableBuilder).
enum GridAction {
    Sort(usize),
    Select { vr: usize, col: Option<usize>, mods: Modifiers },
    StartEdit(usize, usize),
    Commit(i32),
    Cancel,
    SetNull(RowRef, usize),
    Revert(RowRef, usize),
    DeleteRows,
    Copy(String),
}

/// Dibuja un result set como grid de datos al estilo DataGrip. `columns_info` permite marcar
/// las claves primarias en la cabecera. Si la vista es editable (`view.edit`), permite editar
/// en línea (doble clic, Enter o F2). Devuelve un mensaje de error si la exportación falla.
pub fn show_result(
    ui: &mut Ui,
    view: &mut ResultView,
    copy_requested: bool,
    columns_info: Option<&[ColumnInfo]>,
) -> Option<String> {
    let p = pal(ui);
    let mut error = None;
    let area_rect = ui.max_rect();

    if view.rs.is_command() {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(format!("{} · {}", affected_message(&view.rs), format_duration(view.rs.elapsed))).color(p.muted),
            )
        });
        return error;
    }

    let mut actions: Vec<GridAction> = Vec::new();
    let editable = view.edit.is_some();
    let editing_now = view.edit.as_ref().is_some_and(|e| e.editing.is_some());

    // Teclado del grid (solo si ningún widget tiene el foco).
    if ui.memory(|m| m.focused().is_none()) && !editing_now {
        if copy_requested
            && let Some((r, c)) = view.selected
            && let Some(row) = view.row_ref(r)
        {
            ui.ctx().copy_text(view.display_value(row, c).flatten().unwrap_or_default());
        }
        if editable && let Some((r, c)) = view.selected {
            let (edit_key, undo) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::F2),
                    i.consume_key(Modifiers::COMMAND, Key::Z),
                )
            });
            if edit_key {
                actions.push(GridAction::StartEdit(r, c));
            }
            if undo {
                view.undo();
            }
        }
    }

    let n_rows = view.n_view_rows();
    let n_cols = view.rs.columns.len();
    let digits = n_rows.max(1).to_string().len() as f32;
    let cell_font = FontId::monospace(12.5);
    let char_w = ui.fonts_mut(|f| f.glyph_width(&cell_font, '0'));
    let index_width = 22.0 + digits * char_w;

    // Anchos iniciales según el contenido de las primeras filas.
    let initial_widths: Vec<f32> = (0..n_cols)
        .map(|c| {
            let header = view.rs.columns[c].chars().count();
            let content = view.rs.rows.iter().take(50).map(|r| r.get(c).and_then(|v| v.as_deref()).map_or(6, |v| v.chars().count())).max().unwrap_or(0);
            // Cabecera: icono (24) + nombre en negrita + embudo y orden (44).
            let header_w = header as f32 * (char_w + 0.6) + 72.0;
            let content_w = content.min(40) as f32 * char_w + 22.0;
            header_w.max(content_w).clamp(60.0, 380.0)
        })
        .collect();

    let sort = view.sort;
    let selected = view.selected;
    let selected_rows = view.selected_rows.clone();
    let scroll_to = view.scroll_to.take();
    // El editor en línea se saca de la vista para poder editarlo mientras se lee `view`.
    let mut inline = view.edit.as_mut().and_then(|e| e.editing.take());
    let edit_id = egui::Id::new(("cell_edit", view.salt));

    let pk_of = |i: usize, name: &str| -> bool {
        if let Some(e) = &view.edit
            && e.target.has_pk
        {
            return e.target.key_cols.contains(&i);
        }
        columns_info.is_some_and(|cols| cols.iter().any(|c| c.primary_key && c.name.eq_ignore_ascii_case(name)))
    };

    egui::ScrollArea::horizontal().id_salt(("result_h", view.salt)).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        {
            let v = ui.visuals_mut();
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.grid);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.control_border);
            v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
            v.widgets.hovered.bg_fill = theme::blend(p.island_bg, p.row_hover);
        }
        let mut table = TableBuilder::new(ui)
            .id_salt(("result_table", view.salt, n_cols))
            .striped(false)
            .resizable(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(index_width).resizable(false));
        if let Some(r) = scroll_to {
            table = table.scroll_to_row(r, None);
        }
        for w in &initial_widths {
            table = table.column(Column::initial(*w).at_least(36.0).clip(true).resizable(true));
        }
        table
            .header(HEADER_H, |mut header| {
                header.col(|ui| {
                    let r = ui.max_rect();
                    ui.painter().rect_filled(r, 0.0, p.header_bg);
                    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.grid));
                });
                for (i, name) in view.rs.columns.iter().enumerate() {
                    let (_, resp) = header.col(|ui| {
                        let r = ui.max_rect();
                        ui.painter().rect_filled(r, 0.0, p.header_bg);
                        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.grid));
                        let painter = ui.painter_at(r);
                        let cy = r.center().y;
                        let ico = if pk_of(i, name) { Ico::Key } else { Ico::Column };
                        icons::paint(&painter, Rect::from_min_size(pos2(r.left() + 6.0, cy - 7.0), egui::Vec2::splat(14.0)), ico, p.icon, &p, true);
                        let text_right = r.right() - 40.0;
                        let galley = painter.layout_no_wrap(name.clone(), theme::mono_bold(12.5), p.text);
                        let tp = pos2(r.left() + 24.0, cy - galley.size().y / 2.0);
                        painter.with_clip_rect(Rect::from_min_max(r.min, pos2(text_right.max(r.left() + 24.0), r.max.y))).galley(tp, galley, p.text);
                        let funnel = Rect::from_min_size(pos2(r.right() - 36.0, cy - 6.5), egui::Vec2::splat(13.0));
                        icons::paint(&painter, funnel, Ico::Filter, p.disabled, &p, true);
                        let sort_ico = match sort {
                            Some((c, SortDir::Asc)) if c == i => Ico::SortAsc,
                            Some((c, SortDir::Desc)) if c == i => Ico::SortDesc,
                            _ => Ico::Sort,
                        };
                        let sort_color = if matches!(sort, Some((c, _)) if c == i) { p.accent } else { p.disabled };
                        let sr = Rect::from_min_size(pos2(r.right() - 20.0, cy - 6.5), egui::Vec2::splat(13.0));
                        icons::paint(&painter, sr, sort_ico, sort_color, &p, true);
                    });
                    if resp.on_hover_text(format!("{name}\nClic para ordenar")).clicked() {
                        actions.push(GridAction::Sort(i));
                    }
                }
            })
            .body(|body| {
                body.rows(ROW_H, n_rows, |mut row| {
                    let vr = row.index();
                    let Some(rref) = view.row_ref(vr) else { return };
                    let edit = view.edit.as_ref();
                    let deleted = edit.is_some_and(|e| e.state.is_deleted(rref));
                    let is_new = matches!(rref, RowRef::New(_));
                    let row_sel = selected_rows.contains(&vr);
                    let row_bg = if deleted {
                        Some(p.row_deleted)
                    } else if is_new {
                        Some(p.row_added)
                    } else {
                        None
                    };
                    let (_, gresp) = row.col(|ui| {
                        let r = ui.max_rect();
                        if row_sel {
                            ui.painter().rect_filled(r, 0.0, p.selection_inactive);
                        } else if let Some(bg) = row_bg {
                            ui.painter().rect_filled(r, 0.0, bg);
                        }
                        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.grid));
                        let (label, color) = match rref {
                            RowRef::New(_) => ("+".to_string(), p.ok),
                            RowRef::Existing(_) if deleted => ((vr + 1).to_string(), p.error),
                            RowRef::Existing(_) => ((vr + 1).to_string(), p.gutter_fg),
                        };
                        ui.painter().text(pos2(r.left() + 8.0, r.center().y), egui::Align2::LEFT_CENTER, label, cell_font.clone(), color);
                    });
                    if gresp.clicked() || gresp.secondary_clicked() {
                        let mods = ui_mods(&gresp);
                        actions.push(GridAction::Select { vr, col: None, mods });
                    }
                    for c in 0..n_cols {
                        let is_sel = selected == Some((vr, c));
                        let value = view.display_value(rref, c);
                        let modified = edit.is_some_and(|e| e.state.is_modified(rref, c)) && !is_new;
                        let can_edit = edit.is_some_and(|e| e.state.cell_editable(&view.rs, &e.target, rref, c));
                        let editing_here = inline.as_ref().is_some_and(|ed| ed.row == rref && ed.col == c);
                        let (_, resp) = row.col(|ui| {
                            let r = ui.max_rect();
                            let bg = if is_sel && !editing_here {
                                Some(p.selection)
                            } else if modified && !deleted {
                                Some(p.cell_modified)
                            } else if row_sel {
                                Some(p.selection_inactive)
                            } else {
                                row_bg
                            };
                            if let Some(bg) = bg {
                                ui.painter().rect_filled(r, 0.0, bg);
                            }
                            ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.grid));
                            if editing_here && let Some(ed) = inline.as_mut() {
                                let focused = ui.memory(|m| m.has_focus(edit_id));
                                if focused {
                                    let keys = ui.input_mut(|i| {
                                        let back = i.consume_key(Modifiers::SHIFT, Key::Tab);
                                        let fwd = !back && i.consume_key(Modifiers::NONE, Key::Tab);
                                        let enter = i.consume_key(Modifiers::NONE, Key::Enter);
                                        let esc = i.consume_key(Modifiers::NONE, Key::Escape);
                                        (back, fwd, enter, esc)
                                    });
                                    match keys {
                                        (_, _, _, true) => actions.push(GridAction::Cancel),
                                        (true, _, _, _) => actions.push(GridAction::Commit(-1)),
                                        (_, true, _, _) => actions.push(GridAction::Commit(1)),
                                        (_, _, true, _) => actions.push(GridAction::Commit(0)),
                                        _ => {}
                                    }
                                }
                                ui.painter().rect_stroke(r.shrink(0.5), 2.0, Stroke::new(1.5, p.accent), egui::StrokeKind::Inside);
                                let edit_rect = r.shrink2(egui::vec2(5.0, 2.0));
                                let te = ui.put(
                                    edit_rect,
                                    egui::TextEdit::singleline(&mut ed.buffer)
                                        .id(edit_id)
                                        .font(cell_font.clone())
                                        .frame(egui::Frame::NONE)
                                        .margin(egui::vec2(2.0, 2.0))
                                        .background_color(Color32::TRANSPARENT)
                                        .vertical_align(egui::Align::Center)
                                        .desired_width(edit_rect.width()),
                                );
                                if ed.focus {
                                    ed.focus = false;
                                    te.request_focus();
                                    let mut st = egui::text_edit::TextEditState::load(ui.ctx(), edit_id).unwrap_or_default();
                                    let n = ed.buffer.chars().count();
                                    st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(n))));
                                    st.store(ui.ctx(), edit_id);
                                } else if te.lost_focus() && !actions.iter().any(|a| matches!(a, GridAction::Commit(_) | GridAction::Cancel)) {
                                    actions.push(GridAction::Commit(0));
                                }
                                return;
                            }
                            let painter = ui.painter_at(r.shrink2(egui::vec2(6.0, 0.0)));
                            let text_rect = match &value {
                                Some(Some(v)) => {
                                    let shown: String =
                                        v.chars().take(300).map(|ch| if ch == '\n' || ch == '\r' { '⏎' } else { ch }).collect();
                                    let (pos, align) = if is_numeric(v) {
                                        (pos2(r.right() - 7.0, r.center().y), egui::Align2::RIGHT_CENTER)
                                    } else {
                                        (pos2(r.left() + 7.0, r.center().y), egui::Align2::LEFT_CENTER)
                                    };
                                    painter.text(pos, align, shown, cell_font.clone(), if deleted { p.muted } else { p.text })
                                }
                                Some(None) => painter.text(
                                    pos2(r.left() + 7.0, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    "<null>",
                                    cell_font.clone(),
                                    p.disabled,
                                ),
                                None => painter.text(
                                    pos2(r.left() + 7.0, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    "<default>",
                                    cell_font.clone(),
                                    p.disabled,
                                ),
                            };
                            if deleted {
                                painter.hline(text_rect.x_range(), text_rect.center().y, Stroke::new(1.0, p.error));
                            }
                        });
                        if editing_here {
                            continue;
                        }
                        if resp.clicked() || resp.secondary_clicked() {
                            let mods = ui_mods(&resp);
                            actions.push(GridAction::Select { vr, col: Some(c), mods });
                        }
                        if resp.double_clicked() && can_edit {
                            actions.push(GridAction::StartEdit(vr, c));
                        }
                        let resp = if editable && !can_edit && resp.hovered() && edit.is_some_and(|e| !e.state.is_deleted(rref)) {
                            resp.on_hover_text("Celda de solo lectura")
                        } else {
                            resp
                        };
                        resp.context_menu(|ui| {
                            if editable {
                                if ui.add_enabled(can_edit, egui::Button::new("Editar").shortcut_text("F2")).clicked() {
                                    actions.push(GridAction::StartEdit(vr, c));
                                    ui.close();
                                }
                                let is_null = matches!(value, Some(None));
                                if ui.add_enabled(can_edit && !is_null, egui::Button::new("Set NULL")).clicked() {
                                    actions.push(GridAction::SetNull(rref, c));
                                    ui.close();
                                }
                                let revertible = deleted || edit.is_some_and(|e| e.state.is_modified(rref, c));
                                if ui.add_enabled(revertible, egui::Button::new("Revertir cambio")).clicked() {
                                    actions.push(GridAction::Revert(rref, c));
                                    ui.close();
                                }
                                if ui.add_enabled(!deleted, egui::Button::new("Eliminar fila")).clicked() {
                                    actions.push(GridAction::DeleteRows);
                                    ui.close();
                                }
                                ui.separator();
                            }
                            if ui.button("Copiar valor").clicked() {
                                actions.push(GridAction::Copy(value.clone().flatten().unwrap_or_default()));
                                ui.close();
                            }
                            if ui.button("Copiar fila (TSV)").clicked() {
                                actions.push(GridAction::Copy(view.row_tsv(vr)));
                                ui.close();
                            }
                            if ui.button("Copiar fila con cabeceras").clicked() {
                                actions.push(GridAction::Copy(format!("{}\n{}", view.rs.columns.join("\t"), view.row_tsv(vr))));
                                ui.close();
                            }
                        });
                    }
                });
            });
    });

    if let Some(e) = view.edit.as_mut() {
        e.editing = inline;
    }

    if let Some(e) = row_count_pill(ui, view, area_rect) {
        error = Some(e);
    }

    for a in actions {
        match a {
            GridAction::Sort(c) => {
                view.toggle_sort(c);
                view.selected = None;
                view.selected_rows.clear();
                view.anchor = None;
            }
            GridAction::Select { vr, col, mods } => {
                if mods.shift
                    && let Some(a) = view.anchor
                {
                    view.selected_rows = (a.min(vr)..=a.max(vr)).collect();
                } else if mods.command {
                    if !view.selected_rows.remove(&vr) {
                        view.selected_rows.insert(vr);
                    }
                    view.anchor = Some(vr);
                } else {
                    view.selected_rows = BTreeSet::from([vr]);
                    view.anchor = Some(vr);
                }
                view.selected = col.map(|c| (vr, c));
                ui.ctx().memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
            GridAction::StartEdit(vr, c) => {
                view.commit_edit(0);
                view.start_edit(vr, c);
            }
            GridAction::Commit(step) => {
                ui.memory_mut(|m| m.surrender_focus(edit_id));
                view.commit_edit(step);
            }
            GridAction::Cancel => {
                ui.memory_mut(|m| m.surrender_focus(edit_id));
                if let Some(e) = view.edit.as_mut() {
                    e.editing = None;
                }
            }
            GridAction::SetNull(row, c) => {
                if let Some(e) = view.edit.as_mut() {
                    e.state.set_value(&view.rs, row, c, None);
                }
            }
            GridAction::Revert(row, c) => {
                if let Some(e) = view.edit.as_mut() {
                    e.state.revert_cell(row, c);
                }
            }
            GridAction::DeleteRows => view.delete_selected(),
            GridAction::Copy(t) => ui.ctx().copy_text(t),
        }
    }
    error
}

fn ui_mods(resp: &egui::Response) -> Modifiers {
    resp.ctx.input(|i| i.modifiers)
}

/// Botones de edición de datos (+, −, deshacer, revertir, enviar) y "Preview".
/// Devuelve true si se pidió la previsualización de cambios.
/// Acción de la barra de edición que debe resolver la app.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    /// ⇧: guardar los cambios directamente.
    Submit,
    /// Abrir la ventana con el SQL de los cambios.
    Preview,
}

pub fn edit_buttons(ui: &mut Ui, view: Option<&mut ResultView>, enabled: bool) -> Option<EditAction> {
    let editable = enabled && view.as_ref().is_some_and(|v| v.edit.is_some());
    let dirty = editable && view.as_ref().is_some_and(|v| v.is_dirty());
    let can_undo = editable && view.as_ref().is_some_and(|v| v.can_undo());
    let has_sel = editable && view.as_ref().is_some_and(|v| v.has_selection());
    let add = theme::icon_button(ui, Ico::Plus, editable, "Añadir fila");
    let del = theme::icon_button(ui, Ico::Minus, has_sel, "Eliminar filas seleccionadas");
    let undo = theme::icon_button(ui, Ico::Undo, can_undo, "Deshacer (⌘Z)");
    let revert = theme::icon_button(ui, Ico::Revert, dirty, "Revertir todos los cambios");
    let submit = theme::icon_button(ui, Ico::Submit, dirty, "Enviar cambios (⌘S)");
    let pv = theme::flat_button(ui, Some(Ico::Eye), "Preview", FontId::proportional(13.0), false, dirty)
        .on_hover_text("Previsualizar el SQL de los cambios pendientes");
    if let Some(v) = view {
        if add.clicked() {
            v.add_row();
        }
        if del.clicked() {
            v.delete_selected();
        }
        if undo.clicked() {
            v.undo();
        }
        if revert.clicked() {
            v.revert_all();
        }
    }
    if submit.clicked() {
        Some(EditAction::Submit)
    } else if pv.clicked() {
        Some(EditAction::Preview)
    } else {
        None
    }
}

/// Píldora flotante centrada en la parte inferior del grid: "N rows ⌄  ⋮".
fn row_count_pill(ui: &Ui, view: &ResultView, area: Rect) -> Option<String> {
    let p = pal(ui);
    let n = view.rs.rows.len();
    let text = match (n, view.rs.truncated) {
        (1, _) => "1 row".to_string(),
        (n, true) => format!("{n}+ rows"),
        (n, false) => format!("{n} rows"),
    };
    let font = FontId::proportional(12.5);
    let tw = ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), p.text).size().x);
    let size = egui::vec2(10.0 + tw + 18.0 + 9.0 + 24.0 + 4.0, 28.0);
    let pos = pos2(area.center().x - size.x / 2.0, area.bottom() - size.y - 12.0);
    let mut error = None;
    egui::Area::new(egui::Id::new(("rows_pill", view.salt)))
        .order(egui::Order::Middle)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().add(
                egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: egui::Color32::from_black_alpha(if p.dark { 60 } else { 18 }) }
                    .as_shape(rect, 8.0),
            );
            ui.painter().rect(rect, 8.0, p.island_bg, Stroke::new(1.0, p.control_border), egui::StrokeKind::Inside);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(2.0, 1.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            child.spacing_mut().item_spacing.x = 0.0;
            let rows_btn = theme::flat_button(&mut child, None, &text, font.clone(), true, true)
                .on_hover_text(format!("Consulta ejecutada en {}", format_duration(view.rs.elapsed)));
            theme::toolbar_separator(&mut child);
            let more = theme::icon_button_sized(&mut child, Ico::MoreVert, true, "", 24.0);
            for resp in [rows_btn, more] {
                egui::Popup::menu(&resp).show(|ui| {
                    if ui.button("Exportar a CSV…").clicked() {
                        if let Err(e) = export_csv(view) {
                            error = Some(format!("No se pudo exportar: {e}"));
                        }
                        ui.close();
                    }
                    if ui.button("Copiar todo (CSV)").clicked() {
                        ui.ctx().copy_text(to_csv(view));
                        ui.close();
                    }
                });
            }
        });
    error
}

/// Log de ejecuciones.
pub fn show_output(ui: &mut Ui, log: &[OutputEntry]) {
    let pal = pal(ui);
    egui::ScrollArea::both()
        .id_salt("output_log")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if log.is_empty() {
                ui.label(RichText::new("Sin ejecuciones todavía.").color(pal.muted));
            }
            for e in log {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("[{}]", e.time)).monospace().color(pal.muted));
                    if !e.statement.is_empty() {
                        ui.label(RichText::new(statement_title_long(&e.statement)).monospace());
                    }
                });
                let color: Color32 = if e.is_error { pal.error } else { pal.ok };
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.add(egui::Label::new(RichText::new(&e.message).monospace().color(color)).wrap());
                });
            }
        });
}

fn statement_title_long(sql: &str) -> String {
    truncate_chars(&sql.split_whitespace().collect::<Vec<_>>().join(" "), 160)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escaping() {
        assert_eq!(csv_escape("abc"), "abc");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("di \"hola\""), "\"di \"\"hola\"\"\"");
        assert_eq!(csv_escape("x\ny"), "\"x\ny\"");
    }

    #[test]
    fn numeric_sort() {
        let a: Cell = Some("10".into());
        let b: Cell = Some("9".into());
        assert_eq!(compare_cells(Some(&a), Some(&b)), Ordering::Greater);
        let n: Cell = None;
        assert_eq!(compare_cells(Some(&n), Some(&b)), Ordering::Less);
    }
}
