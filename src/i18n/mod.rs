//! Interface translations.
//!
//! The source language is English: every user-facing string is written in English in the
//! code and wrapped in [`tr`] (plain text) or [`trf`] (text with `{name}` placeholders).
//! The active language is global, so the worker thread can translate too. Spanish
//! entries live in the `es_*` tables, one per area of the code, keyed by the exact
//! English text; a missing entry falls back to English.

mod es_app;
mod es_console;
mod es_core;
mod es_mysql;
mod es_oracle;
mod es_objects;
mod es_snowflake;
mod es_ui;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    En,
    Es,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Es];

    /// Name of the language written in that language (for the selector).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Es => "Español",
        }
    }

    /// Language of the operating system: Spanish if the preferred locale is Spanish,
    /// English otherwise.
    pub fn system() -> Lang {
        match sys_locale::get_locale() {
            Some(l) if l.to_lowercase().starts_with("es") => Lang::Es,
            _ => Lang::En,
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Es,
        _ => Lang::En,
    }
}

const ALL_TABLES: [&[(&str, &str)]; 8] =
    [es_app::ENTRIES, es_console::ENTRIES, es_ui::ENTRIES, es_core::ENTRIES, es_mysql::ENTRIES, es_oracle::ENTRIES, es_snowflake::ENTRIES, es_objects::ENTRIES];

fn spanish() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| ALL_TABLES.iter().flat_map(|t| t.iter().copied()).collect())
}

/// `en` in the active language.
pub fn tr(en: &'static str) -> &'static str {
    match lang() {
        Lang::En => en,
        Lang::Es => spanish().get(en).copied().unwrap_or(en),
    }
}

/// Like [`tr`] for words whose translation depends on where they appear (e.g. the "Edit"
/// menu is «Edición» but the "Edit" action is «Editar»). The Spanish table key is
/// `"{context}|{en}"`; without an entry the plain English text is shown.
pub fn tr_ctx(context: &'static str, en: &'static str) -> &'static str {
    match lang() {
        Lang::En => en,
        Lang::Es => {
            let key = format!("{context}|{en}");
            spanish().get(key.as_str()).copied().unwrap_or_else(|| tr(en))
        }
    }
}

/// Like [`tr`], replacing each `{name}` placeholder with its value:
/// `trf("{n} rows", &[("n", &count)])`.
pub fn trf(template: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = tr(template).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn placeholders(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            let Some(j) = rest[i..].find('}') else { break };
            out.push(rest[i..i + j + 1].to_string());
            rest = &rest[i + j + 1..];
        }
        out.sort();
        out
    }

    #[test]
    fn keys_are_unique_across_tables() {
        let mut seen = HashSet::new();
        for (en, _) in ALL_TABLES.iter().flat_map(|t| t.iter()) {
            assert!(seen.insert(*en), "duplicated translation key: {en:?}");
        }
    }

    #[test]
    fn translations_keep_placeholders() {
        for (en, es) in ALL_TABLES.iter().flat_map(|t| t.iter()) {
            assert_eq!(placeholders(en), placeholders(es), "placeholders differ in {en:?}");
            assert!(!es.trim().is_empty(), "empty translation for {en:?}");
        }
    }

    #[test]
    fn falls_back_to_english() {
        // Tests never change the global language, so it is the default (English) here.
        assert_eq!(trf("{a} of {b} (untranslated)", &[("a", &1), ("b", &"x")]), "1 of x (untranslated)");
    }
}
