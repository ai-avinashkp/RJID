//! Spring Boot DevTools reload-on-save.
//!
//! DevTools' own file watcher restarts the running app when it sees
//! `target/classes` (Maven) or `build/classes` (Gradle) change — it does not
//! itself recompile anything. Without an IDE-side nudge, the classes on disk
//! only change on a full `mvn package`/`gradle build`, which is much heavier
//! than a normal edit-save loop. So on every `.java` save, if the workspace
//! has `spring-boot-devtools` on its classpath, `CodeEditorView::save`
//! fire-and-forgets a background `compile`-only build to refresh just the
//! changed class file(s) and let DevTools' own watcher do the actual
//! restart. Mirrors the shared-`Rc<RefCell<>>` pattern used for
//! `SharedLspClient` in `lsp_shared.rs`, for the same reason: both the root
//! view (which knows the detected build tool) and every open editor tab
//! (which knows when a save happens) need read access on the single UI
//! thread without threading a callback through every layer.

use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Clone)]
pub struct DevtoolsHook {
    pub workspace_root: PathBuf,
    /// A compile-only shell command line, e.g. `mvn compile -q` or
    /// `.\gradlew.bat compileJava -q` — run with the workspace root as its
    /// working directory.
    pub compile_command: String,
}

impl DevtoolsHook {
    /// Spawns the compile command detached, ignoring its output/exit code —
    /// this is a best-effort nudge to DevTools' watcher, not something a
    /// save should ever block on or fail because of.
    pub fn trigger(&self) {
        let (shell, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
        let _ = Command::new(shell)
            .args([flag, &self.compile_command])
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

pub type SharedDevtoolsHook = std::rc::Rc<std::cell::RefCell<Option<DevtoolsHook>>>;
