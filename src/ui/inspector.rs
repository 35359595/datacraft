//! Shared, compact information pane for objects and table editors.
use crate::db::object_info::ObjectInfo;
use crate::i18n::tr;
use crate::ui::{
    icons::{self, Ico},
    theme::{self, pal},
};
use egui::{Color32, Rect, RichText, Ui, pos2, vec2};

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorMode {
    #[default]
    Information,
    Columns,
    Record,
}

pub fn tabs(ui: &mut Ui, mode: &mut InspectorMode, record: bool) {
    for (value, icon, label) in [
        (InspectorMode::Information, Ico::Info, tr("Information")),
        (
            InspectorMode::Columns,
            Ico::Column,
            if record { tr("Column") } else { tr("Columns") },
        ),
        (InspectorMode::Record, Ico::Record, tr("Record")),
    ] {
        if value == InspectorMode::Record && !record {
            continue;
        }
        if theme::icon_tab(ui, icon, label, *mode == value).clicked() {
            *mode = value;
        }
    }
}

fn hero(ui: &mut Ui, rect: Rect, icon: Ico) {
    let painter = ui.painter();
    if icon == Ico::Database {
        let green = Color32::from_rgb(35, 168, 125);
        let body = rect.shrink2(vec2(5.0, 7.0));
        painter.rect_filled(body, 5.0, green);
        for (y, color) in [
            (body.bottom() - 3.0, Color32::from_rgb(19, 128, 94)),
            (body.center().y, green),
            (body.top(), Color32::from_rgb(105, 221, 175)),
        ] {
            painter.add(egui::epaint::EllipseShape::filled(
                pos2(body.center().x, y),
                vec2(body.width() / 2.0, 7.0),
                color,
            ));
        }
    } else {
        let blue = Color32::from_rgb(28, 143, 215);
        painter.rect_filled(rect.shrink(3.0), 7.0, blue);
        let body = Rect::from_min_max(rect.min + vec2(3.0, 15.0), rect.max - vec2(3.0, 3.0));
        let colors = [
            Color32::from_rgb(139, 211, 250),
            Color32::from_rgb(92, 181, 238),
            Color32::from_rgb(173, 225, 252),
        ];
        for row in 0..3 {
            for col in 0..3 {
                let min = body.min
                    + vec2(
                        col as f32 * body.width() / 3.0,
                        row as f32 * body.height() / 3.0,
                    );
                painter.rect_filled(
                    Rect::from_min_size(min, body.size() / 3.0).shrink(0.35),
                    1.0,
                    colors[(row + col) % 3],
                );
            }
        }
        if matches!(icon, Ico::View | Ico::Column | Ico::Record) {
            let badge =
                Rect::from_center_size(rect.right_bottom() - vec2(7.0, 7.0), vec2(22.0, 22.0));
            let p = pal(ui);
            painter.rect_filled(badge, 5.0, p.island_bg);
            icons::paint(painter, badge.shrink(3.0), icon, p.accent, &p, true);
        }
    }
}

pub fn header(ui: &mut Ui, name: &str, kind: &str, connection: &str, database: &str, icon: Ico) {
    let p = pal(ui);
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(56.0, 56.0), egui::Sense::hover());
        hero(ui, rect, icon);
        ui.vertical(|ui| {
            ui.add(egui::Label::new(RichText::new(name).font(theme::bold(13.0))).truncate())
                .on_hover_text(name);
            ui.label(RichText::new(kind).small().color(p.muted));
        });
    });
    ui.add_space(16.0);
    if !connection.is_empty() {
        ui.label(RichText::new(connection).small().color(p.muted));
    }
    if !database.is_empty() {
        ui.label(RichText::new(database).small().color(p.muted));
    }
    ui.add_space(18.0);
}

pub fn property(ui: &mut Ui, label: &str, value: Option<&str>) {
    let p = pal(ui);
    ui.label(RichText::new(label).size(12.0).color(p.muted));
    ui.add(egui::Label::new(RichText::new(value.unwrap_or("—")).size(12.5).color(p.text)).wrap())
        .on_hover_text(if value.is_none() {
            tr("Not available")
        } else {
            ""
        });
    ui.add_space(12.0);
}

pub fn table_fields(ui: &mut Ui, info: Option<&ObjectInfo>) {
    let rows = info.and_then(|i| i.rows).map(|n| {
        if info.is_some_and(|i| i.rows_exact) {
            n.to_string()
        } else {
            format!("≈ {n}")
        }
    });
    let auto = info.and_then(|i| i.auto_increment).map(|n| n.to_string());
    let bytes = |n: Option<u64>| {
        n.map(|n| {
            if n >= 1_048_576 {
                format!("{:.2} MiB ({n})", n as f64 / 1_048_576.0)
            } else {
                format!("{:.2} KiB ({n})", n as f64 / 1024.0)
            }
        })
    };
    let data = bytes(info.and_then(|i| i.data_bytes));
    let index = bytes(info.and_then(|i| i.index_bytes));
    let max = bytes(info.and_then(|i| i.max_data_bytes));
    let free = bytes(info.and_then(|i| i.free_bytes));
    let average = bytes(info.and_then(|i| i.average_row_bytes));
    for (label, value) in [
        (tr("Rows"), rows.as_deref()),
        (tr("Engine"), info.and_then(|i| i.engine.as_deref())),
        (tr("Auto increment"), auto.as_deref()),
        (tr("Row format"), info.and_then(|i| i.row_format.as_deref())),
        (
            tr("Modified date"),
            info.and_then(|i| i.modified.as_deref()),
        ),
        (tr("Created date"), info.and_then(|i| i.created.as_deref())),
        (tr("Check time"), info.and_then(|i| i.checked.as_deref())),
        (tr("Index length"), index.as_deref()),
        (tr("Data length"), data.as_deref()),
        (tr("Max data length"), max.as_deref()),
        (tr("Data free"), free.as_deref()),
        (tr("Average row length"), average.as_deref()),
        (tr("Collation"), info.and_then(|i| i.collation.as_deref())),
        (
            tr("Create options"),
            info.and_then(|i| i.create_options.as_deref()),
        ),
        (tr("Comment"), info.and_then(|i| i.comment.as_deref())),
    ] {
        property(ui, label, value);
    }
}
