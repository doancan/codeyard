wit_bindgen::generate!({ path: "../wit", world: "ext" });

use std::sync::OnceLock;

struct Ext;

static RE: OnceLock<regex::Regex> = OnceLock::new();

#[derive(serde::Serialize, serde::Deserialize)]
struct Reply {
    input: String,
    words: Vec<String>,
    host: Option<String>,
    when: Option<chrono::DateTime<chrono::Utc>>,
    table: toml::Table,
}

impl Guest for Ext {
    fn activate() {
        RE.get_or_init(|| regex::Regex::new(r"(?i)\b[\p{L}\d_-]+\b").unwrap());
        codeyard::spike::host::log("activated");
    }

    fn call(input: String) -> String {
        let re = RE.get().expect("activated");
        let words = re.find_iter(&input).map(|m| m.as_str().to_owned()).collect();
        let host = url::Url::parse(&input).ok().and_then(|u| u.host_str().map(str::to_owned));
        let when = chrono::DateTime::parse_from_rfc3339(&input).ok().map(|d| d.to_utc());
        let table = toml::from_str(&input).unwrap_or_default();
        serde_json::to_string(&Reply { input, words, host, when, table }).unwrap()
    }
}

export!(Ext);
