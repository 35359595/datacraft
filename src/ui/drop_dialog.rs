//! Confirmation dialog to drop a database, table or view, showing the SQL that will run.

use crate::db::{Driver, ObjectKind};
use crate::i18n::{tr, trf};
use crate::sql;
use crate::ui::quote_ident_if_needed;
use crate::ui::theme::{self, palette};
use crate::worker::ConnId;
use egui::{FontId, Margin, RichText, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropKind {
    Database,
    Table,
    View,
}

impl From<ObjectKind> for DropKind {
    fn from(k: ObjectKind) -> Self {
        match k {
            ObjectKind::Table => DropKind::Table,
            ObjectKind::View => DropKind::View,
        }
    }
}

pub struct DropDialog {
    pub conn: ConnId,
    pub driver: Driver,
    pub kind: DropKind,
    /// Database being dropped, or the one holding the table/view.
    pub db: String,
    /// Schema of the table/view (empty for databases).
    pub schema: String,
    /// Name of the table/view (for databases, same as `db`).
    pub name: String,
    /// Databases must be confirmed by typing their name.
    confirm: String,
    pub running: bool,
    pub error: Option<String>,
}

pub enum DropResult {
    Drop(Vec<String>),
    Cancel,
}

pub fn drop_sql(driver: Driver, kind: DropKind, schema: &str, name: &str) -> String {
    let q = |s: &str| quote_ident_if_needed(driver, s);
    let object = |s: &str, n: &str| if driver == Driver::Sqlite || s.is_empty() { q(n) } else { format!("{}.{}", q(s), q(n)) };
    match kind {
        DropKind::Database => format!("drop database {}", q(name)),
        DropKind::Table => format!("drop table {}", object(schema, name)),
        DropKind::View => format!("drop view {}", object(schema, name)),
    }
}

impl DropDialog {
    pub fn database(conn: ConnId, driver: Driver, db: String) -> Self {
        Self::new(conn, driver, DropKind::Database, db.clone(), String::new(), db)
    }

    pub fn object(conn: ConnId, driver: Driver, kind: DropKind, db: String, schema: String, name: String) -> Self {
        Self::new(conn, driver, kind, db, schema, name)
    }

    fn new(conn: ConnId, driver: Driver, kind: DropKind, db: String, schema: String, name: String) -> Self {
        Self { conn, driver, kind, db, schema, name, confirm: String::new(), running: false, error: None }
    }

    fn sql(&self) -> String {
        drop_sql(self.driver, self.kind, &self.schema, &self.name)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<DropResult> {
        let mut result = None;
        let p = palette(ctx.global_style().visuals.dark_mode);
        let dark = ctx.global_style().visuals.dark_mode;
        let frame = egui::Frame::popup(&ctx.global_style())
            .fill(p.popup_bg)
            .corner_radius(12)
            .inner_margin(Margin::same(20))
            .stroke(egui::Stroke::new(1.0, p.popup_border));
        let (title, question) = match self.kind {
            DropKind::Database => (tr("Drop Database"), trf("Drop database “{name}” and all its data?", &[("name", &self.name)])),
            DropKind::Table => (tr("Drop Table"), trf("Drop table “{name}” and all its data?", &[("name", &self.name)])),
            DropKind::View => (tr("Drop View"), trf("Drop view “{name}”?", &[("name", &self.name)])),
        };
        let needs_typing = self.kind == DropKind::Database;
        egui::Modal::new(egui::Id::new("drop_dialog")).frame(frame).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(title).font(theme::bold(15.0)).color(p.text));
            ui.add_space(8.0);
            ui.label(question);
            ui.label(RichText::new(tr("This action cannot be undone.")).color(p.error));
            ui.add_space(10.0);
            egui::Frame::NONE
                .fill(p.editor_bg)
                .stroke(egui::Stroke::new(1.0, p.control_border))
                .corner_radius(6)
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut job = sql::highlight(&format!("{};", self.sql()), dark, FontId::monospace(12.5));
                    job.wrap.max_width = ui.available_width();
                    ui.label(job);
                });
            if needs_typing {
                ui.add_space(10.0);
                ui.label(trf("Type “{name}” to confirm:", &[("name", &self.name)]));
                ui.add_space(4.0);
                ui.add_enabled(
                    !self.running,
                    egui::TextEdit::singleline(&mut self.confirm).margin(vec2(8.0, 5.0)).desired_width(f32::INFINITY),
                );
            }
            if let Some(e) = &self.error {
                ui.add_space(8.0);
                ui.add(egui::Label::new(RichText::new(e).monospace().color(p.error)).wrap());
            }
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if self.running {
                    ui.add(egui::Spinner::new().size(14.0).color(p.muted));
                    ui.label(RichText::new(tr("Dropping…")).color(p.muted));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let confirmed = !needs_typing || self.confirm == self.name;
                    ui.add_enabled_ui(!self.running && confirmed, |ui| {
                        if theme::primary_button(ui, tr("Drop")).clicked() {
                            result = Some(DropResult::Drop(vec![self.sql()]));
                        }
                    });
                    ui.add_enabled_ui(!self.running, |ui| {
                        if theme::secondary_button(ui, tr("Cancel")).clicked() {
                            result = Some(DropResult::Cancel);
                        }
                    });
                });
            });
        });
        if result.is_none() && !self.running && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            result = Some(DropResult::Cancel);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_sql_per_dialect() {
        assert_eq!(drop_sql(Driver::Postgres, DropKind::Database, "", "Shop"), "drop database \"Shop\"");
        assert_eq!(drop_sql(Driver::MsSql, DropKind::Table, "dbo", "Order"), "drop table dbo.[Order]");
        assert_eq!(drop_sql(Driver::Postgres, DropKind::View, "public", "v1"), "drop view public.v1");
        assert_eq!(drop_sql(Driver::Sqlite, DropKind::Table, "main", "items"), "drop table items");
    }
}
