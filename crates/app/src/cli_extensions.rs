//! Command-line actions that run without a window: `--install-extension`, `--update-extensions` and the update hand-over file.

use crate::{ext, ext_registry, ext_updates, i18n, settings, update_restart, workbench};

/// `--install-extension`: install each package into the user's store with the checks of the Extensions view, and report
/// one line per package. A package that asks for permissions prints its consent sheet and installs only with
/// `--accept-permissions`. Exit status 1 when any failed; the others are still installed.
pub(crate) fn install_extensions(files: &[String], accept_permissions: bool) -> i32 {
    let Some(cfg) = settings::config_dir() else {
        eprintln!("codeyard: no settings folder");
        return 1;
    };
    let store = ext::store(&cfg);
    let settings =
        std::fs::read_to_string(cfg.join("settings.jsonc")).ok().and_then(|t| settings::parse_layers(&t, &[]).ok()).map(|(s, _)| s).unwrap_or_default();
    let registries = ext_registry::registries(&settings);
    let mut status = 0;
    for f in files {
        let path = std::fs::canonicalize(f).unwrap_or_else(|_| f.into());
        // A file (or anything that looks like one) is a package; `publisher.name[@version]` comes from a registry.
        let from_file = path.exists() || f.ends_with(".cyx") || f.contains(['/', '\\']);
        let result = if from_file {
            ext::install_file(&store, &path, accept_permissions)
        } else {
            ext_registry::install_from_command_line(&cfg, &registries, f, accept_permissions)
        };
        match result {
            Ok(m) => {
                // What was granted, as the sheet put it.
                let source = store.locked().ok().and_then(|l| l.get(&m.id).map(|l| l.source.clone()));
                if let Some(sheet) = source.and_then(|s| ext::permission_sheet(&m, &s)) {
                    println!("{sheet}");
                }
                println!("installed {} {}", m.id, m.version)
            }
            Err(e) => {
                eprintln!("{f}: {e}");
                status = 1;
            }
        }
    }
    status
}

/// `--update-extensions`: what the daily update check does for extensions installed from a registry (update, roll
/// back a yanked version, stop a revoked one), asking those registries now, without a window. One line per change;
/// exit status 1 when a registry could not be read.
pub(crate) fn update_extensions() -> i32 {
    let Some(cfg) = settings::config_dir() else {
        eprintln!("codeyard: no settings folder");
        return 1;
    };
    let settings =
        std::fs::read_to_string(cfg.join("settings.jsonc")).ok().and_then(|t| settings::parse_layers(&t, &[]).ok()).map(|(s, _)| s).unwrap_or_default();
    let registries = ext_registry::registries(&settings);
    let report = ext_updates::check_now(&cfg, &registries, settings.extension_auto_update, ext_registry::now());
    for line in report.done.iter().chain(&report.stopped) {
        println!("{line}");
    }
    for (id, s) in &report.status {
        if let (ext_core::update::Standing::Revoked { reason }, None) = (&s.standing, &s.offer) {
            println!("{}", i18n::tf("{0} stays stopped: the registry revoked the installed version ({1}) and has no other version to install", &[id, reason]));
        }
        let Some(offer) = &s.offer else { continue };
        let v = offer.version();
        match offer {
            ext_updates::Offer::Waiting { .. } => println!("{id} {v} runs code: it installs by itself 48 h after it was published"),
            ext_updates::Offer::Available { .. } => println!("{id} {v} is available"),
            ext_updates::Offer::Failed { error, .. } => println!("{id} {v} could not be installed: {error}"),
            _ => println!("{id} {v} needs your review in the Extensions view"),
        }
    }
    for e in &report.errors {
        eprintln!("{e}");
    }
    i32::from(!report.errors.is_empty())
}

/// The windows an update restart left for this process; the file is consumed here. Anything wrong with it is logged and means
/// an ordinary start.
pub(crate) fn update_handover() -> Option<update_restart::Handover> {
    let dir = workbench::state_path()?.parent()?.to_path_buf();
    match update_restart::take(&dir, env!("CARGO_PKG_VERSION"), std::time::SystemTime::now()) {
        Ok(h) => h,
        Err(e) => {
            crate::diagnostics::log::warn("update", format_args!("not reopening the windows from before the update: {e}"));
            None
        }
    }
}
