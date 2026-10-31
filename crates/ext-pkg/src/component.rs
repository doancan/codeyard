//! The import check (docs/plugin-architecture.md §6.2): a code extension's component may import only what its
//! manifest's permissions allow. `cyx`, the registry and the installer all call `check_component`, so they agree.
//! This is a coarse first fence; the broker still checks every call's path, host or program.

use ext_api::{Import, Need, Version};
use ext_manifest::{Code, Diagnostic, Manifest};
use wasmparser::{Encoding, Parser, Payload, Validator, WasmFeatures};

/// The compiled component inside a code extension's package.
pub const COMPONENT: &str = "extension.wasm";

/// What a component that passed the check uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    /// The `codeyard:api` interfaces it imports, sorted.
    pub interfaces: Vec<String>,
    /// The oldest API version that has every item (interface, function, type) it imports: the minimum Codeyard the
    /// extension needs is `ext_api::codeyard_for(min_api)`.
    pub min_api: Version,
    /// The API version it was built against (the newest version its imports name). A 0.x version here is one of
    /// `ext_api::TRANSITION`: it runs, but `cyx` warns and `cyx publish` refuses it.
    pub built_for: Version,
}

impl Component {
    /// Built against a 0.x API that this Codeyard still runs during the move to 1.0.
    pub fn in_transition(&self) -> bool {
        self.built_for.in_transition() && !self.built_for.runs_on(ext_api::VERSION)
    }
}

/// The API a component is checked against: this build's, or another in tests.
pub(crate) struct Host<'a> {
    pub version: Version,
    pub items: &'a ext_api::since::Items,
}

/// Check `wasm` against `manifest`. Every problem is reported, each with its fix.
pub fn check_component(wasm: &[u8], manifest: &Manifest) -> Result<Component, Vec<Diagnostic>> {
    check_against(wasm, manifest, &Host { version: ext_api::VERSION, items: ext_api::since::items() })
}

pub(crate) fn check_against(wasm: &[u8], manifest: &Manifest, host: &Host) -> Result<Component, Vec<Diagnostic>> {
    let bad = |m: String, fix: &str| vec![Diagnostic::new(Code::BadComponent, m).fix(fix)];
    let features = WasmFeatures::default() | WasmFeatures::COMPONENT_MODEL;
    let types = match Validator::new_with_features(features).validate_all(wasm) {
        Ok(t) => t,
        Err(e) => return Err(bad(format!("{COMPONENT} is not valid WebAssembly: {}", e.message()), "rebuild it with `cyx package`")),
    };
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut depth = 0usize;
    for payload in Parser::new(0).parse_all(wasm) {
        let Ok(payload) = payload else { return Err(bad(format!("{COMPONENT} cannot be read"), "rebuild it with `cyx package`")) };
        match payload {
            Payload::Version { encoding, .. } => {
                if depth == 0 && encoding != Encoding::Component {
                    return Err(bad(
                        format!("{COMPONENT} is a core WebAssembly module, not a component"),
                        "build for the wasm32-wasip2 target, or with `cyx package`",
                    ));
                }
                depth += 1;
            }
            Payload::End(_) => depth = depth.saturating_sub(1),
            // Only the outer component's imports reach the host; nested ones are satisfied inside it.
            Payload::ComponentImportSection(r) if depth == 1 => {
                for i in r.into_iter().flatten() {
                    imports.push(i.name.full_name().into_owned());
                }
            }
            Payload::ComponentExportSection(r) if depth == 1 => {
                for e in r.into_iter().flatten() {
                    exports.push(e.name.full_name().into_owned());
                }
            }
            _ => {}
        }
    }

    let mut problems = Vec::new();
    let mut interfaces = Vec::new();
    let mut min_api = Version(1, 0, 0);
    let mut built_for: Option<Version> = None;
    let perms = &manifest.permissions;
    let newer = |m: String| Diagnostic::new(Code::NeedsNewerCodeyard, m).fix("update Codeyard");
    for name in &imports {
        match ext_api::classify(name) {
            Import::Api { interface, version } => {
                let Some(version) = version else {
                    problems.push(
                        Diagnostic::new(Code::BadComponent, format!("{COMPONENT} imports '{name}' without a version")).fix("build it with the Codeyard SDK"),
                    );
                    continue;
                };
                let transition = version.in_transition() && !version.same_line(host.version);
                if !transition && !version.same_line(host.version) {
                    if version > host.version {
                        problems.push(newer(format!("the extension uses API {version}; this Codeyard has {}", host.version)));
                    } else {
                        problems.push(
                            Diagnostic::new(
                                Code::BadComponent,
                                format!("the extension was built against API {version}, which this Codeyard ({}) no longer runs", host.version),
                            )
                            .fix("rebuild it with the current SDK; docs/extensions/migrating-to-1.0.md says what changed"),
                        );
                    }
                    continue;
                }
                built_for = built_for.max(Some(version));
                let (Some(iface), Some(since)) = (ext_api::interface(interface), host.items.interface(interface)) else {
                    problems.push(if version > host.version {
                        newer(format!("the extension uses `{interface}` (API {version}); this Codeyard has API {}", host.version))
                    } else {
                        Diagnostic::new(Code::BadComponent, format!("{COMPONENT} imports '{name}', which is not part of the API"))
                            .fix("build it with the Codeyard SDK")
                    });
                    continue;
                };
                // Each function and type the component imports from the interface: one this Codeyard does not have
                // means a newer one; the newest `@since` among them is the version the extension needs.
                let mut item_since = since;
                for item in imported_items(&types, name) {
                    match host.items.item(interface, &item) {
                        Some(v) => item_since = item_since.max(v),
                        None => {
                            problems.push(newer(format!("the extension uses `{interface}.{item}`, which this Codeyard (API {}) does not have", host.version)))
                        }
                    }
                }
                if !iface.need.granted(perms) {
                    problems.push(not_declared(iface.name, iface.need));
                }
                min_api = min_api.max(item_since);
                interfaces.push(iface.name.to_string());
            }
            Import::Wasi(_) => {}
            Import::ForbiddenWasi(n) => problems.push(
                Diagnostic::new(Code::BadComponent, format!("{COMPONENT} imports '{n}'; extensions have no sockets of their own"))
                    .fix(r#"use `fetch` from the SDK and declare the hosts: net = ["<host>"]"#),
            ),
            Import::Unknown(n) => problems.push(
                Diagnostic::new(Code::BadComponent, format!("{COMPONENT} imports '{n}', which Codeyard does not provide"))
                    .fix("remove the dependency that needs it"),
            ),
        }
    }
    let guest = format!("{}/guest@", ext_api::PACKAGE);
    if !exports.iter().any(|e| e.starts_with(&guest)) {
        problems
            .push(Diagnostic::new(Code::BadComponent, format!("{COMPONENT} does not export {}/guest", ext_api::PACKAGE)).fix("build it with the Codeyard SDK"));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    interfaces.sort();
    interfaces.dedup();
    let built_for = built_for.unwrap_or(min_api);
    Ok(Component { interfaces, min_api, built_for })
}

/// The names of the functions and types a component's import `name` (an instance) asks for.
fn imported_items(types: &wasmparser::types::Types, name: &str) -> Vec<String> {
    use wasmparser::component_types::ComponentEntityType;
    match types.component_item_for_import(name).map(|i| &i.ty) {
        Some(ComponentEntityType::Instance(id)) => types[*id].exports.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn not_declared(interface: &str, need: Need) -> Diagnostic {
    Diagnostic::new(Code::NotDeclared, format!("the code uses `{interface}`, which needs the {} permission, but the manifest does not declare it", need.name()))
        .fix(format!("add to [permissions]:  {}", need.manifest_line()))
}
