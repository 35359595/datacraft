//! Utilidades de texto SQL: troceo de sentencias, sentencia bajo el cursor
//! y resaltado de sintaxis para el editor.

use crate::db::Driver;
use egui::Color32;
use egui::text::{LayoutJob, TextFormat};

/// Trozo de script con su rango en bytes dentro del texto original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// Divide un script en sentencias. Respeta comillas simples/dobles, `[ident]` (SQL Server),
/// `$tag$...$tag$` (PostgreSQL), comentarios `--` y `/* */`. Separadores: `;` y, en SQL Server,
/// una línea que solo contiene `GO` (sin distinguir mayúsculas). Omite trozos vacíos o solo comentarios.
/// `text` va recortado (trim) y sin el `;` final.
pub fn split_statements(script: &str, driver: Driver) -> Vec<Statement> {
    let dialect = Dialect::for_driver(driver);
    let mut out = Vec::new();
    // Rango de tokens significativos (ni espacio ni comentario) de la sentencia en curso.
    let mut current: Option<(usize, usize)> = None;
    let mut flush = |current: &mut Option<(usize, usize)>| {
        if let Some((start, end)) = current.take() {
            out.push(Statement {
                text: script[start..end].to_string(),
                start,
                end,
            });
        }
    };

    for tok in Lexer::new(script, dialect) {
        match tok.kind {
            Kind::Whitespace | Kind::LineComment | Kind::BlockComment => {}
            Kind::Semicolon => flush(&mut current),
            Kind::Word if dialect.go_separator && is_go_line(script, &tok) => flush(&mut current),
            _ => match &mut current {
                Some((_, end)) => *end = tok.end,
                None => current = Some((tok.start, tok.end)),
            },
        }
    }
    flush(&mut current);
    out
}

/// Sentencia que contiene la posición `cursor` (en bytes). Si el cursor está en espacio entre
/// sentencias, devuelve la anterior más cercana (como DataGrip); si no hay, la siguiente.
pub fn statement_at(script: &str, cursor: usize, driver: Driver) -> Option<Statement> {
    let stmts = split_statements(script, driver);
    if stmts.is_empty() {
        return None;
    }
    let mut cursor = cursor.min(script.len());
    while !script.is_char_boundary(cursor) {
        cursor -= 1;
    }

    let next_idx = stmts.partition_point(|s| s.start <= cursor);
    // Si el cursor está en la sangría de la línea donde empieza la siguiente sentencia, esa.
    if let Some(next) = stmts.get(next_idx) {
        let line_start = script[..cursor].rfind('\n').map_or(0, |i| i + 1);
        let before_cursor_blank = script[line_start..cursor].trim().is_empty();
        let gap_blank = !script[cursor..next.start].contains('\n')
            && script[cursor..next.start].trim().is_empty();
        if next_idx == 0 || (before_cursor_blank && gap_blank) {
            return Some(next.clone());
        }
    }
    Some(stmts[next_idx - 1].clone())
}

/// Trozos a enviar al servidor al ejecutar un script entero o una selección.
/// SQL Server: lotes separados solo por `GO` (un `DECLARE` o un `BEGIN…END` con `;` siguen juntos).
/// SQLite: el script entero (el driver ya lo recorre sentencia a sentencia, respetando triggers).
/// PostgreSQL: sentencias por `;` (el dollar-quoting protege los cuerpos de funciones).
pub fn split_batches(script: &str, driver: Driver) -> Vec<Statement> {
    match driver {
        Driver::Postgres => split_statements(script, driver),
        Driver::MsSql | Driver::Sqlite => {
            let dialect = Dialect::for_driver(driver);
            let mut out = Vec::new();
            let mut current: Option<(usize, usize)> = None;
            let mut flush = |current: &mut Option<(usize, usize)>| {
                if let Some((start, end)) = current.take() {
                    let text = script[start..end].to_string();
                    // Un lote formado solo por `;` no aporta nada.
                    if text.chars().any(|c| c != ';' && !c.is_whitespace()) {
                        out.push(Statement { text, start, end });
                    }
                }
            };
            for tok in Lexer::new(script, dialect) {
                match tok.kind {
                    Kind::Whitespace | Kind::LineComment | Kind::BlockComment => {}
                    Kind::Word if dialect.go_separator && is_go_line(script, &tok) => flush(&mut current),
                    _ => match &mut current {
                        Some((_, end)) => *end = tok.end,
                        None => current = Some((tok.start, tok.end)),
                    },
                }
            }
            flush(&mut current);
            out
        }
    }
}

/// LayoutJob coloreado para `egui::TextEdit::layouter`.
pub fn highlight(text: &str, dark_mode: bool, font: egui::FontId) -> egui::text::LayoutJob {
    let palette = if dark_mode {
        Palette::DARK
    } else {
        Palette::LIGHT
    };
    let mut job = LayoutJob::default();
    let mut pending: Option<(usize, usize, Style)> = None;

    for tok in Lexer::new(text, Dialect::PERMISSIVE) {
        let style = Style::of(&tok, text);
        // Agrupa tokens consecutivos con el mismo estilo para no inflar las secciones.
        match &mut pending {
            Some((_, end, s)) if *s == style => *end = tok.end,
            _ => {
                if let Some((s, e, st)) = pending.take() {
                    job.append(&text[s..e], 0.0, palette.format(st, &font));
                }
                pending = Some((tok.start, tok.end, style));
            }
        }
    }
    if let Some((s, e, st)) = pending {
        job.append(&text[s..e], 0.0, palette.format(st, &font));
    }
    job
}

/// Palabras clave SQL comunes (mayúsculas), usadas por el resaltado y el autocompletado.
pub const KEYWORDS: &[&str] = &[
    "ABORT",
    "ACTION",
    "ADD",
    "AFTER",
    "ALL",
    "ALTER",
    "ALWAYS",
    "ANALYZE",
    "AND",
    "ANY",
    "ARRAY",
    "AS",
    "ASC",
    "ATTACH",
    "AUTHORIZATION",
    "AUTOINCREMENT",
    "BACKUP",
    "BEFORE",
    "BEGIN",
    "BETWEEN",
    "BIGINT",
    "BIGSERIAL",
    "BINARY",
    "BIT",
    "BOOLEAN",
    "BOTH",
    "BREAK",
    "BROWSE",
    "BULK",
    "BY",
    "BYTEA",
    "CALL",
    "CASCADE",
    "CASE",
    "CAST",
    "CATCH",
    "CHAR",
    "CHARACTER",
    "CHECK",
    "CHECKPOINT",
    "CLOSE",
    "CLUSTER",
    "CLUSTERED",
    "COALESCE",
    "COLLATE",
    "COLUMN",
    "COMMENT",
    "COMMIT",
    "COMMITTED",
    "CONCURRENTLY",
    "CONFLICT",
    "CONSTRAINT",
    "CONSTRAINTS",
    "CONTINUE",
    "CONVERT",
    "COPY",
    "CREATE",
    "CROSS",
    "CUBE",
    "CURRENT",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURSOR",
    "CYCLE",
    "DATABASE",
    "DATE",
    "DATETIME",
    "DATETIME2",
    "DEALLOCATE",
    "DEC",
    "DECIMAL",
    "DECLARE",
    "DEFAULT",
    "DEFERRABLE",
    "DEFERRED",
    "DELETE",
    "DENY",
    "DESC",
    "DETACH",
    "DISTINCT",
    "DO",
    "DOUBLE",
    "DROP",
    "EACH",
    "ELSE",
    "ELSIF",
    "END",
    "ESCAPE",
    "EXCEPT",
    "EXCLUSIVE",
    "EXEC",
    "EXECUTE",
    "EXISTS",
    "EXPLAIN",
    "EXTENSION",
    "FALSE",
    "FETCH",
    "FILTER",
    "FIRST",
    "FLOAT",
    "FOLLOWING",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GLOB",
    "GO",
    "GRANT",
    "GROUP",
    "GROUPING",
    "HAVING",
    "IDENTITY",
    "IF",
    "ILIKE",
    "IMMEDIATE",
    "IN",
    "INDEX",
    "INNER",
    "INOUT",
    "INSERT",
    "INSTEAD",
    "INT",
    "INTEGER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IS",
    "ISNULL",
    "JOIN",
    "JSON",
    "JSONB",
    "KEY",
    "LANGUAGE",
    "LAST",
    "LATERAL",
    "LEADING",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LOCK",
    "MATERIALIZED",
    "MERGE",
    "MONEY",
    "NATURAL",
    "NCHAR",
    "NEXT",
    "NO",
    "NOCHECK",
    "NOLOCK",
    "NONCLUSTERED",
    "NOT",
    "NOTHING",
    "NOTNULL",
    "NOWAIT",
    "NULL",
    "NULLIF",
    "NULLS",
    "NUMERIC",
    "NVARCHAR",
    "OF",
    "OFF",
    "OFFSET",
    "ON",
    "ONLY",
    "OPEN",
    "OR",
    "ORDER",
    "OUT",
    "OUTER",
    "OUTPUT",
    "OVER",
    "OWNER",
    "PARTITION",
    "PERCENT",
    "PIVOT",
    "PLAN",
    "PRAGMA",
    "PRECEDING",
    "PRECISION",
    "PRIMARY",
    "PRINT",
    "PROC",
    "PROCEDURE",
    "RAISE",
    "RAISERROR",
    "RANGE",
    "READ",
    "REAL",
    "RECURSIVE",
    "REFERENCES",
    "REFRESH",
    "REINDEX",
    "RELEASE",
    "RENAME",
    "REPEATABLE",
    "REPLACE",
    "RESTRICT",
    "RETURN",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "ROLE",
    "ROLLBACK",
    "ROLLUP",
    "ROW",
    "ROWID",
    "ROWS",
    "SAVEPOINT",
    "SCHEMA",
    "SELECT",
    "SEQUENCE",
    "SERIAL",
    "SERIALIZABLE",
    "SESSION",
    "SET",
    "SETOF",
    "SHOW",
    "SMALLINT",
    "SOME",
    "STRICT",
    "TABLE",
    "TABLESAMPLE",
    "TEMP",
    "TEMPORARY",
    "TEXT",
    "THEN",
    "THROW",
    "TIES",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "TINYINT",
    "TO",
    "TOP",
    "TRAILING",
    "TRAN",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "TRUNCATE",
    "TRY",
    "TYPE",
    "UNBOUNDED",
    "UNCOMMITTED",
    "UNION",
    "UNIQUE",
    "UNIQUEIDENTIFIER",
    "UNLOGGED",
    "UNPIVOT",
    "UPDATE",
    "USE",
    "USING",
    "UUID",
    "VACUUM",
    "VALUES",
    "VARCHAR",
    "VARYING",
    "VIEW",
    "VIRTUAL",
    "VOLATILE",
    "WAITFOR",
    "WHEN",
    "WHERE",
    "WHILE",
    "WINDOW",
    "WITH",
    "WITHOUT",
    "WORK",
    "XML",
    "ZONE",
];

fn is_keyword(word: &str) -> bool {
    if word.len() > 32 || !word.is_ascii() {
        return false;
    }
    let upper = word.to_ascii_uppercase();
    KEYWORDS.binary_search(&upper.as_str()).is_ok()
}

/// ¿Es `tok` un `GO` solo en su línea (admite espacios y un comentario `--` detrás)?
fn is_go_line(script: &str, tok: &Token) -> bool {
    if !script[tok.start..tok.end].eq_ignore_ascii_case("go") {
        return false;
    }
    let line_start = script[..tok.start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = script[tok.end..]
        .find('\n')
        .map_or(script.len(), |i| tok.end + i);
    let before = &script[line_start..tok.start];
    let after = script[tok.end..line_end].trim_start();
    before.trim().is_empty() && (after.is_empty() || after.starts_with("--"))
}

// ---------------------------------------------------------------------------
// Tokenizador
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Whitespace,
    Word,
    Number,
    Str,
    QuotedIdent,
    LineComment,
    BlockComment,
    Semicolon,
    Punct,
    Op,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy)]
struct Dialect {
    brackets: bool,
    backticks: bool,
    dollar_quotes: bool,
    nested_comments: bool,
    /// `@var` y `#temp` como palabras (T-SQL).
    tsql_names: bool,
    go_separator: bool,
}

impl Dialect {
    /// Para el resaltado, que no conoce el motor: acepta todas las formas de comillas.
    const PERMISSIVE: Dialect = Dialect {
        brackets: true,
        backticks: true,
        dollar_quotes: true,
        nested_comments: true,
        tsql_names: true,
        go_separator: false,
    };

    fn for_driver(driver: Driver) -> Self {
        match driver {
            Driver::Postgres => Dialect {
                brackets: false,
                backticks: false,
                dollar_quotes: true,
                nested_comments: true,
                tsql_names: false,
                go_separator: false,
            },
            Driver::Sqlite => Dialect {
                brackets: true,
                backticks: true,
                dollar_quotes: false,
                nested_comments: false,
                tsql_names: false,
                go_separator: false,
            },
            Driver::MsSql => Dialect {
                brackets: true,
                backticks: false,
                dollar_quotes: false,
                nested_comments: false,
                tsql_names: true,
                go_separator: true,
            },
        }
    }
}

/// Todas las decisiones se toman sobre bytes ASCII; los bytes >= 0x80 solo aparecen dentro
/// de palabras, cadenas o comentarios, así que los límites de token caen siempre en límite de char.
struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    dialect: Dialect,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str, dialect: Dialect) -> Self {
        Lexer {
            src: text.as_bytes(),
            pos: 0,
            dialect,
        }
    }

    fn peek(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn eat_while(&mut self, f: impl Fn(u8) -> bool) {
        while self.pos < self.src.len() && f(self.src[self.pos]) {
            self.pos += 1;
        }
    }

    /// Avanza hasta pasar `close` (si no aparece, hasta el final). `escape_double`: `''` → `'`.
    fn eat_quoted(&mut self, close: u8, escape_double: bool, backslash: bool) {
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            self.pos += 1;
            if backslash && b == b'\\' {
                self.pos = (self.pos + 1).min(self.src.len());
                // El byte saltado podría ser el inicio de un char multibyte: completa el char.
                self.eat_while(|b| (0x80..0xC0).contains(&b));
            } else if b == close {
                if escape_double && self.peek(0) == Some(close) {
                    self.pos += 1;
                } else {
                    return;
                }
            }
        }
    }

    fn eat_block_comment(&mut self) {
        self.pos += 2;
        let mut depth = 1usize;
        while self.pos < self.src.len() {
            if self.src[self.pos] == b'*' && self.peek(1) == Some(b'/') {
                self.pos += 2;
                depth -= 1;
                if depth == 0 {
                    return;
                }
            } else if self.dialect.nested_comments
                && self.src[self.pos] == b'/'
                && self.peek(1) == Some(b'*')
            {
                self.pos += 2;
                depth += 1;
            } else {
                self.pos += 1;
            }
        }
    }

    /// Si en la posición actual hay `$tag$`, devuelve su longitud.
    fn dollar_tag_len(&self) -> Option<usize> {
        let rest = &self.src[self.pos..];
        let mut i = 1;
        if rest.get(i).is_some_and(|b| b.is_ascii_digit()) {
            return None; // $1: parámetro
        }
        while rest
            .get(i)
            .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80)
        {
            i += 1;
        }
        (rest.get(i) == Some(&b'$')).then_some(i + 1)
    }

    fn is_word_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
    }

    fn next_kind(&mut self) -> Kind {
        let b = self.src[self.pos];
        let next = self.peek(1);
        match b {
            b if b.is_ascii_whitespace() => {
                self.eat_while(|b| b.is_ascii_whitespace());
                Kind::Whitespace
            }
            b'-' if next == Some(b'-') => {
                self.eat_while(|b| b != b'\n');
                Kind::LineComment
            }
            b'/' if next == Some(b'*') => {
                self.eat_block_comment();
                Kind::BlockComment
            }
            b'\'' => {
                self.pos += 1;
                self.eat_quoted(b'\'', true, false);
                Kind::Str
            }
            // Prefijos de cadena: E'..' (escapes con \), N'..', X'..', B'..'.
            b'e' | b'E' | b'n' | b'N' | b'x' | b'X' | b'b' | b'B' if next == Some(b'\'') => {
                self.pos += 2;
                self.eat_quoted(b'\'', true, matches!(b, b'e' | b'E'));
                Kind::Str
            }
            b'"' => {
                self.pos += 1;
                self.eat_quoted(b'"', true, false);
                Kind::QuotedIdent
            }
            b'[' if self.dialect.brackets && !self.follows_operand() => {
                self.pos += 1;
                self.eat_quoted(b']', true, false);
                Kind::QuotedIdent
            }
            b'`' if self.dialect.backticks => {
                self.pos += 1;
                self.eat_quoted(b'`', true, false);
                Kind::QuotedIdent
            }
            b'$' if self.dialect.dollar_quotes && self.dollar_tag_len().is_some() => {
                let len = self.dollar_tag_len().unwrap_or(1);
                let tag = &self.src[self.pos..self.pos + len];
                self.pos += len;
                let rest = &self.src[self.pos..];
                self.pos = rest
                    .windows(len)
                    .position(|w| w == tag)
                    .map_or(self.src.len(), |i| self.pos + i + len);
                Kind::Str
            }
            b'0'..=b'9' => {
                self.eat_number();
                Kind::Number
            }
            b'.' if next.is_some_and(|n| n.is_ascii_digit()) => {
                self.eat_number();
                Kind::Number
            }
            b'@' | b'#' if self.dialect.tsql_names && next.is_some_and(Self::is_word_byte) => {
                self.pos += 1;
                self.eat_while(|b| Self::is_word_byte(b) || b == b'@' || b == b'#');
                Kind::Word
            }
            b if b.is_ascii_alphabetic() || b == b'_' || b >= 0x80 => {
                self.eat_while(Self::is_word_byte);
                Kind::Word
            }
            b';' => {
                self.pos += 1;
                Kind::Semicolon
            }
            b'(' | b')' | b',' | b'.' | b'[' | b']' | b'{' | b'}' => {
                self.pos += 1;
                Kind::Punct
            }
            _ => {
                self.pos += 1;
                Kind::Op
            }
        }
    }

    fn eat_number(&mut self) {
        let mut prev = 0u8;
        while let Some(b) = self.peek(0) {
            let sign_in_exp = (b == b'+' || b == b'-')
                && (prev == b'e' || prev == b'E')
                && self.peek(1).is_some_and(|n| n.is_ascii_digit());
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || sign_in_exp {
                prev = b;
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// `arr[1]`: un `[` pegado a una palabra o `)` es subíndice, no identificador (salvo T-SQL).
    fn follows_operand(&self) -> bool {
        !self.dialect.go_separator
            && self.pos > 0
            && (matches!(self.src[self.pos - 1], b')' | b']' | b'"')
                || Self::is_word_byte(self.src[self.pos - 1]))
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        if self.pos >= self.src.len() {
            return None;
        }
        let start = self.pos;
        let kind = self.next_kind();
        debug_assert!(self.pos > start);
        Some(Token {
            kind,
            start,
            end: self.pos,
        })
    }
}

// ---------------------------------------------------------------------------
// Resaltado
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Normal,
    Keyword,
    Str,
    Number,
    Comment,
    Ident,
}

impl Style {
    fn of(tok: &Token, text: &str) -> Style {
        match tok.kind {
            Kind::Word if is_keyword(&text[tok.start..tok.end]) => Style::Keyword,
            Kind::Str => Style::Str,
            Kind::Number => Style::Number,
            Kind::LineComment | Kind::BlockComment => Style::Comment,
            Kind::QuotedIdent => Style::Ident,
            _ => Style::Normal,
        }
    }
}

struct Palette {
    text: Color32,
    keyword: Color32,
    string: Color32,
    number: Color32,
    comment: Color32,
    ident: Color32,
}

impl Palette {
    // Darcula (DataGrip oscuro)
    const DARK: Palette = Palette {
        text: Color32::from_rgb(0xBC, 0xBE, 0xC4),
        keyword: Color32::from_rgb(0xCF, 0x8E, 0x6D),
        string: Color32::from_rgb(0x6A, 0xAB, 0x73),
        number: Color32::from_rgb(0x2A, 0xAC, 0xB8),
        comment: Color32::from_rgb(0x7A, 0x7E, 0x85),
        ident: Color32::from_rgb(0xC7, 0x7D, 0xBB),
    };
    // IntelliJ Light
    const LIGHT: Palette = Palette {
        text: Color32::from_rgb(0x08, 0x08, 0x08),
        keyword: Color32::from_rgb(0x00, 0x33, 0xB3),
        string: Color32::from_rgb(0x06, 0x7D, 0x17),
        number: Color32::from_rgb(0x17, 0x50, 0xEB),
        comment: Color32::from_rgb(0x8C, 0x8C, 0x8C),
        ident: Color32::from_rgb(0x87, 0x10, 0x94),
    };

    fn format(&self, style: Style, font: &egui::FontId) -> TextFormat {
        let color = match style {
            Style::Normal => self.text,
            Style::Keyword => self.keyword,
            Style::Str => self.string,
            Style::Number => self.number,
            Style::Comment => self.comment,
            Style::Ident => self.ident,
        };
        let mut fmt = TextFormat::simple(font.clone(), color);
        fmt.italics = style == Style::Comment;
        fmt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(script: &str, driver: Driver) -> Vec<String> {
        split_statements(script, driver)
            .into_iter()
            .map(|s| s.text)
            .collect()
    }

    #[test]
    fn keywords_sorted_unique_uppercase() {
        assert!(KEYWORDS.len() >= 200);
        assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]));
        assert!(KEYWORDS.iter().all(|k| k.to_ascii_uppercase() == *k));
        assert!(is_keyword("select") && is_keyword("Where") && !is_keyword("usuarios"));
    }

    #[test]
    fn split_basic_and_ranges() {
        let s = "  SELECT 1 ;\n\nselect 2;select 3  ";
        let st = split_statements(s, Driver::Postgres);
        assert_eq!(st.len(), 3);
        for x in &st {
            assert_eq!(&s[x.start..x.end], x.text);
        }
        assert_eq!(st[0].text, "SELECT 1");
        assert_eq!(st[0].start, 2);
        assert_eq!(st[1].text, "select 2");
        assert_eq!(st[2].text, "select 3");
        assert_eq!(st[2].end, s.len() - 2);
    }

    #[test]
    fn semicolon_in_strings_and_comments() {
        let s = "SELECT 'a;b', \"x;y\" -- c;d\nFROM t /* e;f */ WHERE x = 'it''s;'; SELECT 2";
        let t = texts(s, Driver::Postgres);
        assert_eq!(t.len(), 2);
        assert!(t[0].ends_with("'it''s;'"));
        assert_eq!(t[1], "SELECT 2");
        assert_eq!(
            texts("SELECT [a;b] FROM `c;d`; SELECT 1", Driver::Sqlite).len(),
            2
        );
        assert_eq!(texts("SELECT [a;b]; SELECT 1", Driver::MsSql).len(), 2);
    }

    #[test]
    fn nested_comments_pg() {
        let t = texts("SELECT 1 /* a /* b; */ c; */; SELECT 2", Driver::Postgres);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn dollar_quoting() {
        let s = "CREATE FUNCTION f() RETURNS int AS $$ BEGIN; RETURN 1; END; $$ LANGUAGE plpgsql;\n\
                 DO $body$ BEGIN PERFORM 1; END $body$; SELECT $1";
        let t = texts(s, Driver::Postgres);
        assert_eq!(t.len(), 3, "{t:?}");
        assert!(t[0].ends_with("LANGUAGE plpgsql"));
        assert!(t[1].starts_with("DO $body$"));
        assert_eq!(t[2], "SELECT $1");
    }

    #[test]
    fn go_separator() {
        let s = "SELECT 1\nGO\nselect 2\n  go  -- fin\nSELECT 'GO'\n/*\nGO\n*/\nSELECT go_x\nGo";
        let t = texts(s, Driver::MsSql);
        assert_eq!(
            t,
            vec![
                "SELECT 1",
                "select 2",
                "SELECT 'GO'\n/*\nGO\n*/\nSELECT go_x"
            ]
        );
        // En Postgres GO no separa.
        assert_eq!(texts("SELECT 1\nGO\nSELECT 2", Driver::Postgres).len(), 1);
    }

    #[test]
    fn empty_and_comment_only() {
        assert!(texts("", Driver::Postgres).is_empty());
        assert!(texts(" ;; \n ; ", Driver::Postgres).is_empty());
        assert!(texts("-- solo\n/* comentario */;", Driver::MsSql).is_empty());
        let t = texts("-- cabecera\nSELECT 1; -- cola\n", Driver::Sqlite);
        assert_eq!(t, vec!["SELECT 1"]);
    }

    #[test]
    fn batches_keep_tsql_and_triggers_together() {
        let ms = split_batches("DECLARE @n int = 5;\nSELECT @n;\nGO\nSELECT 2;\n go -- fin\n;", Driver::MsSql);
        let texts: Vec<_> = ms.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["DECLARE @n int = 5;\nSELECT @n;", "SELECT 2;"]);
        let trig = "CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE a SET x = 1; END;\nSELECT 1;";
        let lite = split_batches(trig, Driver::Sqlite);
        assert_eq!(lite.len(), 1);
        assert_eq!(lite[0].text, trig);
        assert_eq!(split_batches("SELECT 1; SELECT 2", Driver::Postgres).len(), 2);
        assert!(split_batches("  ;; -- nada\n", Driver::MsSql).is_empty());
    }

    #[test]
    fn unclosed_constructs_do_not_panic() {
        for d in Driver::ALL {
            for s in [
                "SELECT 'abc",
                "SELECT \"abc",
                "SELECT /* abc",
                "SELECT $$ abc",
                "SELECT $tag$ abc $ta",
                "SELECT [abc",
                "[",
                "[x]",
                "[a] FROM t",
                "SELECT `abc",
                "SELECT E'abc\\",
                "SELECT E'\\ñ",
                "1e+",
                "'",
                "$",
                "--",
                "/*",
            ] {
                let t = texts(s, d);
                let expected = usize::from(!(s.starts_with("--") || s.starts_with("/*")));
                assert_eq!(t.len(), expected, "{s} {d:?}");
                assert_eq!(highlight_text(s), s);
                let _ = statement_at(s, 3, d);
            }
        }
        assert_eq!(texts("SELECT 'a; SELECT 2", Driver::Postgres).len(), 1);
    }

    #[test]
    fn unicode_text() {
        let s = "SELECT 'canción 🎵;' AS título; -- ñandú 🦤\nSELECT \"año\" FROM café;";
        for d in Driver::ALL {
            let st = split_statements(s, d);
            assert_eq!(st.len(), 2);
            assert_eq!(st[1].text, "SELECT \"año\" FROM café");
            for x in &st {
                assert_eq!(&s[x.start..x.end], x.text);
            }
            // Cualquier posición, incluso a mitad de un char multibyte.
            for c in 0..=s.len() + 5 {
                assert!(statement_at(s, c, d).is_some());
            }
        }
        assert_eq!(highlight_text(s), s);
    }

    #[test]
    fn statement_at_positions() {
        let s = "SELECT 1;\n\n  SELECT 2;   \n\nSELECT 3";
        let d = Driver::Postgres;
        let at = |c| statement_at(s, c, d).unwrap().text;
        assert_eq!(at(0), "SELECT 1");
        assert_eq!(at(3), "SELECT 1");
        assert_eq!(at(8), "SELECT 1"); // justo antes del ';'
        assert_eq!(at(9), "SELECT 1"); // tras el ';'
        assert_eq!(at(10), "SELECT 1"); // línea en blanco
        assert_eq!(at(11), "SELECT 2"); // sangría de la línea de SELECT 2
        let p2 = s.find("SELECT 2").unwrap();
        assert_eq!(at(p2 + 4), "SELECT 2");
        assert_eq!(at(p2 + 12), "SELECT 2"); // espacios tras el ';'
        assert_eq!(at(s.len()), "SELECT 3");
        assert_eq!(at(s.len() + 100), "SELECT 3");

        let s2 = "\n\n  -- hola\n SELECT 1; SELECT 2";
        assert_eq!(statement_at(s2, 0, d).unwrap().text, "SELECT 1");
        let st = statement_at(s2, s2.len(), d).unwrap();
        assert_eq!((st.start, st.end), (s2.len() - 8, s2.len()));
        assert_eq!(statement_at("  -- nada\n", 3, d), None);
        assert_eq!(statement_at("", 0, d), None);
    }

    fn highlight_text(s: &str) -> String {
        let job = highlight(s, true, egui::FontId::monospace(13.0));
        job.sections
            .iter()
            .map(|sec| &job.text[sec.byte_range.start.0..sec.byte_range.end.0])
            .collect()
    }

    #[test]
    fn highlight_reconstructs_and_colors() {
        let s = "select \"Nombre\", [x], 42, 'a''b' -- c\nFROM t /* d */ WHERE arr[1] = $$x$$;";
        assert_eq!(highlight_text(s), s);
        assert_eq!(highlight_text(""), "");
        let job = highlight(s, false, egui::FontId::monospace(13.0));
        let color_of = |needle: &str| {
            let at = s.find(needle).unwrap();
            job.sections
                .iter()
                .find(|sec| (sec.byte_range.start.0..sec.byte_range.end.0).contains(&at))
                .unwrap()
                .format
                .clone()
        };
        assert_eq!(color_of("select").color, Palette::LIGHT.keyword);
        assert_eq!(color_of("FROM").color, Palette::LIGHT.keyword);
        assert_eq!(color_of("42").color, Palette::LIGHT.number);
        assert_eq!(color_of("'a''b'").color, Palette::LIGHT.string);
        assert_eq!(color_of("\"Nombre\"").color, Palette::LIGHT.ident);
        assert_eq!(color_of("[x]").color, Palette::LIGHT.ident);
        assert_eq!(color_of("[1]").color, Palette::LIGHT.text);
        assert_eq!(color_of("$$x$$").color, Palette::LIGHT.string);
        let c = color_of("-- c");
        assert!(c.italics && c.color == Palette::LIGHT.comment);
        assert_eq!(color_of(" t ").color, Palette::LIGHT.text);
    }
}
