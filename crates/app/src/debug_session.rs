//! Wires `rji_jdwp_client` into the app.
//!
//! `start_debug` in `root.rs` launches the active Run Config under the JDWP
//! agent and attaches; this module's [`DebugSession`] then owns everything
//! about the live session: keeping the target's breakpoints in sync with
//! the gutter (including toggles made mid-session), arming breakpoints in
//! classes as they load, and building the paused state (call stack, local
//! variables, source file + line) when the program stops.
//!
//! Line numbers: the editor uses 0-based lines, JDWP/`javac` 1-based ones.
//! Everything crossing into the client converts explicitly.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use rji_jdwp_client::{
    DebugEvent, EVENT_KIND_BREAKPOINT, EVENT_KIND_SINGLE_STEP, JdwpClient, Location, StepDepth,
    Variable, signature_to_class_name,
};

const MAX_FRAMES: i32 = 30;

/// Picks a free TCP port for the JDWP agent. There's a tiny race between
/// closing this listener and the JVM binding the port — acceptable for a
/// local debugging tool.
pub fn free_port() -> anyhow::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Polls for the target's JDWP socket: the JVM needs a moment to start
/// after the launch command is typed into the terminal, and nothing tells
/// us when it's listening. Gives up after ~15 s (e.g. the command failed).
pub async fn connect_with_retry(addr: String) -> anyhow::Result<JdwpClient> {
    let mut last_err = None;
    for _ in 0..100 {
        match JdwpClient::attach(&addr) {
            Ok(client) => return Ok(client),
            Err(err) => {
                last_err = Some(err);
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("could not connect to {addr}")))
}

/// Best-effort fully-qualified class name for a Java source file: its file
/// stem, prefixed with any `package ...;` declaration near the top. A
/// wrong guess only means that file's breakpoints never resolve.
pub fn java_class_name(path: &Path, content: &str) -> Option<String> {
    let stem = path.file_stem()?.to_str()?.to_string();
    let package = content.lines().take(40).find_map(|line| {
        let rest = line.trim().strip_prefix("package ")?;
        Some(rest.trim_end_matches(';').trim().to_string())
    });
    Some(match package {
        Some(pkg) if !pkg.is_empty() => format!("{pkg}.{stem}"),
        _ => stem,
    })
}

/// How to run a command so its JVM waits for a debugger: the shell
/// command line to type, and the port the JVM will listen on.
#[derive(Debug, PartialEq, Eq)]
pub struct DebugLaunch {
    pub command: String,
    pub port: u16,
}

/// Gradle's `--debug-jvm` always uses this port (suspended until attach).
const GRADLE_DEBUG_PORT: u16 = 5005;

/// Rewrites a run command to start its JVM under the JDWP agent, for the
/// shapes the IDE understands; `None` for anything else (the Debug button
/// is hidden then). `port` is a free port chosen by the caller; Gradle
/// ignores it (see `GRADLE_DEBUG_PORT`). Commands are typed into
/// PowerShell, so `-D...` arguments are quoted (PowerShell would otherwise
/// split them at `.`/`,`).
pub fn debug_launch(command: &str, port: u16) -> Option<DebugLaunch> {
    let agent = format!("-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address={port}");
    let command = command.trim();
    let first = command.split_whitespace().next()?.to_lowercase();
    let tool = first.trim_start_matches(".\\").trim_start_matches("./");
    let has = |goal: &str| command.split_whitespace().any(|w| w == goal);

    if tool == "java" || tool == "java.exe" {
        let rest = command.split_once(char::is_whitespace)?.1.trim_start();
        return Some(DebugLaunch {
            command: format!("{first} {agent} {rest}"),
            port,
        });
    }
    if matches!(tool, "mvn" | "mvn.cmd" | "mvnw" | "mvnw.cmd") {
        if has("spring-boot:run") {
            return Some(DebugLaunch {
                command: format!("{command} \"-Dspring-boot.run.jvmArguments={agent}\""),
                port,
            });
        }
        if has("test") || has("verify") {
            return Some(DebugLaunch {
                command: format!("{command} \"-Dmaven.surefire.debug={agent}\""),
                port,
            });
        }
        if command.contains("exec:java") {
            // exec:java runs inside Maven's own JVM. The variable is scoped to
            // this one command so later builds in the shell don't also wait.
            let command = if cfg!(windows) {
                format!("$env:MAVEN_OPTS='{agent}'; {command}; Remove-Item Env:MAVEN_OPTS")
            } else {
                format!("MAVEN_OPTS='{agent}' {command}")
            };
            return Some(DebugLaunch {
                command,
                port,
            });
        }
        return None;
    }
    if matches!(tool, "gradle" | "gradle.bat" | "gradlew" | "gradlew.bat")
        && (has("run") || has("bootRun") || has("test"))
    {
        return Some(DebugLaunch {
            command: format!("{command} --debug-jvm"),
            port: GRADLE_DEBUG_PORT,
        });
    }
    // Compound commands (`javac …; if ($?) { java -cp out Main }`, or
    // `gradle classes && java …`): the agent goes after the `java` launcher.
    let launcher = command.match_indices("java ").find(|(at, _)| {
        command[..*at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_whitespace() || c == '{')
    })?;
    let insert_at = launcher.0 + "java".len();
    Some(DebugLaunch {
        command: format!("{} {agent}{}", &command[..insert_at], &command[insert_at..]),
        port,
    })
}

fn jni_signature(class_name: &str) -> String {
    format!("L{};", class_name.replace('.', "/"))
}

/// One frame of the call stack, ready to display.
#[derive(Debug, Clone)]
pub struct FrameInfo {
    pub label: String,
    /// Source file, if it could be located in the workspace.
    pub path: Option<PathBuf>,
    /// 0-based editor line.
    pub line: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct PausedState {
    pub thread_id: u64,
    pub frames: Vec<FrameInfo>,
    /// Locals of the top frame, or why they're unavailable.
    pub variables: Result<Vec<Variable>, String>,
}

/// What the UI needs to react to after an event.
pub enum SessionUpdate {
    None,
    Paused,
    Ended,
}

pub struct DebugSession {
    pub client: Rc<JdwpClient>,
    workspace_root: PathBuf,
    /// Breakpoints the user wants: file -> (class name, 0-based lines).
    desired: HashMap<PathBuf, (String, BTreeSet<u32>)>,
    /// Breakpoints actually set in the VM: (file, 0-based line) -> request.
    armed: HashMap<(PathBuf, u32), i32>,
    prepare_requested: HashSet<String>,
    class_names: HashMap<u64, String>,
    step_request: Option<i32>,
    pub paused: Option<PausedState>,
}

impl DebugSession {
    pub fn new(client: JdwpClient, workspace_root: PathBuf) -> Self {
        DebugSession {
            client: Rc::new(client),
            workspace_root,
            desired: HashMap::new(),
            armed: HashMap::new(),
            prepare_requested: HashSet::new(),
            class_names: HashMap::new(),
            step_request: None,
            paused: None,
        }
    }

    /// Replaces the breakpoints for one file and syncs the VM: clears the
    /// removed ones, arms new ones now if the class is already loaded, or
    /// asks to be told when it loads.
    pub fn set_breakpoints(&mut self, path: PathBuf, class_name: String, lines: BTreeSet<u32>) {
        let stale: Vec<(PathBuf, u32)> = self
            .armed
            .keys()
            .filter(|(p, line)| *p == path && !lines.contains(line))
            .cloned()
            .collect();
        for key in stale {
            if let Some(request_id) = self.armed.remove(&key) {
                let _ = self.client.clear_event_request(EVENT_KIND_BREAKPOINT, request_id);
            }
        }
        self.desired.insert(path.clone(), (class_name.clone(), lines));

        let loaded = self
            .client
            .classes_by_signature(&jni_signature(&class_name))
            .unwrap_or_default();
        for class_id in loaded {
            self.arm_for_class(class_id, &class_name);
        }
        if self.prepare_requested.insert(class_name.clone()) {
            let _ = self.client.set_class_prepare_request(&class_name);
        }
    }

    /// Arms every desired-but-unarmed breakpoint belonging to `class_name`.
    fn arm_for_class(&mut self, class_id: u64, class_name: &str) {
        let targets: Vec<(PathBuf, u32)> = self
            .desired
            .iter()
            .filter(|(_, (name, _))| name == class_name)
            .flat_map(|(path, (_, lines))| lines.iter().map(move |l| (path.clone(), *l)))
            .filter(|key| !self.armed.contains_key(key))
            .collect();
        for (path, line) in targets {
            if let Ok(Some((method_id, index))) = self.client.resolve_line(class_id, line + 1) {
                let location = Location {
                    class_id,
                    method_id,
                    index,
                };
                if let Ok(request_id) = self.client.set_breakpoint(&location) {
                    self.armed.insert((path, line), request_id);
                }
            }
        }
    }

    /// Whether a breakpoint was actually set in the VM (vs. only toggled
    /// in the gutter — e.g. on a blank or comment line with no code).
    pub fn is_armed(&self, path: &Path, line: u32) -> bool {
        self.armed.contains_key(&(path.to_path_buf(), line))
    }

    pub fn handle_event(&mut self, event: DebugEvent) -> SessionUpdate {
        match event {
            DebugEvent::VmStart => {
                // Breakpoint/prepare requests were registered before this
                // point (the VM starts suspended); let it run.
                let _ = self.client.resume();
                SessionUpdate::None
            }
            DebugEvent::ClassPrepare { reference_type_id } => {
                if let Some(name) = self.class_name(reference_type_id) {
                    self.arm_for_class(reference_type_id, &name);
                }
                let _ = self.client.resume();
                SessionUpdate::None
            }
            DebugEvent::Breakpoint {
                thread_id,
                location,
            } => {
                self.on_stop(thread_id, &location);
                SessionUpdate::Paused
            }
            DebugEvent::Step {
                thread_id,
                location,
                request_id,
            } => {
                let _ = self.client.clear_event_request(EVENT_KIND_SINGLE_STEP, request_id);
                self.step_request = None;
                self.on_stop(thread_id, &location);
                SessionUpdate::Paused
            }
            DebugEvent::VmDeath => SessionUpdate::Ended,
        }
    }

    pub fn resume(&mut self) {
        self.paused = None;
        let _ = self.client.resume();
    }

    /// Step Into / Over / Out from the paused location.
    pub fn step(&mut self, depth: StepDepth) {
        let Some(paused) = self.paused.take() else {
            return;
        };
        if let Some(previous) = self.step_request.take() {
            let _ = self.client.clear_event_request(EVENT_KIND_SINGLE_STEP, previous);
        }
        if let Ok(request_id) = self.client.step(paused.thread_id, depth) {
            self.step_request = Some(request_id);
        }
        let _ = self.client.resume();
    }

    /// Terminates the debugged program.
    pub fn stop(&mut self) {
        self.paused = None;
        let _ = self.client.exit(1);
    }

    fn class_name(&mut self, class_id: u64) -> Option<String> {
        if let Some(name) = self.class_names.get(&class_id) {
            return Some(name.clone());
        }
        let signature = self.client.class_signature(class_id).ok()?;
        let name = signature_to_class_name(&signature);
        self.class_names.insert(class_id, name.clone());
        Some(name)
    }

    fn on_stop(&mut self, thread_id: u64, location: &Location) {
        let frames = self
            .client
            .frames(thread_id, MAX_FRAMES)
            .unwrap_or_default();
        let mut infos = Vec::new();
        for frame in &frames {
            let class = self
                .class_name(frame.location.class_id)
                .unwrap_or_else(|| "?".to_string());
            let method = self
                .client
                .method_name(frame.location.class_id, frame.location.method_id)
                .unwrap_or_else(|_| "?".to_string());
            let line = self.client.line_for_location(&frame.location).ok().flatten();
            let simple = class.rsplit('.').next().unwrap_or(&class).to_string();
            infos.push(FrameInfo {
                label: match line {
                    Some(l) => format!("{simple}.{method}:{l}"),
                    None => format!("{simple}.{method}"),
                },
                path: self.source_path(&class),
                line: line.map(|l| l.saturating_sub(1)),
            });
        }
        if infos.is_empty() {
            // Couldn't read the stack; still report where it stopped.
            let class = self.class_name(location.class_id).unwrap_or_default();
            let line = self.client.line_for_location(location).ok().flatten();
            infos.push(FrameInfo {
                label: class.clone(),
                path: self.source_path(&class),
                line: line.map(|l| l.saturating_sub(1)),
            });
        }

        let variables = match frames.first() {
            Some(top) => match self.client.local_variables(thread_id, top) {
                Ok(Some(vars)) => Ok(vars),
                Ok(None) => Err("No local variable info — compile with `javac -g`".to_string()),
                Err(err) => Err(format!("Could not read variables: {err}")),
            },
            None => Err("No stack frames".to_string()),
        };

        self.paused = Some(PausedState {
            thread_id,
            frames: infos,
            variables,
        });
    }

    /// Finds the source file for a class: an open/breakpointed file first,
    /// then the conventional Maven/Gradle source roots. JDK and library
    /// classes have no source here, so they return `None`.
    fn source_path(&self, class_name: &str) -> Option<PathBuf> {
        // Inner/anonymous classes (`App$1`) live in their outer class's file.
        let outer = class_name.split('$').next().unwrap_or(class_name);
        if let Some((path, _)) = self.desired.iter().find(|(_, (name, _))| name == outer) {
            return Some(path.clone());
        }
        let relative = format!("{}.java", outer.replace('.', "/"));
        ["src/main/java", "src/test/java", "src", ""]
            .iter()
            .map(|root| self.workspace_root.join(root).join(&relative))
            .find(|candidate| candidate.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_name_uses_package_declaration() {
        let src = "// header\npackage com.example.app;\n\npublic class App {}";
        assert_eq!(
            java_class_name(Path::new("src/App.java"), src).as_deref(),
            Some("com.example.app.App")
        );
        assert_eq!(
            java_class_name(Path::new("Main.java"), "class Main {}").as_deref(),
            Some("Main")
        );
    }

    #[test]
    fn debug_launch_for_each_tool() {
        let agent = "-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=7000";
        assert_eq!(
            debug_launch("java -cp target/classes demo.Main", 7000).unwrap().command,
            format!("java {agent} -cp target/classes demo.Main")
        );
        let boot = debug_launch("mvn spring-boot:run", 7000).unwrap();
        assert_eq!(boot.command, format!("mvn spring-boot:run \"-Dspring-boot.run.jvmArguments={agent}\""));
        assert!(debug_launch("mvn test", 7000).unwrap().command.contains("maven.surefire.debug"));
        let exec = debug_launch("mvn compile exec:java -Dexec.mainClass=demo.Main", 7000).unwrap();
        if cfg!(windows) {
            assert!(exec.command.starts_with("$env:MAVEN_OPTS=") && exec.command.ends_with("Remove-Item Env:MAVEN_OPTS"));
        } else {
            assert!(exec.command.starts_with("MAVEN_OPTS='") && exec.command.contains(" mvn compile exec:java"), "{}", exec.command);
        }
        let gradle = debug_launch(".\\gradlew.bat bootRun", 7000).unwrap();
        assert_eq!((gradle.command.as_str(), gradle.port), (".\\gradlew.bat bootRun --debug-jvm", 5005));
        assert_eq!(debug_launch("mvn package", 7000), None);
        assert_eq!(debug_launch("npm start", 7000), None);
        let plain = debug_launch("javac -d out (Get-ChildItem src); if ($?) { java -cp out Main }", 7000).unwrap();
        assert_eq!(plain.command, format!("javac -d out (Get-ChildItem src); if ($?) {{ java {agent} -cp out Main }}"));
        assert_eq!(debug_launch("mvn package", 7000), None);
    }

    #[test]
    fn jni_signature_format() {
        assert_eq!(jni_signature("com.example.App"), "Lcom/example/App;");
    }

    /// Drives a real JVM through `DebugSession`: breakpoint on an editor
    /// (0-based) line, pause there with the right file/line/locals, step
    /// over, then run to the end. Needs `javac`/`java` on PATH:
    /// `cargo test -p rji-app -- --ignored debug_session_against_real_jvm`
    #[test]
    #[ignore]
    fn debug_session_against_real_jvm() {
        use std::process::{Command, Stdio};

        let dir = std::env::temp_dir().join(format!("rji-session-test-{}", std::process::id()));
        let src_dir = dir.join("src/main/java/demo");
        let classes = dir.join("classes");
        std::fs::create_dir_all(&src_dir).unwrap();
        // Editor lines (0-based): 4 `int a`, 5 `int b = twice(a)`, 6 println,
        // 9 inside `twice`.
        let source = "package demo;\n\npublic class Main {\n    public static void main(String[] args) {\n        int a = 20;\n        int b = twice(a);\n        System.out.println(b);\n    }\n    static int twice(int n) {\n        return n * 2;\n    }\n}\n";
        let file = src_dir.join("Main.java");
        std::fs::write(&file, source).unwrap();
        let ok = Command::new("javac")
            .args(["-g", "-d"])
            .arg(&classes)
            .arg(&file)
            .status()
            .unwrap()
            .success();
        assert!(ok, "javac failed");

        let port = free_port().unwrap();
        let mut child = Command::new("java")
            .arg(format!("-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address={port}"))
            .arg("-cp")
            .arg(&classes)
            .arg("demo.Main")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let client = futures_lite_block_on(connect_with_retry(format!("127.0.0.1:{port}"))).unwrap();
        let events = client.events_rx.clone();
        let mut session = DebugSession::new(client, dir.clone());

        // Editor line 4 (0-based) is `int a = 20;` — javac's line 5.
        let class = java_class_name(&file, source).unwrap();
        assert_eq!(class, "demo.Main");
        session.set_breakpoints(file.clone(), class, BTreeSet::from([4]));

        let pause_until = |session: &mut DebugSession| loop {
            let event = events.recv_blocking().expect("VM ended before pausing");
            if let SessionUpdate::Paused = session.handle_event(event) {
                return;
            }
        };

        pause_until(&mut session);
        assert!(session.is_armed(&file, 4));
        let paused = session.paused.clone().unwrap();
        assert_eq!(paused.frames[0].line, Some(4), "stopped on the wrong line");
        assert_eq!(paused.frames[0].path.as_deref(), Some(file.as_path()));
        assert_eq!(paused.frames[0].label, "Main.main:5");

        session.step(StepDepth::Over);
        pause_until(&mut session);
        let paused = session.paused.clone().unwrap();
        assert_eq!(paused.frames[0].line, Some(5));
        let vars = paused.variables.unwrap();
        assert!(vars.iter().any(|v| v.name == "a" && v.value == "20"), "{vars:?}");

        // Into `twice(a)`: now two frames deep, with `n` bound.
        session.step(StepDepth::Into);
        pause_until(&mut session);
        let paused = session.paused.clone().unwrap();
        assert_eq!(paused.frames[0].line, Some(9), "{:?}", paused.frames);
        assert_eq!(paused.frames[0].label, "Main.twice:10");
        assert_eq!(paused.frames[1].label, "Main.main:6");
        let vars = paused.variables.unwrap();
        assert!(vars.iter().any(|v| v.name == "n" && v.value == "20"), "{vars:?}");

        // Out: back in main (still on the assignment line, which finishes
        // after the call returns).
        session.step(StepDepth::Out);
        pause_until(&mut session);
        let paused = session.paused.clone().unwrap();
        assert_eq!(paused.frames.len(), 1, "{:?}", paused.frames);
        assert_eq!(paused.frames[0].line, Some(5));

        session.resume();
        loop {
            match events.recv_blocking() {
                Ok(event) => {
                    if let SessionUpdate::Ended = session.handle_event(event) {
                        break;
                    }
                }
                Err(_) => break, // connection closed as the VM exited
            }
        }
        assert!(child.wait().unwrap().success());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `connect_with_retry` is async only so the app can run it on its
    /// executor; it never actually awaits, so polling it once completes it.
    fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut future = std::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("connect_with_retry unexpectedly pending"),
        }
    }
}
