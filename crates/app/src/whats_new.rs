//! "What's new": the notes of the running version, read from the CHANGELOG that was compiled in, so the first start after an
//! update can show them offline. The headings are the ones the release notes use (Features, Improvements, Fixes, Security).

/// The CHANGELOG of this build.
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// The notes of `version` in `changelog`: its section without the heading, with Keep a Changelog's `Added`, `Changed` and
/// `Fixed` named as the release page names them. `None` when the version has no section or the section is empty.
pub(crate) fn notes_in(changelog: &str, version: &str) -> Option<String> {
    let start = format!("## [{version}]");
    let mut notes = String::new();
    let mut on = false;
    for line in changelog.lines() {
        if line.starts_with(&start) {
            on = true;
            continue;
        }
        // The next release, or the link list at the end of the file, ends the section.
        if on && (line.starts_with("## [") || line.starts_with('[') && line.contains("]: ")) {
            break;
        }
        if on {
            notes.push_str(match line {
                "### Added" => "### Features",
                "### Changed" => "### Improvements",
                "### Fixed" => "### Fixes",
                other => other,
            });
            notes.push('\n');
        }
    }
    let notes = notes.trim();
    (!notes.is_empty()).then(|| notes.to_string())
}

/// The notes of `version` in this build's CHANGELOG.
pub(crate) fn notes_for(version: &str) -> Option<String> {
    notes_in(CHANGELOG, version)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\n\n## [Unreleased]\n\n## [1.2.0] - 2026-01-02\n\n### Added\n\n- A.\n\n### Changed\n\n- B.\n\n### Fixed\n\n- C.\n\n### Security\n\n- D.\n\n## [1.1.0] - 2025-12-01\n\n### Added\n\n- Old.\n\n[1.2.0]: https://example.test\n";

    #[test]
    fn a_version_gets_its_own_section_under_the_release_page_headings() {
        let notes = notes_in(LOG, "1.2.0").unwrap();
        assert!(notes.starts_with("### Features\n\n- A."), "{notes}");
        for heading in ["### Improvements", "### Fixes", "### Security"] {
            assert!(notes.contains(heading), "{heading} in {notes}");
        }
        assert!(!notes.contains("Old.") && !notes.contains("Added") && !notes.contains("example.test"), "{notes}");
    }

    #[test]
    fn a_version_without_notes_has_none() {
        assert_eq!(notes_in(LOG, "9.9.9"), None);
        assert_eq!(notes_in(LOG, "1.1.0").as_deref(), Some("### Features\n\n- Old."));
        assert_eq!(notes_in("## [1.0.0] - x\n\n## [0.9.0] - y\n", "1.0.0"), None);
    }
}
