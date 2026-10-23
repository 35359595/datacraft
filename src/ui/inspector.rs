//! Shared, compact information pane for objects and table editors.
use crate::db::object_info::ObjectInfo;
use crate::i18n::tr;
use crate::ui::{
    icons::{self, Ico},
    theme::{self, pal},
};
use egui::{RichText, Ui, vec2};

pub fn header(ui: &mut Ui, name: &str, kind: &str, connection: &str, database: &str) {
    let p = pal(ui);
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), egui::Sense::hover());
        icons::paint(
            ui.painter(),
            rect.shrink(2.0),
            Ico::Table,
            p.accent,
            &p,
            true,
        );
        ui.vertical(|ui| {
            ui.label(RichText::new(name).font(theme::bold(15.0)));
            ui.label(RichText::new(kind).small().color(p.muted));
        });
    });
    ui.add_space(12.0);
    if !connection.is_empty() {
        ui.label(RichText::new(connection).small().color(p.muted));
    }
    ui.label(RichText::new(database).small().color(p.muted));
    ui.add_space(14.0);
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
