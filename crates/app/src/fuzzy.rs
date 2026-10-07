//! Fuzzy matching for the Go to File / command palette: the query's
//! characters must appear in order (case-insensitive); matches at word
//! starts, right after a path separator, and in a run score higher, and a
//! match in the file name beats one spread across the folders.

/// Score of `query` against `text`, or `None` if it doesn't match. Higher
/// is better. Spaces in the query are ignored.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut qi = 0;
    let mut total = 0;
    let mut prev_match: Option<usize> = None;
    for (i, &c) in chars.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c.to_lowercase().next() != Some(query[qi]) {
            continue;
        }
        let mut s = 10;
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let word_start = match prev {
            None => true,
            Some(p) => matches!(p, '/' | '\\' | '.' | '_' | '-' | ' ') || (p.is_lowercase() && c.is_uppercase()),
        };
        if word_start {
            s += 12;
        }
        match prev_match {
            Some(p) if p + 1 == i => s += 14,
            Some(p) => s -= ((i - p - 1) as i32).min(8),
            None => s -= (i as i32).min(10),
        }
        total += s;
        prev_match = Some(i);
        qi += 1;
    }
    (qi == query.len()).then_some(total)
}

/// Score of `query` against a relative path: file-name matches first.
pub fn score_path(query: &str, path: &str) -> Option<i32> {
    let name_start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let name = &path[name_start..];
    let searches_folders = query.contains(['/', '\\']);
    if !searches_folders && let Some(s) = score(query, name) {
        // Exact / prefix file-name hits on top; shorter paths break ties.
        let stem = name.split('.').next().unwrap_or(name);
        let exact = if stem.eq_ignore_ascii_case(query.trim()) { 500 } else { 0 };
        return Some(1000 + exact + s - path.len() as i32 / 8);
    }
    score(query, path).map(|s| s - path.len() as i32 / 8)
}

/// Indices of `items` matching `query`, best first, at most `limit`.
pub fn rank<'a>(query: &str, items: impl IntoIterator<Item = &'a str>, limit: usize, by_path: bool) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, text)| if by_path { score_path(query, text) } else { score(query, text) }.map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored.into_iter().map(|(_, i)| i).collect()
}

/// `App.java:42` -> (`App.java`, Some(42)); line numbers are 1-based.
pub fn split_line_suffix(query: &str) -> (&str, Option<u32>) {
    match query.rsplit_once(':') {
        Some((head, line)) if !head.is_empty() && !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()) => {
            (head, line.parse().ok())
        }
        _ => (query, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_order_case_insensitive() {
        assert!(score("hc", "HelloController.java").is_some());
        assert!(score("hcj", "HelloController.java").is_some());
        assert!(score("ctrl", "HelloController.java").is_some());
        assert!(score("ch", "HelloController.java").is_none());
        assert!(score("xyz", "HelloController.java").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn ranks_file_names_and_word_starts_first() {
        let files = [
            "src/main/java/com/example/boot/HelloController.java",
            "src/main/java/com/example/boot/Application.java",
            "src/main/resources/application.properties",
            "src/test/java/com/example/boot/ApplicationTests.java",
            "pom.xml",
        ];
        let best = |q: &str| files[rank(q, files.iter().copied(), 10, true)[0]];
        assert_eq!(best("app"), "src/main/java/com/example/boot/Application.java");
        assert_eq!(best("hc"), "src/main/java/com/example/boot/HelloController.java");
        assert_eq!(best("pom"), "pom.xml");
        assert_eq!(best("appprop"), "src/main/resources/application.properties");
        assert_eq!(best("test/app"), "src/test/java/com/example/boot/ApplicationTests.java");
    }

    #[test]
    fn line_suffix() {
        assert_eq!(split_line_suffix("App.java:42"), ("App.java", Some(42)));
        assert_eq!(split_line_suffix("App.java"), ("App.java", None));
        assert_eq!(split_line_suffix("App:x"), ("App:x", None));
    }
}
