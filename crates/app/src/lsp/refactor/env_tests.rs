//! The plan's view of a real folder, through the local agent.

use std::path::{Path, PathBuf};

use super::*;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-refactor-env-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    crate::ui::canonical_for_test(d)
}

fn env() -> HostEnv {
    HostEnv::new(Arc::new(host::local()), HashMap::new())
}

fn s(p: &Path) -> String {
    crate::ui::local_path(p)
}

#[test]
fn kinds_distinguish_files_folders_and_missing_paths() {
    let d = dir("kind");
    std::fs::write(d.join("şablon 😀.txt"), "x").unwrap();
    std::fs::create_dir(d.join("alt")).unwrap();
    let env = env();
    assert_eq!(env.kind(&s(&d.join("şablon 😀.txt"))), Kind::File);
    assert_eq!(env.kind(&s(&d.join("alt"))), Kind::Dir);
    assert_eq!(env.kind(&s(&d.join("yok"))), Kind::Missing);
    assert_eq!(env.read(&s(&d.join("şablon 😀.txt"))).unwrap().0, b"x");
    assert!(env.read(&s(&d.join("yok"))).is_err());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_path_that_does_not_exist_is_resolved_through_its_nearest_existing_parent() {
    let d = dir("resolve");
    std::fs::create_dir(d.join("src")).unwrap();
    let env = env();
    let asked = format!("{}/src/new/deeper/a.rs", s(&d));
    assert_eq!(env.resolve(&asked), asked);
    // `..` is resolved before the host sees it.
    let up = format!("{}/src/../src/a.rs", s(&d));
    assert_eq!(env.resolve(&up), format!("{}/src/a.rs", s(&d)));
    std::fs::remove_dir_all(&d).ok();
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_followed_even_below_a_path_that_does_not_exist_yet() {
    let d = dir("link");
    let outside = dir("link-outside");
    std::fs::write(outside.join("keys.txt"), "k").unwrap();
    std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();
    let env = env();
    assert_eq!(env.resolve(&format!("{}/link/keys.txt", s(&d))), format!("{}/keys.txt", s(&outside)));
    assert_eq!(env.resolve(&format!("{}/link/new/a.txt", s(&d))), format!("{}/new/a.txt", s(&outside)));
    // The link itself is a file or folder of its own for rename and delete.
    assert_eq!(env.kind(&format!("{}/link", s(&d))), Kind::File);
    // One request for many paths gives the same answers.
    let paths = vec![format!("{}/link/keys.txt", s(&d)), format!("{}/nothing", s(&d))];
    env.prepare(&paths);
    assert_eq!(env.resolve(&paths[0]), format!("{}/keys.txt", s(&outside)));
    assert_eq!(env.resolve(&paths[1]), paths[1]);
    std::fs::remove_dir_all(&d).ok();
    std::fs::remove_dir_all(&outside).ok();
}

#[test]
fn windows_api_prefixes_and_backslashes_are_removed() {
    assert_eq!(tidy(r"\\?\C:\Users\ada\a.rs"), "C:/Users/ada/a.rs");
    assert_eq!(tidy(r"\\?\UNC\server\share\a.rs"), "//server/share/a.rs");
    assert_eq!(tidy("/home/ada/a.rs"), "/home/ada/a.rs");
}
