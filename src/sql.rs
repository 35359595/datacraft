//! SQL text utilities: statement splitting, statement under the cursor
//! and syntax highlighting for the editor.

use crate::db::Driver;
use egui::Color32;
use egui::text::{LayoutJob, TextFormat};

/// Script fragment with its byte range within the original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// Splits a script into statements. Honors single/double quotes, `[ident]` (SQL Server),
/// `$tag$...$tag$` (PostgreSQL), `--` and `/* */` comments. Separators: `;` and, on SQL Server,
/// a line containing only `GO` (case-insensitive). Skips empty or comment-only fragments.
/// `text` is trimmed and has no trailing `;`.
pub fn split_statements(script: &str, driver: Driver) -> Vec<Statement> {
    let dialect = Dialect::for_driver(driver);
    let mut out = Vec::new();
    // Range of significant tokens (neither whitespace nor comment) of the current statement.
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

/// Statement containing the `cursor` position (in bytes). If the cursor is in the space between
/// statements, returns the closest previous one (like DataGrip); if there is none, the next one.
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
    // If the cursor is in the indentation of the line where the next statement starts, that one.
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

/// Fragments to send to the server when running a whole script or a selection.
/// SQL Server: batches separated only by `GO` (a `DECLARE` or a `BEGIN…END` with `;` stay together).
/// SQLite: the whole script (the driver already walks it statement by statement, honoring triggers).
/// PostgreSQL, MySQL and Oracle: statements by `;` (dollar-quoting protects PG function bodies).
pub fn split_batches(script: &str, driver: Driver) -> Vec<Statement> {
    match driver {
        Driver::Postgres | Driver::MySql | Driver::MariaDb | Driver::Oracle => split_statements(script, driver),
        Driver::MsSql | Driver::Sqlite => {
            let dialect = Dialect::for_driver(driver);
            let mut out = Vec::new();
            let mut current: Option<(usize, usize)> = None;
            let mut flush = |current: &mut Option<(usize, usize)>| {
                if let Some((start, end)) = current.take() {
                    let text = script[start..end].to_string();
                    // A batch made only of `;` adds nothing.
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

/// Colored LayoutJob for `egui::TextEdit::layouter`.
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
        // Group consecutive tokens with the same style so the sections don't pile up.
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

/// Common SQL keywords (uppercase), used by highlighting and autocompletion.
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
    "APPLY",
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

/// Is `tok` a `GO` alone on its line (allowing spaces and a trailing `--` comment)?
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
// Explain plan
// ---------------------------------------------------------------------------

/// Statements that show the execution plan of `stmt` on each engine, to run one by one
/// (on SQL Server each one is its own batch: `SET SHOWPLAN_TEXT` must be alone in its batch).
/// Empty if `stmt` has nothing to explain.
pub fn explain_statements(stmt: &str, driver: Driver) -> Vec<String> {
    let s = stmt.trim().trim_end_matches(';').trim_end();
    if s.is_empty() {
        return Vec::new();
    }
    match driver {
        Driver::Postgres | Driver::MySql | Driver::MariaDb => vec![format!("EXPLAIN {s}")],
        Driver::Sqlite => vec![format!("EXPLAIN QUERY PLAN {s}")],
        Driver::MsSql => vec!["SET SHOWPLAN_TEXT ON".into(), s.to_string(), "SET SHOWPLAN_TEXT OFF".into()],
        Driver::Oracle => vec![
            format!("EXPLAIN PLAN FOR {s}"),
            "SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY())".into(),
        ],
    }
}

// ---------------------------------------------------------------------------
// Formatter
// ---------------------------------------------------------------------------

/// Keywords that are never used as plain identifiers in practice: always uppercased.
/// The rest of [`KEYWORDS`] (types and nouns such as `date`, `type`, `key`, `text`) are
/// uppercased only where they cannot be a table, column or alias name.
const RESERVED: &[&str] = &[
    "ALL", "ALTER", "AND", "ANY", "AS", "ASC", "BEGIN", "BETWEEN", "BY", "CASE", "CAST", "CHECK", "COMMIT",
    "CONSTRAINT", "CREATE", "CROSS", "DECLARE", "DEFAULT", "DELETE", "DESC", "DISTINCT", "DROP", "ELSE", "END",
    "EXCEPT", "EXEC", "EXECUTE", "EXISTS", "EXPLAIN", "FALSE", "FOREIGN", "FROM", "FULL", "FUNCTION", "GRANT",
    "GROUP", "HAVING", "IF", "ILIKE", "IN", "INDEX", "INNER", "INSERT", "INTERSECT", "INTO", "IS", "JOIN",
    "LATERAL", "LEFT", "LIKE", "LIMIT", "NATURAL", "NOT", "NULL", "OFFSET", "ON", "ONLY", "OR", "ORDER", "OUTER",
    "PRIMARY", "PROCEDURE", "REFERENCES", "RETURNING", "REVOKE", "RIGHT", "ROLLBACK", "SELECT", "SET", "SOME",
    "TABLE", "THEN", "TOP", "TRIGGER", "TRUE", "TRUNCATE", "UNION", "UNIQUE", "UPDATE", "USING", "VALUES", "VIEW",
    "WHEN", "WHERE", "WITH",
];

/// Words after which a non-reserved keyword is a name (`FROM date`, `AS type`...).
const NAME_INTRODUCERS: &[&str] = &["AS", "FROM", "JOIN", "INTO", "UPDATE", "TABLE", "VIEW"];

/// Keywords followed by a space before `(` (never added after other words: on MySQL a
/// space between a built-in function and `(` changes how it is parsed).
const SPACE_BEFORE_PAREN: &[&str] = &[
    "AND", "AS", "ELSE", "EXCEPT", "EXISTS", "FROM", "IN", "INTERSECT", "JOIN", "NOT", "ON", "OR", "THEN", "UNION",
    "USING", "WHEN", "WHERE",
];

const JOIN_MODIFIERS: &[&str] = &["INNER", "LEFT", "RIGHT", "FULL", "OUTER", "CROSS", "NATURAL"];

/// Words that start a main clause (a comment right before one is indented like it).
const CLAUSE_WORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "UNION", "INTERSECT", "EXCEPT", "VALUES", "SET",
    "JOIN", "RETURNING",
];

/// Clause of a query level, which decides how commas and `AND`/`OR` are laid out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Clause {
    None,
    With,
    Select,
    From,
    Where,
    Having,
    Set,
    Values,
    Other,
}

impl Clause {
    /// One item per line after a comma.
    fn is_list(self) -> bool {
        matches!(self, Clause::With | Clause::Select | Clause::Set | Clause::Values)
    }
}

/// Nesting level: the whole statement or a parenthesis.
struct Level {
    /// Column where its lines start (the statement's or the one right after the `(`).
    base: usize,
    /// Statement or subquery (clauses go on their own lines); otherwise a plain parenthesis
    /// (function arguments, column lists...) laid out on one line.
    query: bool,
    clause: Clause,
    /// Column of the first item of a list clause, where the next items are aligned.
    list_col: Option<usize>,
    /// Inside `BETWEEN … AND`: that `AND` doesn't break the line.
    between: bool,
    case_depth: usize,
    /// No token emitted in this level yet.
    empty: bool,
}

impl Level {
    fn new(base: usize, query: bool) -> Self {
        Level { base, query, clause: Clause::None, list_col: None, between: false, case_depth: 0, empty: true }
    }

    /// Column for a line that continues the current clause.
    fn cont_col(&self) -> usize {
        if !self.query || self.empty {
            self.base
        } else if self.clause.is_list() {
            self.list_col.unwrap_or(self.base + 4)
        } else {
            self.base + 4
        }
    }
}

/// Formatted output with column tracking.
struct Out {
    s: String,
    /// Column where the text starts (the first line is not indented by the formatter).
    indent0: usize,
}

impl Out {
    fn col(&self) -> usize {
        match self.s.rfind('\n') {
            Some(i) => self.s[i + 1..].chars().count(),
            None => self.indent0 + self.s.chars().count(),
        }
    }

    fn at_line_start(&self) -> bool {
        match self.s.rfind('\n') {
            Some(i) => self.s[i + 1..].bytes().all(|b| b == b' '),
            None => self.s.is_empty(),
        }
    }

    /// New line (or the current one if it is still empty) indented to `col`; `blank` leaves
    /// an empty line before it. Nothing at the very start of the text.
    fn newline(&mut self, col: usize, blank: bool) {
        let keep = self.s.trim_end_matches(' ').len();
        self.s.truncate(keep);
        if self.s.is_empty() {
            return;
        }
        if !self.s.ends_with('\n') {
            self.s.push('\n');
        }
        if blank && !self.s.ends_with("\n\n") {
            self.s.push('\n');
        }
        self.s.extend(std::iter::repeat_n(' ', col));
    }
}

/// Reformats SQL like DataGrip's "Reformat Code": keywords in uppercase, each main clause
/// (`SELECT`, `FROM`, `WHERE`, `GROUP BY`, `HAVING`, `ORDER BY`, `LIMIT`, joins, `UNION`,
/// `VALUES`, `SET`, `RETURNING`) on its own line, one select / `SET` / `VALUES` / CTE item per
/// line aligned with the first one, `AND` / `OR` of `WHERE` and `HAVING` on their own lines and
/// subqueries aligned after their `(`.
///
/// Only whitespace and the case of keywords change: strings, comments, quoted identifiers
/// and dollar-quoted bodies are copied verbatim, the spacing around operators is kept as
/// typed (`::`, `->>`, `-1`, `$1`, `@var`), blank lines are kept and a line comment always
/// ends its line. Non-reserved keywords that `is_ident` reports as known names (tables,
/// columns, lowercase) keep their case. The result is stable: formatting it again changes
/// nothing. `indent` is the column where `text` starts in the editor.
pub fn format_sql(text: &str, driver: Driver, indent: usize, is_ident: &dyn Fn(&str) -> bool) -> String {
    let dialect = Dialect::for_driver(driver);
    let toks: Vec<Token> = Lexer::new(text, dialect).filter(|t| t.kind != Kind::Whitespace).collect();
    let src = |t: &Token| &text[t.start..t.end];
    let is_comment = |t: &Token| matches!(t.kind, Kind::LineComment | Kind::BlockComment);
    let upper = |t: &Token| if t.kind == Kind::Word { src(t).to_ascii_uppercase() } else { src(t).to_string() };
    // The next `n` significant tokens after `i` (words uppercased).
    let peek = |i: usize, n: usize| -> Vec<String> {
        toks[i + 1..].iter().filter(|t| !is_comment(t)).take(n).map(upper).collect()
    };

    let mut out = Out { s: String::with_capacity(text.len() + text.len() / 4), indent0: indent };
    let mut stack = vec![Level::new(indent, true)];
    let mut stmt_start = true;
    let mut stmt_kind = String::new();
    // A line comment was just emitted: the next token goes on a new line.
    let mut force_nl = false;
    // After a list comma: column of the next item's line.
    let mut comma_nl: Option<usize> = None;
    // Remaining words of a multi-word clause keyword (`GROUP BY`, `LEFT OUTER JOIN`).
    let mut phrase_left = 0usize;
    let mut want_list_col = false;
    let mut prev: Option<Token> = None;
    let mut prev_sig: Option<Token> = None;
    let mut prev_word = String::new();
    let mut prev_end = 0usize;
    // `ON DUPLICATE KEY UPDATE` / `ON CONFLICT DO UPDATE`: `VALUES(x)` is a function there.
    let mut upsert = false;
    let mut after_fake_go = false;

    for (i, tok) in toks.iter().enumerate() {
        let gap = &text[prev_end..tok.start];
        prev_end = tok.end;
        let had_ws = !gap.is_empty();
        let src_nl = gap.contains('\n');
        let blank = gap.matches('\n').count() >= 2;
        let raw = src(tok);

        if is_comment(tok) {
            let lvl = stack.last().expect("root level");
            let next_breaks = peek(i, 1)
                .first()
                .is_some_and(|w| CLAUSE_WORDS.contains(&w.as_str()) || JOIN_MODIFIERS.contains(&w.as_str()));
            let col = if stmt_start {
                stack[0].base
            } else if next_breaks && lvl.query {
                lvl.base
            } else {
                comma_nl.unwrap_or_else(|| lvl.cont_col())
            };
            if force_nl || src_nl {
                out.newline(col, blank);
            } else if !out.at_line_start() {
                out.s.push(' ');
            }
            out.s.push_str(if tok.kind == Kind::LineComment { raw.trim_end() } else { raw });
            force_nl = tok.kind == Kind::LineComment;
            prev = Some(*tok);
            continue;
        }

        let word = upper(tok);
        let is_word = tok.kind == Kind::Word;

        // T-SQL `GO`: alone on its line, ends the batch.
        if is_word && dialect.go_separator && is_go_line(text, tok) {
            out.newline(0, blank);
            out.s.push_str(raw);
            force_nl = true;
            stack.truncate(1);
            stack[0] = Level::new(indent, true);
            stmt_start = true;
            upsert = false;
            comma_nl = None;
            phrase_left = 0;
            want_list_col = false;
            prev = Some(*tok);
            prev_sig = Some(*tok);
            prev_word.clear();
            continue;
        }

        let depth = stack.len();
        let lvl = stack.last_mut().expect("root level");
        let in_phrase = phrase_left > 0;
        if in_phrase {
            phrase_left -= 1;
        }

        // Main clause starting at this word?
        let mut clause: Option<(Clause, usize)> = None;
        if is_word && lvl.query && !in_phrase {
            let next = peek(i, 4);
            let n0 = next.first().map(String::as_str).unwrap_or("");
            clause = match word.as_str() {
                "WITH" if lvl.empty => Some((Clause::With, 1)),
                "SELECT" => Some((Clause::Select, 1)),
                "FROM" if prev_word != "DELETE" && prev_word != "DISTINCT" => Some((Clause::From, 1)),
                "WHERE" => Some((Clause::Where, 1)),
                "GROUP" | "ORDER" if n0 == "BY" => Some((Clause::Other, 2)),
                "HAVING" => Some((Clause::Having, 1)),
                "LIMIT" | "RETURNING" => Some((Clause::Other, 1)),
                "UNION" | "INTERSECT" | "EXCEPT" => {
                    Some((Clause::None, if n0 == "ALL" || n0 == "DISTINCT" { 2 } else { 1 }))
                }
                "VALUES" if prev_word != "DEFAULT" && !upsert => Some((Clause::Values, 1)),
                "SET" if depth == 1 && stmt_kind == "UPDATE" => Some((Clause::Set, 1)),
                "JOIN" | "STRAIGHT_JOIN" => Some((Clause::From, 1)),
                w if JOIN_MODIFIERS.contains(&w) => {
                    let k = next.iter().take_while(|n| JOIN_MODIFIERS.contains(&n.as_str())).count();
                    matches!(next.get(k).map(String::as_str), Some("JOIN" | "APPLY")).then_some((Clause::From, k + 2))
                }
                _ => None,
            };
        }

        // Where this token goes: a new line at some column, or the current line.
        let mut nl: Option<usize> = None;
        let pending_comma = comma_nl.take();
        if stmt_start {
            if !out.s.is_empty() {
                nl = Some(lvl.base);
            }
        } else if clause.is_some() && !lvl.empty {
            nl = Some(lvl.base);
        } else if is_word
            && (word == "AND" || word == "OR")
            && !in_phrase
            && matches!(lvl.clause, Clause::Where | Clause::Having)
            && lvl.case_depth == 0
        {
            if word == "AND" && lvl.between {
                lvl.between = false;
            } else {
                nl = Some(lvl.base + if word == "AND" { 2 } else { 3 });
            }
        } else if let Some(col) = pending_comma {
            nl = Some(col);
        }
        let closing =
            tok.kind == Kind::Semicolon || (tok.kind == Kind::Punct && matches!(raw, "," | ")" | "."));
        if nl.is_none() && (force_nl || (blank && !closing)) {
            nl = Some(lvl.cont_col());
        }
        // A T-SQL `go` that is not a batch separator must never end up alone on its line
        // (it would become one): it stays with the previous token and the next one with it.
        let fake_go = is_word && dialect.go_separator && word == "GO";
        if (fake_go || after_fake_go) && !force_nl {
            nl = None;
        }
        after_fake_go = fake_go;
        match nl {
            Some(col) => out.newline(col, blank && !closing),
            None => {
                if let Some(p) = prev
                    && !out.at_line_start()
                    && space_between(&p, src(&p), &prev_word, tok, raw, had_ws)
                {
                    out.s.push(' ');
                }
            }
        }
        force_nl = false;

        if want_list_col && !in_phrase {
            lvl.list_col = Some(out.col());
            want_list_col = false;
        }

        // Keyword case.
        let next_is_dot = peek(i, 1).first().is_some_and(|n| n == ".");
        let prev_is_dot = prev_sig.is_some_and(|p| p.kind == Kind::Punct && src(&p) == ".");
        let upper_it = is_word && is_keyword(raw) && !prev_is_dot && !next_is_dot && {
            let reserved = RESERVED.contains(&word.as_str());
            let cast_type = prev_word == "AS" && !lvl.query;
            let name_pos = NAME_INTRODUCERS.contains(&prev_word.as_str()) && !cast_type;
            reserved || (!name_pos && !is_ident(&raw.to_lowercase()))
        };
        out.s.push_str(if upper_it { &word } else { raw });

        // State after the token.
        lvl.empty = false;
        if let Some((c, len)) = clause {
            lvl.clause = c;
            lvl.list_col = None;
            lvl.between = false;
            want_list_col = c.is_list();
            phrase_left = len - 1;
        }
        if is_word {
            match word.as_str() {
                "CASE" => lvl.case_depth += 1,
                "END" => lvl.case_depth = lvl.case_depth.saturating_sub(1),
                "BETWEEN" => lvl.between = true,
                _ => {}
            }
        }
        if tok.kind == Kind::Punct
            && raw == ","
            && lvl.case_depth == 0
            && (matches!(lvl.clause, Clause::With | Clause::Select | Clause::Set)
                || (lvl.clause == Clause::Values && prev_sig.is_some_and(|p| src(&p) == ")")))
        {
            comma_nl = Some(lvl.list_col.unwrap_or(lvl.base + 4));
        }
        if stmt_start {
            stmt_kind = word.clone();
            stmt_start = false;
        } else if is_word && word == "UPDATE" && lvl.query {
            lvl.clause = Clause::Other;
            upsert = true;
        }
        if tok.kind == Kind::Punct && raw == "(" {
            let query = peek(i, 1).first().is_some_and(|n| matches!(n.as_str(), "SELECT" | "WITH" | "VALUES"));
            let col = out.col();
            stack.push(Level::new(col, query));
        } else if tok.kind == Kind::Punct && raw == ")" {
            if stack.len() > 1 {
                stack.pop();
            }
        } else if tok.kind == Kind::Semicolon {
            stack.truncate(1);
            stack[0] = Level::new(indent, true);
            stmt_start = true;
            upsert = false;
            comma_nl = None;
            phrase_left = 0;
            want_list_col = false;
        }
        prev = Some(*tok);
        prev_sig = Some(*tok);
        prev_word = if is_word { word } else { String::new() };
    }
    out.s
}

/// Whether a space goes between two tokens on the same line. Never glues tokens that were
/// apart in a way that matters, nor separates operators typed together.
fn space_between(p: &Token, ptext: &str, pword: &str, c: &Token, ctext: &str, had_ws: bool) -> bool {
    let is = |t: &Token, text: &str, s: &str| t.kind == Kind::Punct && text == s;
    if c.kind == Kind::Semicolon || is(c, ctext, ",") || is(c, ctext, ")") {
        return false;
    }
    if is(p, ptext, ".") || is(c, ctext, ".") {
        // `1 .x` / `x. 5` would lex as a number if glued.
        return had_ws && (p.kind == Kind::Number || c.kind == Kind::Number);
    }
    if is(p, ptext, "(") {
        return false;
    }
    if is(p, ptext, ",") || matches!(p.kind, Kind::LineComment | Kind::BlockComment) {
        return true;
    }
    let tight =
        |t: &Token, text: &str| t.kind == Kind::Op || (t.kind == Kind::Punct && matches!(text, "[" | "]" | "{" | "}"));
    if tight(p, ptext) || tight(c, ctext) {
        return had_ws;
    }
    // Prefixed literals (`_utf8'x'`, Oracle `q'[…]'`): keep them glued.
    if !had_ws
        && matches!(c.kind, Kind::Str | Kind::QuotedIdent)
        && matches!(p.kind, Kind::Word | Kind::Number | Kind::Str | Kind::QuotedIdent)
    {
        return false;
    }
    if is(c, ctext, "(") {
        return had_ws || (p.kind == Kind::Word && SPACE_BEFORE_PAREN.contains(&pword));
    }
    true
}

// ---------------------------------------------------------------------------
// Tokenizer
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
    /// `@var` and `#temp` as words (T-SQL).
    tsql_names: bool,
    go_separator: bool,
    /// `#` starts a line comment (MySQL / MariaDB).
    hash_comments: bool,
}

impl Dialect {
    /// For highlighting, which doesn't know the engine: accepts every quoting form.
    const PERMISSIVE: Dialect = Dialect {
        brackets: true,
        backticks: true,
        dollar_quotes: true,
        nested_comments: true,
        tsql_names: true,
        go_separator: false,
        hash_comments: false,
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
                hash_comments: false,
            },
            Driver::Sqlite => Dialect {
                brackets: true,
                backticks: true,
                dollar_quotes: false,
                nested_comments: false,
                tsql_names: false,
                go_separator: false,
                hash_comments: false,
            },
            Driver::MsSql => Dialect {
                brackets: true,
                backticks: false,
                dollar_quotes: false,
                nested_comments: false,
                tsql_names: true,
                go_separator: true,
                hash_comments: false,
            },
            Driver::MySql | Driver::MariaDb => Dialect {
                brackets: false,
                backticks: true,
                dollar_quotes: false,
                nested_comments: false,
                tsql_names: false,
                go_separator: false,
                hash_comments: true,
            },
            Driver::Oracle => Dialect {
                brackets: false,
                backticks: false,
                dollar_quotes: false,
                nested_comments: false,
                tsql_names: false,
                go_separator: false,
                hash_comments: false,
            },
        }
    }
}

/// Every decision is made on ASCII bytes; bytes >= 0x80 only appear inside words, strings
/// or comments, so token boundaries always fall on char boundaries.
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

    /// Advances past `close` (to the end if it doesn't appear). `escape_double`: `''` → `'`.
    fn eat_quoted(&mut self, close: u8, escape_double: bool, backslash: bool) {
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            self.pos += 1;
            if backslash && b == b'\\' {
                self.pos = (self.pos + 1).min(self.src.len());
                // The skipped byte could start a multibyte char: complete the char.
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

    /// If there is a `$tag$` at the current position, returns its length.
    fn dollar_tag_len(&self) -> Option<usize> {
        let rest = &self.src[self.pos..];
        let mut i = 1;
        if rest.get(i).is_some_and(|b| b.is_ascii_digit()) {
            return None; // $1: parameter
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
            b'#' if self.dialect.hash_comments => {
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
            // String prefixes: E'..' (escapes with \), N'..', X'..', B'..'.
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

    /// `arr[1]`: a `[` right after a word or `)` is a subscript, not an identifier (except in T-SQL).
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
// Highlighting
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
    // Islands "Dark" scheme (DataGrip 2026)
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
        assert!(is_keyword("select") && is_keyword("Where") && !is_keyword("customers"));
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
        let s = "SELECT 1\nGO\nselect 2\n  go  -- end\nSELECT 'GO'\n/*\nGO\n*/\nSELECT go_x\nGo";
        let t = texts(s, Driver::MsSql);
        assert_eq!(
            t,
            vec![
                "SELECT 1",
                "select 2",
                "SELECT 'GO'\n/*\nGO\n*/\nSELECT go_x"
            ]
        );
        // On Postgres GO doesn't separate.
        assert_eq!(texts("SELECT 1\nGO\nSELECT 2", Driver::Postgres).len(), 1);
    }

    #[test]
    fn empty_and_comment_only() {
        assert!(texts("", Driver::Postgres).is_empty());
        assert!(texts(" ;; \n ; ", Driver::Postgres).is_empty());
        assert!(texts("-- only\n/* comment */;", Driver::MsSql).is_empty());
        let t = texts("-- header\nSELECT 1; -- tail\n", Driver::Sqlite);
        assert_eq!(t, vec!["SELECT 1"]);
    }

    #[test]
    fn batches_keep_tsql_and_triggers_together() {
        let ms = split_batches("DECLARE @n int = 5;\nSELECT @n;\nGO\nSELECT 2;\n go -- end\n;", Driver::MsSql);
        let texts: Vec<_> = ms.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["DECLARE @n int = 5;\nSELECT @n;", "SELECT 2;"]);
        let trig = "CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE a SET x = 1; END;\nSELECT 1;";
        let lite = split_batches(trig, Driver::Sqlite);
        assert_eq!(lite.len(), 1);
        assert_eq!(lite[0].text, trig);
        assert_eq!(split_batches("SELECT 1; SELECT 2", Driver::Postgres).len(), 2);
        assert!(split_batches("  ;; -- nothing\n", Driver::MsSql).is_empty());
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
            // Any position, even in the middle of a multibyte char.
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
        assert_eq!(at(8), "SELECT 1"); // right before the ';'
        assert_eq!(at(9), "SELECT 1"); // after the ';'
        assert_eq!(at(10), "SELECT 1"); // blank line
        assert_eq!(at(11), "SELECT 2"); // indentation of the SELECT 2 line
        let p2 = s.find("SELECT 2").unwrap();
        assert_eq!(at(p2 + 4), "SELECT 2");
        assert_eq!(at(p2 + 12), "SELECT 2"); // spaces after the ';'
        assert_eq!(at(s.len()), "SELECT 3");
        assert_eq!(at(s.len() + 100), "SELECT 3");

        let s2 = "\n\n  -- hello\n SELECT 1; SELECT 2";
        assert_eq!(statement_at(s2, 0, d).unwrap().text, "SELECT 1");
        let st = statement_at(s2, s2.len(), d).unwrap();
        assert_eq!((st.start, st.end), (s2.len() - 8, s2.len()));
        assert_eq!(statement_at("  -- nothing\n", 3, d), None);
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
        let s = "select \"Name\", [x], 42, 'a''b' -- c\nFROM t /* d */ WHERE arr[1] = $$x$$;";
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
        assert_eq!(color_of("\"Name\"").color, Palette::LIGHT.ident);
        assert_eq!(color_of("[x]").color, Palette::LIGHT.ident);
        assert_eq!(color_of("[1]").color, Palette::LIGHT.text);
        assert_eq!(color_of("$$x$$").color, Palette::LIGHT.string);
        let c = color_of("-- c");
        assert!(c.italics && c.color == Palette::LIGHT.comment);
        assert_eq!(color_of(" t ").color, Palette::LIGHT.text);
    }

    // ------------------------------------------------------------------ formatter

    /// Significant tokens (words compared without case, line comments trimmed).
    fn token_texts(s: &str, d: Driver) -> Vec<(Kind, String)> {
        Lexer::new(s, Dialect::for_driver(d))
            .filter(|t| t.kind != Kind::Whitespace)
            .map(|t| {
                let text = &s[t.start..t.end];
                let text = match t.kind {
                    Kind::Word => text.to_ascii_uppercase(),
                    Kind::LineComment => text.trim_end().to_string(),
                    _ => text.to_string(),
                };
                (t.kind, text)
            })
            .collect()
    }

    /// Formats checking that only whitespace / keyword case changed and that it is stable.
    fn fmt_core(s: &str, d: Driver, idents: &[&str]) -> String {
        let is_ident = |w: &str| idents.contains(&w);
        let out = format_sql(s, d, 0, &is_ident);
        assert_eq!(token_texts(s, d), token_texts(&out, d), "tokens changed:\n{s}\n---\n{out}");
        let again = format_sql(&out, d, 0, &is_ident);
        assert_eq!(out, again, "not idempotent:\n{out}\n---\n{again}");
        out
    }

    fn fmt_with(s: &str, d: Driver, idents: &[&str]) -> String {
        let out = fmt_core(s, d, idents);
        assert!(!out.lines().any(|l| l.ends_with(' ')), "trailing spaces:\n{out}");
        out
    }

    fn fmt(s: &str, d: Driver) -> String {
        fmt_with(s, d, &[])
    }

    #[test]
    fn format_basic_select() {
        let out = fmt("select o.id, u.name, o.total from orders o join users u on u.id = o.user_id where o.total > 10 and u.name like 'a%' order by o.total desc limit 5", Driver::Postgres);
        assert_eq!(
            out,
            "SELECT o.id,\n       u.name,\n       o.total\nFROM orders o\nJOIN users u ON u.id = o.user_id\nWHERE o.total > 10\n  AND u.name LIKE 'a%'\nORDER BY o.total DESC\nLIMIT 5"
        );
        // A single item stays on the SELECT line.
        assert_eq!(fmt("SELECT *   FROM t", Driver::Sqlite), "SELECT *\nFROM t");
        assert_eq!(fmt("select count(*) from t", Driver::Sqlite), "SELECT count(*)\nFROM t");
    }

    #[test]
    fn format_joins_groups_and_set_ops() {
        let out = fmt(
            "select a, count(*) from t left outer join u on t.id=u.id cross join v group by a having count(*) > 1 or a is null union all select 1, 2",
            Driver::MySql,
        );
        assert_eq!(
            out,
            "SELECT a,\n       count(*)\nFROM t\nLEFT OUTER JOIN u ON t.id=u.id\nCROSS JOIN v\nGROUP BY a\nHAVING count(*) > 1\n   OR a IS NULL\nUNION ALL\nSELECT 1,\n       2"
        );
        // LEFT( is a function, not a join.
        assert_eq!(fmt("select left(name, 2) from t", Driver::MySql), "SELECT LEFT(name, 2)\nFROM t");
        // T-SQL APPLY.
        assert!(fmt("select * from a cross apply f(a.x) b", Driver::MsSql).contains("\nCROSS APPLY f(a.x) b"));
    }

    #[test]
    fn format_dml() {
        assert_eq!(
            fmt("insert into t (a, b) values (1, 'x'), (2, 'y')", Driver::Postgres),
            "INSERT INTO t (a, b)\nVALUES (1, 'x'),\n       (2, 'y')"
        );
        assert_eq!(
            fmt("update t set a = 1, b = 'z' where id = 3 returning id", Driver::Postgres),
            "UPDATE t\nSET a = 1,\n    b = 'z'\nWHERE id = 3\nRETURNING id"
        );
        assert_eq!(fmt("delete from t where a between 1 and 5 and b = 2", Driver::Sqlite), "DELETE FROM t\nWHERE a BETWEEN 1 AND 5\n  AND b = 2");
        // ALTER … SET is not an UPDATE clause; DEFAULT VALUES stays together.
        assert_eq!(fmt("alter table t alter column c set default 1", Driver::Postgres), "ALTER TABLE t ALTER COLUMN c SET DEFAULT 1");
        assert_eq!(fmt("insert into t default values", Driver::Postgres), "INSERT INTO t DEFAULT VALUES");
        // MySQL ON DUPLICATE KEY UPDATE: VALUES() is a function there and keeps no space.
        let out = fmt("insert into t (a) values (1) on duplicate key update a = values(a), b = 2", Driver::MySql);
        assert!(out.ends_with("ON duplicate KEY UPDATE a = VALUES(a), b = 2"), "{out}");
    }

    #[test]
    fn format_subqueries_and_ctes() {
        assert_eq!(
            fmt("select * from (select a, b from t where x = 1) s where s.a in (select id from u)", Driver::Postgres),
            "SELECT *\nFROM (SELECT a,\n             b\n      FROM t\n      WHERE x = 1) s\nWHERE s.a IN (SELECT id\n              FROM u)"
        );
        assert_eq!(
            fmt("with a as (select 1 as x), b as (select 2 as y) select * from a, b", Driver::Postgres),
            "WITH a AS (SELECT 1 AS x),\n     b AS (SELECT 2 AS y)\nSELECT *\nFROM a, b"
        );
        assert_eq!(fmt("select exists(select 1 from t)", Driver::Postgres), "SELECT EXISTS (SELECT 1\n               FROM t)");
        // Non-query parentheses stay on one line, even with clause words inside.
        assert_eq!(fmt("select extract(year from d), trim(both 'x' from s) from t", Driver::Postgres), "SELECT extract(year FROM d),\n       trim(BOTH 'x' FROM s)\nFROM t");
        assert_eq!(
            fmt("select row_number() over (partition by a order by b) from t", Driver::Postgres),
            "SELECT row_number() OVER (PARTITION BY a ORDER BY b)\nFROM t"
        );
        assert_eq!(fmt("select a is distinct from b from t", Driver::Postgres), "SELECT a IS DISTINCT FROM b\nFROM t");
    }

    #[test]
    fn format_case_and_between() {
        assert_eq!(
            fmt("select case when a > 1 and b < 2 then 'x' else 'y' end as c from t where a between 1 and 2 or b = 1", Driver::Sqlite),
            "SELECT CASE WHEN a > 1 AND b < 2 THEN 'x' ELSE 'y' END AS c\nFROM t\nWHERE a BETWEEN 1 AND 2\n   OR b = 1"
        );
    }

    #[test]
    fn format_keeps_strings_comments_and_quotes() {
        // Keywords inside strings, quoted identifiers and comments are untouched.
        let out = fmt("select 'select from where', \"from\" , [order], `group` -- select x from y\nfrom t /* where and or */ where a='and or'", Driver::Sqlite);
        assert_eq!(
            out,
            "SELECT 'select from where',\n       \"from\",\n       [order],\n       `group` -- select x from y\nFROM t /* where and or */\nWHERE a='and or'"
        );
        // Line comment on its own line keeps its line; the next token never joins it.
        let out = fmt("-- header\nselect a, -- first\n b\nfrom t", Driver::Postgres);
        assert_eq!(out, "-- header\nSELECT a, -- first\n       b\nFROM t");
        // Comment before a clause goes at the clause's indentation.
        assert_eq!(fmt("select a\n-- the table\nfrom t", Driver::Postgres), "SELECT a\n-- the table\nFROM t");
        // MySQL # comments.
        let out = fmt("select a # from x\nfrom t", Driver::MySql);
        assert_eq!(out, "SELECT a # from x\nFROM t");
    }

    #[test]
    fn format_dialect_quoting() {
        // PG dollar quotes, casts, JSON operators and parameters keep their spacing.
        let s = "create function f() returns int as $$ begin\n  select  1 from t;  return 1; end $$ language plpgsql;\nselect x::text, j->>'k', -1, $1 from t where a=$2";
        let out = fmt(s, Driver::Postgres);
        assert!(out.contains("$$ begin\n  select  1 from t;  return 1; end $$"), "{out}");
        assert!(out.ends_with("SELECT x::TEXT,\n       j->>'k',\n       -1,\n       $1\nFROM t\nWHERE a=$2"), "{out}");
        // T-SQL brackets, variables, temp tables and GO.
        let out = fmt("declare @n int = 5\nselect [Order Id], @n from #tmp where [x] = @n\ngo\nselect 1", Driver::MsSql);
        assert_eq!(out, "DECLARE @n INT = 5\nSELECT [Order Id],\n       @n\nFROM #tmp\nWHERE [x] = @n\ngo\nSELECT 1");
        // MySQL backticks and user variables.
        assert_eq!(fmt("select `from`, @x:=1 from `select`", Driver::MySql), "SELECT `from`,\n       @x:=1\nFROM `select`");
        // Prefixed literals stay glued.
        assert_eq!(fmt("select _utf8'x', N'y'", Driver::MySql), "SELECT _utf8'x',\n       N'y'");
    }

    #[test]
    fn format_statements_and_blank_lines() {
        let out = fmt("select 1;select 2;\n\n\nselect 3", Driver::Sqlite);
        assert_eq!(out, "SELECT 1;\nSELECT 2;\n\nSELECT 3");
        let out = fmt("CREATE TABLE IF NOT EXISTS t (id integer primary key, name text not null, price real);", Driver::Sqlite);
        assert_eq!(out, "CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, price REAL);");
    }

    #[test]
    fn format_keyword_case_of_names() {
        // Non-reserved keywords used as names keep their case; reserved ones are uppercased.
        let out = fmt("select t.date, type from text as key where date > 1", Driver::Postgres);
        assert_eq!(out, "SELECT t.date,\n       TYPE\nFROM text AS key\nWHERE DATE > 1");
        let out = fmt_with("select t.date, type from text where date > 1", Driver::Postgres, &["type", "date"]);
        assert_eq!(out, "SELECT t.date,\n       type\nFROM text\nWHERE date > 1");
        // CAST(… AS type) is a type, not an alias.
        assert_eq!(fmt("select cast(x as int) as n", Driver::Postgres), "SELECT CAST(x AS INT) AS n");
    }

    #[test]
    fn format_indent_and_robustness() {
        let is_ident = |_: &str| false;
        // Starting at column 4: the next lines align with it.
        assert_eq!(format_sql("select a, b from t", Driver::Postgres, 4, &is_ident), "SELECT a,\n           b\n    FROM t");
        for d in Driver::ALL {
            for s in [
                "", "   ", "SELECT 'abc", "SELECT /* abc", "select (((", "))) from", "select ,,, ;;", "-- x", "a -- x\n-- y\nb",
                "SELECT $$ abc", "SELECT [abc", "go", "SELECT 1\nGO\nGO", "select case end end end", "SELECT 'canción 🎵' AS título FROM café",
            ] {
                let out = format_sql(s, d, 0, &is_ident);
                assert_eq!(token_texts(s, d), token_texts(&out, d), "{s:?} {d:?}");
                assert_eq!(format_sql(&out, d, 0, &is_ident), out, "{s:?} {d:?}");
            }
        }
    }

    #[test]
    fn format_random_token_soup() {
        // Pseudo-random scripts: only whitespace / keyword case may change, and it is stable.
        let vocab = [
            "select", "from", "where", "and", "or", "between", "case", "when", "then", "end", "group", "by", "order",
            "left", "join", "on", "union", "all", "values", "update", "set", "insert", "into", "with", "as", "(", ")",
            ",", ";", ".", "a", "t", "date", "1", "'x y'", "\"q\"", "[b]", "`c`", "-- note\n", "/* c */", "=", "-",
            "*", "::", "$1", "$$ body $$", "@v", "#t", "\n", "\n\n", "go\n", "  ", "1.5", "e", "n", "'b'", "[", "]",
            "<", ">", "!", "/", "+", "_utf8", "q", "{", "}", "exists", "distinct", "is", "having", "limit", "apply",
            "cross", "outer", "default", "returning",
        ];
        let mut seed = 0x2545F491u64;
        let mut rnd = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..1500 {
            let len = 1 + rnd(40);
            let mut s = String::new();
            for _ in 0..len {
                s.push_str(vocab[rnd(vocab.len())]);
                if rnd(3) > 0 {
                    s.push(' ');
                }
            }
            for d in Driver::ALL {
                fmt_core(&s, d, &[]);
            }
        }
    }

    #[test]
    fn explain_per_dialect() {
        assert_eq!(explain_statements("select 1;", Driver::Postgres), ["EXPLAIN select 1"]);
        assert_eq!(explain_statements(" select 1 ", Driver::MySql), ["EXPLAIN select 1"]);
        assert_eq!(explain_statements("select 1", Driver::MariaDb), ["EXPLAIN select 1"]);
        assert_eq!(explain_statements("select 1", Driver::Sqlite), ["EXPLAIN QUERY PLAN select 1"]);
        assert_eq!(
            explain_statements("select 1", Driver::MsSql),
            ["SET SHOWPLAN_TEXT ON", "select 1", "SET SHOWPLAN_TEXT OFF"]
        );
        assert_eq!(
            explain_statements("select 1 from dual", Driver::Oracle),
            ["EXPLAIN PLAN FOR select 1 from dual", "SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY())"]
        );
        assert!(explain_statements("  ; ", Driver::Postgres).is_empty());
    }

    #[test]
    fn mysql_hash_comments() {
        assert_eq!(texts("SELECT 1 # a; b\n; SELECT 2", Driver::MySql), ["SELECT 1", "SELECT 2"]);
        // Elsewhere # is not a comment.
        assert_eq!(texts("SELECT #t; SELECT 2", Driver::MsSql).len(), 2);
    }
}
