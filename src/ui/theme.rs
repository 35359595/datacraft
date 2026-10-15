//! Tema visual "Islands" (Light / Dark) tomado de los temas de DataGrip 2026.2
//! (`ManyIslandsLight.theme.json`, `ManyIslandsDark.theme.json` y los esquemas de editor
//! "Light" / "Dark"), fuentes Inter + JetBrains Mono y helpers de pintado comunes.

use crate::ui::icons::{self, Ico};
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Rect, Response, Sense, Shadow,
    Stroke, TextStyle, Ui, Vec2, vec2,
};
use std::sync::Arc;

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// `#RRGGBBAA` al estilo de los temas de IntelliJ (alfa no premultiplicado).
const fn hexa(rgba: u32) -> Color32 {
    let a = rgba & 0xFF;
    Color32::from_rgba_premultiplied(
        (((rgba >> 24) & 0xFF) * a / 255) as u8,
        (((rgba >> 16) & 0xFF) * a / 255) as u8,
        (((rgba >> 8) & 0xFF) * a / 255) as u8,
        a as u8,
    )
}

/// Paleta usada por los componentes propios. Cada campo indica la clave del tema de origen.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// MainWindow.background / MainToolbar / StatusBar / ToolWindow.Stripe (main-window-bg).
    pub main_bg: Color32,
    /// Island.borderColor / ToolWindow.background / editor-bg.
    pub island_bg: Color32,
    /// editor-border (borde sutil de las islas y separadores internos).
    pub island_border: Color32,
    /// text-default.
    pub text: Color32,
    /// text-muted (StatusBar.Widget.foreground, Breadcrumbs).
    pub text_muted: Color32,
    /// text-secondary (infoForeground: contadores, tipos de columna).
    pub muted: Color32,
    /// text-disabled.
    pub disabled: Color32,
    /// Color de los iconos lineales (gris "new UI").
    pub icon: Color32,
    /// toolbar-bg-hovered.
    pub hover: Color32,
    /// toolbar-bg-pressed.
    pub pressed: Color32,
    /// Tree/Table.selectionBackground (selection-bg-active).
    pub selection: Color32,
    /// selectionInactiveBackground.
    pub selection_inactive: Color32,
    /// List/Tree.hoverBackground.
    pub row_hover: Color32,
    /// EditorTabs.underlinedTabBackground / underlinedBorderColor.
    pub tab_selected_bg: Color32,
    pub tab_selected_border: Color32,
    /// Component.borderColor (control-border).
    pub control_border: Color32,
    /// TextField.background (control-bg).
    pub control_bg: Color32,
    /// Button.default.startBackground (control-brand-bg).
    pub accent: Color32,
    /// Table.gridColor.
    pub grid: Color32,
    /// Fondo de cabecera de tabla.
    pub header_bg: Color32,
    /// Popup.background.
    pub popup_bg: Color32,
    /// Popup.borderColor.
    pub popup_border: Color32,
    /// Fondo del botón activo de la franja de tool windows.
    pub stripe_selected: Color32,
    /// List.Tag (badges del árbol).
    pub badge_bg: Color32,
    pub badge_fg: Color32,
    // --- esquema de editor ---
    pub editor_bg: Color32,
    pub gutter_fg: Color32,
    pub gutter_fg_caret: Color32,
    pub caret_row: Color32,
    pub editor_selection: Color32,
    // --- estados ---
    pub error: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub run: Color32,
    pub stop: Color32,
    // --- iconos de objetos ---
    pub db_icon: Color32,
    pub table_icon: Color32,
    pub view_icon: Color32,
    pub folder_icon: Color32,
    pub schema_icon: Color32,
    pub column_icon: Color32,
    pub key_icon: Color32,
    pub project_badge: Color32,
    // --- edición de datos (aproximados al grid de DataGrip) ---
    /// Celda modificada pendiente de guardar.
    pub cell_modified: Color32,
    /// Fila nueva pendiente de insertar.
    pub row_added: Color32,
    /// Fila marcada para borrar.
    pub row_deleted: Color32,
}

pub const LIGHT: Palette = Palette {
    dark: false,
    main_bg: hex(0xE9EAEE),
    island_bg: hex(0xFFFFFF),
    island_border: hex(0xE9EAEE),
    text: hex(0x000000),
    text_muted: hex(0x5F6269),
    muted: hex(0x73767C),
    disabled: hex(0x9FA2A8),
    icon: hex(0x6C707E),
    hover: hexa(0x00000012),
    pressed: hexa(0x00000020),
    selection: hex(0xD0DFFE),
    selection_inactive: hex(0xE9EAEE),
    row_hover: hexa(0x00000008),
    tab_selected_bg: hex(0xE3EBFE),
    tab_selected_border: hex(0xA7C5FF),
    control_border: hex(0xD1D3D9),
    control_bg: hex(0xFFFFFF),
    accent: hex(0x3871E1),
    grid: hex(0xE9EAEE),
    header_bg: hex(0xF7F8F9),
    popup_bg: hex(0xFFFFFF),
    popup_border: hex(0xD1D3D9),
    stripe_selected: hex(0xDDDFE4),
    badge_bg: hex(0xEBECF0),
    badge_fg: hex(0x494B57),
    editor_bg: hex(0xFFFFFF),
    gutter_fg: hex(0xAEB3C2),
    gutter_fg_caret: hex(0x767A8A),
    caret_row: hex(0xF5F8FE),
    editor_selection: hex(0xA6D2FF),
    error: hex(0xC54E58),
    ok: hex(0x338555),
    warn: hex(0xA56906),
    run: hex(0x369650),
    stop: hex(0xDB5C5C),
    db_icon: hex(0x3871E1),
    table_icon: hex(0x6B85AE),
    view_icon: hex(0x4E9D6C),
    folder_icon: hex(0x7A8CAD),
    schema_icon: hex(0x8060DB),
    column_icon: hex(0x7B8094),
    key_icon: hex(0xD9A343),
    project_badge: hex(0x8E6CEF),
    cell_modified: hex(0xD6E4FD),
    row_added: hex(0xDDF1E1),
    row_deleted: hex(0xF3E1E2),
};

pub const DARK: Palette = Palette {
    dark: true,
    main_bg: hex(0x26282C),
    island_bg: hex(0x191A1C),
    island_border: hex(0x26282C),
    text: hex(0xD1D3D9),
    text_muted: hex(0x9FA2A8),
    muted: hex(0x73767C),
    disabled: hex(0x4C4F56),
    icon: hex(0xA4A7AE),
    hover: hexa(0xFFFFFF17),
    pressed: hexa(0xFFFFFF21),
    selection: hex(0x2A4371),
    selection_inactive: hex(0x33353B),
    row_hover: hexa(0xFFFFFF10),
    tab_selected_bg: hex(0x233558),
    tab_selected_border: hex(0x2E4D89),
    control_border: hex(0x40434A),
    control_bg: hex(0x191A1C),
    accent: hex(0x3871E1),
    grid: hex(0x2B2D31),
    header_bg: hex(0x212326),
    popup_bg: hex(0x26282C),
    popup_border: hex(0x33353B),
    stripe_selected: hex(0x40434A),
    badge_bg: hex(0x33353B),
    badge_fg: hex(0xB5B7BD),
    editor_bg: hex(0x191A1C),
    gutter_fg: hex(0x4B5059),
    gutter_fg_caret: hex(0xA1A3AB),
    caret_row: hex(0x26282E),
    editor_selection: hex(0x214283),
    error: hex(0xF57E84),
    ok: hex(0x6DB083),
    warn: hex(0xD59637),
    run: hex(0x5FB865),
    stop: hex(0xDB5C5C),
    db_icon: hex(0x71A1FE),
    table_icon: hex(0x8B9EC0),
    view_icon: hex(0x6DB083),
    folder_icon: hex(0x8E9BB4),
    schema_icon: hex(0xA894F6),
    column_icon: hex(0x9FA2A8),
    key_icon: hex(0xE4AD5F),
    project_badge: hex(0x8060DB),
    cell_modified: hex(0x263A5C),
    row_added: hex(0x233F2D),
    row_deleted: hex(0x45282B),
};

pub fn palette(dark: bool) -> Palette {
    if dark { DARK } else { LIGHT }
}

/// Paleta según el modo del `Ui`.
pub fn pal(ui: &Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

// ------------------------------------------------------------------ fuentes

pub const FONT_BOLD: &str = "bold";
pub const FONT_MONO_BOLD: &str = "mono-bold";
pub const FONT_MONO_ITALIC: &str = "mono-italic";

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FONT_BOLD.into()))
}

pub fn mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FONT_MONO_BOLD.into()))
}

/// Inter (UI) y JetBrains Mono (editor y grid). Las fuentes por defecto de egui quedan
/// como respaldo para símbolos y emoji.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "Inter", include_bytes!("../../assets/fonts/Inter-Regular.otf"));
    add(&mut fonts, "Inter-SemiBold", include_bytes!("../../assets/fonts/Inter-SemiBold.otf"));
    add(&mut fonts, "JBMono", include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"));
    add(&mut fonts, "JBMono-Bold", include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf"));
    add(&mut fonts, "JBMono-Italic", include_bytes!("../../assets/fonts/JetBrainsMono-Italic.ttf"));

    let fallback_prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fallback_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let with = |first: &str, rest: &[String]| -> Vec<String> {
        let mut v = vec![first.to_owned()];
        v.extend(rest.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with("Inter", &fallback_prop));
    fonts.families.insert(FontFamily::Monospace, with("JBMono", &fallback_mono));
    fonts.families.insert(FontFamily::Name(FONT_BOLD.into()), with("Inter-SemiBold", &fallback_prop));
    fonts.families.insert(FontFamily::Name(FONT_MONO_BOLD.into()), with("JBMono-Bold", &fallback_mono));
    fonts.families.insert(FontFamily::Name(FONT_MONO_ITALIC.into()), with("JBMono-Italic", &fallback_mono));
    ctx.set_fonts(fonts);
}

// ------------------------------------------------------------------ visuals

pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = palette(dark);
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.main_bg;
    v.window_fill = p.popup_bg;
    v.extreme_bg_color = p.control_bg;
    v.text_edit_bg_color = Some(p.control_bg);
    v.faint_bg_color = p.header_bg;
    v.code_bg_color = p.editor_bg;
    v.window_stroke = Stroke::new(1.0, p.popup_border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 38 }) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if dark { 90 } else { 28 }) };
    v.override_text_color = None;
    v.hyperlink_color = p.accent;
    v.error_fg_color = p.error;
    v.warn_fg_color = p.warn;
    v.selection.bg_fill = p.editor_selection;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.striped = false;
    v.interact_cursor = None;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.handle_shape = egui::style::HandleShape::Circle;

    let r = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.island_bg;
    w.noninteractive.weak_bg_fill = p.island_bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.island_border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.control_bg;
    w.inactive.weak_bg_fill = p.control_bg;
    w.inactive.bg_stroke = Stroke::new(1.0, p.control_border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = p.control_bg;
    w.hovered.weak_bg_fill = blend(p.control_bg, p.hover);
    w.hovered.bg_stroke = Stroke::new(1.0, if dark { hex(0x5F6269) } else { hex(0xB5B7BD) });
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.control_bg;
    w.active.weak_bg_fill = blend(p.control_bg, p.pressed);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.control_bg;
    w.open.weak_bg_fill = blend(p.control_bg, p.hover);
    w.open.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r;

    let theme = if dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(theme);
    ctx.set_visuals_of(theme, v);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, bold(15.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.spacing.button_padding = vec2(8.0, 3.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.menu_margin = Margin::same(6);
        s.spacing.window_margin = Margin::same(16);
        s.spacing.combo_height = 300.0;
        s.spacing.icon_width = 14.0;
        s.spacing.icon_width_inner = 8.0;
        s.spacing.scroll = scroll_style();
        s.interaction.selectable_labels = false;
    });
}

/// Barras de scroll al estilo IntelliJ: finas, grises y redondeadas sobre un carril
/// transparente. Flotan sobre el contenido (no cambian el layout) pero, cuando hay algo que
/// desplazar, se ven siempre; se ensanchan al pasar el ratón por encima.
pub fn scroll_style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        floating: true,
        bar_width: 9.0,
        floating_width: 6.0,
        floating_allocated_width: 0.0,
        bar_inner_margin: 2.0,
        bar_outer_margin: 2.0,
        handle_min_length: 24.0,
        // Color del texto (no `bg_fill`, que en Islands Light es blanco como la isla).
        foreground_color: true,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 0.26,
        active_handle_opacity: 0.36,
        interact_handle_opacity: 0.5,
        ..egui::style::ScrollStyle::floating()
    }
}

/// Mezcla un color translúcido sobre uno opaco.
pub fn blend(base: Color32, over: Color32) -> Color32 {
    let a = over.a() as f32 / 255.0;
    if a <= 0.0 {
        return base;
    }
    // `over` está premultiplicado.
    let mix = |b: u8, o: u8| (b as f32 * (1.0 - a) + o as f32).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(mix(base.r(), over.r()), mix(base.g(), over.g()), mix(base.b(), over.b()))
}

// ------------------------------------------------------------------ helpers de widgets

/// Botón de icono de barra de herramientas (ActionButton): 26x26, hover redondeado.
pub fn icon_button(ui: &mut Ui, ico: Ico, enabled: bool, tooltip: &str) -> Response {
    icon_button_sized(ui, ico, enabled, tooltip, 26.0)
}

pub fn icon_button_sized(ui: &mut Ui, ico: Ico, enabled: bool, tooltip: &str, size: f32) -> Response {
    let p = pal(ui);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), sense);
    if enabled && (resp.hovered() || resp.is_pointer_button_down_on()) {
        let fill = if resp.is_pointer_button_down_on() { p.pressed } else { p.hover };
        ui.painter().rect_filled(rect, 6.0, fill);
    }
    let color = if enabled { p.icon } else { p.disabled };
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(16.0)), ico, color, &p, enabled);
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

/// Botón plano de texto (con chevron opcional) al estilo MainToolbar.Dropdown.
pub fn flat_button(ui: &mut Ui, leading: Option<Ico>, text: &str, font: FontId, chevron: bool, enabled: bool) -> Response {
    let p = pal(ui);
    let color = if enabled { p.text } else { p.disabled };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let lead_w = if leading.is_some() { 22.0 } else { 0.0 };
    let chev_w = if chevron { 16.0 } else { 0.0 };
    let w = 8.0 + lead_w + galley.size().x + chev_w + 8.0;
    let h = 26.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), sense);
    if enabled && (resp.hovered() || resp.is_pointer_button_down_on()) {
        let fill = if resp.is_pointer_button_down_on() { p.pressed } else { p.hover };
        ui.painter().rect_filled(rect, 6.0, fill);
    }
    let mut x = rect.left() + 8.0;
    if let Some(ico) = leading {
        icons::paint(ui.painter(), Rect::from_min_size(egui::pos2(x, rect.center().y - 8.0), Vec2::splat(16.0)), ico, p.icon, &p, enabled);
        x += lead_w;
    }
    ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley.clone(), color);
    x += galley.size().x + 2.0;
    if chevron {
        icons::paint(
            ui.painter(),
            Rect::from_min_size(egui::pos2(x, rect.center().y - 7.0), Vec2::splat(14.0)),
            Ico::ChevronDown,
            p.muted,
            &p,
            enabled,
        );
    }
    resp
}

/// Separador vertical fino de barra de herramientas.
pub fn toolbar_separator(ui: &mut Ui) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 18.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, if p.dark { hex(0x393B40) } else { hex(0xDDDFE4) }));
}

/// Botón primario azul (Button.default).
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let p = pal(ui);
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE))
            .fill(p.accent)
            .stroke(Stroke::new(1.0, p.accent))
            .corner_radius(6)
            .min_size(vec2(72.0, 28.0)),
    )
}

/// Botón secundario (borde control-border).
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(text).corner_radius(6).min_size(vec2(72.0, 28.0)))
}

/// Badge redondeado gris (List.Tag).
pub fn paint_badge(ui: &Ui, left_center: egui::Pos2, text: &str) -> Rect {
    let p = pal(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.0), p.badge_fg);
    let rect = Rect::from_min_size(
        egui::pos2(left_center.x, left_center.y - 9.0),
        vec2(galley.size().x + 12.0, 18.0),
    );
    ui.painter().rect_filled(rect, 5.0, p.badge_bg);
    ui.painter().galley(egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0), galley, p.badge_fg);
    rect
}

/// Marco de una "isla": tarjeta redondeada con el fondo de tool window.
pub fn island_frame(p: &Palette) -> egui::Frame {
    egui::Frame::NONE.fill(p.island_bg).corner_radius(CornerRadius::same(10))
}

/// Isla que ocupa exactamente todo el espacio disponible del `ui` (p.ej. un panel
/// redimensionable): el contenido se recorta dentro y nunca agranda la isla ni el panel, así
/// que las esquinas redondeadas y el hueco entre islas se mantienen a cualquier tamaño.
pub fn fixed_island<R>(ui: &mut Ui, p: &Palette, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, CornerRadius::same(10), p.island_bg);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let r = add_contents(&mut child);
    ui.advance_cursor_after_rect(rect);
    r
}

/// Fila de alto fijo que no puede ensanchar a su padre: lo que no cabe se recorta.
pub fn clipped_row<R>(ui: &mut Ui, height: f32, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width().max(0.0), height));
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let r = add_contents(&mut child);
    ui.advance_cursor_after_rect(rect);
    r
}

/// Mitad del hueco entre islas (Island.borderWidth = 6).
pub const ISLAND_GAP: i8 = 3;

/// Pestaña redondeada (EditorTabs / pestañas de tool window). Devuelve la respuesta de la
/// pestaña y si se pulsó su "×".
pub fn tab(ui: &mut Ui, selected: bool, ico: Option<Ico>, text: &str, closable: bool) -> (Response, bool) {
    let p = pal(ui);
    let font = FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, p.text);
    let icon_w = if ico.is_some() { 22.0 } else { 0.0 };
    let close_w = if closable { 22.0 } else { 0.0 };
    let w = 10.0 + icon_w + galley.size().x + close_w + 10.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    if selected {
        ui.painter().rect(rect, 6.0, p.tab_selected_bg, Stroke::new(1.0, p.tab_selected_border), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, p.hover);
    }
    let mut x = rect.left() + 10.0;
    if let Some(ico) = ico {
        icons::paint(ui.painter(), Rect::from_min_size(egui::pos2(x, rect.center().y - 8.0), Vec2::splat(16.0)), ico, p.icon, &p, true);
        x += icon_w;
    }
    ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley.clone(), p.text);
    x += galley.size().x + 4.0;
    let mut close = false;
    if closable {
        let cr = Rect::from_min_size(egui::pos2(x, rect.center().y - 8.0), Vec2::splat(16.0));
        let cresp = ui.interact(cr, resp.id.with("close"), Sense::click());
        if cresp.hovered() {
            ui.painter().rect_filled(cr.expand(1.0), 4.0, p.hover);
        }
        if selected || resp.hovered() || cresp.hovered() {
            icons::paint(ui.painter(), cr.shrink(2.0), Ico::Close, p.muted, &p, true);
        }
        close = cresp.clicked();
    }
    (resp, close)
}
