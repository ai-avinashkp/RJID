//! A real end-to-end exercise of the client against an actual JVM: compiles
//! a trivial Java program, launches it under the JDWP agent, attaches,
//! resumes past the initial suspend, sets a breakpoint via
//! `ClassPrepare` + line-table resolution, confirms the target actually
//! stops there (checked by *not* having printed the line after the
//! breakpoint yet), resumes again, and confirms the program then runs to
//! completion with the expected full output. Not a `#[test]` because it
//! needs a real `java`/`javac` on PATH and a free TCP port — run explicitly
//! with `cargo run -p rji-jdwp-client --example smoke`.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use rji_jdwp_client::{DebugEvent, JdwpClient};

const SOURCE: &str = r#"
public class DebugTarget {
    public static void main(String[] args) throws Exception {
        String greeting = "hi";
        System.out.println("BEFORE");
        int x = 1 + 1;
        System.out.println("AFTER " + x + greeting);
    }
}
"#;
const BREAKPOINT_LINE: u32 = 6; // "int x = 1 + 1;"

fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rji-jdwp-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let src_path = dir.join("DebugTarget.java");
    std::fs::write(&src_path, SOURCE)?;

    println!("compiling {}", src_path.display());
    // `-g` for local-variable debug info (Maven/Gradle compile this way too).
    let javac = Command::new("javac").arg("-g").arg(&src_path).status()?;
    anyhow::ensure!(javac.success(), "javac failed");

    let port = free_port()?;
    println!("launching target on JDWP port {port}");
    let mut child = Command::new("java")
        .arg(format!(
            "-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address={port}"
        ))
        .arg("-cp")
        .arg(&dir)
        .arg("DebugTarget")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let stdout = child.stdout.take().expect("piped stdout");
    let (line_tx, line_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            println!("[target] {line}");
            let _ = line_tx.send(line);
        }
    });

    let addr = format!("127.0.0.1:{port}");
    let client = connect_with_retry(&addr)?;
    println!("attached, id_sizes = {:?}", client.id_sizes);

    match client.events_rx.recv_blocking()? {
        DebugEvent::VmStart => println!("got VMStart"),
        other => anyhow::bail!("expected VMStart first, got {other:?}"),
    }
    client.resume()?;
    println!("resumed past initial suspend");

    client.set_class_prepare_request("DebugTarget")?;
    let reference_type_id = loop {
        match client.events_rx.recv_blocking()? {
            DebugEvent::ClassPrepare { reference_type_id } => break reference_type_id,
            other => println!("(ignoring event while waiting for ClassPrepare: {other:?})"),
        }
    };
    println!("DebugTarget class prepared: reference_type_id={reference_type_id}");

    let (method_id, code_index) = client
        .resolve_line(reference_type_id, BREAKPOINT_LINE)?
        .ok_or_else(|| anyhow::anyhow!("line {BREAKPOINT_LINE} not found in DebugTarget's line table"))?;
    println!("resolved line {BREAKPOINT_LINE} -> method_id={method_id} index={code_index}");
    client.set_breakpoint(&rji_jdwp_client::Location {
        class_id: reference_type_id,
        method_id,
        index: code_index,
    })?;
    // ClassPrepare suspended the thread that triggered it; let it continue
    // now that the breakpoint is armed.
    client.resume()?;

    let (thread_id, location) = loop {
        match client.events_rx.recv_blocking()? {
            DebugEvent::Breakpoint {
                thread_id,
                location,
            } => break (thread_id, location),
            other => println!("(ignoring event while waiting for Breakpoint: {other:?})"),
        }
    };
    println!(
        "BREAKPOINT HIT: thread={} ({}), location={:?}",
        thread_id,
        client.thread_name(thread_id)?,
        location
    );

    // Give the drained-stdout reader a moment to catch up, then check we
    // have "BEFORE" but *not yet* "AFTER" — proof the target really is
    // suspended at the breakpoint rather than having run to completion.
    std::thread::sleep(Duration::from_millis(300));
    let seen_before_break: Vec<String> = line_rx.try_iter().collect();
    anyhow::ensure!(
        seen_before_break.iter().any(|l| l == "BEFORE"),
        "expected to have seen \"BEFORE\" printed before the breakpoint, got {seen_before_break:?}"
    );
    anyhow::ensure!(
        !seen_before_break.iter().any(|l| l.starts_with("AFTER")),
        "target printed the line AFTER the breakpoint before we resumed it — breakpoint did not actually suspend it! got {seen_before_break:?}"
    );
    println!("confirmed: target is suspended before printing AFTER");

    // Call stack + source line mapping + locals at the breakpoint.
    let frames = client.frames(thread_id, 20)?;
    anyhow::ensure!(!frames.is_empty(), "no frames for suspended thread");
    let top_line = client.line_for_location(&frames[0].location)?;
    anyhow::ensure!(top_line == Some(BREAKPOINT_LINE), "top frame line {top_line:?}");
    let method = client.method_name(frames[0].location.class_id, frames[0].location.method_id)?;
    anyhow::ensure!(method == "main", "top frame method {method}");
    let locals = client.local_variables(thread_id, &frames[0])?.unwrap_or_default();
    println!("locals at line {BREAKPOINT_LINE}: {locals:?}");
    anyhow::ensure!(
        locals.iter().any(|v| v.name == "greeting" && v.value == "\"hi\""),
        "expected greeting = \"hi\" in {locals:?}"
    );
    anyhow::ensure!(
        !locals.iter().any(|v| v.name == "x"),
        "x must not be in scope before its assignment runs"
    );

    // Step over the assignment: should stop on the next line with x == 2.
    let step_request = client.step_over(thread_id)?;
    client.resume()?;
    let (step_thread, step_location) = loop {
        match client.events_rx.recv_blocking()? {
            DebugEvent::Step {
                thread_id,
                location,
                ..
            } => break (thread_id, location),
            other => println!("(ignoring event while waiting for Step: {other:?})"),
        }
    };
    let _ = client.clear_event_request(rji_jdwp_client::EVENT_KIND_SINGLE_STEP, step_request);
    let step_line = client.line_for_location(&step_location)?;
    anyhow::ensure!(step_line == Some(BREAKPOINT_LINE + 1), "step landed on {step_line:?}");
    let frames = client.frames(step_thread, 1)?;
    let locals = client.local_variables(step_thread, &frames[0])?.unwrap_or_default();
    println!("locals after step over: {locals:?}");
    anyhow::ensure!(
        locals.iter().any(|v| v.name == "x" && v.value == "2" && v.type_name == "int"),
        "expected x = 2 after stepping over the assignment, got {locals:?}"
    );
    println!("confirmed: frames, line mapping, locals, and step-over all correct");

    client.resume()?;
    println!("resumed past breakpoint, waiting for target to exit");
    let status = child.wait()?;
    anyhow::ensure!(status.success(), "target process exited with {status:?}");

    let mut all_lines = seen_before_break;
    for line in line_rx.try_iter() {
        all_lines.push(line);
    }
    // Drain any remaining buffered lines the reader thread sent just before
    // the process actually exited.
    std::thread::sleep(Duration::from_millis(200));
    for line in line_rx.try_iter() {
        all_lines.push(line);
    }
    anyhow::ensure!(
        all_lines.iter().any(|l| l == "AFTER 2hi"),
        "expected \"AFTER 2hi\" after resuming past the breakpoint, got {all_lines:?}"
    );

    println!("\nSMOKE TEST PASSED: attach, resume, class-prepare, line-resolve, breakpoint hit+verified-suspended, frames, locals, step-over, resume-to-completion all worked against a real JVM.");
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

fn free_port() -> anyhow::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn connect_with_retry(addr: &str) -> anyhow::Result<JdwpClient> {
    let mut last_err = None;
    for _ in 0..50 {
        match JdwpClient::attach(addr) {
            Ok(client) => return Ok(client),
            Err(err) => {
                last_err = Some(err);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("could not connect to {addr}")))
}
