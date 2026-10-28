//! User-selected Web Preview options shared by the settings page and server.
use std::net::{IpAddr, Ipv4Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub host: IpAddr,
    pub port: u16,
    pub live_reload: bool,
    pub reload_interval: u64,
    pub root: String,
}

impl Default for Config {
    fn default() -> Self {
        Self { host: Ipv4Addr::LOCALHOST.into(), port: 0, live_reload: true, reload_interval: 1000, root: String::new() }
    }
}

impl Config {
    /// Resolve a root on the active host; relative roots are based on the selected workspace.
    pub fn resolve_root(&self, base: Option<&str>, home: &str, os: &str) -> Option<String> {
        let root = self.root.trim();
        if root.is_empty() {
            return base.map(str::to_owned);
        }
        let root = if os == "windows" { root.replace('\\', "/") } else { root.to_owned() };
        let root = if root == "~" {
            home.to_owned()
        } else if let Some(relative) = root.strip_prefix("~/") {
            crate::ui::join(home, relative)
        } else if crate::ui::is_absolute_path(&root) {
            root
        } else {
            crate::ui::join(base?, &root)
        };
        let root = if os == "windows" { root.replace('\\', "/") } else { root };
        Some(crate::workspace_file::normalize(&root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::parse_layers_trusted;

    #[test]
    fn web_preview_settings_validate_and_only_use_the_user_layer() {
        let user =
            r#"{"webPreview.port":8080,"webPreview.host":"::1","webPreview.liveReload":false,"webPreview.reloadInterval":500,"webPreview.root":"public"}"#;
        let (settings, warnings) = parse_layers_trusted(user, &[], true).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            settings.web_preview,
            Config { port: 8080, host: std::net::Ipv6Addr::LOCALHOST.into(), live_reload: false, reload_interval: 500, root: "public".into() }
        );
        for trusted in [false, true] {
            let (settings, _) = parse_layers_trusted("{}", &[("workspace", user, false)], trusted).unwrap();
            assert_eq!(settings.web_preview, Config::default());
        }
        for (key, value) in [
            ("port", "-1"),
            ("port", "65536"),
            ("port", "1.5"),
            ("port", "\"8080\""),
            ("host", "\"attacker.example\""),
            ("reloadInterval", "249"),
            ("reloadInterval", "60001"),
            ("reloadInterval", "250.5"),
            ("liveReload", "\"yes\""),
            ("root", "\"bad\\npath\""),
        ] {
            let input = format!("{{\"webPreview.{key}\":{value}}}");
            let (settings, warnings) = parse_layers_trusted(&input, &[], true).unwrap();
            assert_eq!(settings.web_preview, Config::default(), "{input}");
            assert!(!warnings.is_empty(), "{input}");
        }
    }

    #[test]
    fn web_preview_root_resolves_on_the_active_host() {
        let base = Some("/workspace");
        let home = "/home/user";
        for (root, expected) in [
            ("", "/workspace"),
            ("public", "/workspace/public"),
            ("./public/../dist", "/workspace/dist"),
            ("/srv/site", "/srv/site"),
            ("~/site", "/home/user/site"),
        ] {
            let config = Config { root: root.into(), ..Config::default() };
            assert_eq!(config.resolve_root(base, home, "linux").as_deref(), Some(expected), "{root}");
        }
        let config = Config { root: "public".into(), ..Config::default() };
        assert_eq!(config.resolve_root(None, home, "linux"), None);
        let config = Config { root: "C:\\site\\public".into(), ..Config::default() };
        assert_eq!(config.resolve_root(None, "C:/Users/user", "windows").as_deref(), Some("C:/site/public"));
        let config = Config { root: "~/site".into(), ..Config::default() };
        assert_eq!(config.resolve_root(None, "C:\\Users\\user", "windows").as_deref(), Some("C:/Users/user/site"));
    }

    #[test]
    fn web_preview_setting_controls_reject_fractional_values() {
        use crate::workbench::settings_ui::{entry, parse_setting_input};
        for key in ["webPreview.port", "webPreview.reloadInterval"] {
            assert!(parse_setting_input(key, entry(key).unwrap().kind, "500.5").is_err());
            assert_eq!(parse_setting_input(key, entry(key).unwrap().kind, "500").unwrap(), serde_json::json!(500));
        }
    }
}
