use super::*;

fn inputs() -> Inputs {
    Inputs {
        version: "0.9.1",
        commit: Some("0123456789abcdef0123"),
        os: "macos",
        arch: "aarch64",
        language: "tr".into(),
        locales: vec!["tr-TR".into(), "en-US".into()],
        settings: vec![("editor.fontSize".into(), "16".into()), ("files.autoSave".into(), "\"afterDelay\"".into())],
        notifications: vec![
            NoticeLine { severity: "Error", source: "Git".into(), message: "push to https://bob:hunter2@github.com/o/r failed\nrejected".into() },
            NoticeLine { severity: "Info", source: "Update".into(), message: "up to date".into() },
        ],
        log: vec!["2026-11-12T10:00:00.000Z INFO  app: started".into()],
        home: Some("/Users/dogan".into()),
    }
}

#[test]
fn the_report_names_version_system_settings_notifications_and_log() {
    let text = build(&inputs());
    for expected in [
        "Version: 0.9.1",
        "Commit: 0123456789ab\n",
        "System: macos aarch64",
        "Language: tr",
        "System locale: tr-TR, en-US",
        "editor.fontSize = 16",
        "[Error] Git: push to https://***@github.com/o/r failed rejected",
        "[Info] Update: up to date",
        "INFO  app: started",
    ] {
        assert!(text.contains(expected), "{expected:?} missing from:\n{text}");
    }
}

#[test]
fn nothing_personal_survives() {
    let mut i = inputs();
    i.settings.push(("terminal.shell".into(), "\"/Users/dogan/bin/zsh\"".into()));
    i.notifications.push(NoticeLine {
        severity: "Warning",
        source: "Remote".into(),
        message: "ssh deploy@build.example.com refused; opened C:\\Users\\Doğan\\x and /Users/dogan/work; Authorization: Bearer abc.def.ghi".into(),
    });
    i.log.push("GITHUB_TOKEN=ghp_0123456789abcdefghijABCDEFGHIJ012345 in /Users/dogan/.config".into());
    let text = build(&i);
    for secret in ["dogan", "hunter2", "deploy@", "abc.def.ghi", "ghp_", "Doğan"] {
        assert!(!text.contains(secret), "{secret:?} leaked:\n{text}");
    }
    assert!(text.contains("***@build.example.com"), "{text}");
    assert!(text.contains("~/work"), "{text}");
}

#[test]
fn empty_parts_say_so_and_a_missing_commit_is_explicit() {
    let text = build(&Inputs { commit: None, settings: vec![], notifications: vec![], log: vec![], locales: vec![], ..inputs() });
    assert!(text.contains("Commit: not recorded") && text.contains("System locale: unknown"), "{text}");
    assert_eq!(text.matches("(none)").count(), 2);
    assert!(text.contains("(empty)"));
}

#[test]
fn only_the_newest_lines_and_notifications_are_kept() {
    let mut i = inputs();
    i.log = (0..500).map(|n| format!("line-{n}")).collect();
    i.notifications = (0..50).map(|n| NoticeLine { severity: "Info", source: "App".into(), message: format!("note-{n}") }).collect();
    let text = build(&i);
    assert!(text.contains("line-499") && text.contains("line-400") && !text.contains("line-399\n"), "{text}");
    assert!(text.contains("note-0\n") && text.contains("note-19\n") && !text.contains("note-20\n"), "{text}");
}

#[test]
fn only_changed_switches_choices_and_numbers_are_listed() {
    let mut s = crate::settings::Settings::default();
    assert!(changed_settings(&s).is_empty());
    s.editor_font_size += 3.0;
    s.editor_font_family = Some("/Users/dogan/fonts/x".into());
    let changed = changed_settings(&s);
    assert!(changed.iter().any(|(k, _)| k == "editor.fontSize"), "{changed:?}");
    assert!(!changed.iter().any(|(k, _)| k == "editor.fontFamily"), "free text is never listed: {changed:?}");
}
