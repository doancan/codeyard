//! Who owns a publisher name, and which names are too close to someone else's. Ids are already lowercase ASCII
//! (`ext_manifest::ExtId`), so the tricks left are look-alike characters (`rn` for `m`, `0` for `o`), dropped hyphens,
//! and one-letter typos of a publisher name.

use ext_core::yard::Publisher;

/// The owner a publisher name belongs to: the account behind the signature, so every repository of one GitHub owner
/// (by numeric id) can publish under it, and nobody else can.
pub fn owner_key(p: &Publisher) -> String {
    match (p.owner_id, &p.subject) {
        (Some(id), _) => format!("{} owner {id}", p.issuer),
        (None, Some(s)) => format!("{} subject {s}", p.issuer),
        (None, None) => p.issuer.clone(),
    }
}

/// What a name looks like at a glance: no separators, look-alikes folded.
pub fn skeleton(s: &str) -> String {
    let folded: String = s
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '.' | '_' | ' '))
        .map(|c| match c {
            '0' => 'o',
            '1' | 'i' | '|' => 'l',
            '5' => 's',
            '3' => 'e',
            '4' => 'a',
            '8' => 'b',
            c => c,
        })
        .collect();
    folded.replace("rn", "m").replace("vv", "w").replace("cl", "d")
}

/// The existing publisher name a new one is confusable with: the same skeleton, or (for names of five or more
/// characters) one edit away.
pub fn clashing_prefix<'a>(new: &str, others: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let s = skeleton(new);
    others.into_iter().find(|o| {
        let t = skeleton(o);
        t == s || (s.len().min(t.len()) >= 5 && within_one_edit(&s, &t))
    })
}

/// The other owner's extension a new id or display name is confusable with.
pub fn clashing_extension(id: &ext_manifest::ExtId, name: &str, others: &[(String, String)]) -> Option<String> {
    let (sid, sname) = (skeleton(id.as_str()), skeleton(name));
    others.iter().find(|(oid, oname)| skeleton(oid) == sid || (!sname.is_empty() && skeleton(oname) == sname)).map(|(oid, _)| oid.clone())
}

/// One insertion, deletion or substitution apart (or equal).
fn within_one_edit(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if long.len() - short.len() > 1 {
        return false;
    }
    let prefix = short.iter().zip(long).take_while(|(x, y)| x == y).count();
    if short.len() == long.len() {
        short[prefix + 1.min(short.len() - prefix)..] == long[prefix + 1.min(long.len() - prefix)..]
    } else {
        short[prefix..] == long[prefix + 1..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_alikes_fold_together() {
        assert_eq!(skeleton("microsoft"), skeleton("rnicr0soft"));
        assert_eq!(skeleton("my-theme"), skeleton("mytheme"));
        assert_eq!(skeleton("paypal"), skeleton("paypa1"));
        assert_ne!(skeleton("vercel"), skeleton("gitlab"));
    }

    #[test]
    fn publisher_names_one_edit_apart_clash_when_long_enough() {
        let existing = ["microsoft", "vercel", "abc"];
        assert_eq!(clashing_prefix("microsft", existing.into_iter()), Some("microsoft"));
        assert_eq!(clashing_prefix("vercell", existing.into_iter()), Some("vercel"));
        assert_eq!(clashing_prefix("rnicrosoft", existing.into_iter()), Some("microsoft"));
        // Short names differ by one letter all the time.
        assert_eq!(clashing_prefix("abd", existing.into_iter()), None);
        assert_eq!(clashing_prefix("gitlab", existing.into_iter()), None);
    }

    #[test]
    fn extension_ids_and_names_clash_with_other_owners() {
        let others = vec![("ms.python".to_string(), "Python".to_string())];
        let id = |s: &str| ext_manifest::ExtId::parse(s).unwrap();
        assert_eq!(clashing_extension(&id("rns.python"), "Snake", &others), Some("ms.python".into()));
        assert_eq!(clashing_extension(&id("me.snake"), "Pyth0n", &others), Some("ms.python".into()));
        assert_eq!(clashing_extension(&id("me.snake"), "Snake", &others), None);
    }

    #[test]
    fn edits() {
        assert!(within_one_edit("abc", "abc"));
        assert!(within_one_edit("abc", "abd"));
        assert!(within_one_edit("abc", "ab"));
        assert!(within_one_edit("abc", "xabc"));
        assert!(!within_one_edit("abc", "acb"));
        assert!(!within_one_edit("abc", "a"));
        assert!(within_one_edit("", "a"));
    }

    #[test]
    fn owners_are_accounts_not_repositories() {
        let a = ext_core::testing::github(7, 1);
        let b = ext_core::testing::github(7, 2);
        assert_eq!(owner_key(&a), owner_key(&b));
        assert_ne!(owner_key(&a), owner_key(&ext_core::testing::github(8, 1)));
    }
}
