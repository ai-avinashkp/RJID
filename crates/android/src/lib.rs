//! Android device support: find an installed Android SDK (never bundled —
//! same posture as JDK detection), list connected devices / USB-debugging
//! phones and emulators, launch an emulator, and build the shell commands
//! for installing an app, starting it, and streaming logcat (those run in
//! the IDE's terminal so their output is visible).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sdk {
    pub root: Option<PathBuf>,
    pub adb: PathBuf,
    pub emulator: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    /// `device` (ready), `offline`, `unauthorized` (accept the USB
    /// debugging prompt on the phone), …
    pub state: String,
    pub model: Option<String>,
    pub is_emulator: bool,
}

impl Device {
    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }

    pub fn label(&self) -> String {
        let name = self.model.clone().unwrap_or_else(|| self.serial.clone());
        if self.is_emulator {
            format!("{name} (emulator)")
        } else {
            name
        }
    }
}

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// Finds the Android SDK: `ANDROID_HOME`, `ANDROID_SDK_ROOT`, then the
/// default Android Studio location; `adb` falls back to PATH.
pub fn detect_sdk() -> Option<Sdk> {
    let home = |var: &str| std::env::var_os(var).map(PathBuf::from);
    let mut candidates: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"].iter().filter_map(|v| home(v)).collect();
    if cfg!(windows) {
        candidates.extend(home("LOCALAPPDATA").map(|p| p.join("Android").join("Sdk")));
    } else if cfg!(target_os = "macos") {
        candidates.extend(home("HOME").map(|p| p.join("Library/Android/sdk")));
    } else {
        candidates.extend(home("HOME").map(|p| p.join("Android/Sdk")));
    }
    let root = candidates.into_iter().find(|p| p.is_dir());
    let sdk_tool = |dir: &str, name: &str| {
        root.as_ref()
            .map(|r| r.join(dir).join(format!("{name}{EXE}")))
            .filter(|p| p.is_file())
    };
    let adb = sdk_tool("platform-tools", "adb").or_else(|| find_on_path(&format!("adb{EXE}")))?;
    Some(Sdk {
        emulator: sdk_tool("emulator", "emulator"),
        root,
        adb,
    })
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// Runs a tool and returns its stdout, giving up after `timeout` (the
/// process is killed) — a wedged `adb`/emulator must never hang the IDE.
fn run(program: &Path, args: &[&str], timeout: Duration) -> anyhow::Result<String> {
    let mut child = hidden(Command::new(program).args(args))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{} didn't respond in time", program.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The tool has exited; a leftover child (e.g. a daemon) could still hold
    // the pipe, so don't wait on the reader forever either.
    let wait_until = Instant::now() + Duration::from_secs(2);
    while !reader.is_finished() && Instant::now() < wait_until {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(if reader.is_finished() { reader.join().unwrap_or_default() } else { String::new() })
}

/// Connected devices and running emulators (`adb devices -l`).
pub fn list_devices(sdk: &Sdk) -> anyhow::Result<Vec<Device>> {
    // Start the adb server first with no pipes attached: when `adb devices`
    // has to start it, the daemon inherits the output pipe and reading it
    // would never finish.
    let _ = hidden(Command::new(&sdk.adb).arg("start-server"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(parse_devices(&run(&sdk.adb, &["devices", "-l"], Duration::from_secs(15))?))
}

pub fn parse_devices(output: &str) -> Vec<Device> {
    output
        .lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?.to_string();
            let model = parts
                .find_map(|p| p.strip_prefix("model:"))
                .map(|m| m.replace('_', " "));
            Some(Device {
                is_emulator: serial.starts_with("emulator-"),
                serial,
                state,
                model,
            })
        })
        .collect()
}

/// Emulator images (AVDs) available to launch (`emulator -list-avds`).
pub fn list_avds(sdk: &Sdk) -> anyhow::Result<Vec<String>> {
    let Some(emulator) = &sdk.emulator else {
        return Ok(Vec::new());
    };
    Ok(parse_avds(&run(emulator, &["-list-avds"], Duration::from_secs(15))?))
}

pub fn parse_avds(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        // The emulator prints INFO/WARNING lines on some setups.
        .filter(|l| !l.is_empty() && !l.contains(' ') && !l.starts_with("INFO") && !l.starts_with("WARNING"))
        .map(str::to_string)
        .collect()
}

/// Starts an emulator in the background (it has its own window).
pub fn launch_emulator(sdk: &Sdk, avd: &str) -> anyhow::Result<()> {
    let emulator = sdk.emulator.as_ref().ok_or_else(|| anyhow::anyhow!("the Android emulator isn't installed"))?;
    anyhow::ensure!(
        !avd.is_empty() && avd.chars().all(|c| c.is_alphanumeric() || "_-.".contains(c)),
        "invalid AVD name"
    );
    Command::new(emulator)
        .args(["-avd", avd])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

/// An Android app module in the workspace (Gradle `com.android.application`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidProject {
    /// Gradle path of the app module, e.g. `:app`.
    pub module: String,
    /// Package name (`applicationId` / `namespace`), if found.
    pub application_id: Option<String>,
}

pub fn detect_android_project(workspace: &Path) -> Option<AndroidProject> {
    // Usual layout: an `app` module; also accept a single-module project.
    for (module, dir) in [(":app", workspace.join("app")), ("", workspace.to_path_buf())] {
        for script in ["build.gradle.kts", "build.gradle"] {
            if let Ok(text) = std::fs::read_to_string(dir.join(script))
                && text.contains("com.android.application")
            {
                return Some(AndroidProject {
                    module: module.to_string(),
                    application_id: application_id(&text),
                });
            }
        }
    }
    None
}

/// `applicationId "com.x.y"` / `applicationId = "com.x.y"` / `namespace`.
pub fn application_id(script: &str) -> Option<String> {
    for key in ["applicationId", "namespace"] {
        for line in script.lines() {
            let line = line.split("//").next().unwrap_or(line);
            if let Some(pos) = line.find(key) {
                let rest = line[pos + key.len()..].trim_start_matches([' ', '=']);
                // The quoted value right after the key.
                let id = rest
                    .strip_prefix(['"', '\''])
                    .and_then(|r| r.split(['"', '\'']).next())
                    .unwrap_or("");
                if !id.is_empty() && id.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_') {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

/// Quotes a path for PowerShell / POSIX shells.
fn shell_quote(path: &Path) -> String {
    let s = path.display().to_string();
    if cfg!(windows) {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `& 'C:\…\adb.exe' -s <serial> …` (PowerShell needs `&` to run a quoted path).
fn adb_command(sdk: &Sdk, serial: &str, args: &str) -> String {
    let call = if cfg!(windows) { "& " } else { "" };
    format!("{call}{} -s {serial} {args}", shell_quote(&sdk.adb))
}

pub fn install_apk_command(sdk: &Sdk, serial: &str, apk: &Path) -> String {
    adb_command(sdk, serial, &format!("install -r {}", shell_quote(apk)))
}

pub fn logcat_command(sdk: &Sdk, serial: &str) -> String {
    adb_command(sdk, serial, "logcat -v color")
}

/// Starts the app's launcher activity (needs no activity name).
pub fn launch_app_command(sdk: &Sdk, serial: &str, application_id: &str) -> String {
    adb_command(
        sdk,
        serial,
        &format!("shell monkey -p {application_id} -c android.intent.category.LAUNCHER 1"),
    )
}

/// Builds and installs the debug app on one device via Gradle
/// (`ANDROID_SERIAL` picks the device), then starts it.
pub fn build_install_run_command(sdk: &Sdk, project: &AndroidProject, serial: &str, gradle: &str) -> String {
    let task = format!("{}:installDebug", project.module);
    let set_serial = if cfg!(windows) {
        format!("$env:ANDROID_SERIAL='{serial}'; ")
    } else {
        format!("ANDROID_SERIAL='{serial}' ")
    };
    let mut command = format!("{set_serial}{gradle} {task}");
    if cfg!(windows) {
        command.push_str("; Remove-Item Env:ANDROID_SERIAL");
    }
    if let Some(id) = &project.application_id {
        let chain = if cfg!(windows) { "; if ($?) { " } else { " && " };
        let close = if cfg!(windows) { " }" } else { "" };
        command.push_str(&format!("{chain}{}{close}", launch_app_command(sdk, serial, id)));
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adb_devices() {
        let out = "* daemon started successfully\nList of devices attached\n\
                   emulator-5554          device product:sdk_gphone64 model:sdk_gphone64_x86_64 device:emu64x transport_id:1\n\
                   R58M12ABCDE            unauthorized usb:1-1 transport_id:2\n\n";
        let devices = parse_devices(out);
        assert_eq!(devices.len(), 2);
        assert!(devices[0].is_emulator && devices[0].is_ready());
        assert_eq!(devices[0].model.as_deref(), Some("sdk gphone64 x86 64"));
        assert_eq!(devices[1].state, "unauthorized");
        assert!(!devices[1].is_ready());
        assert!(parse_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn parses_avds_ignoring_noise() {
        let out = "INFO    | Storing crashdata in: C:\\x\nPixel_7_API_34\nMedium_Phone\n";
        assert_eq!(parse_avds(out), vec!["Pixel_7_API_34", "Medium_Phone"]);
    }

    #[test]
    fn finds_application_id() {
        assert_eq!(application_id("android {\n  defaultConfig {\n    applicationId \"com.ex.app\"\n").as_deref(), Some("com.ex.app"));
        assert_eq!(application_id("android {\n    namespace = \"com.ex.k\"\n").as_deref(), Some("com.ex.k"));
        assert_eq!(application_id("nothing here"), None);
    }

    #[test]
    fn builds_commands() {
        let sdk = Sdk {
            root: None,
            adb: PathBuf::from("C:\\sdk\\platform-tools\\adb.exe"),
            emulator: None,
        };
        let project = AndroidProject {
            module: ":app".into(),
            application_id: Some("com.ex.app".into()),
        };
        let cmd = build_install_run_command(&sdk, &project, "emulator-5554", ".\\gradlew.bat");
        assert!(cmd.contains(":app:installDebug"), "{cmd}");
        assert!(cmd.contains("monkey -p com.ex.app"), "{cmd}");
        assert!(logcat_command(&sdk, "emulator-5554").contains("-s emulator-5554 logcat"));
        assert!(install_apk_command(&sdk, "x", Path::new("C:\\a b\\app.apk")).contains("'C:\\a b\\app.apk'"));
    }

    #[test]
    fn detects_android_project() {
        let dir = std::env::temp_dir().join(format!("rji-android-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::write(
            dir.join("app/build.gradle.kts"),
            "plugins { id(\"com.android.application\") }\nandroid { namespace = \"com.ex.app\" }\n",
        )
        .unwrap();
        let project = detect_android_project(&dir).unwrap();
        assert_eq!((project.module.as_str(), project.application_id.as_deref()), (":app", Some("com.ex.app")));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Against this machine's real SDK: `cargo test -p rji-android -- --ignored`
    #[test]
    #[ignore]
    fn live_sdk() {
        let sdk = detect_sdk().expect("an Android SDK");
        println!("{sdk:?}");
        println!("devices: {:?}", list_devices(&sdk).unwrap());
        println!("avds: {:?}", list_avds(&sdk).unwrap());
    }
}
