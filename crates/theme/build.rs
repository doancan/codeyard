//! design/tokens*.json -> Rust constants. That is the single source of tokens;
//! no colors or sizes are written by hand here.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Map, Value};

fn ident(key: &str) -> String {
    key.replace('-', "_")
}

fn load(path: &PathBuf) -> Map<String, Value> {
    println!("cargo:rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    match serde_json::from_str(&text).expect("valid JSON") {
        Value::Object(m) => m,
        _ => panic!("{}: expected an object", path.display()),
    }
}

fn hex(v: &Value) -> u32 {
    let s = v.as_str().expect("color must be a string");
    // Only `#rrggbb`: `#fff` would silently become 0x000fff, and `#rrggbbaa` a meaningless 32-bit constant.
    let digits = s.strip_prefix('#').filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_hexdigit()));
    let Some(digits) = digits else { panic!("invalid color {s:?}: expected #rrggbb") };
    u32::from_str_radix(digits, 16).expect("hexadecimal")
}

/// Sizes are written with `{:.1}`: extra decimals must not be silently truncated.
fn tenth(v: &Value, what: &str) -> f64 {
    let x = v.as_f64().unwrap_or_else(|| panic!("{what}: expected a number"));
    assert!((x * 10.0 - (x * 10.0).round()).abs() < 1e-9, "{what}: at most one decimal place ({x})");
    x
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("design");
    let dark = load(&root.join("tokens.json"));
    let light = load(&root.join("tokens.light.json"));
    let dark_colors = dark["color"].as_object().unwrap();
    let light_colors = light["color"].as_object().unwrap();
    // The dark and light themes must define the same roles (in both directions): an extra role would be silently ignored.
    for k in light_colors.keys() {
        assert!(dark_colors.contains_key(k), "tokens.light.json: role '{k}' is missing from tokens.json");
    }

    let mut out = String::new();
    out.push_str("/// Color roles (0xRRGGBB).\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub struct Palette {\n");
    for k in dark_colors.keys() {
        writeln!(out, "    pub {}: u32,", ident(k)).unwrap();
    }
    out.push_str("}\n\n");
    for (name, colors) in [("DARK", dark_colors), ("LIGHT", light_colors)] {
        writeln!(out, "pub const {name}: Palette = Palette {{").unwrap();
        for k in dark_colors.keys() {
            let v = colors.get(k).unwrap_or_else(|| panic!("role '{k}' is missing from the {name} theme"));
            writeln!(out, "    {}: 0x{:06x},", ident(k), hex(v)).unwrap();
        }
        out.push_str("};\n\n");
    }

    // Access by role name: for runtime-loaded themes and the contrast test.
    out.push_str("/// All color roles, by their names in tokens.json.\npub const ROLES: &[&str] = &[");
    for k in dark_colors.keys() {
        write!(out, "{k:?}, ").unwrap();
    }
    out.push_str("];\n\nimpl Palette {\n    pub fn get(&self, role: &str) -> Option<u32> {\n        match role {\n");
    for k in dark_colors.keys() {
        writeln!(out, "            {k:?} => Some(self.{}),", ident(k)).unwrap();
    }
    out.push_str("            _ => None,\n        }\n    }\n\n    /// Unknown role: false.\n    pub fn set(&mut self, role: &str, value: u32) -> bool {\n        match role {\n");
    for k in dark_colors.keys() {
        writeln!(out, "            {k:?} => self.{} = value,", ident(k)).unwrap();
    }
    out.push_str("            _ => return false,\n        }\n        true\n    }\n}\n\n");

    // Contrast pairs: (foreground, background, minimum ratio). Every theme must pass them.
    out.push_str("/// WCAG AA pairs: (foreground, background, minimum ratio).\npub const PAIRS: &[(&str, &str, f64)] = &[\n");
    for p in dark["pairs"].as_array().unwrap() {
        let p = p.as_array().unwrap();
        let (fg, bg) = (p[0].as_str().unwrap(), p[1].as_str().unwrap());
        // A misspelled role pair would be silently skipped by the contrast test, and an unknown level would count as 4.5.
        for role in [fg, bg] {
            assert!(dark_colors.contains_key(role), "pairs: unknown role '{role}'");
        }
        let min = match p.get(2).and_then(Value::as_str) {
            Some("ui") | Some("large") => 3.0,
            None => 4.5,
            Some(other) => panic!("pairs: unknown level '{other}' ({fg} / {bg}); must be 'ui' or 'large'"),
        };
        writeln!(out, "    ({fg:?}, {bg:?}, {min:.1}),").unwrap();
    }
    out.push_str("];\n\n");

    // Themes shipped with the app (themes/*.json), embedded in the binary.
    let themes_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("themes");
    println!("cargo:rerun-if-changed={}", themes_dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&themes_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    files.sort();
    out.push_str("/// Embedded theme files: (id, JSON).\npub const BUILTIN_JSON: &[(&str, &str)] = &[\n");
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let id = f.file_stem().unwrap().to_string_lossy();
        writeln!(out, "    ({id:?}, include_str!({:?})),", f.display().to_string()).unwrap();
    }
    out.push_str("];\n\n");

    out.push_str("/// Sizes (px).\npub mod layout {\n");
    for (k, v) in dark["layout"].as_object().unwrap() {
        writeln!(out, "    pub const {}: f32 = {:.1};", ident(k).to_uppercase(), tenth(v, k)).unwrap();
    }
    out.push_str("}\n\n/// Type scale: (size, line height, weight).\npub mod text {\n");
    for (k, v) in dark["type"].as_object().unwrap() {
        let name = match k.as_str() {
            "2xs" => "XXS".to_string(),
            other => ident(other).to_uppercase(),
        };
        let weight = v["weight"].as_u64().unwrap();
        let entry = |size: &Value, line: &Value| format!("({:.1}, {:.1}, {weight})", tenth(size, k), tenth(line, k));
        let other = entry(&v["size"], &v["line"]);
        match v.get("macos") {
            // A step with a macOS value: the target platform picks it (written as `#[cfg]`, since this script runs on the
            // build machine), and both values stay available for docs and tests as `<NAME>_MACOS` / `<NAME>_OTHER`.
            Some(mac) => {
                let mac = entry(&mac["size"], &mac["line"]);
                writeln!(out, "    #[cfg(target_os = \"macos\")]\n    pub const {name}: (f32, f32, u16) = {mac};").unwrap();
                writeln!(out, "    #[cfg(not(target_os = \"macos\"))]\n    pub const {name}: (f32, f32, u16) = {other};").unwrap();
                writeln!(out, "    pub const {name}_MACOS: (f32, f32, u16) = {mac};\n    pub const {name}_OTHER: (f32, f32, u16) = {other};").unwrap();
            }
            None => writeln!(out, "    pub const {name}: (f32, f32, u16) = {other};").unwrap(),
        }
    }
    out.push_str("}\n\n/// Spacing scale: SPACE[n] px.\npub const SPACE: [f32; ");
    let space = dark["space"].as_array().unwrap();
    write!(out, "{}] = [", space.len()).unwrap();
    for v in space {
        write!(out, "{:.1}, ", tenth(v, "space")).unwrap();
    }
    out.push_str("];\n\npub mod radius {\n");
    for (k, v) in dark["radius"].as_object().unwrap() {
        writeln!(out, "    pub const {}: f32 = {:.1};", ident(k).to_uppercase(), tenth(v, k)).unwrap();
    }
    out.push_str("}\n");

    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("tokens.rs");
    std::fs::write(dest, out).unwrap();
}
