//! The `@since` of every item of the package: which API version added each interface, function and type. Read from
//! the WIT text itself (a test checks it against `wit-parser`), so `ext-pkg` can tell which version a component needs
//! from the items it imports without linking a WIT parser.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::Version;

/// Items by interface, then by name (`""` for the interface itself).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Items {
    map: BTreeMap<String, BTreeMap<String, Version>>,
    /// Types an interface brings in with `use`, by interface and local name: where each is declared. A component's
    /// import of the interface names them too.
    uses: BTreeMap<String, BTreeMap<String, (String, String)>>,
}

impl Items {
    /// Read the `@since` gates of WIT files. An item without one is left out: `wit-parser` and the tests reject such a
    /// package anyway.
    pub fn parse<'a>(files: impl IntoIterator<Item = &'a str>) -> Items {
        let mut items = Items::default();
        for text in files {
            let mut interface: Option<String> = None;
            let mut since: Option<Version> = None;
            for line in text.lines() {
                let trimmed = line.trim();
                if let Some(v) = trimmed.strip_prefix("@since(version = ").and_then(|r| r.strip_suffix(')')) {
                    since = Version::parse(v.trim());
                    continue;
                }
                if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('@') {
                    continue;
                }
                let top = !line.starts_with(' ');
                if top {
                    interface = None;
                    if let Some(rest) = trimmed.strip_prefix("interface ") {
                        let name = rest.trim_end_matches('{').trim().to_string();
                        if let Some(v) = since {
                            items.map.entry(name.clone()).or_default().insert(String::new(), v);
                        }
                        interface = Some(name);
                    }
                } else if let (Some(i), Some(rest)) = (&interface, trimmed.strip_prefix("use ")) {
                    // `use types.{ticket, uri as path};`
                    if let Some((from, names)) = rest.trim_end_matches(';').split_once(".{") {
                        for n in names.trim_end_matches('}').split(',').map(str::trim).filter(|n| !n.is_empty()) {
                            let (orig, local) = n.split_once(" as ").map_or((n, n), |(o, l)| (o.trim(), l.trim()));
                            let entry = (from.trim().trim_start_matches('%').to_string(), orig.trim_start_matches('%').to_string());
                            items.uses.entry(i.clone()).or_default().insert(local.trim_start_matches('%').to_string(), entry);
                        }
                    }
                } else if let (Some(i), Some(v), true, Some(name)) = (&interface, since, line.starts_with("  ") && !line.starts_with("   "), declared(trimmed))
                {
                    items.map.entry(i.clone()).or_default().insert(name.to_string(), v);
                }
                since = None;
            }
        }
        items
    }

    /// When `interface` was added; none when it is not part of the API.
    pub fn interface(&self, interface: &str) -> Option<Version> {
        self.item(interface, "")
    }

    /// When function or type `name` of `interface` was added; none when it is not part of the API.
    /// A type brought in with `use` counts as the type it names.
    pub fn item(&self, interface: &str, name: &str) -> Option<Version> {
        if let Some(v) = self.map.get(interface).and_then(|m| m.get(name)) {
            return Some(*v);
        }
        let (from, orig) = self.uses.get(interface)?.get(name)?;
        self.item(from, orig)
    }

    /// Every item declared in the WIT (not those brought in with `use`), as `(interface, name, since)`; the interface
    /// itself has an empty name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, Version)> {
        self.map.iter().flat_map(|(i, m)| m.iter().map(move |(n, v)| (i.as_str(), n.as_str(), *v)))
    }
}

/// The name a declaration line inside an interface declares: `fetch: func(…)`, `record http-request {`,
/// `type uri = string;`. `use` lines declare nothing of their own.
fn declared(line: &str) -> Option<&str> {
    for kw in ["record ", "enum ", "variant ", "flags ", "resource ", "type "] {
        if let Some(rest) = line.strip_prefix(kw) {
            let end = rest.find([' ', '{', '=', ';']).unwrap_or(rest.len());
            return Some(rest[..end].trim_start_matches('%'));
        }
    }
    let (name, rest) = line.split_once(':')?;
    rest.trim_start().starts_with("func").then(|| name.trim().trim_start_matches('%'))
}

/// The items of this build's API.
pub fn items() -> &'static Items {
    static ITEMS: OnceLock<Items> = OnceLock::new();
    ITEMS.get_or_init(|| Items::parse(crate::WIT.iter().map(|(_, text)| *text)))
}
