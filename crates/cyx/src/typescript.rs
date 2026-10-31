//! Building a TypeScript or JavaScript extension: esbuild bundles `[code] entry` with the SDK (`@codeyard/extension`,
//! shipped inside cyx so its glue always matches the API), then ComponentizeJS builds the WebAssembly component against
//! a world that imports only the interfaces the manifest grants. Node.js is the only thing the author installs: cyx
//! writes the pinned build tools (`sdk/typescript/toolchain`) into a cache folder and installs them there once with
//! `npm ci`.

use std::path::{Path, PathBuf};
use std::process::Command;

use ext_manifest::Manifest;
use sha2::{Digest, Sha256};

/// The bundle's source map, written next to `extension.wasm` and packaged with it: stack traces name the bundle
/// (`extension.js:12:5`), and `cyx test` and the Inspector map them back to the `.ts` files.
pub const SOURCE_MAP: &str = "extension.js.map";
/// The bundle's file name, as stack traces show it.
pub const BUNDLE: &str = "extension.js";

/// The build tools, pinned by the lock file. `build.mjs` runs esbuild and ComponentizeJS.
const TOOLCHAIN: [(&str, &str); 3] = [
    ("package.json", include_str!("../../../sdk/typescript/toolchain/package.json")),
    ("package-lock.json", include_str!("../../../sdk/typescript/toolchain/package-lock.json")),
    ("build.mjs", include_str!("../../../sdk/typescript/toolchain/build.mjs")),
];

/// `@codeyard/extension`'s sources, except the mock host (`testing/`), which only `npm test` uses.
pub const SDK: [(&str, &str); 14] = [
    ("context.ts", include_str!("../../../sdk/typescript/src/context.ts")),
    ("errors.ts", include_str!("../../../sdk/typescript/src/errors.ts")),
    ("fetch.ts", include_str!("../../../sdk/typescript/src/fetch.ts")),
    ("globals.ts", include_str!("../../../sdk/typescript/src/globals.ts")),
    ("guest.ts", include_str!("../../../sdk/typescript/src/guest.ts")),
    ("handlers.ts", include_str!("../../../sdk/typescript/src/handlers.ts")),
    ("index.ts", include_str!("../../../sdk/typescript/src/index.ts")),
    ("pulls.ts", include_str!("../../../sdk/typescript/src/pulls.ts")),
    ("runtime.ts", include_str!("../../../sdk/typescript/src/runtime.ts")),
    ("scm.ts", include_str!("../../../sdk/typescript/src/scm.ts")),
    ("types.ts", include_str!("../../../sdk/typescript/src/types.ts")),
    ("ui.ts", include_str!("../../../sdk/typescript/src/ui.ts")),
    ("view-schema.ts", include_str!("../../../sdk/typescript/src/view-schema.ts")),
    ("wit.ts", include_str!("../../../sdk/typescript/src/wit.ts")),
];

/// The Node.js major version the build tools and `npm test` (TypeScript without a compile step) need.
const NODE_MAJOR: u32 = 22;

/// Build the component of the TypeScript or JavaScript extension in `dir`; returns where `extension.wasm` was written.
pub fn build(dir: &Path, m: &Manifest, entry: &str) -> Result<PathBuf, String> {
    let dir = std::fs::canonicalize(dir).map_err(|e| format!("error: {}: {e}", dir.display()))?;
    let entry_path = dir.join(entry);
    if !entry_path.is_file() {
        return Err(format!(
            "error: code.entry '{entry}' does not exist\n  --> {}\n   = fix: point code.entry at the extension's main .ts or .js file",
            ext_manifest::FILE_NAME
        ));
    }
    check_node()?;
    let (tools, sdk) = toolchain()?;
    let work = cache_dir()?.join("work").join(short_hash(dir.to_string_lossy().as_bytes()));
    let wit = work.join("wit");
    std::fs::create_dir_all(&wit).map_err(|e| format!("error: cannot create {}: {e}", wit.display()))?;
    let interfaces = granted(m);
    for (name, text) in ext_api::WIT {
        let text = if name == "world.wit" { narrow_world(text, &interfaces) } else { text.to_string() };
        std::fs::write(wit.join(name), text).map_err(|e| format!("error: cannot write {}: {e}", wit.display()))?;
    }
    // Written beside and moved into place by `build::install`.
    let partial = dir.join(crate::build::PARTIAL);
    let job = serde_json::json!({
        "entry": entry_path,
        "dir": dir,
        "sdk": sdk,
        "wit": wit,
        "interfaces": interfaces,
        "api": ext_api::VERSION.to_string(),
        "bundle": work.join(BUNDLE),
        "out": partial,
        "map": dir.join(SOURCE_MAP),
    });
    let job_path = work.join("job.json");
    std::fs::write(&job_path, job.to_string()).map_err(|e| format!("error: cannot write {}: {e}", job_path.display()))?;

    println!("Building {} (TypeScript, with esbuild and ComponentizeJS)…", m.id);
    let output = Command::new("node")
        .arg(tools.join("build.mjs"))
        .arg(&job_path)
        // Where the bundle and the WIT are: ComponentizeJS gets them as relative paths.
        .current_dir(&work)
        .output()
        .map_err(|e| format!("error: cannot run node: {e}\n   = fix: install Node.js {NODE_MAJOR} or newer from https://nodejs.org"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let _ = std::fs::remove_file(&partial);
        return Err(if stderr.starts_with("error") { stderr } else { format!("error: the TypeScript build failed\n{stderr}") });
    }
    let wasm = std::fs::read(&partial).map_err(|e| format!("error: cannot read {}: {e}", partial.display()))?;
    crate::build::install(&dir, &wasm)
}

/// The interfaces the component may import: the free ones and those the manifest's permissions grant. ComponentizeJS
/// imports every interface of the world it builds against, so the world is narrowed to these; the SDK refuses a call
/// to any other with the manifest line that would allow it.
pub fn granted(m: &Manifest) -> Vec<&'static str> {
    // `types`, `host-requests`, `responses` and `pull-requests` hold only types; the world pulls them in through the others.
    let types_only = ["types", "host-requests", "responses", "pull-requests"];
    ext_api::INTERFACES.iter().filter(|i| !types_only.contains(&i.name) && i.need.granted(&m.permissions)).map(|i| i.name).collect()
}

/// `world.wit` with only `interfaces` imported.
fn narrow_world(text: &str, interfaces: &[&str]) -> String {
    text.lines()
        .filter(|line| {
            let t = line.trim();
            match t.strip_prefix("import ").and_then(|r| r.strip_suffix(';')) {
                Some(name) => interfaces.contains(&name),
                None => true,
            }
        })
        .map(|l| format!("{l}\n"))
        .collect()
}

fn check_node() -> Result<(), String> {
    let fix = format!("   = fix: install Node.js {NODE_MAJOR} or newer from https://nodejs.org; TypeScript extensions need nothing else");
    let out = Command::new("node").arg("--version").output().map_err(|_| format!("error: Node.js was not found\n{fix}"))?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let major = version.trim_start_matches('v').split('.').next().and_then(|m| m.parse::<u32>().ok()).unwrap_or(0);
    if major < NODE_MAJOR {
        return Err(format!("error: Node.js {version} is too old to build TypeScript extensions\n{fix}"));
    }
    Ok(())
}

/// The npm to run (`npm.cmd` on Windows, where npm is a script).
fn npm() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

/// Where cyx keeps the build tools: `CODEYARD_CYX_CACHE`, else the platform's cache folder.
pub fn cache_dir() -> Result<PathBuf, String> {
    if let Some(d) = std::env::var_os("CODEYARD_CYX_CACHE") {
        return Ok(PathBuf::from(d));
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home().map(|h| h.join(".cache")))
    };
    base.map(|b| b.join("codeyard").join("cyx"))
        .ok_or_else(|| "error: cyx found no cache folder for its build tools\n   = fix: set CODEYARD_CYX_CACHE to a folder cyx may write".to_string())
}

fn short_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn digest(files: &[(&str, &str)]) -> String {
    let mut all = Vec::new();
    for (name, text) in files {
        all.extend_from_slice(name.as_bytes());
        all.push(0);
        all.extend_from_slice(text.as_bytes());
        all.push(0);
    }
    short_hash(&all)
}

/// Write `files` into a new folder `dir` (through a temporary folder renamed into place, so two builds at once never
/// see half of it), after `prepare` ran in it. `ready` tells a finished folder.
fn materialize(dir: &Path, files: &[(&str, &str)], ready: &dyn Fn(&Path) -> bool, prepare: &dyn Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    if ready(dir) {
        return Ok(());
    }
    let parent = dir.parent().ok_or("error: no cache folder")?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = parent.join(format!(".{name}-{}-{nanos}", std::process::id()));
    for (rel, text) in files {
        let p = tmp.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("error: cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&p, text).map_err(|e| format!("error: cannot write {}: {e}", p.display()))?;
    }
    if let Err(e) = prepare(&tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if std::fs::rename(&tmp, dir).is_err() {
        // Another build finished first; use its copy.
        let _ = std::fs::remove_dir_all(&tmp);
        if !ready(dir) {
            return Err(format!("error: cannot move {} into place", dir.display()));
        }
    }
    Ok(())
}

/// The installed build tools, one folder per version of them (installed on first use, then reused), and the SDK's
/// sources, one folder per version of those inside it.
fn toolchain() -> Result<(PathBuf, PathBuf), String> {
    let tools = cache_dir()?.join(format!("typescript-{}", digest(&TOOLCHAIN)));
    std::fs::create_dir_all(tools.parent().ok_or("error: no cache folder")?).map_err(|e| format!("error: cannot create the cache folder: {e}"))?;
    let installed = |d: &Path| d.join("node_modules").join(".cyx-ready").is_file();
    let install = |tmp: &Path| {
        println!("Installing the TypeScript build tools into {} (once per version of them)…", tools.display());
        let output =
            Command::new(npm()).args(["ci", "--ignore-scripts", "--no-audit", "--no-fund", "--loglevel=error"]).current_dir(tmp).output().map_err(|e| {
                format!("error: cannot run npm: {e}\n   = fix: install Node.js {NODE_MAJOR} or newer (it comes with npm) from https://nodejs.org")
            })?;
        if !output.status.success() {
            return Err(format!(
                "error: npm could not install the TypeScript build tools\n{}\n   = fix: check the network connection and npm's registry setting, then build again",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        std::fs::write(tmp.join("node_modules").join(".cyx-ready"), "").map_err(|e| format!("error: {e}"))
    };
    materialize(&tools, &TOOLCHAIN, &installed, &install)?;
    let sdk = tools.join(format!("sdk-{}", digest(&SDK)));
    materialize(&sdk, &SDK, &|d: &Path| d.join("index.ts").is_file(), &|_: &Path| Ok(()))?;
    Ok((tools, sdk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_sdk_is_every_source_file() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/typescript/src");
        let mut on_disk: Vec<String> =
            std::fs::read_dir(&src).unwrap().flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        on_disk.sort();
        let shipped: Vec<&str> = SDK.iter().map(|(n, _)| *n).collect();
        assert_eq!(on_disk, shipped, "list every file of sdk/typescript/src in typescript::SDK");
    }

    #[test]
    fn the_world_imports_only_what_is_granted() {
        let world = ext_api::WIT.iter().find(|(n, _)| *n == "world.wit").unwrap().1;
        let narrowed = narrow_world(world, &["log", "net"]);
        assert!(narrowed.contains("  import log;\n") && narrowed.contains("  import net;\n"), "{narrowed}");
        assert!(!narrowed.contains("import process;") && !narrowed.contains("import window;"));
        assert!(narrowed.contains("export guest;"));
        let m = ext_manifest::parse_with(
            "[extension]\nid = \"a.b\"\nversion = \"0.1.0\"\nname = \"B\"\nlicense = \"MIT\"\ndescription = \"d\"\n\n[permissions]\nnet = [\"api.github.com\"]\n",
            ext_manifest::Options { require_why: false },
        )
        .unwrap();
        let g = granted(&m);
        assert!(g.contains(&"net") && g.contains(&"window") && !g.contains(&"process") && !g.contains(&"types"), "{g:?}");
    }
}
