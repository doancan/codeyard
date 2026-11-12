use std::path::{Path, PathBuf};

use super::*;

fn config(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-profiles-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn ordinary_names_are_accepted_and_trimmed() {
    for name in ["Work", "  Work  ", "my-profile_2", "a b", "v1.2", "Çalışma", "İş", "ığüşöç", "ÇAĞRI", "日本語", "Ünal 2", &"x".repeat(MAX_NAME_CHARS)]
    {
        assert_eq!(validate_name(name).as_deref(), Ok(name.trim()), "{name:?}");
    }
}

#[test]
fn names_that_leave_the_profiles_folder_are_rejected() {
    for name in ["../x", "..", ".", "a/b", "a\\b", "/etc", "C:\\x", "x/../../y", "..\\..\\x", "%2e%2e", "~"] {
        assert!(validate_name(name).is_err(), "{name:?}");
    }
    assert!(matches!(validate_name("a/b"), Err(NameError::Character('/'))));
    assert!(matches!(validate_name("a\\b"), Err(NameError::Character('\\'))));
    assert_eq!(validate_name(".."), Err(NameError::LeadingDot));
    assert_eq!(validate_name("."), Err(NameError::LeadingDot));
    assert_eq!(validate_name("../x"), Err(NameError::Character('/')));
}

#[test]
fn characters_windows_and_macos_cannot_use_are_rejected() {
    for c in ['*', '?', ':', '"', '<', '>', '|', '\0', '\t', '\n', '\u{200b}', '\u{202e}', '😀', '/', '\\'] {
        assert_eq!(validate_name(&format!("a{c}b")), Err(NameError::Character(c)), "{c:?}");
    }
}

#[test]
fn windows_device_names_are_reserved_with_or_without_an_extension() {
    for name in ["con", "CON", "Con", "nul", "NUL", "prn", "aux", "com1", "COM9", "lpt1", "LPT9", "nul.txt", "Com1.x", "aux.tar.gz", "CON ."] {
        assert!(matches!(validate_name(name), Err(NameError::Reserved(_) | NameError::TrailingDot)), "{name:?} -> {:?}", validate_name(name));
    }
    // Names that only start like a device name are fine.
    for name in ["console", "nulled", "com10", "lpt", "communication", "auxiliary", "con-1"] {
        assert!(validate_name(name).is_ok(), "{name}");
    }
}

#[test]
fn empty_long_dotted_and_decomposed_names_are_rejected() {
    assert_eq!(validate_name(""), Err(NameError::Empty));
    assert_eq!(validate_name("   \t "), Err(NameError::Empty));
    assert_eq!(validate_name(&"x".repeat(MAX_NAME_CHARS + 1)), Err(NameError::TooLong));
    assert_eq!(validate_name(&"ç".repeat(MAX_NAME_CHARS + 1)), Err(NameError::TooLong), "characters are counted, not bytes");
    assert!(validate_name(&"ç".repeat(MAX_NAME_CHARS)).is_ok());
    assert_eq!(validate_name(".hidden"), Err(NameError::LeadingDot));
    assert_eq!(validate_name("work."), Err(NameError::TrailingDot));
    assert_eq!(validate_name("work.."), Err(NameError::TrailingDot));
    assert_eq!(validate_name("e\u{301}"), Err(NameError::CombiningMark), "a decomposed é is the same folder as a precomposed one on macOS");
    assert!(validate_name("\u{e9}").is_ok());
}

#[test]
fn superscript_digits_and_other_spellings_of_one_folder_are_rejected() {
    for name in ["COM¹", "lpt²", "LPT³", "a²", "٣", "\u{212a}elvin", "\u{212b}ngstr\u{f6}m", "\u{1112}\u{1161}\u{11ab}", "\u{f900}"] {
        assert!(matches!(validate_name(name), Err(NameError::Character(_) | NameError::Equivalent(_))), "{name:?} -> {:?}", validate_name(name));
    }
    // Precomposed Turkish letters and Hangul syllables are the usual spelling and stay allowed; the decomposed forms are not.
    for name in ["çşğöüıİ", "\u{d55c}\u{ae00}"] {
        assert!(validate_name(name).is_ok(), "{name}");
    }
    for name in ["c\u{327}", "s\u{327}", "g\u{306}", "o\u{308}", "u\u{308}"] {
        assert_eq!(validate_name(name), Err(NameError::CombiningMark), "{name:?}");
    }
    assert!(NameError::Equivalent('K').message().contains('K'));
}

#[test]
fn default_is_taken_in_every_case_and_every_error_has_a_message() {
    for name in ["Default", "default", "DEFAULT", " default "] {
        assert_eq!(validate_name(name), Err(NameError::Default), "{name:?}");
    }
    let errors = [
        NameError::Empty,
        NameError::TooLong,
        NameError::Character('/'),
        NameError::CombiningMark,
        NameError::LeadingDot,
        NameError::TrailingDot,
        NameError::Reserved("nul".into()),
        NameError::Default,
        NameError::Equivalent('K'),
        NameError::Exists("Work".into()),
    ];
    let messages: Vec<String> = errors.iter().map(NameError::message).collect();
    assert!(messages.iter().all(|m| !m.is_empty() && !m.contains("{}")), "{messages:?}");
    assert!(messages[2].contains('/') && messages[6].contains("nul") && messages[9].contains("Work"), "{messages:?}");
    // A control character is shown escaped, not typed into the message.
    assert!(NameError::Character('\n').message().contains("\\n"));
}

#[test]
fn profiles_that_differ_only_in_case_collide_also_in_turkish() {
    let cfg = config("case");
    create(&cfg, "Work").unwrap();
    for other in ["work", "WORK", "wORK"] {
        assert_eq!(check_new_name(&cfg, other, None), Err(NameError::Exists("Work".into())), "{other}");
        assert!(matches!(create(&cfg, other), Err(ProfileError::Name(NameError::Exists(_)))), "{other}");
    }
    create(&cfg, "İş").unwrap();
    for other in ["iş", "İŞ", "i̇ş"] {
        assert!(matches!(check_new_name(&cfg, other, None), Err(NameError::Exists(_))) || validate_name(other).is_err(), "{other}");
    }
    assert!(check_new_name(&cfg, "ıs", None).is_ok(), "a dotless ı is another letter");
    assert_eq!(list(&cfg), ["İş", "Work"]);
}

#[test]
fn nothing_exists_until_a_profile_is_made() {
    let cfg = config("nothing");
    assert_eq!(list(&cfg), Vec::<String>::new());
    assert_eq!(read_active(&cfg), Active::default());
    assert_eq!(files_dir(&cfg, None), cfg);
    assert!(std::fs::read_dir(&cfg).unwrap().next().is_none(), "reading creates nothing");
}

#[test]
fn a_new_profile_has_the_default_files() {
    let cfg = config("create");
    assert_eq!(create(&cfg, "  Work ").unwrap(), "Work");
    let dir = profile_dir(&cfg, "Work");
    assert_eq!(read(&dir.join("settings.jsonc")), crate::settings::TEMPLATE);
    assert_eq!(read(&dir.join("keybindings.jsonc")), crate::settings::KEYBINDINGS_TEMPLATE);
    assert!(!cfg.join("settings.jsonc").exists(), "the default profile is untouched");
    assert!(matches!(create(&cfg, "Work"), Err(ProfileError::Name(NameError::Exists(_)))));
    assert!(matches!(create(&cfg, "../x"), Err(ProfileError::Name(_))));
    assert!(matches!(create(&cfg, "nul"), Err(ProfileError::Name(NameError::Reserved(_)))));
    assert_eq!(list(&cfg), ["Work"]);
    assert!(!cfg.join("x").exists() && !cfg.join(DIR).join("x").exists());
}

#[test]
fn a_duplicate_copies_settings_keybindings_and_snippets_and_not_the_other_profiles() {
    let cfg = config("duplicate");
    write(&cfg.join("settings.jsonc"), "{ \"theme\": \"nord\" } // mine\n");
    write(&cfg.join("keybindings.jsonc"), "[]\r\n");
    write(&cfg.join("snippets/rust.json"), "{}");
    write(&cfg.join("themes/x.json"), "{}");
    create(&cfg, "Other").unwrap();
    assert_eq!(duplicate(&cfg, None, "Copy").unwrap(), "Copy");
    let dir = profile_dir(&cfg, "Copy");
    assert_eq!(read(&dir.join("settings.jsonc")), "{ \"theme\": \"nord\" } // mine\n");
    assert_eq!(read(&dir.join("keybindings.jsonc")), "[]\r\n", "bytes are copied as they are");
    assert_eq!(read(&dir.join("snippets/rust.json")), "{}");
    assert!(!dir.join("profiles").exists() && !dir.join("themes").exists() && !dir.join(DIR).exists());

    write(&profile_dir(&cfg, "Other").join("snippets/go.json"), "{}");
    duplicate(&cfg, Some("Other"), "Copy 2").unwrap();
    assert!(profile_dir(&cfg, "Copy 2").join("snippets/go.json").is_file());
    assert!(!profile_dir(&cfg, "Copy 2").join("snippets/rust.json").exists());
    assert!(matches!(duplicate(&cfg, Some("Missing"), "X"), Err(ProfileError::Missing(_))));
    assert!(matches!(duplicate(&cfg, Some("../Other"), "X"), Err(ProfileError::Name(_))));
    assert!(matches!(duplicate(&cfg, None, "copy"), Err(ProfileError::Name(NameError::Exists(_)))));
}

#[test]
fn a_duplicate_of_a_config_without_files_is_an_empty_profile() {
    let cfg = config("duplicate-empty");
    duplicate(&cfg, None, "Blank").unwrap();
    assert_eq!(list(&cfg), ["Blank"]);
    assert!(!profile_dir(&cfg, "Blank").join("settings.jsonc").exists());
}

#[cfg(unix)]
#[test]
fn a_duplicate_does_not_follow_links_out_of_the_snippets_folder() {
    let cfg = config("duplicate-link");
    let outside = config("duplicate-link-outside");
    write(&outside.join("secret.txt"), "secret");
    write(&cfg.join("snippets/ok.json"), "{}");
    std::os::unix::fs::symlink(outside.join("secret.txt"), cfg.join("snippets/link.json")).unwrap();
    std::os::unix::fs::symlink(&outside, cfg.join("snippets/dir")).unwrap();
    duplicate(&cfg, None, "Safe").unwrap();
    let copied = profile_dir(&cfg, "Safe").join("snippets");
    assert!(copied.join("ok.json").is_file());
    assert!(!copied.join("link.json").exists() && !copied.join("dir").exists(), "links are skipped");
}

#[test]
fn a_duplicate_of_too_many_files_is_refused_and_leaves_nothing() {
    let cfg = config("duplicate-many");
    for i in 0..=MAX_COPIED_FILES {
        write(&cfg.join(format!("snippets/s{i}.json")), "{}");
    }
    let err = duplicate(&cfg, None, "Big").unwrap_err();
    assert!(err.message().contains("snippet"), "{}", err.message());
    assert_eq!(list(&cfg), Vec::<String>::new(), "a half-made profile is removed");
}

#[test]
fn renaming_moves_the_folder_and_checks_the_new_name() {
    let cfg = config("rename");
    create(&cfg, "Work").unwrap();
    write(&profile_dir(&cfg, "Work").join("snippets/a.json"), "{}");
    create(&cfg, "Home").unwrap();
    assert_eq!(rename(&cfg, "Work", "Ofis").unwrap(), "Ofis");
    assert!(profile_dir(&cfg, "Ofis").join("snippets/a.json").is_file() && !profile_dir(&cfg, "Work").exists());
    assert!(matches!(rename(&cfg, "Ofis", "home"), Err(ProfileError::Name(NameError::Exists(_)))), "case-insensitive collision");
    assert!(matches!(rename(&cfg, "Ofis", "con"), Err(ProfileError::Name(NameError::Reserved(_)))));
    assert!(matches!(rename(&cfg, "Ofis", "../Ofis"), Err(ProfileError::Name(_))));
    assert!(matches!(rename(&cfg, "Missing", "X"), Err(ProfileError::Missing(_))));
    assert!(matches!(rename(&cfg, "../Home", "X"), Err(ProfileError::Name(_))));
    assert_eq!(rename(&cfg, "Ofis", "Ofis").unwrap(), "Ofis", "the same name changes nothing");
    assert_eq!(list(&cfg), ["Home", "Ofis"]);
}

#[test]
fn a_profile_can_change_only_the_case_of_its_own_name() {
    let cfg = config("rename-case");
    create(&cfg, "work").unwrap();
    assert_eq!(rename(&cfg, "work", "Work").unwrap(), "Work");
    assert_eq!(list(&cfg), ["Work"]);
    assert!(profile_dir(&cfg, "Work").join("settings.jsonc").is_file());
}

#[test]
fn deleting_removes_the_folder_and_only_that_one() {
    let cfg = config("delete");
    create(&cfg, "Work").unwrap();
    create(&cfg, "Home").unwrap();
    write(&profile_dir(&cfg, "Work").join("snippets/deep/a.json"), "{}");
    write(&cfg.join("settings.jsonc"), "{}");
    delete(&cfg, "Work").unwrap();
    assert_eq!(list(&cfg), ["Home"]);
    assert!(cfg.join("settings.jsonc").is_file());
    assert!(matches!(delete(&cfg, "Work"), Err(ProfileError::Missing(_))));
    for bad in ["..", "../Home", "", "a/b", "Default"] {
        assert!(matches!(delete(&cfg, bad), Err(ProfileError::Name(_))), "{bad:?}");
    }
    assert_eq!(list(&cfg), ["Home"]);
}

#[cfg(unix)]
#[test]
fn deleting_a_profile_does_not_delete_what_its_links_point_to() {
    let cfg = config("delete-link");
    let outside = config("delete-link-outside");
    write(&outside.join("keep.txt"), "keep");
    create(&cfg, "Work").unwrap();
    std::os::unix::fs::symlink(&outside, profile_dir(&cfg, "Work").join("snippets")).unwrap();
    delete(&cfg, "Work").unwrap();
    assert!(outside.join("keep.txt").is_file());
    // The profile folder itself as a link: the link goes, the target stays.
    std::os::unix::fs::symlink(&outside, profile_dir(&cfg, "Linked")).unwrap();
    delete(&cfg, "Linked").unwrap();
    assert!(outside.join("keep.txt").is_file() && !profile_dir(&cfg, "Linked").exists());
}

#[test]
fn the_active_profile_is_read_from_its_file_and_a_wrong_one_falls_back() {
    let cfg = config("active");
    create(&cfg, "Work").unwrap();
    set_active(&cfg, Some("Work")).unwrap();
    assert_eq!(read_active(&cfg), Active { name: Some("Work".into()), ..Active::default() });
    assert_eq!(read(&cfg.join(POINTER)), "Work\n");
    // Another editor of the file may add spaces and blank lines.
    write(&cfg.join(POINTER), "  Work \r\n\r\n");
    assert_eq!(read_active(&cfg).name.as_deref(), Some("Work"));
    set_active(&cfg, None).unwrap();
    assert_eq!(read_active(&cfg), Active::default());
    assert!(!cfg.join(POINTER).exists());
    set_active(&cfg, None).unwrap();

    // A folder that is gone, a name that is not a profile name, a file in place of the folder, an empty or huge file.
    for (text, falls_back) in [("Gone", true), ("../Work", true), ("..", true), ("nul", true), ("a/b", true), ("", false), ("   \n", false)] {
        write(&cfg.join(POINTER), text);
        let active = read_active(&cfg);
        assert_eq!(active.name, None, "{text:?}");
        assert_eq!(active.fell_back.is_some(), falls_back, "{text:?}");
    }
    write(&cfg.join(POINTER), &"x".repeat(5000));
    assert!(read_active(&cfg).fell_back.is_some_and(|n| n.chars().count() <= MAX_NAME_CHARS));
    write(&cfg.join(POINTER), "Plain");
    write(&cfg.join(DIR).join("Plain"), "I am a file");
    assert!(read_active(&cfg).fell_back.is_some(), "a file is not a profile folder");
    std::fs::write(cfg.join(POINTER), [0xff, 0xfe, 0x00]).unwrap();
    assert_eq!(read_active(&cfg).name, None, "bytes that are not text do not name a profile");
}

#[test]
fn a_huge_pointer_file_is_not_read_whole() {
    let cfg = config("pointer-huge");
    write(&cfg.join(POINTER), &"x".repeat(5_000_000));
    let active = read_active(&cfg);
    assert_eq!((active.name, active.transient), (None, false));
    assert!(active.fell_back.is_some());
}

#[cfg(unix)]
#[test]
fn an_unreadable_profile_folder_is_a_temporary_fall_back_and_a_missing_one_is_not() {
    use std::os::unix::fs::PermissionsExt;
    let cfg = config("transient");
    create(&cfg, "Work").unwrap();
    set_active(&cfg, Some("Work")).unwrap();
    let dir = profile_dir(&cfg, "Work");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = std::fs::read_dir(&dir).is_ok(); // running as root
    let active = read_active(&cfg);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !readable_anyway {
        assert_eq!((active.name.clone(), active.fell_back.clone(), active.transient), (None, Some("Work".into()), true));
        assert!(cfg.join(POINTER).exists());
        assert_eq!(read_active(&cfg).name.as_deref(), Some("Work"), "it comes back when the folder can be read again");
    }
    std::fs::remove_dir_all(&dir).unwrap();
    let gone = read_active(&cfg);
    assert_eq!((gone.name, gone.transient), (None, false), "a folder that is gone is not temporary");
}

#[test]
fn files_of_a_profile_are_found_under_its_own_folder() {
    let cfg = config("files");
    assert_eq!(files_dir(&cfg, Some("Work")), cfg.join(DIR).join("Work"));
    assert_eq!(files_dir(&cfg, None), cfg);
}

#[test]
fn list_skips_files_hidden_folders_and_names_that_are_not_valid() {
    let cfg = config("list");
    create(&cfg, "b").unwrap();
    create(&cfg, "A").unwrap();
    std::fs::create_dir_all(cfg.join(DIR).join(".hidden")).unwrap();
    // Windows strips a trailing dot when a folder is made, so there the name is valid; elsewhere it is a folder to skip.
    #[cfg(unix)]
    std::fs::create_dir_all(cfg.join(DIR).join("trailing.")).unwrap();
    std::fs::create_dir_all(cfg.join(DIR).join("Default")).unwrap();
    write(&cfg.join(DIR).join("note.txt"), "x");
    assert_eq!(list(&cfg), ["A", "b"], "sorted without regard to case, only valid names");
}
