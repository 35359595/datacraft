//! Edición de datos en el grid (lógica pura, sin UI): cambios pendientes, detección de
//! consultas editables sobre una única tabla y conversión a `edit::Change`.

use crate::db::{Cell, ColumnInfo, ObjectKind, ResultSet, SchemaInfo};
use crate::edit::{Change, is_lossy_display};
use std::collections::{BTreeMap, BTreeSet};

/// Tabla destino de un resultado editable.
#[derive(Clone, Debug, PartialEq)]
pub struct EditTarget {
    pub schema: String,
    pub table: String,
    /// Nombre real (en la tabla) de cada columna del resultado.
    pub col_names: Vec<String>,
    /// Columnas del resultado que identifican una fila (PK o, sin PK, todas las no binarias).
    pub key_cols: Vec<usize>,
    /// Columnas binarias: no se editan (el grid las muestra en hex).
    pub binary_cols: Vec<bool>,
    pub has_pk: bool,
}

/// Fila del grid: original del result set o nueva (índice en `EditState::inserted`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Hash)]
pub enum RowRef {
    Existing(usize),
    New(usize),
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Snapshot {
    changes: BTreeMap<(usize, usize), Cell>,
    inserted: Vec<Vec<Option<Cell>>>,
    deleted: BTreeSet<usize>,
}

/// Cambios pendientes de un result set. Las filas existentes se indexan por su posición
/// ORIGINAL en `ResultSet::rows` (no por la posición visual tras ordenar).
#[derive(Clone, Debug, Default)]
pub struct EditState {
    /// (fila original, columna) -> valor nuevo.
    pub changes: BTreeMap<(usize, usize), Cell>,
    /// Filas nuevas; `None` = columna sin tocar (no va en el INSERT).
    pub inserted: Vec<Vec<Option<Cell>>>,
    /// Filas originales marcadas para borrar.
    pub deleted: BTreeSet<usize>,
    history: Vec<Snapshot>,
}

const MAX_HISTORY: usize = 500;

/// Tipos que el grid muestra como hex y no se pueden editar como texto.
pub fn is_binary_type(data_type: &str) -> bool {
    let t = data_type.to_lowercase();
    ["blob", "binary", "bytea", "image", "rowversion"].iter().any(|k| t.contains(k))
}

impl EditState {
    pub fn is_dirty(&self) -> bool {
        !self.changes.is_empty() || !self.inserted.is_empty() || !self.deleted.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { changes: self.changes.clone(), inserted: self.inserted.clone(), deleted: self.deleted.clone() }
    }

    /// Guarda el estado actual para poder deshacer el siguiente cambio.
    fn checkpoint(&mut self) {
        let s = self.snapshot();
        self.history.push(s);
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.history.pop() {
            Some(s) => {
                self.changes = s.changes;
                self.inserted = s.inserted;
                self.deleted = s.deleted;
                true
            }
            None => false,
        }
    }

    pub fn revert_all(&mut self) {
        if self.is_dirty() {
            self.checkpoint();
            self.changes.clear();
            self.inserted.clear();
            self.deleted.clear();
        }
    }

    /// Descarta todo, también el historial (tras guardar o al recargar).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Valor actual de una celda. `None` = celda de una fila nueva sin tocar (DEFAULT).
    pub fn current(&self, rs: &ResultSet, row: RowRef, col: usize) -> Option<Cell> {
        match row {
            RowRef::Existing(r) => match self.changes.get(&(r, col)) {
                Some(v) => Some(v.clone()),
                None => rs.rows.get(r).and_then(|x| x.get(col)).cloned(),
            },
            RowRef::New(i) => self.inserted.get(i).and_then(|x| x.get(col)).cloned().flatten(),
        }
    }

    pub fn is_modified(&self, row: RowRef, col: usize) -> bool {
        match row {
            RowRef::Existing(r) => self.changes.contains_key(&(r, col)),
            RowRef::New(i) => self.inserted.get(i).and_then(|x| x.get(col)).is_some_and(|v| v.is_some()),
        }
    }

    pub fn is_deleted(&self, row: RowRef) -> bool {
        matches!(row, RowRef::Existing(r) if self.deleted.contains(&r))
    }

    /// true si la fila se puede identificar (su clave no tiene valores recortados).
    pub fn row_identifiable(&self, rs: &ResultSet, target: &EditTarget, row: RowRef) -> bool {
        match row {
            RowRef::New(_) => true,
            RowRef::Existing(r) => rs
                .rows
                .get(r)
                .is_some_and(|vals| target.key_cols.iter().all(|&k| vals.get(k).is_some_and(|v| !is_lossy_display(v)))),
        }
    }

    pub fn cell_editable(&self, rs: &ResultSet, target: &EditTarget, row: RowRef, col: usize) -> bool {
        if col >= target.col_names.len() || target.binary_cols.get(col).copied().unwrap_or(true) {
            return false;
        }
        match row {
            RowRef::New(i) => i < self.inserted.len(),
            RowRef::Existing(r) => {
                !self.deleted.contains(&r)
                    && self.row_identifiable(rs, target, row)
                    && rs.rows.get(r).and_then(|x| x.get(col)).is_some_and(|v| !is_lossy_display(v))
            }
        }
    }

    /// Asigna un valor. Si coincide con el original, la celda deja de estar modificada.
    /// Devuelve true si cambió algo.
    pub fn set_value(&mut self, rs: &ResultSet, row: RowRef, col: usize, value: Cell) -> bool {
        match row {
            RowRef::Existing(r) => {
                let Some(original) = rs.rows.get(r).and_then(|x| x.get(col)) else { return false };
                let current = self.changes.get(&(r, col)).unwrap_or(original);
                if *current == value {
                    return false;
                }
                self.checkpoint();
                if *original == value {
                    self.changes.remove(&(r, col));
                } else {
                    self.changes.insert((r, col), value);
                }
                true
            }
            RowRef::New(i) => {
                let Some(slot) = self.inserted.get(i).and_then(|x| x.get(col)) else { return false };
                if slot.as_ref() == Some(&value) {
                    return false;
                }
                self.checkpoint();
                if let Some(slot) = self.inserted.get_mut(i).and_then(|x| x.get_mut(col)) {
                    *slot = Some(value);
                }
                true
            }
        }
    }

    /// Deshace los cambios de una celda (o el borrado de su fila).
    pub fn revert_cell(&mut self, row: RowRef, col: usize) -> bool {
        match row {
            RowRef::Existing(r) => {
                if !self.changes.contains_key(&(r, col)) && !self.deleted.contains(&r) {
                    return false;
                }
                self.checkpoint();
                if !self.deleted.remove(&r) {
                    self.changes.remove(&(r, col));
                }
                true
            }
            RowRef::New(i) => {
                if !self.is_modified(row, col) {
                    return false;
                }
                self.checkpoint();
                if let Some(slot) = self.inserted.get_mut(i).and_then(|x| x.get_mut(col)) {
                    *slot = None;
                }
                true
            }
        }
    }

    /// Añade una fila vacía al final. Devuelve su índice en `inserted`.
    pub fn add_row(&mut self, n_cols: usize) -> usize {
        self.checkpoint();
        self.inserted.push(vec![None; n_cols]);
        self.inserted.len() - 1
    }

    /// Marca filas existentes para borrar y quita las nuevas. Devuelve true si cambió algo.
    pub fn delete_rows(&mut self, rs: &ResultSet, target: &EditTarget, rows: &[RowRef]) -> bool {
        let existing: Vec<usize> = rows
            .iter()
            .filter_map(|r| match r {
                RowRef::Existing(i) if !self.deleted.contains(i) && self.row_identifiable(rs, target, *r) => Some(*i),
                _ => None,
            })
            .collect();
        let mut new: Vec<usize> = rows
            .iter()
            .filter_map(|r| match r {
                RowRef::New(i) if *i < self.inserted.len() => Some(*i),
                _ => None,
            })
            .collect();
        if existing.is_empty() && new.is_empty() {
            return false;
        }
        self.checkpoint();
        self.deleted.extend(existing);
        new.sort_unstable();
        new.dedup();
        for i in new.into_iter().rev() {
            self.inserted.remove(i);
        }
        true
    }

    /// Número de sentencias que generará el guardado.
    pub fn pending_count(&self) -> usize {
        let updated: BTreeSet<usize> =
            self.changes.keys().map(|(r, _)| *r).filter(|r| !self.deleted.contains(r)).collect();
        updated.len() + self.inserted.len() + self.deleted.len()
    }

    /// Cambios en orden: updates, inserts y deletes.
    pub fn build_changes(&self, rs: &ResultSet, target: &EditTarget) -> Result<Vec<Change>, String> {
        let key_of = |r: usize| -> Result<Vec<(String, Cell)>, String> {
            let vals = rs.rows.get(r).ok_or_else(|| format!("Fila {} inexistente", r + 1))?;
            target
                .key_cols
                .iter()
                .map(|&k| {
                    let v = vals.get(k).cloned().ok_or_else(|| "Columna clave inexistente".to_string())?;
                    if is_lossy_display(&v) {
                        return Err(format!("La fila {} no se puede identificar (valor binario recortado)", r + 1));
                    }
                    let name = target.col_names.get(k).cloned().ok_or_else(|| "Columna clave inexistente".to_string())?;
                    Ok((name, v))
                })
                .collect()
        };
        if target.key_cols.is_empty() && (!self.changes.is_empty() || !self.deleted.is_empty()) {
            return Err("La tabla no tiene columnas con las que identificar las filas".into());
        }
        let mut out = Vec::new();
        let mut by_row: BTreeMap<usize, Vec<(String, Cell)>> = BTreeMap::new();
        for ((r, c), v) in &self.changes {
            if self.deleted.contains(r) {
                continue;
            }
            let name = target.col_names.get(*c).cloned().ok_or_else(|| "Columna inexistente".to_string())?;
            by_row.entry(*r).or_default().push((name, v.clone()));
        }
        for (r, set) in by_row {
            out.push(Change::Update { key: key_of(r)?, set });
        }
        for row in &self.inserted {
            let values = row
                .iter()
                .enumerate()
                .filter_map(|(c, v)| Some((target.col_names.get(c)?.clone(), v.clone()?)))
                .collect();
            out.push(Change::Insert { values });
        }
        for r in &self.deleted {
            out.push(Change::Delete { key: key_of(*r)? });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- consulta editable

/// Tabla de una consulta trivial (`SELECT cols FROM t [WHERE…] [ORDER BY…]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTable {
    pub schema: Option<String>,
    pub table: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    /// Identificador entrecomillado ("x", [x], `x`).
    Quoted(String),
    Str,
    Num,
    Punct(char),
}

fn tokenize(sql: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if c == '\'' {
            i += 1;
            loop {
                match chars.get(i) {
                    None => return None,
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => i += 2,
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(_) => i += 1,
                }
            }
            out.push(Tok::Str);
        } else if c == '"' || c == '[' || c == '`' {
            let close = match c {
                '[' => ']',
                x => x,
            };
            let mut s = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return None,
                    Some(&x) if x == close && chars.get(i + 1) == Some(&close) => {
                        s.push(x);
                        i += 2;
                    }
                    Some(&x) if x == close => {
                        i += 1;
                        break;
                    }
                    Some(&x) => {
                        s.push(x);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Quoted(s));
        } else if c.is_alphabetic() || c == '_' || c == '@' || c == '#' {
            let mut s = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '$' | '@' | '#')) {
                s.push(chars[i]);
                i += 1;
            }
            out.push(Tok::Word(s));
        } else if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            out.push(Tok::Num);
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    Some(out)
}

fn is_kw(t: &Tok, kw: &str) -> bool {
    matches!(t, Tok::Word(w) if w.eq_ignore_ascii_case(kw))
}

fn ident(t: &Tok) -> Option<&str> {
    match t {
        Tok::Word(w) => Some(w),
        Tok::Quoted(q) => Some(q),
        _ => None,
    }
}

/// Palabras que hacen que una consulta deje de ser "trivial sobre una tabla".
const NOT_TRIVIAL: &[&str] = &[
    "JOIN", "GROUP", "HAVING", "DISTINCT", "UNION", "INTERSECT", "EXCEPT", "INTO", "WITH", "SELECT", "APPLY", "PIVOT",
    "UNPIVOT", "OVER", "WINDOW", "LATERAL", "RETURNING", "FOR",
];

/// Detecta `SELECT [TOP n] (* | col, …) FROM [esquema.]tabla [alias] [WHERE …] [ORDER BY …]
/// [LIMIT/OFFSET/FETCH …]`. Devuelve la tabla, o `None` si la consulta no es de ese tipo.
pub fn single_table_query(sql: &str) -> Option<QueryTable> {
    let mut toks = tokenize(sql)?;
    while toks.last() == Some(&Tok::Punct(';')) {
        toks.pop();
    }
    if toks.contains(&Tok::Punct(';')) {
        return None;
    }
    let mut i = 0;
    if !is_kw(toks.first()?, "SELECT") {
        return None;
    }
    i += 1;
    // El resto no puede contener otra SELECT ni construcciones de varias tablas.
    if toks[i..].iter().any(|t| NOT_TRIVIAL.iter().any(|k| is_kw(t, k))) {
        return None;
    }
    if is_kw(toks.get(i)?, "ALL") {
        i += 1;
    }
    if is_kw(toks.get(i)?, "TOP") {
        i += 1;
        match toks.get(i)? {
            Tok::Num => i += 1,
            Tok::Punct('(') => {
                if toks.get(i + 1) != Some(&Tok::Num) || toks.get(i + 2) != Some(&Tok::Punct(')')) {
                    return None;
                }
                i += 3;
            }
            _ => return None,
        }
    }
    // Lista de columnas: `*`, `t.*`, `col`, `t.col` separadas por comas.
    loop {
        let start = i;
        loop {
            match toks.get(i)? {
                Tok::Punct('*') => {
                    i += 1;
                    break;
                }
                t if ident(t).is_some() && !is_kw(t, "FROM") => {
                    i += 1;
                    if toks.get(i) == Some(&Tok::Punct('.')) {
                        i += 1;
                    } else {
                        break;
                    }
                }
                _ => return None,
            }
        }
        if i == start {
            return None;
        }
        match toks.get(i)? {
            Tok::Punct(',') => i += 1,
            t if is_kw(t, "FROM") => break,
            _ => return None,
        }
    }
    i += 1; // FROM
    let mut parts: Vec<String> = Vec::new();
    loop {
        let name = ident(toks.get(i)?)?;
        parts.push(name.to_string());
        i += 1;
        if toks.get(i) == Some(&Tok::Punct('.')) {
            i += 1;
        } else {
            break;
        }
    }
    if parts.len() > 3 {
        return None;
    }
    // Alias opcional.
    let clause_kw = ["WHERE", "ORDER", "LIMIT", "OFFSET", "FETCH"];
    if let Some(t) = toks.get(i)
        && !clause_kw.iter().any(|k| is_kw(t, k))
    {
        if is_kw(t, "AS") {
            i += 1;
        }
        ident(toks.get(i)?)?;
        i += 1;
    }
    // Lo que queda solo pueden ser cláusulas de filtro/orden/límite, sin más tablas.
    match toks.get(i) {
        None => {}
        Some(t) if clause_kw.iter().any(|k| is_kw(t, k)) => {}
        Some(_) => return None,
    }
    let table = parts.pop()?;
    let schema = parts.pop();
    Some(QueryTable { schema, table })
}

/// Construye el destino de edición de un resultado sobre `schema.table` (ya resuelta).
/// Devuelve el motivo si no es editable.
pub fn target_for(result_cols: &[String], schema: &str, kind: ObjectKind, table: &str, columns: &[ColumnInfo]) -> Result<EditTarget, String> {
    if kind != ObjectKind::Table {
        return Err("Las vistas son de solo lectura".into());
    }
    if result_cols.is_empty() {
        return Err("Sin columnas".into());
    }
    let mut col_names = Vec::with_capacity(result_cols.len());
    let mut binary_cols = Vec::with_capacity(result_cols.len());
    for rc in result_cols {
        let found = columns
            .iter()
            .find(|c| c.name == *rc)
            .or_else(|| columns.iter().find(|c| c.name.eq_ignore_ascii_case(rc)))
            .ok_or_else(|| format!("La columna «{rc}» no pertenece a {table}"))?;
        if col_names.contains(&found.name) {
            return Err(format!("La columna «{rc}» aparece dos veces"));
        }
        col_names.push(found.name.clone());
        binary_cols.push(is_binary_type(&found.data_type));
    }
    let pk: Vec<&ColumnInfo> = columns.iter().filter(|c| c.primary_key).collect();
    let has_pk = !pk.is_empty();
    let key_cols: Vec<usize> = if has_pk {
        let mut keys = Vec::new();
        for c in pk {
            let idx = col_names
                .iter()
                .position(|n| *n == c.name)
                .ok_or_else(|| format!("El resultado no incluye la clave primaria «{}»", c.name))?;
            keys.push(idx);
        }
        keys
    } else {
        (0..col_names.len()).filter(|&i| !binary_cols[i]).collect()
    };
    if key_cols.is_empty() {
        return Err("No hay columnas con las que identificar las filas".into());
    }
    Ok(EditTarget { schema: schema.to_string(), table: table.to_string(), col_names, key_cols, binary_cols, has_pk })
}

/// Destino de edición de un resultado de consola, si la consulta es trivial sobre una tabla
/// conocida de `schemas`.
pub fn console_target(statement: &str, result_cols: &[String], schemas: &[SchemaInfo]) -> Result<EditTarget, String> {
    let q = single_table_query(statement).ok_or_else(|| "Consulta de solo lectura (no es un SELECT simple sobre una tabla)".to_string())?;
    let mut found = Vec::new();
    for s in schemas {
        if q.schema.as_deref().is_some_and(|x| !x.eq_ignore_ascii_case(&s.name)) {
            continue;
        }
        for t in &s.tables {
            if t.name.eq_ignore_ascii_case(&q.table) {
                found.push((s, t));
            }
        }
    }
    // Varias coincidencias: preferir la exacta y luego el esquema por defecto.
    if found.len() > 1 {
        let exact: Vec<_> = found.iter().copied().filter(|(_, t)| t.name == q.table).collect();
        if !exact.is_empty() {
            found = exact;
        }
    }
    if found.len() > 1 {
        let default: Vec<_> =
            found.iter().copied().filter(|(s, _)| ["dbo", "public", "main"].contains(&s.name.as_str())).collect();
        if default.len() == 1 {
            found = default;
        }
    }
    match found.as_slice() {
        [(s, t)] => target_for(result_cols, &s.name, t.kind, &t.name, &t.columns),
        [] => Err(format!("Tabla «{}» desconocida", q.table)),
        _ => Err(format!("Tabla «{}» ambigua", q.table)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TableInfo;

    fn col(name: &str, pk: bool) -> ColumnInfo {
        ColumnInfo { name: name.into(), data_type: "TEXT".into(), nullable: true, primary_key: pk }
    }

    fn rs(rows: Vec<Vec<Option<&str>>>) -> ResultSet {
        ResultSet {
            columns: vec!["id".into(), "name".into()],
            rows: rows.into_iter().map(|r| r.into_iter().map(|v| v.map(str::to_string)).collect()).collect(),
            ..Default::default()
        }
    }

    fn target(pk: bool) -> EditTarget {
        target_for(&["id".into(), "name".into()], "main", ObjectKind::Table, "users", &[col("id", pk), col("name", false)]).unwrap()
    }

    #[test]
    fn consultas_triviales() {
        let q = |s: &str| single_table_query(s);
        assert_eq!(q("SELECT * FROM users"), Some(QueryTable { schema: None, table: "users".into() }));
        assert_eq!(q("select id, name from public.users where id > 3 order by name desc, id;"), Some(QueryTable { schema: Some("public".into()), table: "users".into() }));
        assert_eq!(q("SELECT TOP (10) [id] FROM [dbo].[t] u WHERE u.id = 'a,b'"), Some(QueryTable { schema: Some("dbo".into()), table: "t".into() }));
        assert_eq!(q("SELECT u.* FROM \"users\" AS u LIMIT 5 OFFSET 2"), Some(QueryTable { schema: None, table: "users".into() }));
        assert!(q("SELECT o.id FROM orders o JOIN users u ON u.id = o.user_id").is_none());
        assert!(q("SELECT count(*) FROM users").is_none());
        assert!(q("SELECT DISTINCT name FROM users").is_none());
        assert!(q("SELECT name FROM users GROUP BY name").is_none());
        assert!(q("SELECT * FROM a, b").is_none());
        assert!(q("SELECT * FROM a UNION SELECT * FROM b").is_none());
        assert!(q("SELECT name AS n FROM users").is_none());
        assert!(q("SELECT * FROM users WHERE id IN (SELECT 1)").is_none());
        assert!(q("SELECT * FROM users; DELETE FROM users").is_none());
        assert!(q("UPDATE users SET x = 1").is_none());
        assert!(q("SELECT 1").is_none());
    }

    #[test]
    fn destino_de_consola() {
        let schemas = vec![SchemaInfo {
            name: "main".into(),
            tables: vec![TableInfo { name: "users".into(), kind: ObjectKind::Table, columns: vec![col("id", true), col("name", false)] }],
        }];
        let t = console_target("select name, id from users", &["name".into(), "id".into()], &schemas).unwrap();
        assert_eq!(t.key_cols, vec![1]);
        assert!(console_target("select name from users", &["name".into()], &schemas).is_err());
        assert!(console_target("select * from nope", &["id".into()], &schemas).is_err());
        assert!(console_target("select id, name from users", &["id".into(), "x".into()], &schemas).is_err());
    }

    #[test]
    fn construye_cambios_en_orden() {
        let rs = rs(vec![vec![Some("1"), Some("a")], vec![Some("2"), None]]);
        let t = target(true);
        let mut st = EditState::default();
        assert!(st.set_value(&rs, RowRef::Existing(1), 1, Some("b".into())));
        // Volver al original elimina el cambio.
        assert!(st.set_value(&rs, RowRef::Existing(0), 1, Some("z".into())));
        assert!(st.set_value(&rs, RowRef::Existing(0), 1, Some("a".into())));
        let n = st.add_row(2);
        st.set_value(&rs, RowRef::New(n), 1, Some("nuevo".into()));
        st.add_row(2);
        st.delete_rows(&rs, &t, &[RowRef::Existing(0)]);
        let ch = st.build_changes(&rs, &t).unwrap();
        let kv = |c: &str, v: Option<&str>| (c.to_string(), v.map(str::to_string));
        assert_eq!(
            ch,
            vec![
                Change::Update { key: vec![kv("id", Some("2"))], set: vec![kv("name", Some("b"))] },
                Change::Insert { values: vec![kv("name", Some("nuevo"))] },
                Change::Insert { values: vec![] },
                Change::Delete { key: vec![kv("id", Some("1"))] },
            ]
        );
        assert_eq!(st.pending_count(), 4);
        // Deshacer quita el borrado; revertir todo deja el estado limpio.
        assert!(st.undo());
        assert!(st.deleted.is_empty());
        st.revert_all();
        assert!(!st.is_dirty());
        assert!(st.undo());
        assert!(st.is_dirty());
    }

    #[test]
    fn sin_pk_usa_todas_las_columnas_y_evita_lossy() {
        let rs = rs(vec![vec![Some("1"), Some("0xABCD…")], vec![Some("2"), Some("x")]]);
        let t = target(false);
        assert!(!t.has_pk);
        assert_eq!(t.key_cols, vec![0, 1]);
        let mut st = EditState::default();
        assert!(!st.cell_editable(&rs, &t, RowRef::Existing(0), 0));
        assert!(st.cell_editable(&rs, &t, RowRef::Existing(1), 0));
        assert!(!st.delete_rows(&rs, &t, &[RowRef::Existing(0)]));
        st.set_value(&rs, RowRef::Existing(1), 0, None);
        let ch = st.build_changes(&rs, &t).unwrap();
        assert_eq!(
            ch,
            vec![Change::Update {
                key: vec![("id".into(), Some("2".into())), ("name".into(), Some("x".into()))],
                set: vec![("id".into(), None)]
            }]
        );
    }

    #[test]
    fn revertir_celda_y_filas_nuevas() {
        let rs = rs(vec![vec![Some("1"), Some("a")]]);
        let t = target(true);
        let mut st = EditState::default();
        let a = st.add_row(2);
        let b = st.add_row(2);
        st.set_value(&rs, RowRef::New(b), 0, Some("9".into()));
        assert!(st.revert_cell(RowRef::New(b), 0));
        assert!(!st.is_modified(RowRef::New(b), 0));
        st.delete_rows(&rs, &t, &[RowRef::New(a)]);
        assert_eq!(st.inserted.len(), 1);
        st.delete_rows(&rs, &t, &[RowRef::Existing(0)]);
        assert!(st.revert_cell(RowRef::Existing(0), 1));
        assert!(st.deleted.is_empty());
    }
}
