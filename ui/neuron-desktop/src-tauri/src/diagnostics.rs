//! L5: extensions and resilience. Which engine runs, why it failed to start or
//! stopped, and a plain-text report for support. Pure functions with no Tauri
//! types, so each is tested on its own; `lib.rs` only registers the commands.
//!
//! Problems reach the webview as `[engine:<code>] <plain recovery text>`; the
//! webview parses the prefix (`src/engineProblems.ts`). Codes: `missing`,
//! `quarantined`, `denied`, `crashed`, `failed`.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Windows: the file contains a virus or unwanted software.
const ERROR_VIRUS_INFECTED: i32 = 225;
/// Windows: the file was removed because it contained a virus.
const ERROR_VIRUS_DELETED: i32 = 226;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Other,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineSource {
    Env,
    Bundled,
    Path,
    Missing,
}

impl EngineSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Bundled => "bundled",
            Self::Path => "path",
            Self::Missing => "missing",
        }
    }
}

/// Which engine would run, and where the bundled one should be.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub source: EngineSource,
    pub path: Option<String>,
    pub bundled_path: Option<String>,
    pub bundled_present: bool,
}

pub fn engine_file_name() -> &'static str {
    if cfg!(windows) {
        "neuron.exe"
    } else {
        "neuron"
    }
}

/// The lookup order, as a pure function: `NEURON_BIN`, beside the app's
/// executable (`exe_dir`), then the PATH.
pub fn resolve_engine(
    neuron_bin: Option<OsString>,
    exe_dir: Option<&Path>,
    path_var: Option<OsString>,
    name: &str,
) -> (EngineSource, Option<PathBuf>) {
    if let Some(path) = neuron_bin.filter(|value| !value.is_empty()) {
        return (EngineSource::Env, Some(PathBuf::from(path)));
    }
    if let Some(sibling) = exe_dir.map(|dir| dir.join(name)) {
        if sibling.is_file() {
            return (EngineSource::Bundled, Some(sibling));
        }
    }
    let found = path_var.and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    });
    match found {
        Some(path) => (EngineSource::Path, Some(path)),
        None => (EngineSource::Missing, None),
    }
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// [`resolve_engine`] over the real environment.
pub fn resolve_from_environment() -> (EngineSource, Option<PathBuf>) {
    resolve_engine(
        std::env::var_os("NEURON_BIN"),
        exe_dir().as_deref(),
        std::env::var_os("PATH"),
        engine_file_name(),
    )
}

pub fn engine_info_for(
    resolved: (EngineSource, Option<PathBuf>),
    exe_dir: Option<&Path>,
    name: &str,
) -> EngineInfo {
    let bundled = exe_dir.map(|dir| dir.join(name));
    EngineInfo {
        source: resolved.0,
        path: resolved.1.map(|path| path.display().to_string()),
        bundled_present: bundled.as_ref().is_some_and(|path| path.is_file()),
        bundled_path: bundled.map(|path| path.display().to_string()),
    }
}

/// What the app would run, for Settings.
pub fn current_engine_info() -> EngineInfo {
    engine_info_for(
        resolve_from_environment(),
        exe_dir().as_deref(),
        engine_file_name(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemCode {
    Missing,
    Quarantined,
    Denied,
    Crashed,
    Failed,
}

impl ProblemCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Quarantined => "quarantined",
            Self::Denied => "denied",
            Self::Crashed => "crashed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineProblem {
    pub code: ProblemCode,
    pub message: String,
}

impl EngineProblem {
    fn new(code: ProblemCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The string the webview receives: `[engine:<code>] <message>`.
    pub fn render(&self) -> String {
        format!("[engine:{}] {}", self.code.as_str(), self.message)
    }
}

fn blocked_text(os: Os) -> &'static str {
    match os {
        Os::Windows => "Windows security blocked the Neuron engine. Restore it from Protection history, or reinstall Neuron.",
        Os::Mac => "macOS blocked the Neuron engine. Open System Settings, Privacy and Security, and choose Open Anyway, or reinstall Neuron.",
        Os::Other => "Your system's security software blocked the Neuron engine. Allow it to run, or reinstall Neuron.",
    }
}

const NOT_FOUND_HELP: &str = "Reinstall Neuron, or set NEURON_BIN to the engine's path. On Windows, check Protection history in Windows Security: it may have been quarantined.";

/// Why starting the engine failed, from the OS error. `raw` 225 and 226 are
/// the Windows "virus" errors (what Defender returns for a quarantined file).
pub fn classify_spawn(raw: Option<i32>, kind: ErrorKind, os: Os, binary: &Path) -> EngineProblem {
    if os == Os::Windows && matches!(raw, Some(ERROR_VIRUS_INFECTED | ERROR_VIRUS_DELETED)) {
        return EngineProblem::new(ProblemCode::Quarantined, blocked_text(os));
    }
    match kind {
        ErrorKind::NotFound => EngineProblem::new(
            ProblemCode::Missing,
            format!(
                "The Neuron engine was not found at {}. {NOT_FOUND_HELP}",
                binary.display()
            ),
        ),
        ErrorKind::PermissionDenied => EngineProblem::new(
            ProblemCode::Denied,
            format!(
                "The system would not let Neuron run the engine at {} (permission denied). Make sure the file is allowed to run, or reinstall Neuron.",
                binary.display()
            ),
        ),
        _ => EngineProblem::new(
            ProblemCode::Failed,
            format!(
                "Could not start the Neuron engine at {}: {}",
                binary.display(),
                os_text(raw, kind)
            ),
        ),
    }
}

fn os_text(raw: Option<i32>, kind: ErrorKind) -> String {
    match raw {
        Some(code) => format!("{kind} (system error {code})"),
        None => kind.to_string(),
    }
}

/// [`classify_spawn`] for a real error, on this OS.
pub fn classify_spawn_error(error: &std::io::Error, binary: &Path) -> EngineProblem {
    classify_spawn(error.raw_os_error(), error.kind(), Os::current(), binary)
}

/// The engine was not found anywhere (no `NEURON_BIN`, no bundled one, not on the PATH).
pub fn missing_problem() -> EngineProblem {
    EngineProblem::new(
        ProblemCode::Missing,
        format!("The Neuron engine was not found. {NOT_FOUND_HELP}"),
    )
}

/// How an engine that ended is reported: `None` for a clean exit. `code` is
/// `None` when the process was killed; on macOS, one killed with nothing said
/// is how Gatekeeper and code-signing checks stop a binary at launch.
pub fn classify_exit(code: Option<i32>, last_error: Option<&str>, os: Os) -> Option<EngineProblem> {
    if code == Some(0) {
        return None;
    }
    let last = last_error.map(str::trim).filter(|line| !line.is_empty());
    if os == Os::Mac && code.is_none() && last.is_none() {
        return Some(EngineProblem::new(
            ProblemCode::Quarantined,
            blocked_text(os),
        ));
    }
    let mut message = format!(
        "The Neuron engine stopped unexpectedly (exit code {}).",
        code.map_or("unknown".to_string(), |code| code.to_string())
    );
    if let Some(line) = last {
        message.push_str(" Last message: ");
        message.push_str(line);
    }
    Some(EngineProblem::new(ProblemCode::Crashed, message))
}

/// The error string for an engine that ended; `None` for a clean exit.
pub fn exit_error(code: Option<i32>, last_error: Option<&str>) -> Option<String> {
    classify_exit(code, last_error, Os::current()).map(|problem| problem.render())
}

/// The host's part of the support report. No account data, no environment, no files.
pub fn diagnostics_text(app_version: &str, os: &str, arch: &str, engine: &EngineInfo) -> String {
    let mut lines = vec![
        format!("App version: {app_version}"),
        format!("OS: {os} {arch}"),
        format!("Engine source: {}", engine.source.as_str()),
        format!("Engine path: {}", engine.path.as_deref().unwrap_or("none")),
        format!(
            "Bundled engine: {} ({})",
            engine.bundled_path.as_deref().unwrap_or("unknown"),
            if engine.bundled_present {
                "present"
            } else {
                "not present"
            }
        ),
    ];
    if matches!(engine.source, EngineSource::Env | EngineSource::Path) {
        lines.push("Not running the bundled engine.".to_string());
    }
    lines.join("\n")
}

pub fn current_diagnostics(app_version: &str) -> String {
    diagnostics_text(
        app_version,
        std::env::consts::OS,
        std::env::consts::ARCH,
        &current_engine_info(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Error;

    fn bin() -> &'static Path {
        Path::new("C:/app/neuron.exe")
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neuron-diag-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn windows_virus_errors_mean_quarantined() {
        for raw in [225, 226] {
            let problem = classify_spawn(Some(raw), ErrorKind::Other, Os::Windows, bin());
            assert_eq!(problem.code, ProblemCode::Quarantined, "os error {raw}");
            assert!(problem
                .render()
                .starts_with("[engine:quarantined] Windows security blocked the Neuron engine."));
            assert!(problem.message.contains("Protection history"));
        }
        // The same numbers elsewhere are not the Windows virus errors.
        assert_ne!(
            classify_spawn(Some(225), ErrorKind::Other, Os::Other, bin()).code,
            ProblemCode::Quarantined
        );
    }

    #[test]
    fn real_os_errors_are_classified() {
        let not_found = Error::from_raw_os_error(2);
        let problem = classify_spawn_error(&not_found, bin());
        assert_eq!(problem.code, ProblemCode::Missing);
        assert!(problem.render().starts_with("[engine:missing] "));
        let denied = Error::from_raw_os_error(if cfg!(windows) { 5 } else { 13 });
        assert_eq!(
            classify_spawn_error(&denied, bin()).code,
            ProblemCode::Denied
        );
        #[cfg(windows)]
        for raw in [225, 226] {
            assert_eq!(
                classify_spawn_error(&Error::from_raw_os_error(raw), bin()).code,
                ProblemCode::Quarantined
            );
        }
    }

    #[test]
    fn other_spawn_errors_are_failed_with_the_reason() {
        let problem = classify_spawn(None, ErrorKind::InvalidInput, Os::Other, bin());
        assert_eq!(problem.code, ProblemCode::Failed);
        assert!(problem.message.contains("invalid input"));
    }

    #[test]
    fn mac_names_gatekeeper_not_windows() {
        let problem = classify_exit(None, None, Os::Mac).unwrap();
        assert_eq!(problem.code, ProblemCode::Quarantined);
        assert!(problem.message.contains("macOS") && !problem.message.contains("Windows"));
    }

    #[test]
    fn exits_carry_the_code_and_the_last_message() {
        assert_eq!(classify_exit(Some(0), Some("x"), Os::Windows), None);
        let problem = classify_exit(Some(3), Some("  out of memory "), Os::Windows).unwrap();
        assert_eq!(problem.code, ProblemCode::Crashed);
        assert_eq!(
            problem.render(),
            "[engine:crashed] The Neuron engine stopped unexpectedly (exit code 3). Last message: out of memory"
        );
        let killed = classify_exit(None, None, Os::Other).unwrap();
        assert!(killed.message.contains("exit code unknown"));
        assert_eq!(exit_error(Some(0), None), None);
    }

    #[test]
    fn missing_engine_is_a_coded_message() {
        assert!(missing_problem().render().starts_with("[engine:missing] "));
    }

    #[test]
    fn the_env_override_wins_even_over_a_bundled_engine() {
        let dir = temp("env");
        std::fs::write(dir.join("neuron-x"), "").unwrap();
        let (source, path) =
            resolve_engine(Some("/custom/neuron".into()), Some(&dir), None, "neuron-x");
        assert_eq!(
            (source, path),
            (EngineSource::Env, Some(PathBuf::from("/custom/neuron")))
        );
        // An empty override is no override.
        let (source, _) = resolve_engine(Some("".into()), Some(&dir), None, "neuron-x");
        assert_eq!(source, EngineSource::Bundled);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bundled_beats_the_path_and_missing_is_reported() {
        let app = temp("app");
        let other = temp("onpath");
        std::fs::write(other.join("neuron-x"), "").unwrap();
        let path_var = std::env::join_paths([&other]).unwrap();
        let (source, path) = resolve_engine(None, Some(&app), Some(path_var.clone()), "neuron-x");
        assert_eq!(
            (source, path),
            (EngineSource::Path, Some(other.join("neuron-x")))
        );
        std::fs::write(app.join("neuron-x"), "").unwrap();
        let (source, path) = resolve_engine(None, Some(&app), Some(path_var), "neuron-x");
        assert_eq!(
            (source, path),
            (EngineSource::Bundled, Some(app.join("neuron-x")))
        );
        let empty = temp("empty");
        assert_eq!(
            resolve_engine(None, Some(&empty), None, "neuron-x"),
            (EngineSource::Missing, None)
        );
        for dir in [app, other, empty] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn engine_info_names_the_bundled_path_and_whether_it_is_there() {
        let dir = temp("info");
        let info = engine_info_for((EngineSource::Missing, None), Some(&dir), "neuron-x");
        assert!(!info.bundled_present && info.path.is_none() && info.bundled_path.is_some());
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["source"], "missing");
        assert_eq!(json["bundledPresent"], false);
        assert!(json["path"].is_null());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_report_has_the_engine_and_no_secrets() {
        let info = EngineInfo {
            source: EngineSource::Env,
            path: Some("/opt/neuron".into()),
            bundled_path: Some("/app/neuron".into()),
            bundled_present: true,
        };
        let text = diagnostics_text("0.2.0", "windows", "x86_64", &info);
        assert!(text.contains("App version: 0.2.0") && text.contains("OS: windows x86_64"));
        assert!(text.contains("Engine source: env") && text.contains("Engine path: /opt/neuron"));
        assert!(text.contains("Not running the bundled engine."));
        // It is built from its arguments only: no environment, no account, no files.
        for needle in ["@", "token", "Bearer", "NEURON_BIN="] {
            assert!(!text.contains(needle), "{needle}");
        }
        let live = current_diagnostics("0.2.0");
        for (key, value) in std::env::vars() {
            let secret = key.to_uppercase().contains("TOKEN") || key.to_uppercase().contains("KEY");
            assert!(
                !(secret && value.len() > 8 && live.contains(&value)),
                "{key}"
            );
        }
    }
}
