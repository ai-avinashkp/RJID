//! Opening Go to Definition targets: project files directly; JDK / library
//! classes (`jdt://` URIs) by fetching their source from jdtls
//! (`java/classFileContents`), caching it under `.rji-settings`, and
//! opening that copy read-only.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use gpui::{Context, Window};
use serde_json::json;

use super::RootView;
use crate::editor_view::Location;

/// Where library sources fetched for Go to Definition are cached.
pub(super) fn library_sources_root() -> PathBuf {
    rji_settings::app_dir().join(".rji-settings").join("library-sources")
}

/// `jdt://contents/java.base/java.io/PrintStream.java?=…` -> `PrintStream.java`.
pub(super) fn jdt_file_name(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("jdt://")?.split('?').next()?;
    let name = path.rsplit('/').next()?;
    let ok = name.ends_with(".java") || name.ends_with(".class");
    let safe = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '$' | '-'));
    (ok && safe).then(|| name.trim_end_matches(".class").trim_end_matches(".java").to_string() + ".java")
}

impl RootView {
    pub(super) fn open_location(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        if location.uri.starts_with("file:") {
            let Some(path) = rji_lsp_client::uri_to_path(&location.uri) else { return };
            self.open_file(path, window, cx);
            self.reveal_in_active(&location, false, cx);
        } else if location.uri.starts_with("jdt:") {
            self.open_library_source(location, window, cx);
        } else {
            self.notify_user(format!("Can't open {}", location.uri));
        }
        cx.notify();
    }

    fn reveal_in_active(&mut self, location: &Location, read_only: bool, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i)) {
            tab.view.update(cx, |editor, cx| {
                if read_only {
                    editor.read_only = true;
                }
                editor.reveal_position(location.line, location.character);
                cx.notify();
            });
        }
    }

    fn open_library_source(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = jdt_file_name(&location.uri) else {
            self.notify_user("Can't show the source of this class");
            return;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        location.uri.hash(&mut hasher);
        let path = library_sources_root().join(format!("{:x}", hasher.finish())).join(&name);
        if path.is_file() {
            self.open_file(path, window, cx);
            self.reveal_in_active(&location, true, cx);
            return;
        }
        let request = self
            .lsp
            .borrow_mut()
            .as_mut()
            .and_then(|client| client.request("java/classFileContents", json!({ "uri": location.uri })).ok());
        let Some(rx) = request else {
            self.notify_user("The Java language server isn't running");
            return;
        };
        self.notify_user(format!("Loading {name}…"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(response) = rx.recv().await else { return };
            let text = response["result"].as_str().unwrap_or_default().to_string();
            this.update_in(cx, |this, window, cx| {
                if text.trim().is_empty() {
                    this.notify_user(format!("No source available for {name}"));
                    cx.notify();
                    return;
                }
                let saved = path
                    .parent()
                    .map(|dir| std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, &text)));
                if !matches!(saved, Some(Ok(()))) {
                    this.notify_user("Couldn't cache the library source");
                    cx.notify();
                    return;
                }
                this.open_file(path.clone(), window, cx);
                this.reveal_in_active(&location, true, cx);
                this.notify_user(format!("{name} — library source (read-only)"));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::jdt_file_name;

    #[test]
    fn names_from_jdt_uris() {
        let uri = "jdt://contents/java.base/java.io/PrintStream.java?=cli/C:%5C/x.jar%60java.base=/%3Cjava.io%28PrintStream.class";
        assert_eq!(jdt_file_name(uri).as_deref(), Some("PrintStream.java"));
        assert_eq!(jdt_file_name("jdt://contents/lib.jar/org.x/Foo.class?=p").as_deref(), Some("Foo.java"));
        assert_eq!(jdt_file_name("jdt://contents/a/..%2F..%2Fevil.java?=x"), None);
        assert_eq!(jdt_file_name("file:///a/B.java"), None);
    }
}
