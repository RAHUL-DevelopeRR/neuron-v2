//! L2: review and git. "Open in editor": a file of the open folder goes to
//! VS Code when it can be launched safely, else it is shown in the system's
//! file manager. Nothing here builds a shell string, and the file is never
//! "opened" by the system (that could run a script): it is only revealed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::git::path_problem;

/// The file's real path, which must exist and sit inside `root` (canonical).
pub fn resolve(root: &Path, file: &str) -> Result<PathBuf, String> {
    if let Some(problem) = path_problem(file) {
        return Err(problem.into());
    }
    let real = std::fs::canonicalize(root.join(file))
        .map_err(|_| "That file is not there.".to_string())?;
    if !real.starts_with(root) {
        return Err("That path is outside the workspace.".into());
    }
    Ok(real)
}

/// Without Windows' `\\?\` prefix, which most programs do not expect.
fn plain(path: &Path) -> String {
    let shown = path.display().to_string();
    shown
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| shown.strip_prefix(r"\\?\").map(ToString::to_string))
        .unwrap_or(shown)
}

/// The first `name` + extension found in the `path_var` folders.
pub fn find_in_path(
    name: &str,
    path_var: &std::ffi::OsStr,
    extensions: &[&str],
) -> Option<PathBuf> {
    std::env::split_paths(path_var).find_map(|dir| {
        extensions.iter().find_map(|extension| {
            let candidate = dir.join(format!("{name}{extension}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

/// VS Code's `bin/code.cmd` shim sits next to the real program (`Code.exe`, one
/// folder up). Running the `.cmd` would need a shell, so the program is run
/// directly; `None` when the layout is not the usual one.
pub fn vscode_exe_from_shim(shim: &Path) -> Option<PathBuf> {
    let install = shim.parent()?.parent()?;
    ["Code.exe", "Code - Insiders.exe", "VSCodium.exe"]
        .iter()
        .map(|name| install.join(name))
        .find(|candidate| candidate.is_file())
}

/// The program that opens VS Code on this machine, if one can be run safely.
fn vscode_program() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    if cfg!(windows) {
        let shim = find_in_path("code", &path_var, &[".cmd"])?;
        vscode_exe_from_shim(&shim)
    } else {
        find_in_path("code", &path_var, &[""])
    }
}

fn spawn(command: &mut Command) -> Result<(), String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn reveal(file: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer wants `/select,"path"` as one argument, quoted by hand
        // (a Windows file name cannot hold a quote).
        let mut command = Command::new("explorer.exe");
        command.raw_arg(format!("/select,\"{}\"", plain(file)));
        spawn(&mut command)
    }
    #[cfg(target_os = "macos")]
    {
        spawn(Command::new("open").arg("-R").arg(plain(file)))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let folder = file.parent().unwrap_or(file);
        spawn(Command::new("xdg-open").arg(plain(folder)))
    }
}

pub fn open(root: &Path, file: &str) -> Result<(), String> {
    let real = resolve(root, file)?;
    if let Some(program) = vscode_program() {
        // `--` so a path can never be read as an option.
        if spawn(Command::new(program).arg("--").arg(plain(&real))).is_ok() {
            return Ok(());
        }
    }
    reveal(&real)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("neuron-desktop-editor-{label}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn a_file_inside_the_folder_resolves() {
        let root = temp("inside");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "x").unwrap();
        assert_eq!(resolve(&root, "src/a.rs").unwrap(), root.join("src/a.rs"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn paths_outside_the_folder_or_missing_are_refused() {
        let root = temp("refuse");
        let sibling = root.parent().unwrap().join("neuron-editor-sibling.txt");
        fs::write(&sibling, "x").unwrap();
        let absolute = sibling.display().to_string();
        for bad in [
            "../neuron-editor-sibling.txt",
            absolute.as_str(),
            "/etc/hosts",
            "-x",
            "",
        ] {
            assert!(resolve(&root, bad).is_err(), "{bad}");
        }
        assert_eq!(
            resolve(&root, "missing.txt").unwrap_err(),
            "That file is not there."
        );
        let _ = fs::remove_file(sibling);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_pointing_out_of_the_folder_is_refused() {
        let root = temp("link");
        let outside = temp("link-target");
        fs::write(outside.join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
        assert!(resolve(&root, "link.txt").unwrap_err().contains("outside"));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn the_code_shim_maps_to_the_real_program_or_nothing() {
        let install = temp("vscode");
        fs::create_dir_all(install.join("bin")).unwrap();
        fs::write(install.join("bin/code.cmd"), "@echo").unwrap();
        let shim = install.join("bin/code.cmd");
        assert_eq!(vscode_exe_from_shim(&shim), None, "no Code.exe beside it");
        fs::write(install.join("Code.exe"), "").unwrap();
        assert_eq!(vscode_exe_from_shim(&shim), Some(install.join("Code.exe")));
        let _ = fs::remove_dir_all(install);
    }

    #[test]
    fn finds_a_program_on_a_path_list() {
        let first = temp("path-a");
        let second = temp("path-b");
        fs::write(second.join("code.cmd"), "").unwrap();
        let joined = std::env::join_paths([&first, &second]).unwrap();
        assert_eq!(
            find_in_path("code", &joined, &[".exe", ".cmd"]),
            Some(second.join("code.cmd"))
        );
        assert_eq!(find_in_path("nope", &joined, &[".cmd"]), None);
        let _ = fs::remove_dir_all(first);
        let _ = fs::remove_dir_all(second);
    }
}
