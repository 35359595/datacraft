//! Panel de resultados: grid de datos (TableBuilder virtualizado), log de salida y exportación CSV.

use crate::db::{Cell, ColumnInfo, ResultSet};
use crate::ui::icons::{self, Ico};
use crate::ui::theme::{self, pal};
use egui::{Color32, FontId, Rect, RichText, Sense, Stroke, Ui, pos2};
use egui_extras::{Column, TableBuilder};
use std::cmp::Ordering;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// Un result set con estado de vista (orden y celda seleccionada).
pub struct ResultView {
    pub rs: ResultSet,
    pub title: String,
    /// Permutación de filas según el orden actual.
    order: Vec<usize>,
    sort: Option<(usize, SortDir)>,
    selected: Option<(usize, usize)>,
    /// Sal para el id de la tabla (cambia en cada ejecución para reiniciar anchos).
    salt: u64,
}

impl ResultView {
    pub fn new(rs: ResultSet, salt: u64) -> Self {
        let title = statement_title(&rs.statement);
        let order = (0..rs.rows.len()).collect();
        Self { rs, title, order, sort: None, selected: None, salt }
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

    fn cell(&self, view_row: usize, col: usize) -> Option<&Cell> {
        let r = *self.order.get(view_row)?;
        self.rs.rows.get(r)?.get(col)
    }

    fn row_tsv(&self, view_row: usize) -> String {
        let Some(&r) = self.order.get(view_row) else { return String::new() };
        self.rs.rows.get(r).map(|row| row_to_tsv(row)).unwrap_or_default()
    }

    pub fn row_count(&self) -> usize {
        self.rs.rows.len()
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

/// Dibuja un result set como grid de datos al estilo DataGrip. `columns_info` permite marcar
/// las claves primarias en la cabecera. Devuelve un mensaje de error si la exportación falla.
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

    // Copiar celda seleccionada (Cmd+C cuando ningún widget tiene el foco).
    if copy_requested
        && let Some((r, c)) = view.selected
        && let Some(cell) = view.cell(r, c)
    {
        ui.ctx().copy_text(cell.clone().unwrap_or_default());
    }

    let n_rows = view.order.len();
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

    let mut sort_click = None;
    let mut new_selection = None;
    let mut copy_text: Option<String> = None;
    let sort = view.sort;
    let selected = view.selected;

    let pk_of = |name: &str| -> bool {
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
                    let (rect, resp) = header.col(|ui| {
                        let r = ui.max_rect();
                        ui.painter().rect_filled(r, 0.0, p.header_bg);
                        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.grid));
                        let painter = ui.painter_at(r);
                        let cy = r.center().y;
                        let ico = if pk_of(name) { Ico::Key } else { Ico::Column };
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
                    let _ = rect;
                    if resp.on_hover_text(format!("{name}\nClic para ordenar")).clicked() {
                        sort_click = Some(i);
                    }
                }
            })
            .body(|body| {
                body.rows(ROW_H, n_rows, |mut row| {
                    let vr = row.index();
                    row.col(|ui| {
                        let r = ui.max_rect();
                        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.grid));
                        ui.painter().text(
                            pos2(r.left() + 8.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            (vr + 1).to_string(),
                            cell_font.clone(),
                            p.gutter_fg,
                        );
                    });
                    for c in 0..n_cols {
                        let is_sel = selected == Some((vr, c));
                        let cell = view.cell(vr, c);
                        let (_, resp) = row.col(|ui| {
                            let r = ui.max_rect();
                            if is_sel {
                                ui.painter().rect_filled(r, 0.0, p.selection);
                            }
                            ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.grid));
                            let painter = ui.painter_at(r.shrink2(egui::vec2(6.0, 0.0)));
                            match cell {
                                Some(Some(v)) => {
                                    let shown: String =
                                        v.chars().take(300).map(|ch| if ch == '\n' || ch == '\r' { '⏎' } else { ch }).collect();
                                    let (pos, align) = if is_numeric(v) {
                                        (pos2(r.right() - 7.0, r.center().y), egui::Align2::RIGHT_CENTER)
                                    } else {
                                        (pos2(r.left() + 7.0, r.center().y), egui::Align2::LEFT_CENTER)
                                    };
                                    painter.text(pos, align, shown, cell_font.clone(), p.text);
                                }
                                _ => {
                                    painter.text(
                                        pos2(r.left() + 7.0, r.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        "<null>",
                                        cell_font.clone(),
                                        p.disabled,
                                    );
                                }
                            }
                        });
                        if resp.clicked() || resp.secondary_clicked() {
                            new_selection = Some((vr, c));
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Copiar valor").clicked() {
                                copy_text = Some(cell.and_then(|c| c.clone()).unwrap_or_default());
                                ui.close();
                            }
                            if ui.button("Copiar fila (TSV)").clicked() {
                                copy_text = Some(view.row_tsv(vr));
                                ui.close();
                            }
                            if ui.button("Copiar fila con cabeceras").clicked() {
                                copy_text = Some(format!("{}\n{}", view.rs.columns.join("\t"), view.row_tsv(vr)));
                                ui.close();
                            }
                        });
                    }
                });
            });
    });

    if let Some(e) = row_count_pill(ui, view, area_rect) {
        error = Some(e);
    }

    if let Some(c) = sort_click {
        view.toggle_sort(c);
        view.selected = None;
    }
    if let Some(sel) = new_selection {
        view.selected = Some(sel);
        ui.ctx().memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
    if let Some(t) = copy_text {
        ui.ctx().copy_text(t);
    }
    error
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
