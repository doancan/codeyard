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
    /// The oldest API version that has all of them: the minimum Codeyard the extension needs.
    pub min_api: Version,
}

/// Check `wasm` against `manifest`. Every problem is reported, each with its fix.
pub fn check_component(wasm: &[u8], manifest: &Manifest) -> Result<Component, Vec<Diagnostic>> {
    let bad = |m: String, fix: &str| vec![Diagnostic::new(Code::BadComponent, m).fix(fix)];
    let features = WasmFeatures::default() | WasmFeatures::COMPONENT_MODEL;
    if let Err(e) = Validator::new_with_features(features).validate_all(wasm) {
        return Err(bad(format!("{COMPONENT} is not valid WebAssembly: {}", e.message()), "rebuild it with `cyx package`"));
    }
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
    let mut min_api = Version(0, 1, 0);
    let perms = &manifest.permissions;
    for name in &imports {
        match ext_api::classify(name) {
            Import::Api { interface, version } => {
                let Some(version) = version else {
                    problems.push(
                        Diagnostic::new(Code::BadComponent, format!("{COMPONENT} imports '{name}' without a version")).fix("build it with the Codeyard SDK"),
                    );
                    continue;
                };
                if version > ext_api::VERSION {
                    problems.push(
                        Diagnostic::new(Code::NeedsNewerCodeyard, format!("the extension uses API {version}; this Codeyard has {}", ext_api::VERSION))
                            .fix("update Codeyard"),
                    );
                    continue;
                }
                if !version.runs_on(ext_api::VERSION) {
                    problems.push(
                        Diagnostic::new(
                            Code::BadComponent,
                            format!("the extension was built against API {version}, which this Codeyard ({}) no longer runs", ext_api::VERSION),
                        )
                        .fix("rebuild it with the current SDK; the release notes say what changed"),
                    );
                    continue;
                }
                let Some(iface) = ext_api::interface(interface) else {
                    problems.push(
                        Diagnostic::new(Code::BadComponent, format!("{COMPONENT} imports '{name}', which is not part of the API"))
                            .fix("build it with the Codeyard SDK"),
                    );
                    continue;
                };
                if !iface.need.granted(perms) {
                    problems.push(not_declared(iface.name, iface.need));
                }
                min_api = min_api.max(iface.since);
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
    Ok(Component { interfaces, min_api })
}

fn not_declared(interface: &str, need: Need) -> Diagnostic {
    Diagnostic::new(Code::NotDeclared, format!("the code uses `{interface}`, which needs the {} permission, but the manifest does not declare it", need.name()))
        .fix(format!("add to [permissions]:  {}", need.manifest_line()))
}
