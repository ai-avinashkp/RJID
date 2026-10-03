//! The sample plugin (`plugins/java-tools`, compiled to WebAssembly) run
//! through the real host. Build it first:
//! `cargo build --release --target wasm32-unknown-unknown` in that folder,
//! then copy the .wasm to `plugin.wasm` (skipped if it isn't built).

use std::path::PathBuf;

use rji_plugins::{CommandInput, CommandOutput, Plugin};

fn plugin() -> Option<Plugin> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/java-tools");
    if !dir.join("plugin.wasm").exists() {
        eprintln!("skipping: plugins/java-tools/plugin.wasm not built");
        return None;
    }
    Some(Plugin::load(&dir).expect("sample plugin loads (and imports nothing)"))
}

fn run(plugin: &Plugin, command: &str, text: &str) -> CommandOutput {
    plugin
        .run(&CommandInput {
            command: command.into(),
            text: text.into(),
            is_selection: true,
            file_name: "Person.java".into(),
            language: "Java".into(),
        })
        .unwrap()
}

#[test]
fn sample_plugin_commands() {
    let Some(plugin) = plugin() else { return };
    // getters-setters is no longer listed (the editor generates them natively)
    // but the plugin still implements it.
    assert_eq!(plugin.manifest.commands.len(), 5);

    let CommandOutput::Replace(out) = run(&plugin, "getters-setters", "    private String name;\n    private boolean active;\n    private final int id;\n") else {
        panic!("expected replace");
    };
    assert!(out.contains("    public String getName() {\n        return name;\n    }"), "{out}");
    assert!(out.contains("public void setName(String name) {"), "{out}");
    assert!(out.contains("public boolean isActive()"), "{out}");
    assert!(out.contains("public int getId()") && !out.contains("setId"), "final fields get no setter: {out}");

    assert_eq!(run(&plugin, "sort-lines", "b\nC\na"), CommandOutput::Replace("a\nb\nC".into()));
    assert_eq!(run(&plugin, "camel-snake", "myValueName"), CommandOutput::Replace("my_value_name".into()));
    assert_eq!(run(&plugin, "camel-snake", "my_value_name"), CommandOutput::Replace("myValueName".into()));
    assert_eq!(run(&plugin, "upper", "abc"), CommandOutput::Replace("ABC".into()));
    assert_eq!(
        run(&plugin, "count", "one two\nthree"),
        CommandOutput::Message("Selection: 2 lines, 3 words, 13 characters".into())
    );
    let err = plugin
        .run(&CommandInput {
            command: "getters-setters".into(),
            text: "not java".into(),
            is_selection: true,
            file_name: String::new(),
            language: String::new(),
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("select field declarations"), "{err}");
}
