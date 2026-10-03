//! Lightweight syntax highlighting: a hand-rolled tokenizer per language
//! (Java, XML, `#`-comment config files, plain text) rather than a
//! tree-sitter grammar — fast, dependency-free, and good enough for
//! readable code. Real grammar-based highlighting is tracked in
//! `docs/ROADMAP.md`.
//!
//! Tokenizing is per line, but constructs that span lines (Java block
//! comments and `"""` text blocks, XML comments) are handled by threading
//! a [`LineState`] from one line into the next: the editor computes the
//! state at the start of every line once per edit via [`scan_line_state`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Plain,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub text: String,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Java,
    Xml,
    /// Properties / YAML / TOML / shell: `#` line comments, quoted strings.
    HashComment,
    Plain,
}

impl Language {
    pub fn for_extension(ext: &str) -> Language {
        match ext {
            "java" | "kt" | "kts" | "gradle" | "groovy" | "js" | "ts" | "c" | "cpp" | "h" | "cs" | "rs" | "go" => {
                Language::Java
            }
            "xml" | "html" | "htm" | "fxml" | "svg" => Language::Xml,
            "properties" | "yml" | "yaml" | "toml" | "sh" | "ps1" | "cfg" | "ini" | "conf" => {
                Language::HashComment
            }
            _ => Language::Plain,
        }
    }
}

/// Where a line starts, lexically: inside nothing, or inside a construct
/// opened on an earlier line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineState {
    #[default]
    Normal,
    /// Inside `/* ... */` (Java) or `<!-- ... -->` (XML).
    BlockComment,
    /// Inside a Java `"""` text block.
    TextBlock,
}

const KEYWORDS: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const",
    "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally",
    "float", "for", "goto", "if", "implements", "import", "instanceof", "int", "interface",
    "long", "native", "new", "package", "private", "protected", "public", "record", "return",
    "sealed", "short", "static", "strictfp", "super", "switch", "synchronized", "this", "throw",
    "throws", "transient", "try", "var", "void", "volatile", "while", "yield", "true", "false",
    "null",
];

/// Back-compat helper: highlights one Java line assuming no open construct.
pub fn highlight_java_line(line: &str) -> Vec<Token> {
    highlight_line(Language::Java, line, LineState::Normal).0
}

/// Tokenizes one line starting in `state`; returns the tokens and the state
/// the *next* line starts in. Never panics (works on chars, not bytes).
pub fn highlight_line(language: Language, line: &str, state: LineState) -> (Vec<Token>, LineState) {
    let mut out = Tokens::default();
    let end_state = match language {
        Language::Java => java(line, state, &mut out),
        Language::Xml => xml(line, state, &mut out),
        Language::HashComment => hash_comment(line, &mut out),
        Language::Plain => {
            out.push(line.to_string(), TokenKind::Plain);
            LineState::Normal
        }
    };
    (out.0, end_state)
}

/// Just the end-of-line state, without building tokens — cheap enough to
/// run over a whole file after each edit.
pub fn scan_line_state(language: Language, line: &str, state: LineState) -> LineState {
    match language {
        Language::Java | Language::Xml => highlight_line(language, line, state).1,
        _ => LineState::Normal,
    }
}

#[derive(Default)]
struct Tokens(Vec<Token>);

impl Tokens {
    fn push(&mut self, text: String, kind: TokenKind) {
        if text.is_empty() {
            return;
        }
        // Merge adjacent same-kind runs: fewer text runs to shape.
        if let Some(last) = self.0.last_mut()
            && last.kind == kind
        {
            last.text.push_str(&text);
            return;
        }
        self.0.push(Token { text, kind });
    }
}

fn find(chars: &[char], from: usize, pattern: &str) -> Option<usize> {
    let pat: Vec<char> = pattern.chars().collect();
    (from..chars.len()).find(|&i| chars[i..].starts_with(&pat))
}

fn java(line: &str, mut state: LineState, out: &mut Tokens) -> LineState {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    // Finish a construct carried over from the previous line.
    match state {
        LineState::BlockComment => match find(&chars, 0, "*/") {
            Some(end) => {
                out.push(chars[..end + 2].iter().collect(), TokenKind::Comment);
                i = end + 2;
                state = LineState::Normal;
            }
            None => {
                out.push(line.to_string(), TokenKind::Comment);
                return LineState::BlockComment;
            }
        },
        LineState::TextBlock => match find(&chars, 0, "\"\"\"") {
            Some(end) => {
                out.push(chars[..end + 3].iter().collect(), TokenKind::String);
                i = end + 3;
                state = LineState::Normal;
            }
            None => {
                out.push(line.to_string(), TokenKind::String);
                return LineState::TextBlock;
            }
        },
        LineState::Normal => {}
    }

    while i < chars.len() {
        let c = chars[i];

        if c == '/' && chars.get(i + 1) == Some(&'/') {
            out.push(chars[i..].iter().collect(), TokenKind::Comment);
            break;
        }

        if c == '/' && chars.get(i + 1) == Some(&'*') {
            match find(&chars, i + 2, "*/") {
                Some(end) => {
                    out.push(chars[i..end + 2].iter().collect(), TokenKind::Comment);
                    i = end + 2;
                    continue;
                }
                None => {
                    out.push(chars[i..].iter().collect(), TokenKind::Comment);
                    return LineState::BlockComment;
                }
            }
        }

        if c == '"' && chars.get(i + 1) == Some(&'"') && chars.get(i + 2) == Some(&'"') {
            match find(&chars, i + 3, "\"\"\"") {
                Some(end) => {
                    out.push(chars[i..end + 3].iter().collect(), TokenKind::String);
                    i = end + 3;
                    continue;
                }
                None => {
                    out.push(chars[i..].iter().collect(), TokenKind::String);
                    return LineState::TextBlock;
                }
            }
        }

        if c == '"' || c == '\'' {
            let end = quoted_end(&chars, i);
            out.push(chars[i..end].iter().collect(), TokenKind::String);
            i = end;
            continue;
        }

        if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '.' || chars[j] == '_') {
                j += 1;
            }
            out.push(chars[i..j].iter().collect(), TokenKind::Number);
            i = j;
            continue;
        }

        if c.is_alphabetic() || c == '_' || c == '$' {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '$') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let kind = if KEYWORDS.contains(&word.as_str()) {
                TokenKind::Keyword
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };
            out.push(word, kind);
            i = j;
            continue;
        }

        if c == '@' {
            // Annotation: `@Override`, `@SpringBootApplication`.
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '.') {
                j += 1;
            }
            out.push(chars[i..j].iter().collect(), TokenKind::Type);
            i = j;
            continue;
        }

        out.push(c.to_string(), TokenKind::Plain);
        i += 1;
    }
    state
}

/// Index just past a `"`/`'` literal starting at `start` (or line end if
/// unterminated), honoring backslash escapes.
fn quoted_end(chars: &[char], start: usize) -> usize {
    let quote = chars[start];
    let mut j = start + 1;
    while j < chars.len() && chars[j] != quote {
        if chars[j] == '\\' {
            j += 1;
        }
        j += 1;
    }
    (j + 1).min(chars.len())
}

fn xml(line: &str, state: LineState, out: &mut Tokens) -> LineState {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    if state == LineState::BlockComment {
        match find(&chars, 0, "-->") {
            Some(end) => {
                out.push(chars[..end + 3].iter().collect(), TokenKind::Comment);
                i = end + 3;
            }
            None => {
                out.push(line.to_string(), TokenKind::Comment);
                return LineState::BlockComment;
            }
        }
    }

    let mut in_tag = false;
    while i < chars.len() {
        let c = chars[i];
        if !in_tag && chars[i..].starts_with(&['<', '!', '-', '-']) {
            match find(&chars, i + 4, "-->") {
                Some(end) => {
                    out.push(chars[i..end + 3].iter().collect(), TokenKind::Comment);
                    i = end + 3;
                    continue;
                }
                None => {
                    out.push(chars[i..].iter().collect(), TokenKind::Comment);
                    return LineState::BlockComment;
                }
            }
        }
        if !in_tag && c == '<' {
            // `<name`, `</name`, `<?xml`: the tag name reads as a keyword.
            let mut j = i + 1;
            while j < chars.len() && (chars[j] == '/' || chars[j] == '?' || chars[j] == '!') {
                j += 1;
            }
            while j < chars.len() && (chars[j].is_alphanumeric() || matches!(chars[j], '-' | '_' | ':' | '.')) {
                j += 1;
            }
            out.push(chars[i..j].iter().collect(), TokenKind::Keyword);
            in_tag = true;
            i = j;
            continue;
        }
        if in_tag {
            if c == '>' || (c == '/' && chars.get(i + 1) == Some(&'>')) || (c == '?' && chars.get(i + 1) == Some(&'>')) {
                let len = if c == '>' { 1 } else { 2 };
                out.push(chars[i..i + len].iter().collect(), TokenKind::Keyword);
                in_tag = false;
                i += len;
                continue;
            }
            if c == '"' || c == '\'' {
                let end = quoted_end(&chars, i);
                out.push(chars[i..end].iter().collect(), TokenKind::String);
                i = end;
                continue;
            }
            if c.is_alphabetic() {
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_alphanumeric() || matches!(chars[j], '-' | '_' | ':' | '.')) {
                    j += 1;
                }
                out.push(chars[i..j].iter().collect(), TokenKind::Type);
                i = j;
                continue;
            }
        }
        out.push(c.to_string(), TokenKind::Plain);
        i += 1;
    }
    LineState::Normal
}

fn hash_comment(line: &str, out: &mut Tokens) -> LineState {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with('!') || trimmed.starts_with(';') {
        out.push(line.to_string(), TokenKind::Comment);
        return LineState::Normal;
    }
    let chars: Vec<char> = line.chars().collect();
    // `key = value` / `key: value`: color the key.
    let key_end = chars
        .iter()
        .position(|&c| c == '=' || c == ':')
        .filter(|&p| p > 0);
    let mut i = 0;
    if let Some(end) = key_end {
        out.push(chars[..end].iter().collect(), TokenKind::Type);
        i = end;
    }
    while i < chars.len() {
        let c = chars[i];
        if c == '#' && (i == 0 || chars[i - 1].is_whitespace()) {
            out.push(chars[i..].iter().collect(), TokenKind::Comment);
            break;
        }
        if c == '"' || c == '\'' {
            let end = quoted_end(&chars, i);
            out.push(chars[i..end].iter().collect(), TokenKind::String);
            i = end;
            continue;
        }
        out.push(c.to_string(), TokenKind::Plain);
        i += 1;
    }
    LineState::Normal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(tokens: &[Token]) -> Vec<(String, TokenKind)> {
        tokens.iter().map(|t| (t.text.clone(), t.kind)).collect()
    }

    #[test]
    fn block_comment_spans_lines() {
        let (t1, s1) = highlight_line(Language::Java, "int a; /** doc", LineState::Normal);
        assert_eq!(s1, LineState::BlockComment);
        assert_eq!(t1.last().unwrap().kind, TokenKind::Comment);
        let (t2, s2) = highlight_line(Language::Java, " * Sample Type", s1);
        assert_eq!(s2, LineState::BlockComment);
        assert!(t2.iter().all(|t| t.kind == TokenKind::Comment));
        let (t3, s3) = highlight_line(Language::Java, " */ class X", s2);
        assert_eq!(s3, LineState::Normal);
        assert_eq!(t3[0].kind, TokenKind::Comment);
        assert!(kinds(&t3).contains(&("class".to_string(), TokenKind::Keyword)));
    }

    #[test]
    fn text_block_spans_lines() {
        let (_, s1) = highlight_line(Language::Java, "String s = \"\"\"", LineState::Normal);
        assert_eq!(s1, LineState::TextBlock);
        let (t2, s2) = highlight_line(Language::Java, "  public class", s1);
        assert!(t2.iter().all(|t| t.kind == TokenKind::String));
        let (_, s3) = highlight_line(Language::Java, "  \"\"\";", s2);
        assert_eq!(s3, LineState::Normal);
    }

    #[test]
    fn tokens_concatenate_back_to_the_line() {
        for line in ["int x = 1; // c", "@Override public void f() {}", "\"unterminated"] {
            let (tokens, _) = highlight_line(Language::Java, line, LineState::Normal);
            let joined: String = tokens.iter().map(|t| t.text.as_str()).collect();
            assert_eq!(joined, line);
        }
    }

    #[test]
    fn xml_tags_attributes_and_multiline_comments() {
        let (tokens, state) = highlight_line(Language::Xml, r#"<plugin id="x"> <!-- a"#, LineState::Normal);
        assert_eq!(state, LineState::BlockComment);
        let k = kinds(&tokens);
        assert!(k.contains(&("<plugin".to_string(), TokenKind::Keyword)));
        assert!(k.contains(&("\"x\"".to_string(), TokenKind::String)));
        let (_, state) = highlight_line(Language::Xml, "b -->", state);
        assert_eq!(state, LineState::Normal);
    }

    #[test]
    fn plain_text_is_never_java_colored() {
        let (tokens, _) = highlight_line(Language::Plain, "public class Readme", LineState::Normal);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Plain);
    }

    #[test]
    fn properties_comment_and_key() {
        let (tokens, _) = highlight_line(Language::HashComment, "# note", LineState::Normal);
        assert_eq!(tokens[0].kind, TokenKind::Comment);
        let (tokens, _) = highlight_line(Language::HashComment, "server.port=8080", LineState::Normal);
        assert_eq!(tokens[0].kind, TokenKind::Type);
    }
}
