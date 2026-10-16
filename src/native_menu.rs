//! Menú nativo de la barra de menús de macOS (crate `muda`).
//!
//! **Estrategia de atajos:** el menú nativo es quien dispara las acciones. Un acelerador de
//! menú consume la pulsación en `-[NSApplication sendEvent:]` antes de que llegue a la vista
//! de winit, así que egui nunca ve esa tecla (no hay doble ejecución) y los atajos duplicados
//! de egui quedan muertos: hay que quitarlos de `DataCraftApp::shortcuts`.
//!
//! **Edición:** la vista de winit no implementa `copy:`/`paste:`/`undo:`…, y muda desactiva el
//! autoenable de los menús, así que los `PredefinedMenuItem` de edición se comerían ⌘C/⌘V sin
//! que nadie los atendiera y egui dejaría de copiar/pegar. Por eso Edición usa ítems propios y
//! [`NativeMenu::poll`] reenvía la acción a egui tal y como la generaría egui-winit: `Event::Copy`
//! / `Event::Cut`, `ViewportCommand::RequestPaste` (egui-winit lee el portapapeles y emite
//! `Event::Paste` en el frame siguiente) y `Event::Key` con ⌘ para deshacer, rehacer y
//! seleccionar todo. Para que esos eventos lleguen en el mismo frame, llamar a `poll` desde
//! `App::logic` (antes de pintar la UI).
//!
//! **Salir:** ítem propio (no `terminate:`) para que la app cierre con
//! `ViewportCommand::Close` y pueda guardar estado / avisar de cambios pendientes.

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::cell::Cell;
use std::sync::mpsc::{Receiver, channel};

const HELP_URL: &str = "https://github.com/javy-fernandez/datacraft";

/// Acción elegida en el menú nativo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    // DataCraft
    About,
    Settings,
    Quit,
    // Archivo
    NewConnection,
    NewConsole,
    OpenSql,
    SaveSqlAs,
    CloseTab,
    // Edición (las gestiona `poll`; solo `Find` llega a la app)
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    // Ver
    ToggleExplorer,
    ToggleResults,
    ThemeLight,
    ThemeDark,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    FontBigger,
    FontSmaller,
    FontReset,
    // Ejecutar
    Run,
    RunAll,
    SaveChanges,
    Reload,
    // Ayuda
    Help,
}

impl MenuCommand {
    const ALL: [MenuCommand; 30] = [
        Self::About,
        Self::Settings,
        Self::Quit,
        Self::NewConnection,
        Self::NewConsole,
        Self::OpenSql,
        Self::SaveSqlAs,
        Self::CloseTab,
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
        Self::Find,
        Self::ToggleExplorer,
        Self::ToggleResults,
        Self::ThemeLight,
        Self::ThemeDark,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomReset,
        Self::FontBigger,
        Self::FontSmaller,
        Self::FontReset,
        Self::Run,
        Self::RunAll,
        Self::SaveChanges,
        Self::Reload,
        Self::Help,
    ];

    /// Id del ítem de muda (`Debug` del variante, único por construcción).
    fn id(self) -> String {
        format!("datacraft.{self:?}")
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    /// Comandos de edición que `poll` reenvía a egui en vez de devolverlos.
    fn is_text_edit(self) -> bool {
        matches!(self, Self::Undo | Self::Redo | Self::Cut | Self::Copy | Self::Paste | Self::SelectAll)
    }
}

/// Menú nativo instalado en NSApp. Mantenerlo vivo mientras dure la app.
pub struct NativeMenu {
    _menu: Menu,
    ctx: egui::Context,
    rx: Receiver<MenuCommand>,
    theme_light: CheckMenuItem,
    theme_dark: CheckMenuItem,
    explorer: CheckMenuItem,
    results: CheckMenuItem,
    /// Último estado sincronizado (dark, explorer, results) para no tocar AppKit cada frame.
    synced: Cell<Option<(bool, bool, bool)>>,
}

impl NativeMenu {
    /// Crea e instala el menú en NSApp. Llamar una vez, desde el hilo principal y con la app
    /// ya lanzada (p.ej. en `DataCraftApp::new` con `&cc.egui_ctx`): sustituye al menú por
    /// defecto de winit. Devuelve `None` si no se está en el hilo principal o si falla.
    pub fn install(ctx: &egui::Context) -> Option<Self> {
        if std::thread::current().name() != Some("main") {
            return None;
        }
        let (menu, theme_light, theme_dark, explorer, results) = match build() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("menú nativo: {e}");
                return None;
            }
        };
        menu.init_for_nsapp();

        let (tx, rx) = channel();
        let repaint = ctx.clone();
        // Solo se puede fijar una vez por proceso (OnceLock en muda).
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some(cmd) = MenuCommand::from_id(e.id().as_ref()) {
                let _ = tx.send(cmd);
                repaint.request_repaint();
            }
        }));

        Some(Self { _menu: menu, ctx: ctx.clone(), rx, theme_light, theme_dark, explorer, results, synced: Cell::new(None) })
    }

    /// Comandos pulsados desde el último frame (no bloquea). Los de edición (deshacer, rehacer,
    /// cortar, copiar, pegar, seleccionar todo) se reenvían a egui aquí y no se devuelven.
    pub fn poll(&self) -> Vec<MenuCommand> {
        let mut out = Vec::new();
        for cmd in self.rx.try_iter() {
            // Un clic en un CheckMenuItem invierte su check: resincronizar en el próximo `sync`.
            self.synced.set(None);
            if cmd.is_text_edit() {
                forward_edit(&self.ctx, cmd);
            } else {
                out.push(cmd);
            }
        }
        out
    }

    /// Sincroniza los checks con el estado de la app (barato: solo toca AppKit si cambia).
    pub fn sync(&self, dark: bool, explorer_visible: bool, results_visible: bool) {
        let state = (dark, explorer_visible, results_visible);
        if self.synced.get() == Some(state) {
            return;
        }
        self.theme_light.set_checked(!dark);
        self.theme_dark.set_checked(dark);
        self.explorer.set_checked(explorer_visible);
        self.results.set_checked(results_visible);
        self.synced.set(Some(state));
    }
}

/// Abre la ayuda (README del repositorio) en el navegador.
pub fn open_help() {
    let _ = std::process::Command::new("open").arg(HELP_URL).spawn();
}

/// Reenvía a egui una acción de edición como si viniera del teclado.
fn forward_edit(ctx: &egui::Context, cmd: MenuCommand) {
    use egui::{Event, Key, Modifiers as M};
    let cmd_mods = M::MAC_CMD | M::COMMAND;
    let key = |key: Key, modifiers: M| Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers };
    let event = match cmd {
        MenuCommand::Copy => Event::Copy,
        MenuCommand::Cut => Event::Cut,
        MenuCommand::Paste => {
            // egui-winit lee el portapapeles y emite `Event::Paste` en el frame siguiente.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            ctx.request_repaint();
            return;
        }
        MenuCommand::Undo => key(Key::Z, cmd_mods),
        MenuCommand::Redo => key(Key::Z, cmd_mods | M::SHIFT),
        MenuCommand::SelectAll => key(Key::A, cmd_mods),
        _ => return,
    };
    ctx.input_mut(|i| i.events.push(event));
}

type Built = (Menu, CheckMenuItem, CheckMenuItem, CheckMenuItem, CheckMenuItem);

fn build() -> muda::Result<Built> {
    let cmd = Modifiers::META;
    let acc = |mods: Modifiers, code: Code| Some(Accelerator::new(mods, code));
    let item = |c: MenuCommand, text: &str, a: Option<Accelerator>| MenuItem::with_id(c.id(), text, true, a);
    let check = |c: MenuCommand, text: &str| CheckMenuItem::with_id(c.id(), text, true, false, None);
    let sep = PredefinedMenuItem::separator;

    let app_menu = Submenu::with_items(
        "DataCraft",
        true,
        &[
            &item(MenuCommand::About, "Acerca de DataCraft", None),
            &sep(),
            &item(MenuCommand::Settings, "Ajustes…", acc(cmd, Code::Comma)),
            &sep(),
            &PredefinedMenuItem::services(Some("Servicios")),
            &sep(),
            &PredefinedMenuItem::hide(Some("Ocultar DataCraft")),
            &PredefinedMenuItem::hide_others(Some("Ocultar otros")),
            &PredefinedMenuItem::show_all(Some("Mostrar todo")),
            &sep(),
            &item(MenuCommand::Quit, "Salir de DataCraft", acc(cmd, Code::KeyQ)),
        ],
    )?;

    let file_menu = Submenu::with_items(
        "Archivo",
        true,
        &[
            &item(MenuCommand::NewConnection, "Nueva conexión…", None),
            &item(MenuCommand::NewConsole, "Nueva consola", acc(cmd | Modifiers::SHIFT, Code::KeyN)),
            &item(MenuCommand::OpenSql, "Abrir script SQL…", acc(cmd, Code::KeyO)),
            &item(MenuCommand::SaveSqlAs, "Guardar consola como…", acc(cmd | Modifiers::SHIFT, Code::KeyS)),
            &sep(),
            &item(MenuCommand::CloseTab, "Cerrar pestaña", acc(cmd, Code::KeyW)),
        ],
    )?;

    let edit_menu = Submenu::with_items(
        "Edición",
        true,
        &[
            &item(MenuCommand::Undo, "Deshacer", acc(cmd, Code::KeyZ)),
            &item(MenuCommand::Redo, "Rehacer", acc(cmd | Modifiers::SHIFT, Code::KeyZ)),
            &sep(),
            &item(MenuCommand::Cut, "Cortar", acc(cmd, Code::KeyX)),
            &item(MenuCommand::Copy, "Copiar", acc(cmd, Code::KeyC)),
            &item(MenuCommand::Paste, "Pegar", acc(cmd, Code::KeyV)),
            &item(MenuCommand::SelectAll, "Seleccionar todo", acc(cmd, Code::KeyA)),
            &sep(),
            &item(MenuCommand::Find, "Buscar…", acc(cmd, Code::KeyF)),
        ],
    )?;

    let theme_light = check(MenuCommand::ThemeLight, "Islands Light");
    let theme_dark = check(MenuCommand::ThemeDark, "Islands Dark");
    let explorer = check(MenuCommand::ToggleExplorer, "Database Explorer");
    let results = check(MenuCommand::ToggleResults, "Panel de resultados");
    let theme_menu = Submenu::with_items("Tema", true, &[&theme_light, &theme_dark])?;
    let zoom_menu = Submenu::with_items(
        "Zoom IDE",
        true,
        &[
            &item(MenuCommand::ZoomIn, "Acercar", acc(cmd, Code::Equal)),
            &item(MenuCommand::ZoomOut, "Alejar", acc(cmd, Code::Minus)),
            &item(MenuCommand::ZoomReset, "Restablecer", acc(cmd, Code::Digit0)),
        ],
    )?;
    let font_menu = Submenu::with_items(
        "Fuente del editor",
        true,
        &[
            &item(MenuCommand::FontBigger, "Aumentar", acc(cmd | Modifiers::ALT, Code::Equal)),
            &item(MenuCommand::FontSmaller, "Reducir", acc(cmd | Modifiers::ALT, Code::Minus)),
            &item(MenuCommand::FontReset, "Restablecer", None),
        ],
    )?;
    let view_menu = Submenu::with_items(
        "Ver",
        true,
        &[&explorer, &results, &sep(), &theme_menu, &zoom_menu, &font_menu],
    )?;

    let run_menu = Submenu::with_items(
        "Ejecutar",
        true,
        &[
            &item(MenuCommand::Run, "Ejecutar sentencia", acc(cmd, Code::Enter)),
            &item(MenuCommand::RunAll, "Ejecutar todo", acc(cmd | Modifiers::SHIFT, Code::Enter)),
            &sep(),
            &item(MenuCommand::SaveChanges, "Guardar cambios del grid", acc(cmd, Code::KeyS)),
            &item(MenuCommand::Reload, "Recargar datos", acc(cmd, Code::KeyR)),
        ],
    )?;

    let window_menu = Submenu::with_items(
        "Ventana",
        true,
        &[
            &PredefinedMenuItem::minimize(Some("Minimizar")),
            &PredefinedMenuItem::maximize(Some("Zoom")),
            &sep(),
            &PredefinedMenuItem::bring_all_to_front(Some("Traer todo al frente")),
        ],
    )?;
    window_menu.set_as_windows_menu_for_nsapp();

    let help_menu = Submenu::with_items("Ayuda", true, &[&item(MenuCommand::Help, "Ayuda de DataCraft", None)])?;
    help_menu.set_as_help_menu_for_nsapp();

    let menu = Menu::with_items(&[&app_menu, &file_menu, &edit_menu, &view_menu, &run_menu, &window_menu, &help_menu])?;
    Ok((menu, theme_light, theme_dark, explorer, results))
}

#[cfg(test)]
mod tests {
    use super::MenuCommand;

    #[test]
    fn ids_round_trip() {
        for c in MenuCommand::ALL {
            assert_eq!(MenuCommand::from_id(&c.id()), Some(c));
        }
        assert_eq!(MenuCommand::from_id("otro"), None);
    }
}
