//! Iconos lineales dibujados con el `Painter`, al estilo de los iconos "new UI" de JetBrains
//! (trazo fino de ~1.3 px sobre una rejilla de 16x16). No se usa ningún SVG externo.

use crate::db::Driver;
use crate::ui::theme::Palette;
use egui::{Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ico {
    ChevronRight,
    ChevronDown,
    Database,
    Driver(Driver),
    Schema,
    Folder,
    Table,
    View,
    Column,
    Key,
    Plus,
    Minus,
    Refresh,
    Stop,
    Console,
    JumpToConsole,
    Ddl,
    Eye,
    Gear,
    Search,
    Properties,
    Play,
    PlayAll,
    Undo,
    Revert,
    Submit,
    History,
    Filter,
    Sort,
    SortAsc,
    SortDesc,
    Lock,
    Services,
    Terminal,
    Problems,
    Bell,
    Structure,
    More,
    MoreVert,
    Close,
    Export,
}

const W: f32 = 1.3;

/// Pinta `ico` dentro de `rect` (se asume ~16x16). `color` es el color de trazo base;
/// algunos iconos de objetos usan su propio color de la paleta.
pub fn paint(painter: &Painter, rect: Rect, ico: Ico, color: Color32, p: &Palette, enabled: bool) {
    let s = rect.width() / 16.0;
    let o = rect.min;
    let at = |x: f32, y: f32| pos2(o.x + x * s, o.y + y * s);
    let st = |c: Color32| Stroke::new(W, c);
    let line = |pts: &[(f32, f32)], c: Color32| {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| at(x, y)).collect();
        painter.add(Shape::line(v, st(c)));
    };
    let obj = |c: Color32| if enabled { c } else { color };
    match ico {
        Ico::ChevronRight => line(&[(6.0, 3.5), (10.5, 8.0), (6.0, 12.5)], color),
        Ico::ChevronDown => line(&[(3.5, 6.0), (8.0, 10.5), (12.5, 6.0)], color),
        Ico::Database | Ico::Driver(_) => {
            let c = match ico {
                Ico::Driver(d) => obj(driver_color(d, p)),
                _ => obj(p.db_icon),
            };
            cylinder(painter, at(8.0, 0.0).x, at(0.0, 3.0).y, 5.5 * s, 2.0 * s, 11.0 * s, c);
        }
        Ico::Schema => {
            let c = obj(p.schema_icon);
            // Tres capas apiladas (rombos).
            for (i, dy) in [0.0f32, 3.0, 6.0].iter().enumerate() {
                let y = 4.0 + dy;
                if i == 0 {
                    let pts = vec![at(8.0, y - 2.5), at(14.0, y), at(8.0, y + 2.5), at(2.0, y)];
                    painter.add(Shape::closed_line(pts, st(c)));
                } else {
                    line(&[(2.0, y), (8.0, y + 2.5), (14.0, y)], c);
                }
            }
        }
        Ico::Folder => {
            let c = obj(p.folder_icon);
            let pts = vec![at(1.5, 3.0), at(6.0, 3.0), at(7.5, 4.8), at(14.5, 4.8), at(14.5, 13.0), at(1.5, 13.0)];
            painter.add(Shape::convex_polygon(pts.clone(), c.gamma_multiply(0.18), Stroke::NONE));
            painter.add(Shape::closed_line(pts, st(c)));
            line(&[(1.5, 6.5), (14.5, 6.5)], c);
        }
        Ico::Table => {
            let c = obj(p.table_icon);
            let r = Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5));
            painter.rect_filled(Rect::from_min_max(r.min, pos2(r.max.x, at(0.0, 6.0).y)), 2.0 * s, c.gamma_multiply(0.22));
            painter.rect_stroke(r, 2.0 * s, st(c), StrokeKind::Middle);
            line(&[(1.5, 6.0), (14.5, 6.0)], c);
            line(&[(1.5, 9.75), (14.5, 9.75)], c);
            line(&[(6.0, 6.0), (6.0, 13.5)], c);
        }
        Ico::View => {
            let c = obj(p.view_icon);
            let r = Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5));
            painter.rect_stroke(r, 2.0 * s, st(c), StrokeKind::Middle);
            line(&[(1.5, 6.0), (14.5, 6.0)], c);
            painter.circle_stroke(at(5.5, 10.0), 1.8 * s, st(c));
            painter.circle_stroke(at(10.5, 10.0), 1.8 * s, st(c));
            line(&[(7.3, 10.0), (8.7, 10.0)], c);
        }
        Ico::Column => {
            let c = obj(p.column_icon);
            let r = Rect::from_min_max(at(2.0, 4.5), at(14.0, 11.5));
            painter.rect_stroke(r, 1.5 * s, st(c), StrokeKind::Middle);
            painter.rect_filled(Rect::from_min_max(at(2.0, 4.5), at(6.0, 11.5)), 1.5 * s, c.gamma_multiply(0.55));
        }
        Ico::Key => {
            let c = obj(p.key_icon);
            painter.circle_stroke(at(5.0, 8.0), 2.7 * s, Stroke::new(1.5, c));
            line(&[(7.7, 8.0), (14.5, 8.0)], c);
            line(&[(12.0, 8.0), (12.0, 10.8)], c);
            line(&[(14.3, 8.0), (14.3, 10.3)], c);
        }
        Ico::Plus => {
            line(&[(8.0, 2.5), (8.0, 13.5)], color);
            line(&[(2.5, 8.0), (13.5, 8.0)], color);
        }
        Ico::Minus => line(&[(2.5, 8.0), (13.5, 8.0)], color),
        Ico::Refresh => {
            arc(painter, at(8.0, 8.0), 5.2 * s, -PI * 0.25, PI * 1.5, st(color));
            // Punta de flecha al final del arco (arriba), apuntando en el sentido de giro.
            let tip = at(8.0, 2.8);
            painter.add(Shape::line(vec![tip + vec2(-2.6, -2.6) * s, tip, tip + vec2(-2.6, 2.6) * s], st(color)));
        }
        Ico::Stop => {
            let c = if enabled { p.stop } else { color };
            painter.rect_filled(Rect::from_min_max(at(3.5, 3.5), at(12.5, 12.5)), 2.0 * s, c);
        }
        Ico::Console => {
            let r = Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5));
            painter.rect_stroke(r, 2.0 * s, st(color), StrokeKind::Middle);
            line(&[(4.5, 6.0), (7.0, 8.0), (4.5, 10.0)], color);
            line(&[(8.5, 10.5), (11.5, 10.5)], color);
        }
        Ico::JumpToConsole => {
            let r = Rect::from_min_max(at(1.5, 3.5), at(11.5, 13.5));
            painter.rect_stroke(r, 2.0 * s, st(color), StrokeKind::Middle);
            line(&[(4.0, 7.0), (6.0, 8.5), (4.0, 10.0)], color);
            line(&[(9.0, 1.5), (14.5, 1.5), (14.5, 7.0)], color);
            line(&[(14.5, 1.5), (9.5, 6.5)], color);
        }
        Ico::Ddl => {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "DDL", FontId::proportional(9.5 * s), color);
        }
        Ico::Eye => {
            let mut top = Vec::new();
            let mut bottom = Vec::new();
            for i in 0..=16 {
                let t = i as f32 / 16.0;
                let x = 1.5 + t * 13.0;
                let h = (t * PI).sin() * 4.2;
                top.push(at(x, 8.0 - h));
                bottom.push(at(x, 8.0 + h));
            }
            painter.add(Shape::line(top, st(color)));
            painter.add(Shape::line(bottom, st(color)));
            painter.circle_stroke(at(8.0, 8.0), 2.0 * s, st(color));
        }
        Ico::Gear => {
            let c = at(8.0, 8.0);
            painter.circle_stroke(c, 2.2 * s, st(color));
            let mut pts = Vec::new();
            let teeth = 8;
            for i in 0..teeth * 4 {
                let a = i as f32 / (teeth * 4) as f32 * TAU;
                let r = if (i / 2) % 2 == 0 { 6.6 } else { 5.0 } * s;
                pts.push(c + vec2(a.cos(), a.sin()) * r);
            }
            painter.add(Shape::closed_line(pts, st(color)));
        }
        Ico::Search => {
            painter.circle_stroke(at(7.0, 7.0), 4.5 * s, st(color));
            line(&[(10.3, 10.3), (14.0, 14.0)], color);
        }
        Ico::Properties => {
            // Deslizadores (propiedades del data source).
            for (y, k) in [(4.0, 10.5), (8.0, 5.0), (12.0, 9.0)] {
                line(&[(1.5, y), (14.5, y)], color);
                painter.circle_filled(at(k, y), 2.0 * s, p.island_bg);
                painter.circle_stroke(at(k, y), 2.0 * s, st(color));
            }
        }
        Ico::Play | Ico::PlayAll => {
            let c = if enabled { p.run } else { color };
            let pts = if ico == Ico::Play {
                vec![at(4.0, 2.5), at(13.0, 8.0), at(4.0, 13.5)]
            } else {
                vec![at(2.0, 3.0), at(9.0, 8.0), at(2.0, 13.0)]
            };
            painter.add(Shape::convex_polygon(pts, c, Stroke::new(1.0, c)));
            if ico == Ico::PlayAll {
                painter.add(Shape::convex_polygon(vec![at(8.0, 3.0), at(15.0, 8.0), at(8.0, 13.0)], c, Stroke::new(1.0, c)));
            }
        }
        Ico::Undo => {
            arc(painter, at(8.5, 9.0), 4.5 * s, -PI * 0.9, PI * 0.5, st(color));
            line(&[(2.5, 4.0), (4.0, 8.0), (8.0, 6.8)], color);
        }
        Ico::Revert => {
            arc(painter, at(8.0, 8.0), 5.0 * s, -PI * 0.8, PI * 1.0, st(color));
            line(&[(2.0, 3.5), (3.6, 6.4), (6.6, 5.6)], color);
        }
        Ico::Submit => {
            line(&[(8.0, 2.5), (8.0, 11.0)], color);
            line(&[(4.5, 6.0), (8.0, 2.5), (11.5, 6.0)], color);
            line(&[(3.0, 13.5), (13.0, 13.5)], color);
        }
        Ico::History => {
            painter.circle_stroke(at(8.0, 8.0), 5.8 * s, st(color));
            line(&[(8.0, 4.5), (8.0, 8.0), (10.5, 9.5)], color);
        }
        Ico::Filter => {
            let pts = vec![at(2.0, 3.0), at(14.0, 3.0), at(9.5, 8.5), at(9.5, 13.0), at(6.5, 11.5), at(6.5, 8.5)];
            painter.add(Shape::closed_line(pts, st(color)));
        }
        Ico::Sort | Ico::SortAsc | Ico::SortDesc => {
            let up = if ico == Ico::SortDesc { p.disabled.gamma_multiply(0.6) } else { color };
            let down = if ico == Ico::SortAsc { p.disabled.gamma_multiply(0.6) } else { color };
            line(&[(5.0, 2.5), (5.0, 13.0)], up);
            line(&[(2.5, 5.0), (5.0, 2.5), (7.5, 5.0)], up);
            line(&[(11.0, 3.0), (11.0, 13.5)], down);
            line(&[(8.5, 11.0), (11.0, 13.5), (13.5, 11.0)], down);
        }
        Ico::Lock => {
            let r = Rect::from_min_max(at(3.0, 7.0), at(13.0, 14.0));
            painter.rect_stroke(r, 1.5 * s, st(color), StrokeKind::Middle);
            arc(painter, at(8.0, 6.5), 3.0 * s, PI, TAU, st(color));
            line(&[(5.0, 6.5), (5.0, 7.0)], color);
            line(&[(11.0, 6.5), (11.0, 7.0)], color);
        }
        Ico::Services => {
            painter.circle_stroke(at(8.0, 8.0), 6.0 * s, st(color));
            painter.add(Shape::convex_polygon(vec![at(6.5, 5.2), at(11.0, 8.0), at(6.5, 10.8)], color, Stroke::NONE));
        }
        Ico::Terminal => {
            let r = Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5));
            painter.rect_stroke(r, 2.0 * s, st(color), StrokeKind::Middle);
            line(&[(4.0, 6.0), (6.5, 8.0), (4.0, 10.0)], color);
            line(&[(8.0, 10.5), (12.0, 10.5)], color);
        }
        Ico::Problems => {
            painter.circle_stroke(at(8.0, 8.0), 6.0 * s, st(color));
            line(&[(8.0, 4.5), (8.0, 8.8)], color);
            painter.circle_filled(at(8.0, 11.2), 0.9 * s, color);
        }
        Ico::Bell => {
            let mut pts = vec![at(3.0, 12.0)];
            pts.push(at(4.0, 10.0));
            pts.push(at(4.0, 7.0));
            let c = at(8.0, 7.0);
            for i in 0..=10 {
                let a = PI + i as f32 / 10.0 * PI;
                pts.push(c + vec2(a.cos(), a.sin()) * 4.0 * s);
            }
            pts.push(at(12.0, 10.0));
            pts.push(at(13.0, 12.0));
            painter.add(Shape::closed_line(pts, st(color)));
            line(&[(6.5, 14.0), (9.5, 14.0)], color);
        }
        Ico::Structure => {
            painter.rect_stroke(Rect::from_min_max(at(2.0, 2.0), at(7.0, 6.0)), 1.0, st(color), StrokeKind::Middle);
            painter.rect_stroke(Rect::from_min_max(at(9.0, 7.0), at(14.0, 10.0)), 1.0, st(color), StrokeKind::Middle);
            painter.rect_stroke(Rect::from_min_max(at(9.0, 11.5), at(14.0, 14.5)), 1.0, st(color), StrokeKind::Middle);
            line(&[(4.5, 6.0), (4.5, 13.0), (9.0, 13.0)], color);
            line(&[(4.5, 8.5), (9.0, 8.5)], color);
        }
        Ico::More => {
            for x in [3.5, 8.0, 12.5] {
                painter.circle_filled(at(x, 8.0), 1.15 * s, color);
            }
        }
        Ico::MoreVert => {
            for y in [3.5, 8.0, 12.5] {
                painter.circle_filled(at(8.0, y), 1.15 * s, color);
            }
        }
        Ico::Close => {
            line(&[(4.0, 4.0), (12.0, 12.0)], color);
            line(&[(12.0, 4.0), (4.0, 12.0)], color);
        }
        Ico::Export => {
            line(&[(8.0, 2.0), (8.0, 10.5)], color);
            line(&[(4.5, 7.0), (8.0, 10.5), (11.5, 7.0)], color);
            line(&[(2.5, 11.0), (2.5, 13.5), (13.5, 13.5), (13.5, 11.0)], color);
        }
    }
}

fn arc(painter: &Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let n = 24;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    painter.add(Shape::line(pts, stroke));
}

/// Cilindro (icono de base de datos): `top` es la y del centro de la elipse superior.
fn cylinder(painter: &Painter, cx: f32, top: f32, rx: f32, ry: f32, h: f32, c: Color32) {
    let stroke = Stroke::new(W, c);
    let ellipse = |cy: f32, from: f32, to: f32| -> Vec<Pos2> {
        let n = 24;
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                pos2(cx + a.cos() * rx, cy + a.sin() * ry)
            })
            .collect()
    };
    let bottom = top + h - 2.0 * ry;
    painter.add(Shape::closed_line(ellipse(top, 0.0, TAU), stroke));
    painter.add(Shape::line(ellipse(top + (bottom - top) / 2.0, 0.0, PI), stroke));
    painter.add(Shape::line(ellipse(bottom, 0.0, PI), stroke));
    painter.line_segment([pos2(cx - rx, top), pos2(cx - rx, bottom)], stroke);
    painter.line_segment([pos2(cx + rx, top), pos2(cx + rx, bottom)], stroke);
}

pub fn driver_color(driver: Driver, p: &Palette) -> Color32 {
    match driver {
        Driver::Postgres => p.db_icon,
        Driver::MsSql => Color32::from_rgb(0xC9, 0x46, 0x46),
        Driver::Sqlite => Color32::from_rgb(0x3F, 0x9B, 0xD8),
    }
}
