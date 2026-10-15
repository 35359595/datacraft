//! Diálogo modal para crear/editar una conexión.

use crate::db::{ConnectionConfig, Driver};
use crate::ui::icons::{self, Ico};
use crate::ui::theme::{self, palette};
use egui::{RichText, Ui};

pub enum TestState {
    Idle,
    Testing,
    Done(Result<(), String>),
}

pub struct ConnectionDialog {
    pub cfg: ConnectionConfig,
    /// true si se edita una conexión existente.
    pub editing: bool,
    pub test: TestState,
    pub error: Option<String>,
}

pub enum DialogResult {
    Save(ConnectionConfig),
    Test(ConnectionConfig),
    Cancel,
}

impl ConnectionDialog {
    pub fn new_connection(id: u64) -> Self {
        Self { cfg: ConnectionConfig::new(id, Driver::Postgres), editing: false, test: TestState::Idle, error: None }
    }

    pub fn edit(cfg: ConnectionConfig) -> Self {
        Self { cfg, editing: true, test: TestState::Idle, error: None }
    }

    fn change_driver(&mut self, new: Driver) {
        let old = self.cfg.driver;
        if old == new {
            return;
        }
        let old_def = ConnectionConfig::new(self.cfg.id, old);
        let new_def = ConnectionConfig::new(self.cfg.id, new);
        if self.cfg.port == old_def.port {
            self.cfg.port = new_def.port;
        }
        if self.cfg.database == old_def.database {
            self.cfg.database = new_def.database;
        }
        if self.cfg.user == old_def.user {
            self.cfg.user = new_def.user;
        }
        if self.cfg.name == old_def.name || self.cfg.name == old.label() {
            self.cfg.name = new.label().to_string();
        }
        self.cfg.driver = new;
        self.test = TestState::Idle;
    }

    fn validate(&self) -> Result<(), String> {
        if self.cfg.name.trim().is_empty() {
            return Err("El nombre es obligatorio".into());
        }
        match self.cfg.driver {
            Driver::Sqlite if self.cfg.file_path.trim().is_empty() => Err("Indica el fichero de la base de datos".into()),
            Driver::Postgres | Driver::MsSql if self.cfg.host.trim().is_empty() => Err("El host es obligatorio".into()),
            _ => Ok(()),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<DialogResult> {
        let mut result = None;
        let p = palette(ctx.global_style().visuals.dark_mode);
        let frame = egui::Frame::popup(&ctx.global_style())
            .fill(p.popup_bg)
            .corner_radius(12)
            .inner_margin(egui::Margin::same(20))
            .stroke(egui::Stroke::new(1.0, p.popup_border));
        egui::Modal::new(egui::Id::new("connection_dialog")).frame(frame).show(ctx, |ui| {
            ui.set_width(500.0);
            let title = if self.editing { "Propiedades del data source" } else { "Nuevo data source" };
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                icons::paint(ui.painter(), r, Ico::Driver(self.cfg.driver), p.icon, &p, true);
                ui.label(RichText::new(title).font(theme::bold(15.0)).color(p.text));
            });
            ui.add_space(14.0);
            self.form(ui);
            ui.add_space(10.0);
            self.test_status(ui);
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(p.error));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let testing = matches!(self.test, TestState::Testing);
                let link = ui.add_enabled(
                    !testing,
                    egui::Button::new(RichText::new("Probar conexión").color(p.accent)).frame(false),
                );
                if link.clicked() {
                    match self.validate() {
                        Ok(()) => {
                            self.error = None;
                            self.test = TestState::Testing;
                            result = Some(DialogResult::Test(self.cfg.clone()));
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                if testing {
                    ui.add(egui::Spinner::new().size(14.0));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::primary_button(ui, "OK").clicked() {
                        match self.validate() {
                            Ok(()) => result = Some(DialogResult::Save(self.cfg.clone())),
                            Err(e) => self.error = Some(e),
                        }
                    }
                    if theme::secondary_button(ui, "Cancelar").clicked() {
                        result = Some(DialogResult::Cancel);
                    }
                });
            });
        });
        if result.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            result = Some(DialogResult::Cancel);
        }
        result
    }

    fn form(&mut self, ui: &mut Ui) {
        egui::Grid::new("conn_form").num_columns(2).spacing([14.0, 10.0]).min_col_width(96.0).show(ui, |ui| {
            ui.label("Nombre:");
            ui.add(egui::TextEdit::singleline(&mut self.cfg.name).margin(egui::vec2(8.0, 5.0)).desired_width(f32::INFINITY));
            ui.end_row();

            ui.label("Driver:");
            let mut driver = self.cfg.driver;
            egui::ComboBox::from_id_salt("driver_combo")
                .selected_text(driver.label())
                .width(200.0)
                .show_ui(ui, |ui| {
                    for d in Driver::ALL {
                        ui.selectable_value(&mut driver, d, d.label());
                    }
                });
            self.change_driver(driver);
            ui.end_row();

            match self.cfg.driver {
                Driver::Sqlite => {
                    ui.label("Fichero:");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.file_path).margin(egui::vec2(8.0, 5.0)).desired_width(260.0));
                        if ui.button("Examinar…").clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .set_title("Abrir base de datos SQLite")
                                .add_filter("SQLite", &["db", "sqlite", "sqlite3", "db3"])
                                .add_filter("Todos", &["*"])
                                .pick_file()
                        {
                            self.cfg.file_path = p.to_string_lossy().into_owned();
                        }
                        if ui.button("Nueva…").on_hover_text("Crear un fichero nuevo").clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .set_title("Nueva base de datos SQLite")
                                .add_filter("SQLite", &["db", "sqlite", "sqlite3"])
                                .set_file_name("nueva.db")
                                .save_file()
                        {
                            // El driver ya no crea ficheros: se crea aquí vacío (SQLite lo
                            // inicializa al abrirlo). Si ya existe se reutiliza.
                            match std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&p) {
                                Ok(_) => {
                                    self.cfg.file_path = p.to_string_lossy().into_owned();
                                    self.error = None;
                                }
                                Err(e) => self.error = Some(format!("No se pudo crear {}: {e}", p.display())),
                            }
                        }
                    });
                    ui.end_row();
                }
                Driver::Postgres | Driver::MsSql => {
                    ui.label("Host:");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.host).margin(egui::vec2(8.0, 5.0)).desired_width(250.0));
                        ui.label("Puerto:");
                        ui.add(egui::DragValue::new(&mut self.cfg.port).range(1..=65535).speed(0));
                    });
                    ui.end_row();

                    ui.label("Base de datos:");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.database).margin(egui::vec2(8.0, 5.0)).desired_width(f32::INFINITY));
                    ui.end_row();

                    ui.label("Usuario:");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.user).margin(egui::vec2(8.0, 5.0)).desired_width(f32::INFINITY));
                    ui.end_row();

                    ui.label("Contraseña:");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.password).password(true).margin(egui::vec2(8.0, 5.0)).desired_width(f32::INFINITY));
                    ui.end_row();

                    if self.cfg.driver == Driver::MsSql {
                        ui.label("");
                        ui.checkbox(&mut self.cfg.trust_cert, "Confiar en el certificado del servidor");
                        ui.end_row();
                    }
                }
            }
        });
    }

    fn test_status(&self, ui: &mut Ui) {
        let pal = palette(ui.visuals().dark_mode);
        match &self.test {
            TestState::Idle => {}
            TestState::Testing => {
                ui.label(RichText::new("Probando conexión…").color(pal.muted));
            }
            TestState::Done(Ok(())) => {
                ui.label(RichText::new("✓ Conexión correcta").color(pal.ok));
            }
            TestState::Done(Err(e)) => {
                ui.add(egui::Label::new(RichText::new(e).color(pal.error)).wrap());
            }
        }
    }
}
