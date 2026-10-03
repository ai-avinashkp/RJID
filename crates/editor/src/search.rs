//! Find / replace over a document. Plain text or regex, optionally
//! case-sensitive and/or whole-word. Pure functions over `&str`, so all of
//! it is unit-tested without a UI.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

/// Stop collecting past this many matches — the UI shows "10000+".
pub const MAX_MATCHES: usize = 10_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// Compiles `query` under `options`. Plain-text queries are escaped, so
/// both modes share one matching engine. Errors are human-readable (shown
/// in the find bar for an invalid regex).
pub fn build_pattern(query: &str, options: SearchOptions) -> Result<Regex, String> {
    let mut pattern = if options.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    if options.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .multi_line(true)
        .build()
        .map_err(|err| match err {
            regex::Error::Syntax(msg) => msg.lines().last().unwrap_or("invalid regex").trim().to_string(),
            other => other.to_string(),
        })
}

/// Every non-empty match of `query` in `text`, as byte ranges.
pub fn find_all(text: &str, query: &str, options: SearchOptions) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = build_pattern(query, options)?;
    Ok(pattern
        .find_iter(text)
        .filter(|m| !m.is_empty())
        .take(MAX_MATCHES)
        .map(|m| m.range())
        .collect())
}

/// The text one match should be replaced with: literal in plain mode;
/// with `$1`/`${name}` capture references expanded in regex mode.
pub fn replacement_for(
    text: &str,
    range: Range<usize>,
    query: &str,
    replacement: &str,
    options: SearchOptions,
) -> String {
    if !options.regex {
        return replacement.to_string();
    }
    let Ok(pattern) = build_pattern(query, options) else {
        return replacement.to_string();
    };
    // Re-match at the exact spot so captures line up with this match.
    match pattern.captures_at(text, range.start) {
        Some(caps) if caps.get(0).is_some_and(|m| m.range() == range) => {
            let mut out = String::new();
            caps.expand(replacement, &mut out);
            out
        }
        _ => replacement.to_string(),
    }
}

/// `text` with every match replaced. Returns the new text and how many
/// replacements were made.
pub fn replace_all(
    text: &str,
    query: &str,
    replacement: &str,
    options: SearchOptions,
) -> Result<(String, usize), String> {
    if query.is_empty() {
        return Ok((text.to_string(), 0));
    }
    let pattern = build_pattern(query, options)?;
    let mut count = 0;
    let replaced = pattern.replace_all(text, |caps: &regex::Captures| {
        count += 1;
        if options.regex {
            let mut out = String::new();
            caps.expand(replacement, &mut out);
            out
        } else {
            replacement.to_string()
        }
    });
    Ok((replaced.into_owned(), count))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(case_sensitive: bool, whole_word: bool, regex: bool) -> SearchOptions {
        SearchOptions {
            case_sensitive,
            whole_word,
            regex,
        }
    }

    #[test]
    fn plain_search_is_case_insensitive_by_default_and_escapes_specials() {
        let text = "Count count COUNT a.b";
        assert_eq!(find_all(text, "count", SearchOptions::default()).unwrap().len(), 3);
        assert_eq!(find_all(text, "count", opts(true, false, false)).unwrap(), vec![6..11]);
        assert_eq!(find_all(text, "a.b", SearchOptions::default()).unwrap(), vec![18..21]);
    }

    #[test]
    fn whole_word_skips_substrings() {
        let text = "int i; int index;";
        assert_eq!(find_all(text, "i", opts(false, true, false)).unwrap(), vec![4..5]);
    }

    #[test]
    fn regex_with_captures_and_errors() {
        let text = "get(1) get(22)";
        let matches = find_all(text, r"get\((\d+)\)", opts(false, false, true)).unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(
            replacement_for(text, matches[1].clone(), r"get\((\d+)\)", "fetch($1)", opts(false, false, true)),
            "fetch(22)"
        );
        let (all, n) = replace_all(text, r"get\((\d+)\)", "fetch($1)", opts(false, false, true)).unwrap();
        assert_eq!((all.as_str(), n), ("fetch(1) fetch(22)", 2));
        assert!(find_all(text, "(", opts(false, false, true)).is_err());
    }

    #[test]
    fn plain_replacement_is_literal() {
        let (out, n) = replace_all("a a", "a", "$1", SearchOptions::default()).unwrap();
        assert_eq!((out.as_str(), n), ("$1 $1", 2));
    }

    #[test]
    fn empty_regex_matches_are_ignored() {
        assert!(find_all("abc", "x*", opts(false, false, true)).unwrap().is_empty());
    }
}
