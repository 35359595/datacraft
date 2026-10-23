//! Local object folders and colors; never changes database objects.

use crate::i18n::tr;
use egui::{Color32, RichText, Ui};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Organization {
    pub conn: u64,
    pub db: String,
    pub groups: Vec<String>,
    objects: Vec<ObjectStyle>,
    #[serde(skip)]
    name: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct ObjectStyle {
    schema: String,
    table: String,
    group: Option<String>,
    color: Option<[u8; 3]>,
}

#[derive(Default, Clone, PartialEq, Eq)]
pub enum GroupFilter {
    #[default]
    All,
    Ungrouped,
    Named(String),
}

impl Organization {
    fn style(&self, schema: &str, table: &str) -> Option<&ObjectStyle> {
        self.objects
            .iter()
            .find(|o| o.schema == schema && o.table == table)
    }

    fn style_mut(&mut self, schema: &str, table: &str) -> &mut ObjectStyle {
        let index = self
            .objects
            .iter()
            .position(|o| o.schema == schema && o.table == table)
            .unwrap_or_else(|| {
                self.objects.push(ObjectStyle {
                    schema: schema.into(),
                    table: table.into(),
                    ..Default::default()
                });
                self.objects.len() - 1
            });
        &mut self.objects[index]
    }

    pub fn color(&self, schema: &str, table: &str) -> Option<Color32> {
        self.style(schema, table)
            .and_then(|o| o.color)
            .map(|[r, g, b]| Color32::from_rgb(r, g, b))
    }

    pub fn matches(&self, schema: &str, table: &str, filter: &GroupFilter) -> bool {
        let group = self.style(schema, table).and_then(|o| o.group.as_deref());
        match filter {
            GroupFilter::All => true,
            GroupFilter::Ungrouped => group.is_none(),
            GroupFilter::Named(name) => group == Some(name.as_str()),
        }
    }

    fn rename(&mut self, old: &str, name: &str) {
        for group in &mut self.groups {
            if group == old {
                *group = name.into();
            }
        }
        for style in &mut self.objects {
            if style.group.as_deref() == Some(old) {
                style.group = Some(name.into());
            }
        }
    }

    fn remove(&mut self, name: &str) {
        self.groups.retain(|g| g != name);
        for style in &mut self.objects {
            if style.group.as_deref() == Some(name) {
                style.group = None;
            }
        }
    }

    pub fn folders(&mut self, ui: &mut Ui, filter: &mut GroupFilter) {
        ui.heading(tr("Groups"));
        ui.selectable_value(filter, GroupFilter::All, tr("All objects"));
        ui.selectable_value(filter, GroupFilter::Ungrouped, tr("Ungrouped"));
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("object_groups")
            .max_height((ui.available_height() - 150.0).max(60.0))
            .show(ui, |ui| {
                for name in &self.groups {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                        let p = crate::ui::theme::pal(ui);
                        crate::ui::icons::paint(
                            ui.painter(),
                            rect,
                            crate::ui::icons::Ico::Folder,
                            p.icon,
                            &p,
                            true,
                        );
                        ui.selectable_value(filter, GroupFilter::Named(name.clone()), name);
                    });
                }
            });
        ui.separator();
        ui.add(
            egui::TextEdit::singleline(&mut self.name)
                .id(egui::Id::new("new_object_group"))
                .char_limit(120)
                .hint_text(tr("Group name"))
                .desired_width(ui.available_width()),
        );
        let name = self.name.trim().to_string();
        let valid = !name.is_empty() && !self.groups.contains(&name);
        if ui
            .add_enabled(valid, egui::Button::new(tr("New group")))
            .clicked()
        {
            self.groups.push(name.clone());
            *filter = GroupFilter::Named(name.clone());
            self.name.clear();
        }
        if let GroupFilter::Named(old) = filter.clone() {
            if ui
                .add_enabled(valid, egui::Button::new(tr("Rename group")))
                .clicked()
            {
                self.rename(&old, &name);
                *filter = GroupFilter::Named(name);
                self.name.clear();
            }
            if ui.button(tr("Delete group")).clicked() {
                self.remove(&old);
                *filter = GroupFilter::All;
            }
        }
        ui.label(
            RichText::new(tr("Groups and colors are stored locally."))
                .small()
                .weak(),
        );
    }

    pub fn menu(&mut self, ui: &mut Ui, schema: &str, table: &str) {
        ui.menu_button(tr("Move to group"), |ui| {
            if ui.button(tr("Ungrouped")).clicked() {
                self.style_mut(schema, table).group = None;
                ui.close();
            }
            let mut chosen = None;
            for group in &self.groups {
                if ui.button(group).clicked() {
                    chosen = Some(group.clone());
                    ui.close();
                }
            }
            if let Some(group) = chosen {
                self.style_mut(schema, table).group = Some(group);
            }
        });
        ui.menu_button(tr("Color"), |ui| {
            if ui.button(tr("Default")).clicked() {
                self.style_mut(schema, table).color = None;
                ui.close();
            }
            for (name, color) in [
                (tr("Red"), [210, 85, 90]),
                (tr("Orange"), [201, 133, 45]),
                (tr("Green"), [65, 163, 112]),
                (tr("Blue"), [81, 143, 221]),
                (tr("Purple"), [161, 116, 212]),
                (tr("Pink"), [204, 110, 165]),
            ] {
                if ui
                    .button(
                        RichText::new(name).color(Color32::from_rgb(color[0], color[1], color[2])),
                    )
                    .clicked()
                {
                    self.style_mut(schema, table).color = Some(color);
                    ui.close();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_colors_roundtrip_without_touching_other_schemas() {
        let mut org = Organization {
            conn: 7,
            db: "shop".into(),
            groups: vec!["Sales".into()],
            ..Default::default()
        };
        let style = org.style_mut("public", "orders");
        style.group = Some("Sales".into());
        style.color = Some([81, 143, 221]);
        org.rename("Sales", "Orders");
        let mut org: Organization =
            serde_json::from_str(&serde_json::to_string(&org).unwrap()).unwrap();
        assert!(org.matches("public", "orders", &GroupFilter::Named("Orders".into())));
        assert!(org.matches("archive", "orders", &GroupFilter::Ungrouped));
        assert!(org.color("public", "orders").is_some());
        org.remove("Orders");
        assert!(org.matches("public", "orders", &GroupFilter::Ungrouped));
        assert!(org.color("public", "orders").is_some());
    }
}
