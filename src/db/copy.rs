//! "Copy Table To…" (DataGrip): copies a table's rows (all, a percentage or those matching a
//! filter) to another connection, creating the target table when it doesn't exist. It runs on
//! dedicated sessions, like the schema search, and inserts in one transaction (all or nothing).

use super::{Cell, ColumnInfo, ConnectionConfig, Driver, Session};
use crate::edit::literal;
use crate::i18n::tr;
use crate::ui::create_dialog::{NewColumn, create_table_sql};
use std::sync::Arc;
use tokio::sync::Notify;

/// Rows read at most: the copy keeps them in memory before inserting (the texts say 1,000,000).
pub const MAX_ROWS: usize = 1_000_000;
/// Rows per multi-row INSERT (SQL Server allows at most 1000).
const BATCH: usize = 100;

#[derive(Clone, Debug)]
pub struct CopyRequest {
    pub source_db: String,
    pub source_schema: String,
    pub source_table: String,
    /// Source columns to copy, in order (binary columns are left out by the dialog).
    pub columns: Vec<ColumnInfo>,
    /// Target name of each column (an existing table may spell them differently).
    pub target_names: Vec<String>,
    /// WHERE condition, without the keyword (empty = every row).
    pub filter: String,
    /// Percentage of the (filtered) rows to copy, 1–100.
    pub percent: u8,
    pub target_db: String,
    pub target_schema: String,
    pub target_table: String,
    /// Create the target table first (it doesn't exist).
    pub create: bool,
    /// Delete the target's rows first, in the same transaction.
    pub clear: bool,
    /// Masking rule of each column (`Keep` = copied as is).
    pub masks: Vec<Mask>,
    /// Secret for the deterministic masks (Hash, Email, Name, Phone).
    pub mask_key: String,
}

/// Masking rule for a column. Every rule is deterministic: with the same key, the same value
/// always gives the same masked value, so masked keys still match across tables and copies.
/// NULL stays NULL.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mask {
    #[default]
    Keep,
    Null,
    Blank,
    Hash,
    /// Keeps the last 4 characters: `************1234`.
    Partial,
    Email,
    Name,
    Phone,
}

const FIRST_NAMES: [&str; 24] = [
    "Alex", "Sam", "Jordan", "Taylor", "Morgan", "Casey", "Robin", "Jamie", "Avery", "Riley", "Quinn", "Drew", "Elena",
    "Marco", "Yuki", "Amara", "Lucas", "Nadia", "Omar", "Ingrid", "Mateo", "Leila", "Hugo", "Sofia",
];
const LAST_NAMES: [&str; 24] = [
    "Smith", "Garcia", "Muller", "Rossi", "Silva", "Kim", "Novak", "Dubois", "Tanaka", "Jensen", "Lopez", "Martin",
    "Costa", "Weber", "Moreau", "Ivanova", "Okafor", "Nguyen", "Larsen", "Romero", "Fischer", "Sato", "Brown", "Ruiz",
];

impl Mask {
    pub const ALL: [Mask; 8] = [Mask::Keep, Mask::Null, Mask::Blank, Mask::Hash, Mask::Partial, Mask::Email, Mask::Name, Mask::Phone];

    pub fn label(self) -> &'static str {
        match self {
            Mask::Keep => tr("Keep"),
            Mask::Null => tr("Set NULL"),
            Mask::Blank => tr("Empty text"),
            Mask::Hash => tr("Hash"),
            Mask::Partial => tr("Keep last 4"),
            Mask::Email => tr("Fake email"),
            Mask::Name => tr("Fake name"),
            Mask::Phone => tr("Fake phone"),
        }
    }

    /// Rules that fit a column: NULL needs a nullable column that isn't a key, and the others
    /// write text (an empty text can't be a key either).
    pub fn allowed(self, driver: Driver, column: &ColumnInfo) -> bool {
        let text = matches!(classify(driver, &column.data_type), Kind::Varchar(_) | Kind::Text);
        match self {
            Mask::Keep => true,
            Mask::Null => column.nullable && !column.primary_key,
            Mask::Blank => text && !column.primary_key,
            _ => text,
        }
    }

    /// Rule suggested from a column's name (`email`, `phone`, `first_name`, `iban`…).
    pub fn suggest(name: &str) -> Mask {
        let n = name.to_ascii_lowercase().replace(['-', ' '], "_");
        let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
        if has(&["email", "e_mail", "mail", "correo"]) {
            Mask::Email
        } else if has(&["phone", "mobile", "cell", "fax", "telef", "tel_", "movil"]) || n == "tel" {
            Mask::Phone
        } else if has(&["first_name", "firstname", "last_name", "lastname", "full_name", "fullname", "surname", "given_name", "family_name", "nombre", "apellido"])
            || matches!(n.as_str(), "name" | "contact" | "contact_name" | "customer_name" | "person_name" | "user_name" | "username")
        {
            Mask::Name
        } else if has(&["iban", "card", "tarjeta", "account_number", "ssn", "dni", "nif", "nie", "passport", "pasaporte", "tax_id", "vat"]) {
            Mask::Partial
        } else if has(&["password", "passwd", "secret", "token", "address", "street", "direccion", "domicilio", "ip_addr"]) {
            Mask::Hash
        } else {
            Mask::Keep
        }
    }

    /// Masked value, cut to `max_len` characters when the column has a limit.
    pub fn apply(self, key: &[u8], value: Cell, max_len: Option<u32>) -> Cell {
        let v = value?;
        let digest = || {
            let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, key);
            aws_lc_rs::hmac::sign(&key, v.as_bytes()).as_ref().to_vec()
        };
        let hex = |d: &[u8], n: usize| d.iter().take(n.div_ceil(2)).map(|b| format!("{b:02x}")).collect::<String>()[..n].to_string();
        let out = match self {
            Mask::Keep => v.clone(),
            Mask::Null => return None,
            Mask::Blank => String::new(),
            Mask::Hash => hex(&digest(), 16),
            Mask::Partial => {
                let chars: Vec<char> = v.chars().collect();
                let keep = if chars.len() > 4 { 4 } else { 0 };
                let mut s = "*".repeat(chars.len() - keep);
                s.extend(&chars[chars.len() - keep..]);
                s
            }
            Mask::Email => format!("user_{}@example.com", hex(&digest(), 10)),
            Mask::Name => {
                let d = digest();
                format!("{} {}", FIRST_NAMES[d[0] as usize % FIRST_NAMES.len()], LAST_NAMES[d[1] as usize % LAST_NAMES.len()])
            }
            Mask::Phone => {
                let d = digest();
                let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 10_000_000;
                format!("555-{:03}-{:04}", n / 10_000, n % 10_000)
            }
        };
        Some(match max_len {
            Some(n) if out.chars().count() > n as usize => out.chars().take(n as usize).collect(),
            _ => out,
        })
    }
}

/// Character limit of a text column (`varchar(20)` → 20), for masked values.
fn text_limit(driver: Driver, data_type: &str) -> Option<u32> {
    match classify(driver, data_type) {
        Kind::Varchar(n) => Some(n),
        _ => None,
    }
}

/// Applies each column's mask to the rows.
pub fn mask_rows(driver: Driver, columns: &[ColumnInfo], masks: &[Mask], key: &str, rows: &mut [Vec<Cell>]) {
    let plan: Vec<(usize, Mask, Option<u32>)> = masks
        .iter()
        .enumerate()
        .filter(|(_, m)| **m != Mask::Keep)
        .filter_map(|(i, m)| Some((i, *m, text_limit(driver, &columns.get(i)?.data_type))))
        .collect();
    if plan.is_empty() {
        return;
    }
    for row in rows {
        for &(i, mask, limit) in &plan {
            if let Some(cell) = row.get_mut(i) {
                *cell = mask.apply(key.as_bytes(), cell.take(), limit);
            }
        }
    }
}

/// Broad type class, to translate a column type from one engine to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    SmallInt,
    Int,
    BigInt,
    Decimal(Option<(u32, u32)>),
    Float,
    Bool,
    Date,
    Time,
    Timestamp,
    TimestampTz,
    Varchar(u32),
    Text,
    Binary,
}

/// Numbers inside the parentheses of a type: `varchar(20)` → [20], `numeric(10,2)` → [10, 2].
fn type_args(t: &str) -> Vec<u32> {
    let Some(start) = t.find('(') else { return Vec::new() };
    let end = t[start..].find(')').map_or(t.len(), |e| start + e);
    t[start + 1..end].split(',').filter_map(|a| a.trim().split(' ').next()?.parse().ok()).collect()
}

fn classify(driver: Driver, data_type: &str) -> Kind {
    let t = data_type.trim().to_ascii_lowercase();
    let base = t.split('(').next().unwrap_or("").trim();
    let args = type_args(&t);
    match base {
        "smallint" | "int2" | "smallserial" => Kind::SmallInt,
        "tinyint" if args.first() == Some(&1) => Kind::Bool,
        "tinyint" | "mediumint" | "int" | "integer" | "int4" | "serial" | "year" => Kind::Int,
        "bigint" | "int8" | "bigserial" => Kind::BigInt,
        "number" | "numeric" | "decimal" | "dec" => match args.as_slice() {
            // Oracle and Snowflake write integers as NUMBER(p) / NUMBER(p,0).
            [p] | [p, 0] if *p <= 4 => Kind::SmallInt,
            [p] | [p, 0] if *p <= 9 => Kind::Int,
            [p] | [p, 0] if *p <= 18 => Kind::BigInt,
            [p, s] => Kind::Decimal(Some((*p, *s))),
            [p] => Kind::Decimal(Some((*p, 0))),
            _ => Kind::Decimal(None),
        },
        "money" | "smallmoney" => Kind::Decimal(Some((19, 4))),
        "real" | "float" | "float4" | "float8" | "double" | "double precision" | "binary_double" | "binary_float" => Kind::Float,
        "boolean" | "bool" | "bit" => Kind::Bool,
        "date" if driver == Driver::Oracle => Kind::Timestamp,
        "date" => Kind::Date,
        "time" | "time without time zone" | "time with time zone" | "timetz" => Kind::Time,
        "timestamp" | "timestamp without time zone" | "datetime" | "datetime2" | "smalldatetime" | "timestamp_ntz" => {
            if t.contains("with time zone") { Kind::TimestampTz } else { Kind::Timestamp }
        }
        "timestamptz" | "timestamp with time zone" | "timestamp with local time zone" | "datetimeoffset" | "timestamp_tz"
        | "timestamp_ltz" => Kind::TimestampTz,
        "varchar" | "character varying" | "nvarchar" | "varchar2" | "nvarchar2" | "char" | "character" | "nchar"
        | "bpchar" | "string" => match args.first() {
            Some(&n) if n > 0 && !t.contains("max") => Kind::Varchar(n),
            _ => Kind::Text,
        },
        "uuid" | "uniqueidentifier" => Kind::Varchar(36),
        "bytea" | "blob" | "tinyblob" | "mediumblob" | "longblob" | "binary" | "varbinary" | "raw" | "long raw" | "image"
        | "bfile" => Kind::Binary,
        _ if t.starts_with("timestamp") && t.contains("time zone") => Kind::TimestampTz,
        _ if t.starts_with("timestamp") => Kind::Timestamp,
        _ => Kind::Text,
    }
}

/// true for binary columns: the grid shows them in hex (sometimes cut), so they can't be copied.
pub fn is_binary(driver: Driver, data_type: &str) -> bool {
    classify(driver, data_type) == Kind::Binary
}

/// Type for the target table's column. Same engine: the source type as is.
pub fn target_type(from: Driver, to: Driver, data_type: &str, primary_key: bool) -> String {
    let family = |d: Driver| if d == Driver::MariaDb { Driver::MySql } else { d };
    let lower = data_type.to_ascii_lowercase();
    // PostgreSQL arrays and user-defined types don't exist elsewhere either.
    if family(from) == family(to) && !lower.ends_with("[]") {
        return data_type.to_string();
    }
    let kind = match classify(from, data_type) {
        // A key needs a bounded type on MySQL and Oracle.
        Kind::Text if primary_key && matches!(to, Driver::MySql | Driver::MariaDb | Driver::Oracle) => Kind::Varchar(255),
        k => k,
    };
    use Driver::*;
    match (kind, to) {
        (_, Sqlite) => match kind {
            Kind::SmallInt | Kind::Int | Kind::BigInt => "INTEGER".into(),
            Kind::Decimal(_) => "NUMERIC".into(),
            Kind::Float => "REAL".into(),
            Kind::Bool => "BOOLEAN".into(),
            Kind::Date => "DATE".into(),
            Kind::Timestamp | Kind::TimestampTz => "DATETIME".into(),
            Kind::Binary => "BLOB".into(),
            Kind::Time | Kind::Varchar(_) | Kind::Text => "TEXT".into(),
        },
        (Kind::SmallInt, Oracle) => "number(5)".into(),
        (Kind::Int, Oracle) => "number(10)".into(),
        (Kind::BigInt, Oracle) => "number(19)".into(),
        (Kind::SmallInt | Kind::Int | Kind::BigInt, Snowflake) => "number(38,0)".into(),
        (Kind::SmallInt, _) => "smallint".into(),
        (Kind::Int, Postgres) => "integer".into(),
        (Kind::Int, _) => "int".into(),
        (Kind::BigInt, _) => "bigint".into(),
        (Kind::Decimal(Some((p, s))), Oracle | Snowflake) => format!("number({},{})", p.min(38), s.min(p.min(38))),
        (Kind::Decimal(Some((p, s))), _) => format!("decimal({},{})", p.min(38), s.min(p.min(38))),
        (Kind::Decimal(None), Postgres) => "numeric".into(),
        (Kind::Decimal(None), Oracle) => "number".into(),
        (Kind::Decimal(None), _) => "decimal(38,10)".into(),
        (Kind::Float, Postgres) => "double precision".into(),
        (Kind::Float, MySql | MariaDb) => "double".into(),
        (Kind::Float, Oracle) => "binary_double".into(),
        (Kind::Float, _) => "float".into(),
        (Kind::Bool, MsSql) => "bit".into(),
        (Kind::Bool, Oracle) => "number(1)".into(),
        (Kind::Bool, _) => "boolean".into(),
        (Kind::Date, _) => "date".into(),
        (Kind::Time, Oracle) => "varchar2(32)".into(),
        (Kind::Time, _) => "time".into(),
        (Kind::Timestamp, MsSql) => "datetime2".into(),
        (Kind::Timestamp, MySql | MariaDb) => "datetime(6)".into(),
        (Kind::Timestamp, Snowflake) => "timestamp_ntz".into(),
        (Kind::Timestamp, _) => "timestamp".into(),
        (Kind::TimestampTz, Postgres) => "timestamptz".into(),
        (Kind::TimestampTz, MsSql) => "datetimeoffset".into(),
        (Kind::TimestampTz, MySql | MariaDb) => "datetime(6)".into(),
        (Kind::TimestampTz, Oracle) => "timestamp with time zone".into(),
        (Kind::TimestampTz, _) => "timestamp_tz".into(),
        (Kind::Varchar(n), MsSql) if n <= 4000 => format!("nvarchar({n})"),
        (Kind::Varchar(n), MySql | MariaDb) if n <= 16_000 => format!("varchar({n})"),
        (Kind::Varchar(n), Oracle) if n <= 4000 => format!("varchar2({n} char)"),
        (Kind::Varchar(n), Postgres | Snowflake) => format!("varchar({n})"),
        (Kind::Text, Postgres) => "text".into(),
        (Kind::Varchar(_) | Kind::Text, MsSql) => "nvarchar(max)".into(),
        (Kind::Varchar(_) | Kind::Text, MySql | MariaDb) => "longtext".into(),
        (Kind::Varchar(_) | Kind::Text, Oracle) => "clob".into(),
        (Kind::Text, Snowflake) => "varchar".into(),
        (Kind::Binary, Postgres) => "bytea".into(),
        (Kind::Binary, MsSql) => "varbinary(max)".into(),
        (Kind::Binary, Oracle | MySql | MariaDb) => "blob".into(),
        (Kind::Binary, Snowflake) => "binary".into(),
    }
}

/// Source value as the target expects it: booleans read as `t`/`true` become `1`/`0` on the
/// engines that store them as numbers.
fn convert(from: Driver, to: Driver, data_type: &str, value: Cell) -> Cell {
    let v = value?;
    if classify(from, data_type) != Kind::Bool {
        return Some(v);
    }
    let truth = match v.trim().to_ascii_lowercase().as_str() {
        "t" | "true" | "1" | "y" | "yes" => true,
        "f" | "false" | "0" | "n" | "no" => false,
        _ => return Some(v),
    };
    Some(match (to, truth) {
        (Driver::Postgres | Driver::Snowflake, true) => "true".into(),
        (Driver::Postgres | Driver::Snowflake, false) => "false".into(),
        (_, true) => "1".into(),
        (_, false) => "0".into(),
    })
}

/// SELECT of the source rows; `limit` caps them.
pub fn select_sql(driver: Driver, schema: &str, table: &str, columns: &[ColumnInfo], filter: &str, limit: Option<u64>) -> String {
    let cols = columns.iter().map(|c| driver.quote_ident(&c.name)).collect::<Vec<_>>().join(", ");
    let t = Session::quote_table(driver, schema, table);
    let top = match (driver, limit) {
        (Driver::MsSql, Some(n)) => format!("TOP ({n}) "),
        _ => String::new(),
    };
    let mut sql = format!("SELECT {top}{cols} FROM {t}");
    let w = filter.trim().trim_end_matches(';').trim();
    if !w.is_empty() {
        sql.push_str(&format!(" WHERE {w}"));
    }
    match (driver, limit) {
        (Driver::MsSql, _) | (_, None) => sql,
        (Driver::Oracle, Some(n)) => format!("SELECT * FROM ({sql}) WHERE ROWNUM <= {n}"),
        (_, Some(n)) => format!("{sql} LIMIT {n}"),
    }
}

/// COUNT(*) of the rows the filter selects (for a percentage).
pub fn count_sql(driver: Driver, schema: &str, table: &str, filter: &str) -> String {
    let mut sql = format!("SELECT COUNT(*) FROM {}", Session::quote_table(driver, schema, table));
    let w = filter.trim().trim_end_matches(';').trim();
    if !w.is_empty() {
        sql.push_str(&format!(" WHERE {w}"));
    }
    sql
}

/// Columns of the table to create on `to`, with their types translated.
pub fn target_columns(from: Driver, to: Driver, columns: &[ColumnInfo]) -> Vec<NewColumn> {
    columns
        .iter()
        .map(|c| NewColumn {
            name: c.name.clone(),
            data_type: target_type(from, to, &c.data_type, c.primary_key),
            not_null: !c.nullable,
            primary_key: c.primary_key,
            default: String::new(),
        })
        .collect()
}

/// `CREATE TABLE` for the target. Names keep their case (quoted when needed), so the INSERTs,
/// which always quote, find them.
pub fn create_sql(from: Driver, to: Driver, schema: &str, table: &str, columns: &[ColumnInfo]) -> String {
    let schema = if to.flat_schema() { "" } else { schema };
    create_table_sql(to, schema, table, &target_columns(from, to, columns))
}

/// Multi-row INSERTs into the `names` columns (Oracle: one row each, it has no multi-row VALUES).
/// `columns` are the source columns, for their types.
pub fn insert_sql(from: Driver, to: Driver, table: &str, columns: &[ColumnInfo], names: &[String], rows: Vec<Vec<Cell>>) -> Vec<String> {
    if columns.is_empty() {
        return Vec::new();
    }
    let cols = names.iter().map(|n| to.quote_ident(n)).collect::<Vec<_>>().join(", ");
    let batch = if to == Driver::Oracle { 1 } else { BATCH };
    let mut out = Vec::new();
    let mut tuples = Vec::new();
    for row in rows {
        let vals: Vec<String> = row
            .into_iter()
            .zip(columns)
            .map(|(v, c)| literal(to, &convert(from, to, &c.data_type, v)))
            .collect();
        tuples.push(format!("({})", vals.join(", ")));
        if tuples.len() == batch {
            out.push(format!("INSERT INTO {table} ({cols})\nVALUES {}", tuples.join(",\n       ")));
            tuples.clear();
        }
    }
    if !tuples.is_empty() {
        out.push(format!("INSERT INTO {table} ({cols})\nVALUES {}", tuples.join(",\n       ")));
    }
    out
}

/// Session for `db` on its own connection (Oracle keeps its service name).
async fn open(mut cfg: ConnectionConfig, db: &str) -> anyhow::Result<Session> {
    if !matches!(cfg.driver, Driver::Oracle | Driver::Sqlite) && !db.is_empty() {
        cfg.database = db.to_string();
    }
    let mut s = Session::connect(&cfg).await?;
    s.use_database(db).await?;
    Ok(s)
}

/// Runs the copy and returns the rows inserted. A cancel stops it and rolls the target back.
pub async fn copy(source: ConnectionConfig, target: ConnectionConfig, req: CopyRequest, cancel: Arc<Notify>) -> anyhow::Result<u64> {
    let (from, to) = (source.driver, target.driver);
    let mut source = source;
    source.read_only = true;
    let cancelled = || anyhow::anyhow!(tr("Copy cancelled"));

    let read = async {
        let mut s = open(source, &req.source_db).await?;
        let limit = if req.percent >= 100 {
            None
        } else {
            let rs = s.execute(&count_sql(from, &req.source_schema, &req.source_table, &req.filter), 1).await?;
            let total: u64 = rs
                .first()
                .and_then(|r| r.rows.first())
                .and_then(|r| r.first().cloned().flatten())
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            Some((total * u64::from(req.percent)).div_ceil(100))
        };
        let sql = select_sql(from, &req.source_schema, &req.source_table, &req.columns, &req.filter, limit);
        let rs = s.execute(&sql, MAX_ROWS).await?.into_iter().next().unwrap_or_default();
        if rs.truncated {
            anyhow::bail!(tr("The table has more than 1,000,000 rows: copy fewer with a filter or a percentage"));
        }
        anyhow::Ok(rs.rows)
    };
    let rows = tokio::select! {
        _ = cancel.notified() => return Err(cancelled()),
        rows = read => rows?,
    };

    let mut t = tokio::select! {
        _ = cancel.notified() => return Err(cancelled()),
        s = open(target, &req.target_db) => s?,
    };
    if req.create {
        let mut cols = req.columns.clone();
        for (c, n) in cols.iter_mut().zip(&req.target_names) {
            c.name.clone_from(n);
        }
        let ddl = create_sql(from, to, &req.target_schema, &req.target_table, &cols);
        tokio::select! {
            _ = cancel.notified() => return Err(cancelled()),
            r = t.execute(&ddl, 0) => { r?; }
        }
    }
    let schema = if to.flat_schema() && to != Driver::Sqlite { req.target_db.as_str() } else { req.target_schema.as_str() };
    let table = Session::quote_table(to, schema, &req.target_table);
    let mut statements = Vec::new();
    if req.clear {
        statements.push(format!("DELETE FROM {table}"));
    }
    let mut rows = rows;
    mask_rows(from, &req.columns, &req.masks, &req.mask_key, &mut rows);
    let n = rows.len() as u64;
    statements.extend(insert_sql(from, to, &table, &req.columns, &req.target_names, rows));
    let handle = t.cancel_handle();
    // Dropping the session on a cancel closes it, so the open transaction is rolled back.
    tokio::select! {
        _ = cancel.notified() => {
            if let Some(h) = handle {
                let _ = h.cancel().await;
            }
            Err(cancelled())
        }
        r = t.run_transaction(&statements) => r.map(|_| n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, pk: bool) -> ColumnInfo {
        ColumnInfo { name: name.into(), data_type: ty.into(), nullable: !pk, primary_key: pk }
    }

    #[test]
    fn translates_types_between_engines() {
        assert_eq!(target_type(Driver::Postgres, Driver::Postgres, "character varying(20)", false), "character varying(20)");
        assert_eq!(target_type(Driver::MySql, Driver::MariaDb, "tinyint(1)", false), "tinyint(1)");
        assert_eq!(target_type(Driver::Postgres, Driver::MsSql, "character varying(20)", false), "nvarchar(20)");
        assert_eq!(target_type(Driver::Postgres, Driver::MySql, "text", true), "varchar(255)");
        assert_eq!(target_type(Driver::Postgres, Driver::Oracle, "boolean", false), "number(1)");
        assert_eq!(target_type(Driver::Oracle, Driver::Postgres, "number(10)", false), "bigint");
        assert_eq!(target_type(Driver::Oracle, Driver::Postgres, "number(10,2)", false), "decimal(10,2)");
        assert_eq!(target_type(Driver::Oracle, Driver::Postgres, "date", false), "timestamp");
        assert_eq!(target_type(Driver::MsSql, Driver::Postgres, "datetimeoffset", false), "timestamptz");
        assert_eq!(target_type(Driver::MsSql, Driver::Sqlite, "nvarchar(max)", false), "TEXT");
        assert_eq!(target_type(Driver::MySql, Driver::Snowflake, "int", false), "number(38,0)");
        assert_eq!(target_type(Driver::Postgres, Driver::MySql, "integer[]", false), "longtext");
        assert!(is_binary(Driver::Postgres, "bytea") && is_binary(Driver::MsSql, "varbinary(max)"));
        assert!(!is_binary(Driver::Oracle, "varchar2(10 char)"));
    }

    #[test]
    fn masks_are_deterministic_and_fit_the_column() {
        let key = b"k";
        let email = Mask::Email.apply(key, Some("ana@corp.com".into()), None).unwrap();
        assert!(email.starts_with("user_") && email.ends_with("@example.com") && email.len() == 27, "{email}");
        assert_eq!(Mask::Email.apply(key, Some("ana@corp.com".into()), None).unwrap(), email);
        assert_ne!(Mask::Email.apply(b"other", Some("ana@corp.com".into()), None).unwrap(), email);
        assert_ne!(Mask::Hash.apply(key, Some("a".into()), None), Mask::Hash.apply(key, Some("b".into()), None));
        assert_eq!(Mask::Hash.apply(key, Some("a".into()), Some(6)).unwrap().len(), 6);
        assert_eq!(Mask::Partial.apply(key, Some("ES91 2100 1234".into()), None).unwrap(), "**********1234");
        assert_eq!(Mask::Partial.apply(key, Some("123".into()), None).unwrap(), "***");
        assert!(Mask::Phone.apply(key, Some("+34 600".into()), None).unwrap().starts_with("555-"));
        assert_eq!(Mask::Name.apply(key, None, None), None);
        assert_eq!(Mask::Null.apply(key, Some("x".into()), None), None);
        assert_eq!(Mask::Blank.apply(key, Some("x".into()), None), Some(String::new()));

        let pk = ColumnInfo { name: "id".into(), data_type: "varchar(10)".into(), nullable: false, primary_key: true };
        let num = ColumnInfo { name: "n".into(), data_type: "integer".into(), nullable: true, primary_key: false };
        assert!(!Mask::Null.allowed(Driver::Postgres, &pk) && !Mask::Blank.allowed(Driver::Postgres, &pk));
        assert!(Mask::Hash.allowed(Driver::Postgres, &pk));
        assert!(Mask::Null.allowed(Driver::Postgres, &num) && !Mask::Hash.allowed(Driver::Postgres, &num));

        let cols = [pk, num];
        let mut rows = vec![vec![Some("abc".into()), Some("7".into())]];
        mask_rows(Driver::Postgres, &cols, &[Mask::Hash, Mask::Null], "k", &mut rows);
        assert_eq!(rows[0][0].as_ref().map(String::len), Some(10));
        assert_eq!(rows[0][1], None);
    }

    #[test]
    fn suggests_masks_from_column_names() {
        assert_eq!(Mask::suggest("Email"), Mask::Email);
        assert_eq!(Mask::suggest("mobile_phone"), Mask::Phone);
        assert_eq!(Mask::suggest("first_name"), Mask::Name);
        assert_eq!(Mask::suggest("name"), Mask::Name);
        assert_eq!(Mask::suggest("product_name"), Mask::Keep);
        assert_eq!(Mask::suggest("IBAN"), Mask::Partial);
        assert_eq!(Mask::suggest("password_hash"), Mask::Hash);
        assert_eq!(Mask::suggest("created_at"), Mask::Keep);
    }

    #[test]
    fn converts_booleans() {
        assert_eq!(convert(Driver::Postgres, Driver::MySql, "boolean", Some("t".into())), Some("1".into()));
        assert_eq!(convert(Driver::MsSql, Driver::Postgres, "bit", Some("false".into())), Some("false".into()));
        assert_eq!(convert(Driver::Postgres, Driver::MySql, "integer", Some("t".into())), Some("t".into()));
        assert_eq!(convert(Driver::Postgres, Driver::MySql, "boolean", None), None);
    }

    #[test]
    fn builds_select_with_filter_and_limit() {
        let cols = [col("id", "int", true), col("name", "text", false)];
        assert_eq!(
            select_sql(Driver::Postgres, "public", "t", &cols, "id > 3;", Some(5)),
            "SELECT \"id\", \"name\" FROM \"public\".\"t\" WHERE id > 3 LIMIT 5"
        );
        assert_eq!(select_sql(Driver::MsSql, "dbo", "t", &cols, "", Some(5)), "SELECT TOP (5) [id], [name] FROM [dbo].[t]");
        assert_eq!(
            select_sql(Driver::Oracle, "HR", "T", &cols, "", Some(5)),
            "SELECT * FROM (SELECT \"id\", \"name\" FROM \"HR\".\"T\") WHERE ROWNUM <= 5"
        );
        assert_eq!(count_sql(Driver::Sqlite, "main", "t", " a = 1 "), "SELECT COUNT(*) FROM \"t\" WHERE a = 1");
    }

    #[test]
    fn builds_create_and_inserts() {
        let cols = [col("id", "integer", true), col("Name", "character varying(20)", false), col("ok", "boolean", false)];
        assert_eq!(
            create_sql(Driver::Postgres, Driver::MySql, "public", "Users", &cols),
            "create table Users\n(\n    id int not null,\n    Name varchar(20),\n    ok boolean,\n    primary key (id)\n)"
        );
        let rows: Vec<Vec<Cell>> = (0..150).map(|i| vec![Some(i.to_string()), Some("it's".into()), Some("t".into())]).collect();
        let names: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
        let s = insert_sql(Driver::Postgres, Driver::MySql, "`db`.`Users`", &cols, &names, rows);
        assert_eq!(s.len(), 2);
        assert!(s[0].starts_with("INSERT INTO `db`.`Users` (`id`, `Name`, `ok`)\nVALUES ('0', 'it''s', '1'),"));
        let one = insert_sql(Driver::Sqlite, Driver::Oracle, "\"T\"", &cols, &names, vec![vec![None, None, Some("f".into())]]);
        assert_eq!(one, vec!["INSERT INTO \"T\" (\"id\", \"Name\", \"ok\")\nVALUES (NULL, NULL, '0')".to_string()]);
    }
}
