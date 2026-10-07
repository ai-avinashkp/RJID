//! pom.xml help, IntelliJ-style: a dependency without a `<version>` (and no
//! parent / BOM managing it) is a build error. When the right version is
//! certain — another dependency, plugin or the parent from the same group
//! already pins one (e.g. every `org.springframework.boot` artifact uses the
//! project's Boot version) — it's inserted automatically. Otherwise the
//! quick fix offers the latest stable release from Maven Central.

use std::ops::Range;

use gpui::Context;
use rji_lsp_client::{FileEdit, TextEdit};

use super::CodeEditorView;
use super::code_actions::Fix;

/// `'dependencies.dependency.version' for g:a:jar is missing.` -> (g, a).
pub(super) fn missing_version(message: &str) -> Option<(String, String)> {
    if !message.contains("dependency.version") || !message.contains("is missing") {
        return None;
    }
    let coords = message.split(" for ").nth(1)?.split_whitespace().next()?;
    let mut parts = coords.split(':');
    let group = parts.next()?.to_string();
    let artifact = parts.next()?.to_string();
    (!group.is_empty() && !artifact.is_empty()).then_some((group, artifact))
}

/// The text between `<tag>` and `</tag>` starting the search at `from`.
fn tag_value<'a>(text: &'a str, tag: &str, range: Range<usize>) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let slice = text.get(range.clone())?;
    let start = slice.find(&open)? + open.len();
    let end = slice[start..].find(&close)? + start;
    Some(slice[start..end].trim())
}

/// A version some other block (`<dependency>`, `<plugin>`, `<parent>`) of the
/// same group pins, with `${property}` resolved.
pub(super) fn known_version(pom: &str, group: &str) -> Option<String> {
    let needle = format!("<groupId>{group}</groupId>");
    let mut search_from = 0;
    while let Some(found) = pom[search_from..].find(&needle).map(|i| i + search_from) {
        search_from = found + needle.len();
        let start = ["<dependency>", "<plugin>", "<parent>"]
            .iter()
            .filter_map(|tag| pom[..found].rfind(tag))
            .max()
            .unwrap_or(0);
        let end = ["</dependency>", "</plugin>", "</parent>"]
            .iter()
            .filter_map(|tag| pom[found..].find(tag).map(|i| i + found))
            .min()
            .unwrap_or(pom.len());
        let Some(version) = tag_value(pom, "version", start..end) else { continue };
        let resolved = match version.strip_prefix("${").and_then(|v| v.strip_suffix('}')) {
            Some(property) => tag_value(pom, property, 0..pom.len()).map(str::to_string),
            None => Some(version.to_string()),
        };
        if let Some(v) = resolved.filter(|v| !v.is_empty() && !v.starts_with("${")) {
            return Some(v);
        }
    }
    None
}

/// Where `<version>` goes in the dependency starting at `from`: after its
/// `</artifactId>` (same indentation), within that `<dependency>` block.
pub(super) fn version_insertion(text: &str, from: usize, version: &str) -> Option<(usize, String)> {
    let block_end = text[from..].find("</dependency>")? + from;
    let artifact_end = text[from..block_end].find("</artifactId>")? + from + "</artifactId>".len();
    let line_start = text[..artifact_end].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    Some((artifact_end, format!("\n{indent}<version>{version}</version>")))
}

impl CodeEditorView {
    pub(super) fn is_pom(&self) -> bool {
        self.path.file_name().and_then(|n| n.to_str()) == Some("pom.xml")
    }

    /// Edit that adds `<version>` to the dependency whose diagnostic starts
    /// on `line` (0-based).
    fn version_edit(&self, line: u32, version: &str) -> Option<TextEdit> {
        let from = self.buffer.line_starts().get(line as usize).copied()?;
        let (offset, text) = version_insertion(self.buffer.text(), from, version)?;
        let position = self.lsp_position(offset);
        Some(TextEdit { start: position, end: position, new_text: text })
    }

    /// New diagnostics for this pom: add versions that are certain, then save
    /// (so the build re-imports). Runs once per distinct set of problems.
    pub(super) fn maybe_fix_pom_versions(&mut self, cx: &mut Context<Self>) {
        if !self.is_pom() || !self.auto_import || self.dirty {
            return;
        }
        let missing: Vec<(u32, String, String)> = self
            .diagnostic_list
            .iter()
            .filter_map(|d| missing_version(&d.message).map(|(g, a)| (d.line, g, a)))
            .collect();
        let key = missing.iter().map(|(_, g, a)| format!("{g}:{a}")).collect::<Vec<_>>().join(",");
        if missing.is_empty() || key == self.last_pom_fix {
            if missing.is_empty() {
                self.last_pom_fix.clear();
            }
            return;
        }
        self.last_pom_fix = key;
        let mut edits = Vec::new();
        let mut added = Vec::new();
        for (line, group, artifact) in &missing {
            if let Some(version) = known_version(self.buffer.text(), group)
                && let Some(edit) = self.version_edit(*line, &version)
            {
                edits.push(edit);
                added.push(format!("{artifact} {version}"));
            }
        }
        if edits.is_empty() {
            return;
        }
        self.apply_text_edits(&edits);
        self.save(cx);
        cx.emit(super::EditorEvent::Notice(format!(
            "Added the missing version: {} (matches the other dependencies from the same group)",
            added.join(", ")
        )));
        cx.notify();
    }

    /// Quick fixes for missing versions: the version other blocks of the
    /// group use, else the latest stable release from Maven Central.
    pub(super) fn pom_version_fixes(&self, diagnostics: &[serde_json::Value], cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Vec<Fix>, &mut Context<Self>) + 'static) {
        let missing: Vec<(u32, String, String)> = diagnostics
            .iter()
            .filter_map(|d| {
                let (group, artifact) = missing_version(d["message"].as_str()?)?;
                Some((d.pointer("/range/start/line")?.as_u64()? as u32, group, artifact))
            })
            .collect();
        let text = self.buffer.text().to_string();
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let mut fixes = Vec::new();
            for (line, group, artifact) in missing {
                let (version, why) = match known_version(&text, &group) {
                    Some(v) => (Some(v), "same as your other dependencies from this group"),
                    None => {
                        let (g, a) = (group.clone(), artifact.clone());
                        let latest = cx
                            .background_executor()
                            .spawn(async move {
                                let versions = rji_updates::maven_central_versions(&g, &a).ok()?;
                                rji_updates::latest_stable(versions.iter().map(String::as_str)).map(|v| v.raw)
                            })
                            .await;
                        (latest, "latest release on Maven Central")
                    }
                };
                let Some(version) = version else { continue };
                let edit = this.read_with(cx, |view, _| view.version_edit(line, &version)).ok().flatten();
                if let Some(edit) = edit {
                    fixes.push(Fix {
                        title: format!("Add version {version} to {artifact} ({why})"),
                        edits: vec![FileEdit { path: path.clone(), edits: vec![edit], create: false }],
                    });
                }
            }
            this.update(cx, |view, cx| then(view, fixes, cx)).ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POM: &str = "<project>\n  <properties>\n    <boot.version>3.2.0</boot.version>\n  </properties>\n  <dependencies>\n    <dependency>\n      <groupId>org.springframework.boot</groupId>\n      <artifactId>spring-boot-starter</artifactId>\n      <version>${boot.version}</version>\n    </dependency>\n    <dependency>\n      <groupId>org.springframework.boot</groupId>\n      <artifactId>spring-boot-starter-web</artifactId>\n    </dependency>\n  </dependencies>\n</project>\n";

    #[test]
    fn parses_the_missing_version_error() {
        let message = "Project build error: 'dependencies.dependency.version' for org.springframework.boot:spring-boot-starter-web:jar is missing.";
        assert_eq!(
            missing_version(message),
            Some(("org.springframework.boot".into(), "spring-boot-starter-web".into()))
        );
        assert_eq!(missing_version("something else"), None);
    }

    #[test]
    fn finds_the_groups_version_and_where_to_insert_it() {
        assert_eq!(known_version(POM, "org.springframework.boot").as_deref(), Some("3.2.0"));
        assert_eq!(known_version(POM, "com.h2database"), None);
        let from = POM.rfind("    <dependency>").unwrap();
        let (offset, text) = version_insertion(POM, from, "3.2.0").unwrap();
        let mut fixed = POM.to_string();
        fixed.insert_str(offset, &text);
        assert!(fixed.contains("<artifactId>spring-boot-starter-web</artifactId>\n      <version>3.2.0</version>\n    </dependency>"));
    }
}
