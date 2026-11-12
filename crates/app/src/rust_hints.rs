//! How rust-analyzer's hints and token modifiers look in the editor: which kinds of inlay hints it sends
//! (`rust.inlayHints.*`, handed to the server as its own configuration) and which semantic token modifiers the editor
//! draws (`rust.semanticModifiers`).

use serde_json::{Map, Value, json};

use crate::lsp::{MOD_CONSUMED, MOD_MUTABLE, MOD_UNSAFE};

/// Which inlay hints are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HintKinds {
    pub types: bool,
    pub parameters: bool,
    pub chaining: bool,
}

impl Default for HintKinds {
    fn default() -> Self {
        Self { types: true, parameters: true, chaining: true }
    }
}

impl HintKinds {
    /// The rust-analyzer setting each kind that is switched off needs: (path in its settings tree).
    fn off(self) -> Vec<[&'static str; 4]> {
        [(self.types, "typeHints"), (self.parameters, "parameterHints"), (self.chaining, "chainingHints")]
            .into_iter()
            .filter(|(on, _)| !on)
            .map(|(_, kind)| ["rust-analyzer", "inlayHints", kind, "enable"])
            .collect()
    }

    /// `settings` (a server's configuration tree, or `Null`) with the kinds switched off added. What the user wrote under
    /// `rust-analyzer.inlayHints` themselves stays: the server's own setting is the more specific one.
    pub fn applied_to(self, settings: &Value) -> Value {
        let mut tree = settings.clone();
        for path in self.off() {
            set_if_missing(&mut tree, &path, json!(false));
        }
        tree
    }
}

fn set_if_missing(tree: &mut Value, path: &[&str], value: Value) {
    if !tree.is_object() {
        *tree = Value::Object(Map::new());
    }
    let Some((last, parents)) = path.split_last() else { return };
    let mut node = tree;
    for key in parents {
        let object = node.as_object_mut().expect("made an object above");
        node = object.entry(*key).or_insert_with(|| Value::Object(Map::new()));
        if !node.is_object() {
            return; // the user wrote something else there
        }
    }
    node.as_object_mut().expect("checked above").entry(*last).or_insert(value);
}

/// Which semantic token modifiers are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifiers(pub u8);

impl Default for Modifiers {
    /// `unsafe` only: it marks code to read twice, and does not mark most of a file.
    fn default() -> Self {
        Self(MOD_UNSAFE)
    }
}

impl Modifiers {
    pub const NAMES: [&'static str; 3] = ["unsafe", "mutable", "consumed"];

    /// The modifiers named in `names`, and the names that are not one.
    pub fn parse(names: &[String]) -> (Self, Vec<String>) {
        let mut flags = 0;
        let mut unknown = Vec::new();
        for name in names {
            match name.as_str() {
                "unsafe" => flags |= MOD_UNSAFE,
                "mutable" => flags |= MOD_MUTABLE,
                "consumed" => flags |= MOD_CONSUMED,
                other => unknown.push(other.to_string()),
            }
        }
        (Self(flags), unknown)
    }

    /// The flags of a token that are drawn.
    pub fn shown(self, token: u8) -> u8 {
        token & self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_switched_off_become_the_servers_own_settings() {
        let all_on = HintKinds::default();
        assert_eq!(all_on.applied_to(&Value::Null), Value::Null, "nothing to add: the tree is left as it is");
        let kinds = HintKinds { types: false, parameters: true, chaining: false };
        let tree = kinds.applied_to(&Value::Null);
        assert_eq!(tree, json!({ "rust-analyzer": { "inlayHints": { "typeHints": { "enable": false }, "chainingHints": { "enable": false } } } }));
    }

    #[test]
    fn what_the_user_wrote_for_the_server_wins() {
        let user = json!({ "rust-analyzer": { "inlayHints": { "typeHints": { "enable": true, "hideClosureInitialization": true } }, "check": { "command": "clippy" } } });
        let tree = HintKinds { types: false, parameters: false, chaining: true }.applied_to(&user);
        assert_eq!(tree["rust-analyzer"]["inlayHints"]["typeHints"], json!({ "enable": true, "hideClosureInitialization": true }), "kept as written");
        assert_eq!(tree["rust-analyzer"]["inlayHints"]["parameterHints"], json!({ "enable": false }));
        assert_eq!(tree["rust-analyzer"]["check"], json!({ "command": "clippy" }));
        // A value of another shape at a parent is not replaced.
        let odd = json!({ "rust-analyzer": { "inlayHints": false } });
        assert_eq!(HintKinds { types: false, ..Default::default() }.applied_to(&odd), odd);
    }

    #[test]
    fn modifier_names_are_checked() {
        let names = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(Modifiers::default(), Modifiers(MOD_UNSAFE));
        let (m, unknown) = Modifiers::parse(&names(&["mutable", "consumed", "shiny"]));
        assert_eq!((m.0, unknown), (MOD_MUTABLE | MOD_CONSUMED, names(&["shiny"])));
        assert_eq!(Modifiers::parse(&[]).0, Modifiers(0));
        assert_eq!(Modifiers(MOD_UNSAFE).shown(MOD_UNSAFE | MOD_MUTABLE), MOD_UNSAFE);
        for name in Modifiers::NAMES {
            assert!(Modifiers::parse(&names(&[name])).1.is_empty(), "{name}");
        }
    }
}
