//! Sample RustedJavaIDE plugin (plugin API 1).
//!
//! The IDE calls `alloc(len)` to get a buffer for the JSON input, writes it
//! there, then calls `run(ptr, len)`, which returns `(ptr << 32) | len` of
//! a JSON reply: `{"replace": "..."}`, `{"message": "..."}`, or
//! `{"error": "..."}`. Plugins import nothing — they only transform the
//! text they're handed.

use serde::Deserialize;

#[derive(Deserialize)]
struct Input {
    command: String,
    text: String,
    is_selection: bool,
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: i32) -> i32 {
    let mut buf = Vec::<u8>::with_capacity(len.max(0) as usize);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn run(ptr: i32, len: i32) -> i64 {
    // SAFETY: the host wrote exactly `len` bytes at `ptr`, a buffer it got
    // from `alloc`.
    let input = unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let reply = match serde_json::from_slice::<Input>(input) {
        Ok(input) => handle(&input),
        Err(err) => serde_json::json!({ "error": format!("bad input: {err}") }),
    };
    let bytes = reply.to_string().into_bytes().into_boxed_slice();
    let out_len = bytes.len() as i64;
    let out_ptr = Box::into_raw(bytes) as *mut u8 as u32 as i64;
    (out_ptr << 32) | out_len
}

fn handle(input: &Input) -> serde_json::Value {
    let text = input.text.as_str();
    match input.command.as_str() {
        "getters-setters" => match getters_setters(text) {
            Some(out) => serde_json::json!({ "replace": out }),
            None => serde_json::json!({ "error": "select field declarations like `private String name;`" }),
        },
        "sort-lines" => {
            let mut lines: Vec<&str> = text.lines().collect();
            lines.sort_by_key(|l| l.trim().to_lowercase());
            let mut out = lines.join("\n");
            if text.ends_with('\n') {
                out.push('\n');
            }
            serde_json::json!({ "replace": out })
        }
        "camel-snake" => serde_json::json!({ "replace": toggle_case_style(text) }),
        "upper" => serde_json::json!({ "replace": text.to_uppercase() }),
        "lower" => serde_json::json!({ "replace": text.to_lowercase() }),
        "count" => {
            let scope = if input.is_selection { "Selection" } else { "File" };
            serde_json::json!({ "message": format!(
                "{scope}: {} lines, {} words, {} characters",
                text.lines().count(),
                text.split_whitespace().count(),
                text.chars().count()
            ) })
        }
        other => serde_json::json!({ "error": format!("unknown command `{other}`") }),
    }
}

/// `private String name;` lines -> the same lines plus Java bean accessors.
fn getters_setters(text: &str) -> Option<String> {
    let indent: String = text
        .lines()
        .find(|l| !l.trim().is_empty())?
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let mut fields = Vec::new();
    for line in text.lines() {
        // Only real field declarations: `[modifiers] Type name [= …];`.
        let trimmed = line.trim();
        if !trimmed.ends_with(';') || trimmed.starts_with("//") || trimmed.contains('(') {
            continue;
        }
        let decl = trimmed.trim_end_matches(';').split('=').next().unwrap_or("").trim();
        let words: Vec<&str> = decl.split_whitespace().collect();
        if words.len() < 2 || words.contains(&"static") {
            continue;
        }
        let name = words[words.len() - 1];
        let ty = words[words.len() - 2];
        if !is_identifier(name) || !is_type(ty) {
            continue;
        }
        let is_final = words.contains(&"final");
        fields.push((ty.to_string(), name.to_string(), is_final));
    }
    if fields.is_empty() {
        return None;
    }
    let mut out = text.trim_end().to_string();
    out.push('\n');
    for (ty, name, is_final) in &fields {
        let cap = capitalize(name);
        let getter = if ty == "boolean" { format!("is{cap}") } else { format!("get{cap}") };
        out.push_str(&format!("\n{indent}public {ty} {getter}() {{\n{indent}    return {name};\n{indent}}}\n"));
        if !is_final {
            out.push_str(&format!(
                "\n{indent}public void set{cap}({ty} {name}) {{\n{indent}    this.{name} = {name};\n{indent}}}\n"
            ));
        }
    }
    Some(out)
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// `String`, `int[]`, `List<Map<String, Integer>>`, `java.util.Date`, …
fn is_type(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_alphanumeric() || "_$<>[].,?".contains(c))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `myValueName` <-> `my_value_name` (direction chosen from the text).
fn toggle_case_style(text: &str) -> String {
    if text.contains('_') {
        let mut out = String::new();
        let mut upper_next = false;
        for c in text.chars() {
            if c == '_' {
                upper_next = true;
            } else if upper_next {
                out.extend(c.to_uppercase());
                upper_next = false;
            } else {
                out.push(c);
            }
        }
        out
    } else {
        let mut out = String::new();
        for (i, c) in text.chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        }
        out
    }
}
