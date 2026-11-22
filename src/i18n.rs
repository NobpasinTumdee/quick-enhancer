//! Tiny key-based i18n: `locales/en.toml` and `locales/th.toml` are embedded at compile
//! time (no runtime file dependency). Keys are `section.name`; values may contain `{arg}`
//! placeholders filled by [`t!`]. A key missing in Thai falls back to English (logged once
//! in debug builds); a key missing everywhere shows the key itself.
//!
//! ```ignore
//! t!("queue.title")
//! t!("queue.added", count = 3)
//! ```

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

pub const EN_SOURCE: &str = include_str!("../locales/en.toml");
pub const TH_SOURCE: &str = include_str!("../locales/th.toml");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lang {
    #[default]
    En,
    Th,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Th];
    /// Name shown in the language selector (always in its own language).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Th => "ไทย",
        }
    }
    fn table(self) -> &'static HashMap<String, String> {
        match self {
            Lang::En => &EN,
            Lang::Th => &TH,
        }
    }
}

static EN: LazyLock<HashMap<String, String>> = LazyLock::new(|| parse(EN_SOURCE, "en"));
static TH: LazyLock<HashMap<String, String>> = LazyLock::new(|| parse(TH_SOURCE, "th"));
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Flatten a locale TOML document into `section.key → text`.
pub fn parse(src: &str, name: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    match src.parse::<toml::Table>() {
        Ok(t) => flatten("", &t, &mut out),
        Err(e) => tracing::error!("locale {name} is invalid: {e}"),
    }
    out
}

fn flatten(prefix: &str, t: &toml::Table, out: &mut HashMap<String, String>) {
    for (k, v) in t {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::String(s) => {
                out.insert(key, s.clone());
            }
            toml::Value::Table(sub) => flatten(&key, sub, out),
            other => {
                out.insert(key, other.to_string());
            }
        }
    }
}

pub fn set_lang(l: Lang) {
    CURRENT.store(l as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Th,
        _ => Lang::En,
    }
}

/// First-run default: Thai when the Windows UI language is Thai, otherwise English.
pub fn system_default() -> Lang {
    #[cfg(windows)]
    {
        // LANGID primary language 0x1E = Thai.
        let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        if id & 0x3ff == 0x1e {
            return Lang::Th;
        }
    }
    #[cfg(not(windows))]
    {
        for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(var)
                && !v.is_empty()
            {
                return if v.starts_with("th") { Lang::Th } else { Lang::En };
            }
        }
    }
    Lang::En
}

#[cfg(debug_assertions)]
fn report_missing(lang: Lang, key: &str) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
    if let Ok(mut s) = SEEN.lock()
        && s.insert(format!("{lang:?}:{key}"))
    {
        tracing::warn!("i18n: key '{key}' missing in {lang:?}");
    }
}

/// Look up `key` in `lang`, falling back to English, then to the key itself.
pub fn lookup(lang: Lang, key: &str) -> &'static str {
    if let Some(s) = lang.table().get(key) {
        return s.as_str();
    }
    #[cfg(debug_assertions)]
    report_missing(lang, key);
    if let Some(s) = EN.get(key) {
        return s.as_str();
    }
    // Leak once per unknown key so callers get a &'static str (only happens on bugs).
    Box::leak(key.to_string().into_boxed_str())
}

/// Text for `key` in the current language.
pub fn tr(key: &str) -> String {
    lookup(lang(), key).to_string()
}

/// Text for `key` with `{name}` placeholders replaced.
pub fn tr_args(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(lookup(lang(), key), args)
}

/// Replace `{name}` placeholders.
pub fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut s = template.to_string();
    for (name, value) in args {
        let pat = format!("{{{name}}}");
        if s.contains(&pat) {
            s = s.replace(&pat, &value.to_string());
        }
    }
    s
}

/// `t!("key")` or `t!("key", name = value, …)`.
#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::i18n::tr($key)
    };
    ($key:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::tr_args($key, &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn keys(src: &str, name: &str) -> BTreeSet<String> {
        let m = parse(src, name);
        assert!(!m.is_empty(), "{name} locale parsed empty");
        m.into_keys().collect()
    }

    #[test]
    fn en_and_th_have_the_same_keys() {
        let en = keys(EN_SOURCE, "en");
        let th = keys(TH_SOURCE, "th");
        let missing_th: Vec<_> = en.difference(&th).collect();
        let missing_en: Vec<_> = th.difference(&en).collect();
        assert!(missing_th.is_empty(), "keys missing in th.toml: {missing_th:?}");
        assert!(missing_en.is_empty(), "keys missing in en.toml: {missing_en:?}");
    }

    #[test]
    fn placeholders_match_between_languages() {
        let en = parse(EN_SOURCE, "en");
        let th = parse(TH_SOURCE, "th");
        let ph = |s: &str| -> BTreeSet<String> {
            let mut out = BTreeSet::new();
            let mut rest = s;
            while let Some(a) = rest.find('{') {
                if let Some(b) = rest[a..].find('}') {
                    out.insert(rest[a + 1..a + b].to_string());
                    rest = &rest[a + b + 1..];
                } else {
                    break;
                }
            }
            out
        };
        for (k, v) in &en {
            if let Some(tv) = th.get(k) {
                assert_eq!(ph(v), ph(tv), "placeholder mismatch for key {k}");
            }
        }
    }

    /// Every literal key used with `t!` / `tr` in the source exists in en.toml.
    #[test]
    fn all_used_keys_exist() {
        let en = keys(EN_SOURCE, "en");
        let mut missing = BTreeSet::new();
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") || p.ends_with("i18n.rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                for marker in ["t!(\"", "setup.need_"] {
                    let mut rest = src.as_str();
                    while let Some(i) = rest.find(marker) {
                        let is_macro = marker.starts_with('t');
                        let start = if is_macro { i + marker.len() } else { i };
                        // `t!(` must not be the end of another macro name (`format!(`).
                        let boundary = !is_macro
                            || i == 0
                            || !rest[..i]
                                .chars()
                                .next_back()
                                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
                        let tail = &rest[start..];
                        if !boundary {
                            rest = tail;
                            continue;
                        }
                        let end = tail
                            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                            .unwrap_or(tail.len());
                        let key = &tail[..end];
                        if !key.is_empty() && !en.contains(key) {
                            missing.insert(format!("{key} ({})", p.display()));
                        }
                        rest = &tail[end..];
                    }
                }
            }
        }
        assert!(missing.is_empty(), "keys used in code but missing in en.toml: {missing:?}");
    }

    #[test]
    fn interpolation_and_fallback() {
        assert_eq!(fill("{count} files", &[("count", &3)]), "3 files");
        assert_eq!(fill("a {x} b {x}", &[("x", &"y")]), "a y b y");
        assert_eq!(lookup(Lang::Th, "no.such.key"), "no.such.key");
    }
}
