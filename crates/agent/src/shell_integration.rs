//! Shell integration: small scripts that make zsh, bash and fish mark their prompts, the start and end of each command
//! (with its exit code) and the working folder, with OSC 133 and OSC 7 sequences the terminal reads. The user's own
//! startup files are still read first; the scripts only add hooks, print nothing and keep `$?`.
//!
//! The scripts are written to the cache folder (`$XDG_CACHE_HOME` or `~/.cache`) under `codeyard/shell-integration/<hash>`,
//! private to the user (0700 folders, 0600 files). A folder that is a symlink or belongs to someone else is not used.

#[cfg(any(unix, test))]
use std::io;
#[cfg(any(unix, test))]
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(any(unix, test))]
const FILES: &[(&str, &str)] = &[
    ("zsh/.zshenv", include_str!("shell/zsh/.zshenv")),
    ("zsh/.zprofile", include_str!("shell/zsh/.zprofile")),
    ("zsh/.zshrc", include_str!("shell/zsh/.zshrc")),
    ("zsh/.zlogin", include_str!("shell/zsh/.zlogin")),
    ("bash.sh", include_str!("shell/bash.sh")),
    ("fishdata/fish/vendor_conf.d/codeyard.fish", include_str!("shell/fish/codeyard.fish")),
];

#[cfg(any(unix, test))]
fn script_files() -> Vec<(&'static str, String)> {
    // Cross builds may embed a Windows checkout. Hash and write the same LF
    // bytes so shell syntax and the cache key do not depend on the build host.
    FILES.iter().map(|(name, text)| (*name, unix_script(text))).collect()
}

#[cfg(any(unix, test))]
fn unix_script(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[cfg(any(unix, test))]
fn write_script(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let short: String = path.file_name().unwrap_or_default().to_string_lossy().chars().take(60).collect();
    for _ in 0..8 {
        let tmp = super::temp_path(parent, &short);
        let mut file = match super::create_private(&tmp, false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = file.write_all(text.as_bytes());
        drop(file);
        let result = written.and_then(|()| std::fs::rename(&tmp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        return result;
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not reserve a shell integration temporary file"))
}

#[cfg(test)]
mod script_tests {
    #[test]
    fn failed_script_publication_cleans_up_its_temporary_file() {
        let root = std::env::temp_dir().join(format!("codeyard-shell-failed-write-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("bash.sh")).unwrap();
        assert!(super::write_script(&root.join("bash.sh"), "script").is_err());
        let names: Vec<_> = std::fs::read_dir(&root).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("bash.sh")]);
        assert!(root.join("bash.sh").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn script_installation_preserves_an_existing_temporary_file() {
        let root = std::env::temp_dir().join(format!("codeyard-shell-temp-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("bash.sh");
        let planted = target.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&planted, "unrelated data").unwrap();
        super::write_script(&target, "shell script").unwrap();
        assert_eq!(std::fs::read_to_string(&planted).unwrap(), "unrelated data");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "shell script");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn simultaneous_script_installations_do_not_share_temporary_files() {
        let root = std::env::temp_dir().join(format!("codeyard-shell-parallel-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("bash.sh");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let target = target.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    super::write_script(&target, &"# script\n".repeat(4096))
                })
            })
            .collect();
        let results: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
        assert!(results.iter().all(Result::is_ok), "concurrent installation failed: {results:?}");
        assert_eq!(std::fs::read_to_string(target).unwrap(), "# script\n".repeat(4096));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn bash_reports_literal_percent_sequences_in_directory_names() {
        let bash = if cfg!(windows) {
            std::path::PathBuf::from(std::env::var_os("ProgramFiles").expect("ProgramFiles must be set")).join("Git/bin/bash.exe")
        } else {
            std::path::PathBuf::from("bash")
        };
        let script = super::script_files().into_iter().find(|(name, _)| *name == "bash.sh").unwrap().1;
        // Run the shipped prompt hook without sourcing the user's startup files.
        let hook = script.split_once("  __cy_prompt() {").unwrap().1.split_once("\n  }").unwrap().0;
        let command = format!("__cy_prompt() {{{hook}\n}}\nPWD='/tmp/rapor%20son%25'; __cy_prompt");
        let output = std::process::Command::new(bash).args(["--noprofile", "--norc", "-c", &command]).output().unwrap();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.contains("/tmp/rapor%2520son%2525"), "wrong OSC directory: {output:?}");
    }

    #[test]
    fn installed_scripts_use_unix_line_endings() {
        assert_eq!(super::unix_script("for f in a; do\r\n  echo \"$f\"\r\ndone\r\n"), "for f in a; do\n  echo \"$f\"\ndone\n");
        for (name, text) in super::script_files() {
            assert!(!text.contains('\r'), "{name} contains Windows line endings");
        }
    }
}

#[cfg(unix)]
fn cache_root() -> Option<PathBuf> {
    match std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        Some(p) => Some(p),
        None => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")),
    }
}

/// A folder we may write to: created private if missing; refused if it is a symlink or not ours.
#[cfg(unix)]
fn private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => return Err(io::Error::other(format!("{} is not a plain folder", path.display()))),
        // SAFETY: geteuid has no preconditions and cannot fail.
        Ok(m) if m.uid() != unsafe { libc::geteuid() } => return Err(io::Error::other(format!("{} belongs to another user", path.display()))),
        Ok(m) => {
            if m.permissions().mode() & 0o077 != 0 {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Err(_) => std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?,
    }
    Ok(())
}

/// Writes the scripts (if they are not there already) and returns their folder.
#[cfg(unix)]
pub fn install_in(root: &Path) -> io::Result<PathBuf> {
    let files = script_files();
    let all: String = files.iter().map(|(n, t)| format!("{n}\n{t}")).collect();
    let base = root.join("codeyard").join("shell-integration");
    let dir = base.join(format!("{:016x}", proto::content_hash(all.as_bytes())));
    std::fs::create_dir_all(root)?;
    for d in [root.join("codeyard"), base, dir.clone()] {
        private_dir(&d)?;
    }
    for (name, text) in &files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            let mut p = dir.clone();
            for part in parent.strip_prefix(&dir).unwrap_or(Path::new("")).components() {
                p.push(part);
                private_dir(&p)?;
            }
        }
        // Rewritten if anything else changed it.
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) && std::fs::read(&path).is_ok_and(|b| b == text.as_bytes()) {
            continue;
        }
        write_script(&path, text)?;
    }
    Ok(dir)
}

/// Arguments (and added environment) to start `shell` with integration; `None` for other shells or when the scripts
/// cannot be written (the shell then starts as usual).
#[cfg(unix)]
pub fn setup(shell: &str, env: &mut Vec<(String, String)>) -> Option<Vec<String>> {
    setup_in(&cache_root()?, shell, env)
}

#[cfg(unix)]
fn setup_in(root: &Path, shell: &str, env: &mut Vec<(String, String)>) -> Option<Vec<String>> {
    let name = Path::new(shell).file_name()?.to_string_lossy().trim_start_matches('-').to_string();
    if !matches!(name.as_str(), "zsh" | "bash" | "fish") {
        return None;
    }
    let dir = install_in(root).ok()?;
    let at = |rel: &str| dir.join(rel).to_string_lossy().into_owned();
    env.push(("CODEYARD_SHELL_INTEGRATION".into(), "1".into()));
    match name.as_str() {
        "zsh" => {
            // Where the user's own zsh files are (a profile may say).
            if !env.iter().any(|(k, _)| k == "CODEYARD_USER_ZDOTDIR") {
                let user = std::env::var("ZDOTDIR").or_else(|_| std::env::var("HOME")).unwrap_or_default();
                env.push(("CODEYARD_USER_ZDOTDIR".into(), user));
            }
            env.push(("ZDOTDIR".into(), at("zsh")));
            Some(vec!["-l".into()])
        }
        // `--rcfile` is ignored by a login shell, so bash starts interactive and the script reads the login files.
        "bash" => Some(vec!["--rcfile".into(), at("bash.sh"), "-i".into()]),
        _ => {
            let rest = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
            env.push(("XDG_DATA_DIRS".into(), format!("{}:{rest}", at("fishdata"))));
            Some(vec!["-l".into()])
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-shint-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn scripts_are_written_privately_and_a_planted_symlink_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("install");
        let dir = install_in(&root).unwrap();
        assert!(dir.join("zsh/.zshrc").is_file() && dir.join("bash.sh").is_file());
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(dir.join("bash.sh")).unwrap().permissions().mode() & 0o777, 0o600);
        // A changed script is put back.
        std::fs::write(dir.join("bash.sh"), "echo pwned").unwrap();
        install_in(&root).unwrap();
        assert!(std::fs::read_to_string(dir.join("bash.sh")).unwrap().contains("133;A"));

        let evil = temp("evil");
        std::fs::create_dir_all(evil.join("target")).unwrap();
        std::fs::create_dir_all(evil.join("codeyard")).unwrap();
        std::os::unix::fs::symlink(evil.join("target"), evil.join("codeyard/shell-integration")).unwrap();
        assert!(install_in(&evil).is_err(), "a symlinked folder is not written through");
    }

    #[test]
    fn each_shell_gets_its_way_in_and_others_are_left_alone() {
        let root = temp("setup");
        let mut env = Vec::new();
        assert!(setup_in(&root, "/bin/sh", &mut env).is_none() && env.is_empty());
        let args = setup_in(&root, "/bin/bash", &mut env).unwrap();
        assert_eq!(args[0], "--rcfile");
        assert!(args[1].ends_with("bash.sh") && args[2] == "-i");
        let mut env = Vec::new();
        assert_eq!(setup_in(&root, "/usr/bin/zsh", &mut env).unwrap(), vec!["-l"]);
        assert!(env.iter().any(|(k, v)| k == "ZDOTDIR" && v.ends_with("/zsh")));
        let mut env = Vec::new();
        setup_in(&root, "/opt/homebrew/bin/fish", &mut env).unwrap();
        assert!(env.iter().any(|(k, v)| k == "XDG_DATA_DIRS" && v.contains("fishdata:")));
    }
}
