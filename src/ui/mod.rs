//! Componentes de interfaz.

pub mod connection_dialog;
pub mod console;
pub mod editing;
pub mod explorer;
pub mod icons;
pub mod results;
pub mod theme;

use crate::db::Driver;

/// Estado resumido de una conexión, para componentes que no necesitan el detalle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnStatusKind {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

/// Resumen de una conexión para los selectores de las consolas.
#[derive(Clone, Debug)]
pub struct ConnSummary {
    pub id: crate::worker::ConnId,
    pub name: String,
    pub driver: Driver,
    pub status: ConnStatusKind,
    /// Bases conocidas (vacío si no está conectada).
    pub databases: Vec<String>,
    /// Base por defecto de la conexión (si está conectada).
    pub default_db: Option<String>,
}

impl ConnSummary {
    /// Etiqueta "conexión · base" (SQLite solo tiene "main": se omite).
    pub fn label(&self, db: Option<&str>) -> String {
        match db.or(self.default_db.as_deref()) {
            Some(d) if self.driver != Driver::Sqlite => format!("{} · {d}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// Índice de char -> índice de byte (acotado al final del texto).
pub fn char_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(text.len())
}

/// Índice de byte -> índice de char.
pub fn byte_to_char(text: &str, byte_idx: usize) -> usize {
    let b = byte_idx.min(text.len());
    text.char_indices().take_while(|(i, _)| *i < b).count()
}

/// Entrecomilla un identificador según el dialecto.
pub fn quote_ident(driver: Driver, name: &str) -> String {
    match driver {
        Driver::MsSql => format!("[{}]", name.replace(']', "]]")),
        Driver::Postgres | Driver::Sqlite => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Entrecomilla solo si hace falta (identificadores simples en minúsculas quedan igual).
pub fn quote_ident_if_needed(driver: Driver, name: &str) -> String {
    let simple = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && (driver != Driver::Postgres || !name.chars().any(|c| c.is_ascii_uppercase()))
        && !crate::sql::KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(name));
    if simple { name.to_string() } else { quote_ident(driver, name) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_byte_roundtrip() {
        let t = "añb€c";
        assert_eq!(char_to_byte(t, 0), 0);
        assert_eq!(char_to_byte(t, 2), 3);
        assert_eq!(char_to_byte(t, 4), 7);
        assert_eq!(char_to_byte(t, 99), t.len());
        assert_eq!(byte_to_char(t, 7), 4);
        assert_eq!(byte_to_char(t, t.len()), 5);
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_ident(Driver::MsSql, "a]b"), "[a]]b]");
        assert_eq!(quote_ident(Driver::Postgres, "a\"b"), "\"a\"\"b\"");
    }
}
