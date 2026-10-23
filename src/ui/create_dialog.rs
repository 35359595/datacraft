//! Modal dialogs to create a database or a table, with a live preview of the generated SQL.

use crate::db::Driver;
use crate::i18n::{tr, trf};
use crate::sql;
use crate::ui::icons::Ico;
use crate::ui::quote_ident_if_needed;
use crate::ui::theme::{self, palette};
use crate::worker::ConnId;
use egui::{FontId, Margin, RichText, Ui, vec2};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewColumn {
    pub name: String,
    pub data_type: String,
    pub not_null: bool,
    pub primary_key: bool,
    /// SQL expression, written as is (e.g. `0`, `'n/a'`, `now()`).
    pub default: String,
}

pub enum CreateTarget {
    Database,
    /// Table in `db`, inside `schema` (ignored on SQLite).
    Table { db: String, schema: String },
}

pub struct CreateDialog {
    pub conn: ConnId,
    pub driver: Driver,
    pub target: CreateTarget,
    pub name: String,
    pub columns: Vec<NewColumn>,
    /// Statements sent to the worker; waiting for the reply.
    pub running: bool,
    pub error: Option<String>,
    /// Focus the name field on the first frame.
    focus_name: bool,
}

pub enum CreateResult {
    Create(Vec<String>),
    Cancel,
}

/// Usual types per engine, offered in each column's type menu.
fn common_types(driver: Driver) -> &'static [&'static str] {
    match driver {
        Driver::Postgres => &[
            "integer", "bigint", "smallint", "serial", "bigserial", "numeric(10,2)", "real", "double precision",
            "boolean", "varchar(255)", "text", "date", "timestamp", "timestamptz", "uuid", "jsonb", "bytea",
        ],
        Driver::MsSql => &[
            "int", "bigint", "smallint", "tinyint", "bit", "decimal(18,2)", "float", "money", "nvarchar(255)",
            "nvarchar(max)", "varchar(255)", "date", "datetime2", "uniqueidentifier", "varbinary(max)",
        ],
        Driver::Sqlite => &["INTEGER", "TEXT", "REAL", "NUMERIC", "BLOB", "BOOLEAN", "DATE", "DATETIME"],
        Driver::MySql | Driver::MariaDb => &[
            "int", "bigint", "smallint", "tinyint", "decimal(10,2)", "double", "boolean", "varchar(255)", "text",
            "longtext", "date", "datetime", "timestamp", "json", "blob",
        ],
        Driver::Oracle => &[
            "number", "number(10)", "number(10,2)", "binary_double", "varchar2(255)", "nvarchar2(255)", "char(1)",
            "clob", "date", "timestamp", "blob", "raw(16)",
        ],
        Driver::Redis | Driver::MongoDb => &[],
        Driver::Snowflake => &[
            "number(38,0)", "number(10,2)", "float", "boolean", "varchar", "varchar(255)", "date", "time",
            "timestamp_ntz", "timestamp_ltz", "timestamp_tz", "variant", "object", "array", "binary",
        ],
    }
}

/// Schema used for new tables when none is selected.
pub fn default_schema(driver: Driver) -> &'static str {
    match driver {
        Driver::Postgres => "public",
        Driver::MsSql => "dbo",
        Driver::Sqlite => "main",
        Driver::Snowflake => "PUBLIC",
        Driver::Redis | Driver::MongoDb => "",
        // MySQL: the database itself; Oracle: the connected user (filled in by the caller).
        Driver::MySql | Driver::MariaDb | Driver::Oracle => "",
    }
}

fn id_column(driver: Driver) -> NewColumn {
    let data_type = match driver {
        Driver::Postgres => "integer",
        Driver::MsSql => "int",
        Driver::Sqlite => "INTEGER",
        Driver::MySql | Driver::MariaDb => "int",
        Driver::Oracle => "number(10)",
        Driver::Snowflake => "number(38,0)",
        Driver::Redis | Driver::MongoDb => "string",
    };
    NewColumn { name: "id".into(), data_type: data_type.into(), not_null: true, primary_key: true, default: String::new() }
}

pub fn create_database_sql(driver: Driver, name: &str) -> String {
    format!("create database {}", quote_ident_if_needed(driver, name.trim()))
}

pub fn create_table_sql(driver: Driver, schema: &str, table: &str, columns: &[NewColumn]) -> String {
    let q = |s: &str| quote_ident_if_needed(driver, s.trim());
    let qualified = if driver == Driver::Sqlite || schema.trim().is_empty() {
        q(table)
    } else {
        format!("{}.{}", q(schema), q(table))
    };
    let mut lines: Vec<String> = columns
        .iter()
        .map(|c| {
            let mut l = format!("    {} {}", q(&c.name), c.data_type.trim());
            if c.not_null || c.primary_key {
                l.push_str(" not null");
            }
            if !c.default.trim().is_empty() {
                l.push_str(&format!(" default {}", c.default.trim()));
            }
            l
        })
        .collect();
    let pk: Vec<String> = columns.iter().filter(|c| c.primary_key).map(|c| q(&c.name)).collect();
    if !pk.is_empty() {
        lines.push(format!("    primary key ({})", pk.join(", ")));
    }
    format!("create table {qualified}\n(\n{}\n)", lines.join(",\n"))
}

/// Demo mode: sample columns for the "New Table" dialog.
pub fn demo_columns() -> Vec<NewColumn> {
    vec![
        NewColumn { name: "name".into(), data_type: "TEXT".into(), not_null: true, primary_key: false, default: String::new() },
        NewColumn { name: "active".into(), data_type: "BOOLEAN".into(), not_null: false, primary_key: false, default: "1".into() },
    ]
}

impl CreateDialog {
    pub fn database(conn: ConnId, driver: Driver) -> Self {
        Self { conn, driver, target: CreateTarget::Database, name: String::new(), columns: vec![], running: false, error: None, focus_name: true }
    }

    pub fn table(conn: ConnId, driver: Driver, db: String, schema: String) -> Self {
        Self {
            conn,
            driver,
            target: CreateTarget::Table { db, schema },
            name: String::new(),
            columns: vec![id_column(driver)],
            running: false,
            error: None,
            focus_name: true,
        }
    }

    pub fn sql(&self) -> String {
        match &self.target {
            CreateTarget::Database => create_database_sql(self.driver, &self.name),
            CreateTarget::Table { schema, .. } => create_table_sql(self.driver, schema, &self.name, &self.columns),
        }
    }

    fn validate(&self) -> Result<Vec<String>, String> {
        if self.name.trim().is_empty() {
            return Err(tr("Name is required").into());
        }
        if let CreateTarget::Table { .. } = self.target {
            if self.columns.is_empty() {
                return Err(tr("Add at least one column").into());
            }
            let mut seen = std::collections::HashSet::new();
            for c in &self.columns {
                let name = c.name.trim();
                if name.is_empty() {
                    return Err(tr("Every column needs a name").into());
                }
                if c.data_type.trim().is_empty() {
                    return Err(trf("Column “{name}” needs a type", &[("name", &name)]));
                }
                if !seen.insert(name.to_lowercase()) {
                    return Err(trf("Duplicate column “{name}”", &[("name", &name)]));
                }
            }
        }
        Ok(vec![self.sql()])
    }

    /// `schemas`: the database's schemas, to choose from (empty if not loaded yet).
    pub fn show(&mut self, ctx: &egui::Context, schemas: &[String]) -> Option<CreateResult> {
        let mut result = None;
        let p = palette(ctx.global_style().visuals.dark_mode);
        let frame = egui::Frame::popup(&ctx.global_style())
            .fill(p.popup_bg)
            .corner_radius(12)
            .inner_margin(Margin::same(20))
            .stroke(egui::Stroke::new(1.0, p.popup_border));
        let is_table = matches!(self.target, CreateTarget::Table { .. });
        egui::Modal::new(egui::Id::new("create_dialog")).frame(frame).show(ctx, |ui| {
            ui.set_width(if is_table { 640.0 } else { 420.0 });
            ui.horizontal(|ui| {
                let title = if is_table { tr("New Table") } else { tr("New Database") };
                ui.label(RichText::new(title).font(theme::bold(15.0)).color(p.text));
                if let CreateTarget::Table { db, .. } = &self.target {
                    ui.add_space(6.0);
                    ui.label(RichText::new(trf("in {db}", &[("db", db)])).color(p.muted));
                }
            });
            ui.add_space(14.0);
            ui.add_enabled_ui(!self.running, |ui| {
                self.form(ui, schemas);
                if is_table {
                    ui.add_space(12.0);
                    self.columns_editor(ui);
                }
            });
            ui.add_space(12.0);
            self.preview(ui);
            if let Some(e) = &self.error {
                ui.add_space(8.0);
                ui.add(egui::Label::new(RichText::new(e).monospace().color(p.error)).wrap());
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::secondary_button(ui, tr("Copy SQL")).clicked() {
                    ui.ctx().copy_text(format!("{};", self.sql()));
                }
                if self.running {
                    ui.add_space(8.0);
                    ui.add(egui::Spinner::new().size(14.0).color(p.muted));
                    ui.label(RichText::new(tr("Creating…")).color(p.muted));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.add_enabled_ui(!self.running, |ui| {
                        if theme::primary_button(ui, tr("Create")).clicked() {
                            match self.validate() {
                                Ok(statements) => result = Some(CreateResult::Create(statements)),
                                Err(e) => self.error = Some(e),
                            }
                        }
                        if theme::secondary_button(ui, tr("Cancel")).clicked() {
                            result = Some(CreateResult::Cancel);
                        }
                    });
                });
            });
        });
        if result.is_none() && !self.running && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            result = Some(CreateResult::Cancel);
        }
        result
    }

    fn form(&mut self, ui: &mut Ui, schemas: &[String]) {
        let driver = self.driver;
        egui::Grid::new("create_form").num_columns(2).spacing([14.0, 10.0]).min_col_width(80.0).show(ui, |ui| {
            ui.label(tr("Name:"));
            let name = ui.add(egui::TextEdit::singleline(&mut self.name).margin(vec2(8.0, 5.0)).desired_width(f32::INFINITY));
            if std::mem::take(&mut self.focus_name) {
                name.request_focus();
            }
            ui.end_row();
            if let CreateTarget::Table { schema, .. } = &mut self.target
                && !driver.flat_schema()
            {
                ui.label(tr("Schema:"));
                if schemas.is_empty() {
                    ui.add(egui::TextEdit::singleline(schema).margin(vec2(8.0, 5.0)).desired_width(220.0));
                } else {
                    egui::ComboBox::from_id_salt("create_schema").selected_text(schema.as_str()).width(220.0).show_ui(ui, |ui| {
                        for s in schemas {
                            ui.selectable_value(schema, s.clone(), s);
                        }
                    });
                }
                ui.end_row();
            }
        });
    }

    fn columns_editor(&mut self, ui: &mut Ui) {
        let p = theme::pal(ui);
        let types = common_types(self.driver);
        ui.label(RichText::new(tr("Columns")).font(theme::bold(13.0)));
        ui.add_space(6.0);
        let mut remove = None;
        let can_remove = self.columns.len() > 1;
        egui::ScrollArea::vertical().id_salt("create_columns").max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("create_columns_grid").num_columns(6).spacing([8.0, 6.0]).show(ui, |ui| {
                for h in [tr("Name"), tr("Type"), tr("Not null"), "PK", tr("Default"), ""] {
                    ui.label(RichText::new(h).small().color(p.muted));
                }
                ui.end_row();
                for (i, c) in self.columns.iter_mut().enumerate() {
                    ui.add_sized([160.0, 26.0], egui::TextEdit::singleline(&mut c.name).id_salt(("col_name", i)).margin(vec2(6.0, 4.0)));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.add_sized(
                            [150.0, 26.0],
                            egui::TextEdit::singleline(&mut c.data_type)
                                .id_salt(("col_type", i))
                                .font(egui::TextStyle::Monospace)
                                .margin(vec2(6.0, 4.0)),
                        );
                        let pick = theme::icon_button(ui, Ico::ChevronDown, true, tr("Choose a type"));
                        egui::Popup::menu(&pick).show(|ui| {
                            for t in types {
                                if ui.button(RichText::new(*t).monospace()).clicked() {
                                    c.data_type = (*t).to_string();
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.add_enabled(!c.primary_key, egui::Checkbox::without_text(&mut c.not_null));
                    if ui.add(egui::Checkbox::without_text(&mut c.primary_key)).changed() && c.primary_key {
                        c.not_null = true;
                    }
                    ui.add_sized(
                        [120.0, 26.0],
                        egui::TextEdit::singleline(&mut c.default)
                            .id_salt(("col_default", i))
                            .font(egui::TextStyle::Monospace)
                            .margin(vec2(6.0, 4.0)),
                    );
                    if theme::icon_button(ui, Ico::Minus, can_remove, tr("Remove column")).clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(i) = remove {
            self.columns.remove(i);
        }
        ui.add_space(4.0);
        if theme::flat_button(ui, Some(Ico::Plus), tr("Add Column"), FontId::proportional(13.0), false, true).clicked() {
            let data_type = types.iter().find(|t| t.contains("varchar") || **t == "TEXT").unwrap_or(&types[0]);
            self.columns.push(NewColumn {
                name: String::new(),
                data_type: (*data_type).to_string(),
                not_null: false,
                primary_key: false,
                default: String::new(),
            });
        }
    }

    fn preview(&self, ui: &mut Ui) {
        let p = theme::pal(ui);
        let dark = ui.visuals().dark_mode;
        let text = format!("{};", self.sql());
        egui::Frame::NONE
            .fill(p.editor_bg)
            .stroke(egui::Stroke::new(1.0, p.control_border))
            .corner_radius(6)
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                egui::ScrollArea::both().id_salt("create_preview").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
                    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _w: f32| {
                        let mut job = sql::highlight(buf.as_str(), dark, FontId::monospace(12.5));
                        job.wrap.max_width = f32::INFINITY;
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    let mut t = text.as_str();
                    ui.add(
                        egui::TextEdit::multiline(&mut t)
                            .id(egui::Id::new("create_preview_sql"))
                            .code_editor()
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY)
                            .desired_rows(2)
                            .layouter(&mut layouter),
                    );
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, not_null: bool, pk: bool, default: &str) -> NewColumn {
        NewColumn { name: name.into(), data_type: ty.into(), not_null, primary_key: pk, default: default.into() }
    }

    #[test]
    fn database_sql_quotes_when_needed() {
        assert_eq!(create_database_sql(Driver::Postgres, " shop "), "create database shop");
        assert_eq!(create_database_sql(Driver::Postgres, "Shop"), "create database \"Shop\"");
        assert_eq!(create_database_sql(Driver::MsSql, "My Shop"), "create database [My Shop]");
    }

    #[test]
    fn table_sql_per_dialect() {
        let cols = [col("id", "integer", false, true, ""), col("name", "varchar(255)", true, false, "'n/a'"), col("note", "text", false, false, "")];
        assert_eq!(
            create_table_sql(Driver::Postgres, "public", "items", &cols),
            "create table public.items\n(\n    id integer not null,\n    name varchar(255) not null default 'n/a',\n    note text,\n    primary key (id)\n)"
        );
        assert!(create_table_sql(Driver::MsSql, "dbo", "Order", &cols).starts_with("create table dbo.[Order]\n"));
        assert!(create_table_sql(Driver::Sqlite, "main", "items", &cols).starts_with("create table items\n"));
    }

    #[test]
    fn composite_primary_key() {
        let cols = [col("a", "int", false, true, ""), col("b", "int", false, true, "")];
        assert!(create_table_sql(Driver::MsSql, "dbo", "t", &cols).ends_with("    primary key (a, b)\n)"));
    }

    #[test]
    fn validation() {
        let mut d = CreateDialog::table(1, Driver::Postgres, "db".into(), "public".into());
        assert!(d.validate().is_err(), "name required");
        d.name = "t".into();
        assert_eq!(d.validate().unwrap().len(), 1);
        d.columns.push(col("ID", "int", false, false, ""));
        assert!(d.validate().is_err(), "duplicate column");
        d.columns[1].name = "x".into();
        d.columns[1].data_type = " ".into();
        assert!(d.validate().is_err(), "missing type");
    }
}
