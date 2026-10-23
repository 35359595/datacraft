//! Database object list/grid and the selection's information pane.

use crate::app::{ConnState, ConnStatus};
use crate::db::{ObjectKind, TableInfo, object_info::ObjectInfo};
use crate::i18n::tr;
use crate::ui::explorer::{ExplorerAction, TableRef};
use crate::ui::icons::{self, Ico};
use crate::ui::object_organization::{GroupFilter, Organization};
use crate::ui::theme::{self, pal};
use egui::{FontId, Key, Rect, RichText, Sense, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectScope {
    pub conn: u64,
    pub db: String,
    pub schema: Option<String>,
    pub kind: Option<ObjectKind>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ObjectLayout {
    #[default]
    List,
    Grid,
}

#[derive(Default)]
pub struct ObjectBrowser {
    pub schema_search: crate::ui::schema_search::SchemaSearch,
    pub organizations: Vec<Organization>,
    group_filter: GroupFilter,
    pub active: bool,
    pub scope: Option<ObjectScope>,
    pub selected: Option<TableRef>,
    pub layout: ObjectLayout,
    pub search: String,
    pub info: Option<Result<ObjectInfo, String>>,
    pub loading: bool,
    pub request_id: u64,
    scroll_selection: bool,
    pub catalog: BTreeMap<(String, String), ObjectInfo>,
    pub catalog_loading: bool,
    pub catalog_request_id: u64,
    pub catalog_error: Option<String>,
}

pub enum BrowserAction {
    Search(Box<crate::worker::Request>),
    Explorer(ExplorerAction),
    Inspect { table: TableRef, count_rows: bool },
}

struct Entry<'a> {
    reference: TableRef,
    info: &'a TableInfo,
}

impl ObjectBrowser {
    pub fn open(&mut self, scope: ObjectScope) -> bool {
        let changed = self.scope.as_ref() != Some(&scope);
        if changed {
            self.group_filter = GroupFilter::All;
            self.selected = None;
            self.info = None;
            self.loading = false;
            self.search.clear();
            self.request_id = self.request_id.wrapping_add(1);
            self.catalog.clear();
            self.catalog_error = None;
            self.catalog_loading = false;
            self.catalog_request_id = self.catalog_request_id.wrapping_add(1);
        }
        self.scope = Some(scope);
        self.active = true;
        changed
    }

    pub fn start_catalog_request(&mut self) -> u64 {
        self.catalog_request_id = self.catalog_request_id.wrapping_add(1);
        self.catalog_loading = true;
        self.catalog_error = None;
        self.catalog_request_id
    }

    pub fn receive_catalog(
        &mut self,
        conn: u64,
        id: u64,
        result: Result<Vec<crate::db::object_info::ObjectInfoRow>, String>,
    ) {
        if self.catalog_request_id != id
            || !self.catalog_loading
            || !self.scope.as_ref().is_some_and(|s| s.conn == conn)
        {
            return;
        }
        self.catalog_loading = false;
        match result {
            Ok(rows) => {
                self.catalog = rows
                    .into_iter()
                    .map(|row| ((row.schema, row.name), row.info))
                    .collect()
            }
            Err(error) => self.catalog_error = Some(error),
        }
    }

    pub fn start_request(&mut self, table: TableRef) -> u64 {
        self.scroll_selection |= self.selected.as_ref() != Some(&table);
        self.selected = Some(table);
        self.info = None;
        self.loading = true;
        self.request_id = self.request_id.wrapping_add(1);
        self.request_id
    }

    pub fn receive(&mut self, conn: u64, request_id: u64, info: Result<ObjectInfo, String>) {
        if self.loading
            && self.request_id == request_id
            && self.selected.as_ref().is_some_and(|t| t.conn == conn)
        {
            self.loading = false;
            if let (Some(table), Ok(info)) = (&self.selected, &info) {
                self.catalog
                    .insert((table.schema.clone(), table.table.clone()), info.clone());
            }
            self.info = Some(info);
        }
    }

    fn entries<'a>(&self, connections: &'a [ConnState]) -> Vec<Entry<'a>> {
        let Some(scope) = &self.scope else {
            return Vec::new();
        };
        let schemas = connections
            .iter()
            .find(|c| c.cfg.id == scope.conn)
            .and_then(|c| c.databases.iter().find(|d| d.name == scope.db))
            .and_then(|d| d.schemas.as_ref());
        let search = self.search.to_lowercase();
        let mut entries: Vec<_> = schemas
            .into_iter()
            .flatten()
            .filter(|s| scope.schema.as_ref().is_none_or(|name| *name == s.name))
            .flat_map(|s| {
                s.tables
                    .iter()
                    .filter(|t| scope.kind.is_none_or(|kind| kind == t.kind))
                    .filter(|t| {
                        self.organizations
                            .iter()
                            .find(|o| o.conn == scope.conn && o.db == scope.db)
                            .is_none_or(|o| o.matches(&s.name, &t.name, &self.group_filter))
                    })
                    .filter(|t| {
                        search.is_empty()
                            || t.name.to_lowercase().contains(&search)
                            || s.name.to_lowercase().contains(&search)
                    })
                    .map(|t| Entry {
                        reference: TableRef {
                            conn: scope.conn,
                            db: scope.db.clone(),
                            schema: s.name.clone(),
                            table: t.name.clone(),
                        },
                        info: t,
                    })
            })
            .collect();
        entries.sort_by(|a, b| {
            a.reference
                .table
                .to_lowercase()
                .cmp(&b.reference.table.to_lowercase())
                .then_with(|| a.reference.schema.cmp(&b.reference.schema))
                .then_with(|| a.reference.table.cmp(&b.reference.table))
        });
        entries
    }

    pub fn show(&mut self, ui: &mut Ui, connections: &[ConnState]) -> Vec<BrowserAction> {
        ui.set_min_size(ui.available_size());
        let mut actions = self.schema_search.show(ui.ctx(), connections);
        let Some(scope) = self.scope.clone() else {
            return actions;
        };
        let p = pal(ui);
        let connected = connections
            .iter()
            .find(|c| c.cfg.id == scope.conn)
            .is_some_and(|c| c.status == ConnStatus::Connected);
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F)) {
            ui.memory_mut(|m| m.request_focus(egui::Id::new("object_search")));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let title = match scope.kind {
                Some(ObjectKind::Table) => tr("Tables"),
                Some(ObjectKind::View) => tr("Views"),
                None => tr("Objects"),
            };
            ui.label(RichText::new(title).font(theme::bold(16.0)));
            ui.label(
                RichText::new(match &scope.schema {
                    Some(s) => format!("{} / {s}", scope.db),
                    None => scope.db.clone(),
                })
                .color(p.muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                if theme::icon_button(ui, Ico::Refresh, connected, tr("Refresh objects")).clicked()
                {
                    actions.push(BrowserAction::Explorer(ExplorerAction::LoadDatabase(
                        scope.conn,
                        scope.db.clone(),
                    )));
                }
                ui.selectable_value(&mut self.layout, ObjectLayout::Grid, tr("Grid"));
                ui.selectable_value(&mut self.layout, ObjectLayout::List, tr("List"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .id(egui::Id::new("object_search"))
                        .desired_width(180.0)
                        .hint_text(tr("Search objects…")),
                );
            });
        });
        ui.add_space(8.0);
        ui.separator();
        if self.catalog_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Loading object statistics…"));
            });
        }
        if let Some(error) = &self.catalog_error {
            ui.colored_label(p.error, error);
        }
        if !connected {
            ui.label(
                RichText::new(tr("Connect to load current object information")).color(p.muted),
            );
        }
        let org_index = self
            .organizations
            .iter()
            .position(|o| o.conn == scope.conn && o.db == scope.db)
            .unwrap_or_else(|| {
                let mut organization = Organization::default();
                organization.conn = scope.conn;
                organization.db.clone_from(&scope.db);
                self.organizations.push(organization);
                self.organizations.len() - 1
            });
        egui::Panel::left("object_folders")
            .default_size(155.0)
            .size_range(130.0..=260.0)
            .resizable(true)
            .show(ui, |ui| {
                self.organizations[org_index].folders(ui, &mut self.group_filter);
            });
        let entries = self.entries(connections);
        if self
            .selected
            .as_ref()
            .is_some_and(|t| !entries.iter().any(|e| &e.reference == t))
        {
            self.selected = None;
            self.info = None;
            self.loading = false;
            self.request_id = self.request_id.wrapping_add(1);
        }
        let scroll_to = if std::mem::take(&mut self.scroll_selection) {
            self.selected
                .as_ref()
                .and_then(|t| entries.iter().position(|e| &e.reference == t))
        } else {
            None
        };
        egui::Panel::right("object_information")
            .default_size(300.0)
            .size_range(220.0..=520.0)
            .resizable(true)
            .show(ui, |ui| {
                self.information(ui, &entries, connected, &mut actions)
            });
        ui.set_clip_rect(ui.clip_rect().intersect(ui.available_rect_before_wrap()));
        let grid_rows = ((ui.available_height() - 14.0) / (22.0 + ui.spacing().item_spacing.y))
            .floor()
            .max(1.0) as usize;
        let background = ui.available_rect_before_wrap();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let mut over_object = false;

        if entries.is_empty() {
            let loading = connections
                .iter()
                .find(|c| c.cfg.id == scope.conn)
                .is_some_and(|c| c.loading.contains(&scope.db));
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                if loading {
                    ui.spinner();
                    ui.label(tr("Loading…"));
                } else {
                    ui.label(RichText::new(tr("No matching objects")).color(p.muted));
                }
            });
            self.background_menu(ui, background, &scope, &mut actions);
            return actions;
        }
        if ui.is_enabled()
            && !egui::Popup::is_any_open(ui.ctx())
            && ui.memory(|m| m.focused().is_none())
        {
            let current = self
                .selected
                .as_ref()
                .and_then(|t| entries.iter().position(|e| &e.reference == t));
            let navigation = ui.input_mut(|i| {
                [
                    Key::ArrowDown,
                    Key::ArrowUp,
                    Key::ArrowLeft,
                    Key::ArrowRight,
                    Key::Home,
                    Key::End,
                    Key::Enter,
                ]
                .into_iter()
                .find(|key| i.consume_key(egui::Modifiers::NONE, *key))
            });
            if let Some(key) = navigation {
                let next = object_navigation(
                    current,
                    key,
                    entries.len(),
                    if self.layout == ObjectLayout::Grid {
                        grid_rows
                    } else {
                        entries.len()
                    },
                );
                let table = entries[next].reference.clone();
                if key == Key::Enter {
                    actions.push(BrowserAction::Explorer(ExplorerAction::OpenTable(table)));
                } else {
                    actions.push(BrowserAction::Inspect {
                        table,
                        count_rows: false,
                    });
                }
            }
        }
        let organization = &mut self.organizations[org_index];
        match self.layout {
            ObjectLayout::List => {
                egui::ScrollArea::horizontal()
                    .id_salt("object_details_horizontal")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(1160.0);
                        ui.style_mut().override_font_id = Some(FontId::proportional(12.0));
                        let mut table = TableBuilder::new(ui)
                            .id_salt("object_list")
                            .resizable(true)
                            .sense(Sense::click());
                        if let Some(row) = scroll_to {
                            table = table.scroll_to_row(row, None);
                        }
                        table
                            .column(Column::initial(220.0).at_least(100.0).clip(true))
                            .column(Column::initial(75.0).clip(true))
                            .column(Column::initial(95.0).clip(true))
                            .column(Column::initial(90.0).clip(true))
                            .column(Column::initial(155.0).clip(true))
                            .column(Column::initial(155.0).clip(true))
                            .column(Column::initial(170.0).clip(true))
                            .column(Column::remainder().at_least(140.0).clip(true))
                            .header(23.0, |mut header| {
                                for label in [
                                    tr("Name"),
                                    tr("Rows"),
                                    tr("Data length"),
                                    tr("Engine"),
                                    tr("Created date"),
                                    tr("Modified date"),
                                    tr("Collation"),
                                    tr("Comment"),
                                ] {
                                    header.col(|ui| {
                                        ui.label(RichText::new(label).strong().color(p.text));
                                    });
                                }
                            })
                            .body(|body| {
                                body.rows(21.0, entries.len(), |mut row| {
                                    let entry = &entries[row.index()];
                                    let info = self.catalog.get(&(
                                        entry.reference.schema.clone(),
                                        entry.reference.table.clone(),
                                    ));
                                    let selected = self.selected.as_ref() == Some(&entry.reference);
                                    row.set_selected(selected);
                                    row.col(|ui| {
                                        ui.horizontal(|ui| {
                                            let (rect, _) = ui.allocate_exact_size(
                                                vec2(16.0, 16.0),
                                                Sense::hover(),
                                            );
                                            icons::paint(
                                                ui.painter(),
                                                rect,
                                                object_icon(entry.info.kind),
                                                organization
                                                    .color(
                                                        &entry.reference.schema,
                                                        &entry.reference.table,
                                                    )
                                                    .unwrap_or(p.icon),
                                                &p,
                                                true,
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&entry.reference.table).color(
                                                        if selected {
                                                            p.text
                                                        } else {
                                                            organization
                                                                .color(
                                                                    &entry.reference.schema,
                                                                    &entry.reference.table,
                                                                )
                                                                .unwrap_or(p.text)
                                                        },
                                                    ),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    });
                                    let rows = info.and_then(|i| i.rows).map(|n| {
                                        if info.is_some_and(|i| i.rows_exact) {
                                            n.to_string()
                                        } else {
                                            format!("≈ {n}")
                                        }
                                    });
                                    let size = info.and_then(|i| i.data_bytes).map(format_bytes);
                                    for value in [
                                        rows.as_deref(),
                                        size.as_deref(),
                                        info.and_then(|i| i.engine.as_deref()),
                                        info.and_then(|i| i.created.as_deref()),
                                        info.and_then(|i| i.modified.as_deref()),
                                        info.and_then(|i| i.collation.as_deref()),
                                        info.and_then(|i| i.comment.as_deref()),
                                    ] {
                                        row.col(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(value.unwrap_or("—"))
                                                        .color(p.text),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    }
                                    let response = row.response();
                                    over_object |=
                                        pointer.is_some_and(|p| response.rect.contains(p));
                                    object_response(response, entry, organization, &mut actions);
                                })
                            });
                    });
            }
            ObjectLayout::Grid => {
                let pitch = 22.0 + ui.spacing().item_spacing.y;
                let width = 240.0;
                let columns = entries.len().div_ceil(grid_rows);
                let height = (ui.available_height() - 14.0).max(22.0);
                let mut scroll = egui::ScrollArea::horizontal()
                    .id_salt("object_grid")
                    .auto_shrink([false, false]);
                if let Some(index) = scroll_to {
                    scroll = scroll.horizontal_scroll_offset((index / grid_rows) as f32 * width);
                }
                scroll.show_viewport(ui, |ui, viewport| {
                    ui.set_min_size(vec2(columns as f32 * width, height));
                    let origin = ui.min_rect().min;
                    let first = (viewport.min.x / width).floor().max(0.0) as usize;
                    let last = ((viewport.max.x / width).ceil() as usize + 1).min(columns);
                    for column in first..last {
                        for row in 0..grid_rows {
                            let index = column * grid_rows + row;
                            let Some(entry) = entries.get(index) else {
                                break;
                            };
                            let rect = Rect::from_min_size(
                                origin + vec2(column as f32 * width, row as f32 * pitch),
                                vec2(width - 6.0, 22.0),
                            );
                            let response = ui.interact(
                                rect,
                                ui.id().with(("object_cell", index)),
                                Sense::click(),
                            );
                            over_object |= pointer.is_some_and(|p| rect.contains(p));
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::SelectableLabel,
                                    ui.is_enabled(),
                                    self.selected.as_ref() == Some(&entry.reference),
                                    &entry.reference.table,
                                )
                            });
                            let selected = self.selected.as_ref() == Some(&entry.reference);
                            if selected || response.hovered() {
                                ui.painter().rect_filled(
                                    rect.shrink(1.0),
                                    3.0,
                                    if selected { p.selection } else { p.row_hover },
                                );
                            }
                            let icon = Rect::from_min_size(
                                pos2(rect.left() + 5.0, rect.center().y - 8.0),
                                vec2(16.0, 16.0),
                            );
                            icons::paint(
                                ui.painter(),
                                icon,
                                object_icon(entry.info.kind),
                                organization
                                    .color(&entry.reference.schema, &entry.reference.table)
                                    .unwrap_or(p.icon),
                                &p,
                                true,
                            );
                            ui.painter_at(rect.shrink(3.0)).text(
                                pos2(rect.left() + 27.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &entry.reference.table,
                                FontId::proportional(12.0),
                                if selected {
                                    p.text
                                } else {
                                    organization
                                        .color(&entry.reference.schema, &entry.reference.table)
                                        .unwrap_or(p.text)
                                },
                            );
                            object_response(response, entry, organization, &mut actions);
                        }
                    }
                });
            }
        }
        if !over_object {
            self.background_menu(ui, background, &scope, &mut actions);
        }
        actions
    }

    fn background_menu(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        scope: &ObjectScope,
        actions: &mut Vec<BrowserAction>,
    ) {
        let response = ui.interact(rect, ui.id().with("object_background"), Sense::click());
        response.context_menu(|ui| {
            if ui.button(tr("Find in Schema…")).clicked() {
                self.schema_search.open(scope);
                ui.close();
            }
            if ui.button(tr("New group")).clicked() {
                ui.memory_mut(|m| m.request_focus(egui::Id::new("new_object_group")));
                ui.close();
            }
            if ui.button(tr("Refresh objects")).clicked() {
                actions.push(BrowserAction::Explorer(ExplorerAction::LoadDatabase(
                    scope.conn,
                    scope.db.clone(),
                )));
                ui.close();
            }
        });
    }

    fn information(
        &self,
        ui: &mut Ui,
        entries: &[Entry<'_>],
        connected: bool,
        actions: &mut Vec<BrowserAction>,
    ) {
        let p = pal(ui);
        ui.add_space(8.0);
        ui.heading(tr("Information"));
        ui.separator();
        let Some(selected) = &self.selected else {
            ui.label(RichText::new(tr("Select an object to see its information")).color(p.muted));
            return;
        };
        let table = entries
            .iter()
            .find(|e| &e.reference == selected)
            .map(|e| e.info);
        egui::ScrollArea::vertical().id_salt("object_information_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(&selected.table).strong().size(17.0)).wrap());
            ui.label(RichText::new(format!("{} / {}", selected.db, selected.schema)).color(p.muted));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(connected, egui::Button::new(tr("Open data"))).clicked() {
                    actions.push(BrowserAction::Explorer(ExplorerAction::OpenTable(selected.clone())));
                }
                if ui.add_enabled(connected, egui::Button::new("DDL")).clicked() {
                    actions.push(BrowserAction::Explorer(ExplorerAction::GenerateDdl(selected.clone())));
                }
                if theme::icon_button(ui, Ico::Refresh, connected && !self.loading, tr("Refresh information")).clicked() {
                    actions.push(BrowserAction::Inspect { table: selected.clone(), count_rows: false });
                }
            });
            if self.loading { ui.horizontal(|ui| { ui.spinner(); ui.label(tr("Loading…")); }); }
            if let Some(Err(error)) = &self.info { ui.colored_label(p.error, error); }
            if ui.add_enabled(connected && !self.loading, egui::Button::new(tr("Count rows exactly")))
                .on_hover_text(tr("Runs COUNT(*). Large tables and views may take time.")).clicked()
            {
                actions.push(BrowserAction::Inspect { table: selected.clone(), count_rows: true });
            }
            let info = self.info.as_ref().and_then(|result| result.as_ref().ok());
            ui.add_space(8.0);
            let rows = info.and_then(|i| i.rows).map(|n| if info.is_some_and(|i| i.rows_exact) { n.to_string() } else { format!("≈ {n}") });
            let columns = table.map(|t| t.columns.len().to_string());
            let data = info.and_then(|i| i.data_bytes).map(format_bytes);
            let indexes = info.and_then(|i| i.index_bytes).map(format_bytes);
            let average = info.and_then(|i| i.average_row_bytes).map(format_bytes);
            let max_data = info.and_then(|i| i.max_data_bytes).map(format_bytes);
            let free = info.and_then(|i| i.free_bytes).map(format_bytes);
            let auto_increment = info.and_then(|i| i.auto_increment).map(|n| n.to_string());
            for (label, value) in [
                (tr("Type"), table.map(|t| object_kind(t.kind))),
                (tr("Columns"), columns.as_deref()),
                (tr("Rows"), rows.as_deref()),
                (tr("Data length"), data.as_deref()),
                (tr("Engine"), info.and_then(|i| i.engine.as_deref())),
                (tr("Created date"), info.and_then(|i| i.created.as_deref())),
                (tr("Modified date"), info.and_then(|i| i.modified.as_deref())),
                (tr("Collation"), info.and_then(|i| i.collation.as_deref())),
                (tr("Row format"), info.and_then(|i| i.row_format.as_deref())),
                (tr("Average row length"), average.as_deref()),
                (tr("Max data length"), max_data.as_deref()),
                (tr("Index length"), indexes.as_deref()),
                (tr("Check time"), info.and_then(|i| i.checked.as_deref())),
                (tr("Auto increment"), auto_increment.as_deref()),
                (tr("Data free"), free.as_deref()),
                (tr("Create options"), info.and_then(|i| i.create_options.as_deref())),
            ] {
                ui.label(RichText::new(label).small().color(p.muted));
                ui.add(egui::Label::new(value.unwrap_or("—")).wrap()).on_hover_text(if value.is_none() { tr("Not available") } else { "" });
                ui.add_space(5.0);
            }
            ui.label(RichText::new(tr("Row estimates and timestamps come from the engine; modified may refer to a definition change.")).small().color(p.muted));
            if let Some(comment) = info.and_then(|i| i.comment.as_deref()) {
                ui.separator(); ui.label(RichText::new(tr("Comment")).strong()); ui.add(egui::Label::new(comment).wrap());
            }
            if let Some(table) = table {
                ui.separator();
                egui::CollapsingHeader::new(tr("Columns")).default_open(true).show(ui, |ui| {
                    for column in &table.columns {
                        ui.horizontal_wrapped(|ui| {
                            if column.primary_key { ui.label(RichText::new("PK").small().color(p.accent)); }
                            ui.strong(&column.name);
                            ui.label(RichText::new(&column.data_type).small().color(p.muted));
                            if !column.nullable { ui.label(RichText::new("NOT NULL").small().color(p.muted)); }
                        });
                    }
                });
            }
        });
    }
}

fn object_navigation(
    current: Option<usize>,
    key: Key,
    total: usize,
    rows_per_column: usize,
) -> usize {
    let i = current.unwrap_or(0);
    match key {
        Key::ArrowDown if i % rows_per_column + 1 < rows_per_column => {
            (i + usize::from(current.is_some())).min(total - 1)
        }
        Key::ArrowUp if !i.is_multiple_of(rows_per_column) => i - 1,
        Key::ArrowLeft if i >= rows_per_column => i - rows_per_column,
        Key::ArrowRight if i + rows_per_column < total => i + rows_per_column,
        Key::Home => 0,
        Key::End => total - 1,
        _ => i,
    }
}

fn object_icon(kind: ObjectKind) -> Ico {
    if kind == ObjectKind::Table {
        Ico::Table
    } else {
        Ico::View
    }
}
fn object_kind(kind: ObjectKind) -> &'static str {
    if kind == ObjectKind::Table {
        tr("Table")
    } else {
        tr("View")
    }
}
fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn object_response(
    response: egui::Response,
    entry: &Entry<'_>,
    organization: &mut Organization,
    actions: &mut Vec<BrowserAction>,
) {
    let response = response.on_hover_text(format!(
        "{}.{}",
        entry.reference.schema, entry.reference.table
    ));
    if response.clicked() {
        actions.push(BrowserAction::Inspect {
            table: entry.reference.clone(),
            count_rows: false,
        });
    }
    if response.double_clicked() {
        actions.push(BrowserAction::Explorer(ExplorerAction::OpenTable(
            entry.reference.clone(),
        )));
    }
    response.context_menu(|ui| {
        organization.menu(ui, &entry.reference.schema, &entry.reference.table);
        ui.separator();
        if ui.button(tr("Open data")).clicked() {
            actions.push(BrowserAction::Explorer(ExplorerAction::OpenTable(
                entry.reference.clone(),
            )));
            ui.close();
        }
        if ui.button(tr("Copy Name")).clicked() {
            ui.ctx().copy_text(entry.reference.table.clone());
            ui.close();
        }
        if ui.button(tr("Show DDL")).clicked() {
            actions.push(BrowserAction::Explorer(ExplorerAction::GenerateDdl(
                entry.reference.clone(),
            )));
            ui.close();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ColumnInfo, ConnectionConfig, DatabaseInfo, Driver, SchemaInfo};
    use egui_kittest::kittest::Queryable;

    fn connection() -> ConnState {
        let mut connection = ConnState::new(ConnectionConfig::new(7, Driver::Postgres));
        connection.status = ConnStatus::Connected;
        let columns = vec![
            ColumnInfo {
                name: "id".into(),
                data_type: "integer".into(),
                nullable: false,
                primary_key: true,
            },
            ColumnInfo {
                name: "name".into(),
                data_type: "text".into(),
                nullable: true,
                primary_key: false,
            },
        ];
        connection.databases.push(DatabaseInfo {
            name: "shop".into(),
            current: true,
            schemas: Some(vec![
                SchemaInfo {
                    name: "public".into(),
                    tables: vec![
                        TableInfo {
                            name: "customers".into(),
                            kind: ObjectKind::Table,
                            columns: columns.clone(),
                        },
                        TableInfo {
                            name: "orders".into(),
                            kind: ObjectKind::Table,
                            columns: columns.clone(),
                        },
                        TableInfo {
                            name: "active_customers".into(),
                            kind: ObjectKind::View,
                            columns: columns.clone(),
                        },
                    ],
                },
                SchemaInfo {
                    name: "audit".into(),
                    tables: vec![TableInfo {
                        name: "events".into(),
                        kind: ObjectKind::Table,
                        columns,
                    }],
                },
            ]),
        });
        connection
    }

    fn scope(kind: Option<ObjectKind>) -> ObjectScope {
        ObjectScope {
            conn: 7,
            db: "shop".into(),
            schema: Some("public".into()),
            kind,
        }
    }

    #[test]
    fn scope_filters_category_schema_and_search_without_changing_identity() {
        let connections = vec![connection()];
        let mut browser = ObjectBrowser::default();
        browser.open(scope(Some(ObjectKind::Table)));
        let entries = browser.entries(&connections);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.reference.table.as_str())
                .collect::<Vec<_>>(),
            vec!["customers", "orders"]
        );
        assert!(
            entries
                .iter()
                .all(|e| e.reference.conn == 7 && e.reference.schema == "public")
        );
        browser.search = "ORDER".into();
        assert_eq!(browser.entries(&connections)[0].reference.table, "orders");
        browser.open(scope(Some(ObjectKind::View)));
        assert_eq!(
            browser.entries(&connections)[0].reference.table,
            "active_customers"
        );
        let mut all = scope(None);
        all.schema = None;
        browser.open(all);
        assert_eq!(browser.entries(&connections).len(), 4);
    }

    #[test]
    fn late_metadata_cannot_replace_a_new_selection_or_a_different_scope() {
        let mut browser = ObjectBrowser::default();
        browser.open(scope(None));
        let first = TableRef {
            conn: 7,
            db: "shop".into(),
            schema: "public".into(),
            table: "customers".into(),
        };
        let mut second = first.clone();
        second.table = "orders".into();
        let old = browser.start_request(first);
        let current = browser.start_request(second);
        browser.receive(7, old, Ok(ObjectInfo::default()));
        assert!(browser.info.is_none() && browser.loading);
        browser.receive(9, current, Ok(ObjectInfo::default()));
        assert!(browser.info.is_none());
        browser.receive(
            7,
            current,
            Ok(ObjectInfo {
                rows: Some(42),
                ..Default::default()
            }),
        );
        assert_eq!(
            browser.info.as_ref().unwrap().as_ref().unwrap().rows,
            Some(42)
        );
        browser.open(scope(Some(ObjectKind::View)));
        browser.receive(7, current, Ok(ObjectInfo::default()));
        assert!(browser.info.is_none() && browser.selected.is_none());
        let request = browser.start_catalog_request();
        browser.open(scope(Some(ObjectKind::Table)));
        browser.receive_catalog(7, request, Ok(Vec::new()));
        assert!(browser.catalog.is_empty());
        let current = browser.start_catalog_request();
        browser.receive_catalog(
            7,
            current,
            Ok(vec![crate::db::object_info::ObjectInfoRow {
                schema: "public".into(),
                name: "orders".into(),
                info: ObjectInfo {
                    rows: Some(7),
                    ..Default::default()
                },
            }]),
        );
        assert_eq!(
            browser
                .catalog
                .get(&("public".into(), "orders".into()))
                .unwrap()
                .rows,
            Some(7)
        );
    }

    struct State {
        browser: ObjectBrowser,
        connections: Vec<ConnState>,
        opened: Option<TableRef>,
    }

    fn harness(layout: ObjectLayout, dark: bool) -> egui_kittest::Harness<'static, State> {
        let mut browser = ObjectBrowser::default();
        browser.open(scope(Some(ObjectKind::Table)));
        browser.layout = layout;
        let mut first = true;
        let mut h = egui_kittest::Harness::builder()
            .with_size(vec2(1180.0, 720.0))
            .build_ui_state(
                move |ui, state: &mut State| {
                    if std::mem::take(&mut first) {
                        theme::install_fonts(ui.ctx());
                        theme::apply(ui.ctx(), dark);
                        return;
                    }
                    for action in state.browser.show(ui, &state.connections) {
                        match action {
                            BrowserAction::Inspect { table, count_rows } => {
                                let request = state.browser.start_request(table);
                                state.browser.receive(
                                    7,
                                    request,
                                    Ok(ObjectInfo {
                                        rows: Some(42),
                                        rows_exact: count_rows,
                                        data_bytes: Some(16384),
                                        index_bytes: Some(8192),
                                        created: None,
                                        modified: None,
                                        engine: None,
                                        comment: Some("Customer records".into()),
                                        ..Default::default()
                                    }),
                                );
                            }
                            BrowserAction::Explorer(ExplorerAction::OpenTable(table)) => {
                                state.opened = Some(table)
                            }
                            _ => {}
                        }
                    }
                },
                State {
                    browser,
                    connections: vec![connection()],
                    opened: None,
                },
            );
        h.run_steps(3);
        h
    }

    #[test]
    fn list_and_grid_selection_show_metadata_and_open_the_selected_object() {
        for layout in [ObjectLayout::List, ObjectLayout::Grid] {
            let mut h = harness(layout, false);
            h.get_by_label("customers").click();
            h.run_steps(3);
            assert_eq!(
                h.state().browser.selected.as_ref().unwrap().table,
                "customers"
            );
            assert!(h.query_all_by_label("≈ 42").next().is_some());
            h.get_by_label("Count rows exactly").click();
            h.run_steps(3);
            assert!(
                h.state()
                    .browser
                    .info
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .rows_exact
            );
            h.get_by_label("Open data").click();
            h.run_steps(2);
            assert_eq!(h.state().opened.as_ref().unwrap().table, "customers");
        }
    }

    #[test]
    fn compact_grid_shows_only_object_names_while_list_has_statistics_columns() {
        let grid = harness(ObjectLayout::Grid, false);
        assert!(grid.query_by_label("customers").is_some());
        assert!(grid.query_by_label("Rows").is_none());
        assert!(grid.query_all_by_label("public").next().is_none());
        let list = harness(ObjectLayout::List, false);
        assert!(list.query_by_label("Rows").is_some());
        assert!(list.query_by_label("Data length").is_some());
        assert!(list.query_by_label("Engine").is_some());
    }

    #[test]
    fn empty_space_menu_opens_schema_search_and_object_menu_keeps_organization_actions() {
        let mut h = harness(ObjectLayout::Grid, true);
        let position = pos2(500.0, 500.0);
        h.event(egui::Event::PointerMoved(position));
        h.run_steps(2);
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(2);
        }
        h.get_by_label("Find in Schema…").click();
        h.run_steps(3);
        assert!(h.state().browser.schema_search.open);
        h.get_by_label("Close").click();
        h.run_steps(3);
        h.get_by_label("customers").click_secondary();
        h.run_steps(3);
        assert!(h.query_by_label("Move to group ⏵").is_some());
        assert!(h.query_by_label("Color ⏵").is_some());
        assert!(h.query_by_label("Open data").is_some());
    }

    #[test]
    fn folders_filter_objects_and_context_menu_assigns_group_and_color() {
        let mut h = harness(ObjectLayout::Grid, true);
        h.ctx
            .memory_mut(|m| m.request_focus(egui::Id::new("new_object_group")));
        h.input_mut().events.push(egui::Event::Text("Sales".into()));
        h.run_steps(3);
        h.get_by_label("New group").click();
        h.run_steps(3);
        assert_eq!(h.state().browser.organizations[0].groups, vec!["Sales"]);
        assert!(h.query_by_label("customers").is_none());
        h.get_by_label("All objects").click();
        h.run_steps(3);
        h.get_by_label("customers").click_secondary();
        h.run_steps(3);
        h.get_by_label("Move to group ⏵").click();
        h.run_steps(3);
        h.get_all_by_label("Sales").last().unwrap().click();
        h.run_steps(3);
        h.get_by_label("Sales").click();
        h.run_steps(3);
        assert!(h.query_by_label("customers").is_some());
        assert!(h.query_by_label("orders").is_none());
        h.get_by_label("customers").click_secondary();
        h.run_steps(3);
        h.get_by_label("Color ⏵").click();
        h.run_steps(3);
        h.get_by_label("Blue").click();
        h.run_steps(3);
        assert!(
            h.state().browser.organizations[0]
                .color("public", "customers")
                .is_some()
        );
    }

    #[test]
    fn grid_navigation_fills_down_then_moves_to_the_next_column() {
        assert_eq!(object_navigation(Some(0), Key::ArrowDown, 70, 30), 1);
        assert_eq!(object_navigation(Some(29), Key::ArrowDown, 70, 30), 29);
        assert_eq!(object_navigation(Some(0), Key::ArrowRight, 70, 30), 30);
        assert_eq!(object_navigation(Some(30), Key::ArrowLeft, 70, 30), 0);
        assert_eq!(object_navigation(Some(30), Key::ArrowUp, 70, 30), 30);
        assert_eq!(object_navigation(Some(69), Key::End, 70, 30), 69);
    }

    #[test]
    #[ignore = "Requires a GPU; writes object browser previews to target/object-browser-snapshots"]
    fn render_object_browser() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/object-browser-snapshots");
        std::fs::create_dir_all(&directory).unwrap();
        for (layout, name) in [(ObjectLayout::List, "list"), (ObjectLayout::Grid, "grid")] {
            let mut h = harness(layout, true);
            h.set_size(vec2(1800.0, 1000.0));
            {
                let state = h.state_mut();
                state.browser.organizations[0] = serde_json::from_value(serde_json::json!({
                    "conn": 7, "db": "shop", "groups": ["Sales", "Archive"],
                    "objects": [
                        { "schema": "public", "table": "customers", "group": "Sales", "color": [81, 143, 221] },
                        { "schema": "public", "table": "orders", "group": "Sales", "color": [65, 163, 112] }
                    ]
                })).unwrap();
                let schema = &mut state.connections[0].databases[0].schemas.as_mut().unwrap()[0];
                let columns = schema.tables[0].columns.clone();
                for i in 1..=40 {
                    schema.tables.push(TableInfo {
                        name: format!("table_{i:02}"),
                        kind: ObjectKind::Table,
                        columns: columns.clone(),
                    });
                }
                for table in &schema.tables {
                    state.browser.catalog.insert(
                        (schema.name.clone(), table.name.clone()),
                        ObjectInfo {
                            rows: Some(42),
                            data_bytes: Some(16384),
                            engine: Some("InnoDB".into()),
                            created: Some("2026-01-01 12:00:00".into()),
                            collation: Some("utf8mb4_unicode_ci".into()),
                            ..Default::default()
                        },
                    );
                }
            }
            h.run_steps(3);
            h.get_by_label("customers").click();
            h.run_steps(3);
            h.render()
                .unwrap()
                .save(directory.join(format!("{name}.png")))
                .unwrap();
        }
    }
}
