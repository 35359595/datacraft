//! Edición de datos en el grid: cambios pendientes y el SQL que los aplica.
//!
//! Los valores viajan como texto (igual que se leen) y se escriben como literales de
//! cadena; los tres motores convierten implícitamente `'42'` al tipo de la columna.

use crate::db::{Cell, Driver, Session};

/// Un cambio pendiente sobre una fila. `key` identifica la fila: las columnas de la PK
/// o, si la tabla no tiene, todas las columnas con sus valores originales.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Update { key: Vec<(String, Cell)>, set: Vec<(String, Cell)> },
    Insert { values: Vec<(String, Cell)> },
    Delete { key: Vec<(String, Cell)> },
}

/// Identificador entrecomillado según el dialecto.
pub fn quote_ident(driver: Driver, name: &str) -> String {
    match driver {
        Driver::MsSql => format!("[{}]", name.replace(']', "]]")),
        Driver::Postgres | Driver::Sqlite => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Literal SQL para un valor. `None` = NULL. SQL Server usa `N'..'` para no perder Unicode.
pub fn literal(driver: Driver, value: &Cell) -> String {
    match value {
        None => "NULL".into(),
        Some(v) => {
            let escaped = v.replace('\'', "''");
            match driver {
                Driver::MsSql => format!("N'{escaped}'"),
                Driver::Postgres | Driver::Sqlite => format!("'{escaped}'"),
            }
        }
    }
}

fn where_clause(driver: Driver, key: &[(String, Cell)]) -> String {
    key.iter()
        .map(|(col, val)| match val {
            None => format!("{} IS NULL", quote_ident(driver, col)),
            Some(_) => format!("{} = {}", quote_ident(driver, col), literal(driver, val)),
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Sentencia que aplica `change` sobre `schema.table`. Cada UPDATE/DELETE debe afectar
/// exactamente a una fila: `Session::apply_changes` lo comprueba y si no, deshace todo.
pub fn change_sql(driver: Driver, schema: &str, table: &str, change: &Change) -> String {
    let target = Session::quote_table(driver, schema, table);
    match change {
        Change::Update { key, set } => {
            let assignments = set
                .iter()
                .map(|(col, val)| format!("{} = {}", quote_ident(driver, col), literal(driver, val)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("UPDATE {target} SET {assignments} WHERE {}", where_clause(driver, key))
        }
        Change::Insert { values } => {
            if values.is_empty() {
                return format!("INSERT INTO {target} DEFAULT VALUES");
            }
            let cols = values.iter().map(|(c, _)| quote_ident(driver, c)).collect::<Vec<_>>().join(", ");
            let vals = values.iter().map(|(_, v)| literal(driver, v)).collect::<Vec<_>>().join(", ");
            format!("INSERT INTO {target} ({cols}) VALUES ({vals})")
        }
        Change::Delete { key } => format!("DELETE FROM {target} WHERE {}", where_clause(driver, key)),
    }
}

/// true para valores que el grid muestra recortados (BLOB/binario en hex con "…"):
/// no se pueden editar ni usar como clave porque no son el valor real.
pub fn is_lossy_display(value: &Cell) -> bool {
    value.as_deref().is_some_and(|v| v.starts_with("0x") && v.ends_with('…'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(c: &str, v: Option<&str>) -> (String, Cell) {
        (c.to_string(), v.map(str::to_string))
    }

    #[test]
    fn update_con_pk_y_comillas() {
        let ch = Change::Update { key: vec![kv("id", Some("7"))], set: vec![kv("name", Some("O'Brien")), kv("note", None)] };
        assert_eq!(
            change_sql(Driver::MsSql, "dbo", "users", &ch),
            "UPDATE [dbo].[users] SET [name] = N'O''Brien', [note] = NULL WHERE [id] = N'7'"
        );
        assert_eq!(
            change_sql(Driver::Postgres, "public", "users", &ch),
            "UPDATE \"public\".\"users\" SET \"name\" = 'O''Brien', \"note\" = NULL WHERE \"id\" = '7'"
        );
    }

    #[test]
    fn clave_con_null_usa_is_null() {
        let ch = Change::Delete { key: vec![kv("a", Some("1")), kv("b", None)] };
        assert_eq!(change_sql(Driver::Sqlite, "main", "t", &ch), "DELETE FROM \"t\" WHERE \"a\" = '1' AND \"b\" IS NULL");
    }

    #[test]
    fn insert_con_y_sin_valores() {
        let ch = Change::Insert { values: vec![kv("x", Some("1")), kv("y", None)] };
        assert_eq!(change_sql(Driver::Sqlite, "main", "t", &ch), "INSERT INTO \"t\" (\"x\", \"y\") VALUES ('1', NULL)");
        let empty = Change::Insert { values: vec![] };
        assert_eq!(change_sql(Driver::MsSql, "dbo", "t", &empty), "INSERT INTO [dbo].[t] DEFAULT VALUES");
    }

    #[test]
    fn identificadores_raros() {
        assert_eq!(quote_ident(Driver::MsSql, "a]b"), "[a]]b]");
        assert_eq!(quote_ident(Driver::Postgres, "a\"b"), "\"a\"\"b\"");
        assert!(is_lossy_display(&Some("0xFF…".into())));
        assert!(!is_lossy_display(&Some("0xFF".into())));
    }
}
