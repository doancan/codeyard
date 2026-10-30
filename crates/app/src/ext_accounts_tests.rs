use std::time::{Duration, Instant};

use ext_core::Source;

use super::*;

fn registry_publisher() -> Publisher {
    let p = ext_core::yard::Publisher {
        issuer: "https://token.actions.githubusercontent.com".into(),
        subject: None,
        owner_id: Some(42),
        repository_id: Some(7),
        workflow: Some(".github/workflows/publish.yml".into()),
    };
    Publisher::of("you.deploy", Some(&Source::Registry { registry: "codeyard".into(), publisher: p }), None)
}

fn acme() -> OAuthProvider {
    OAuthProvider {
        id: "acme".into(),
        name: "Acme".into(),
        authorize_url: "https://acme.example.com/oauth/authorize".into(),
        token_url: "https://acme.example.com/oauth/token".into(),
        client_id: "cy-123".into(),
        scopes: vec!["read".into(), "deploy:write".into()],
        hosts: vec!["api.acme.example.com".into()],
        account_url: None,
        instances: false,
    }
}

#[test]
fn pkce_follows_rfc_7636() {
    // RFC 7636, appendix B.
    let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
    assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    let (a, b) = (Pkce::new(), Pkce::new());
    assert_ne!(a.verifier, b.verifier, "a fresh verifier every time");
    // 32 bytes → 43 characters, inside RFC 7636's 43..=128 of the unreserved set.
    assert_eq!(a.verifier.len(), 43);
    assert!(a.verifier.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
    assert_eq!(Pkce::from_verifier(a.verifier.clone()).challenge, a.challenge);
    for (input, want) in [(&b""[..], ""), (b"f", "Zg"), (b"fo", "Zm8"), (b"foo", "Zm9v"), (b"foob", "Zm9vYg"), (&[0xfb, 0xff], "-_8")] {
        assert_eq!(b64url(input), want);
    }
}

#[test]
fn the_sign_in_address_carries_the_challenge_and_returns_to_the_extension() {
    let url = authorize_url(&acme(), "you.deploy", &["read".into(), "deploy:write".into()], "st4te", "ch4llenge");
    assert!(url.starts_with("https://acme.example.com/oauth/authorize?response_type=code&client_id=cy-123&"), "{url}");
    assert!(url.contains("redirect_uri=codeyard%3A%2F%2Fext%2Fyou.deploy%2Fcallback"), "{url}");
    assert!(url.contains("scope=read%20deploy%3Awrite&state=st4te&code_challenge=ch4llenge&code_challenge_method=S256"), "{url}");
    let mut with_query = acme();
    with_query.authorize_url.push_str("?audience=x");
    assert!(authorize_url(&with_query, "you.deploy", &[], "s", "c").starts_with("https://acme.example.com/oauth/authorize?audience=x&response_type=code"));
    // Scopes: a subset of the declared ones, or all of them.
    assert_eq!(scopes_for(&acme(), &[]).unwrap(), ["read", "deploy:write"]);
    assert_eq!(scopes_for(&acme(), &["read".into()]).unwrap(), ["read"]);
    assert!(matches!(scopes_for(&acme(), &["admin".into()]), Err(v::Error::Invalid(m)) if m.contains("admin")));
}

fn flow(extension: &str, state: &str) -> Flow {
    Flow {
        extension: extension.into(),
        provider: "acme".into(),
        state: state.into(),
        pkce: Pkce::new(),
        scopes: Vec::new(),
        ticket: 1,
        started: Instant::now(),
    }
}

#[test]
fn a_callback_completes_only_its_own_extensions_flow() {
    let cb = parse_callback("codeyard://ext/you.deploy/callback?code=abc%2B1&state=S1").unwrap();
    assert_eq!(cb, Callback { extension: "you.deploy".into(), state: "S1".into(), result: Ok("abc+1".into()) });
    let err = parse_callback("codeyard://ext/you.deploy/callback?error=access_denied&error_description=No+thanks&state=S1").unwrap();
    assert_eq!(err.result, Err("No thanks".into()));
    for bad in [
        "codeyard://ext/you.deploy/callback?code=x",
        "codeyard://ext/../callback?code=x&state=s",
        "https://ext/you.deploy/callback?state=s",
        "codeyard://ext/you.deploy?state=s",
    ] {
        assert!(parse_callback(bad).is_none(), "{bad}");
    }

    let mut flows = Flows::default();
    flows.start(flow("you.deploy", "S1"));
    flows.start(flow("evil.ext", "S2"));
    // evil.ext's callback carrying you.deploy's state completes nothing, and you.deploy's flow stays open.
    let stolen = parse_callback("codeyard://ext/evil.ext/callback?code=c&state=S1").unwrap();
    assert_eq!(flows.take(&stolen).err(), Some(Refused::OtherExtension { owner: "you.deploy".into() }));
    // A state nobody started, or one that only shares a prefix, is unknown.
    for s in ["S3", "S", "S11"] {
        let cb = parse_callback(&format!("codeyard://ext/you.deploy/callback?code=c&state={s}")).unwrap();
        assert_eq!(flows.take(&cb).err(), Some(Refused::Unknown), "{s}");
    }
    let mine = flows.take(&cb).expect("the right extension and state");
    assert_eq!(mine.extension, "you.deploy");
    assert_eq!(flows.take(&cb).err(), Some(Refused::Unknown), "a flow completes once");
    // Starting again replaces the extension's earlier sign-in for that provider.
    flows.start(flow("evil.ext", "S4"));
    let s2 = parse_callback("codeyard://ext/evil.ext/callback?code=c&state=S2").unwrap();
    assert_eq!(flows.take(&s2).err(), Some(Refused::Unknown));
    let s4 = parse_callback("codeyard://ext/evil.ext/callback?code=c&state=S4").unwrap();
    assert!(flows.take(&s4).is_ok());
    assert!(flows.is_empty());
    let mut old = flow("you.deploy", "S5");
    old.started = Instant::now() - FLOW_TIMEOUT - Duration::from_secs(1);
    flows.start(old);
    let late = parse_callback("codeyard://ext/you.deploy/callback?code=c&state=S5").unwrap();
    assert_eq!(flows.take(&late).err(), Some(Refused::Unknown), "an expired sign-in is forgotten");
}

fn token(access: &str) -> Token {
    Token { access: access.into(), refresh: Some("r1".into()), expires: Some(1000), label: "ada".into(), scopes: vec!["read".into()] }
}

#[test]
fn tokens_are_keyed_by_publisher_identity() {
    let kc = MemoryKeychain::default();
    let registry = registry_publisher();
    let sideloaded = Publisher::of("you.deploy", Some(&Source::File { path: "/downloads/deploy.cyx".into() }), None);
    let dev = Publisher::of("you.deploy", None, Some(Path::new("/src/deploy")));
    let mine = Vault { keychain: &kc, publisher: &registry, extension: "you.deploy" };
    mine.save_account("github", &token("gho_registry")).unwrap();
    assert_eq!(mine.account("github").unwrap().map(|t| t.access), Some("gho_registry".into()));
    // The same id from a file, from a folder in development, or under another registry identity finds nothing.
    for other in [&sideloaded, &dev] {
        assert_eq!(Vault { keychain: &kc, publisher: other, extension: "you.deploy" }.account("github").unwrap(), None);
    }
    let mut moved = ext_core::yard::Publisher {
        issuer: "https://token.actions.githubusercontent.com".into(),
        subject: None,
        owner_id: Some(42),
        repository_id: Some(8),
        workflow: Some(".github/workflows/publish.yml".into()),
    };
    let other_repo = Publisher::of("you.deploy", Some(&Source::Registry { registry: "codeyard".into(), publisher: moved.clone() }), None);
    assert_eq!(Vault { keychain: &kc, publisher: &other_repo, extension: "you.deploy" }.account("github").unwrap(), None);
    moved.repository_id = Some(7);
    let same = Publisher::of("you.deploy", Some(&Source::Registry { registry: "codeyard".into(), publisher: moved }), None);
    assert!(Vault { keychain: &kc, publisher: &same, extension: "you.deploy" }.account("github").unwrap().is_some(), "the same identity finds it");
    // Another extension of the same publisher does not share it either.
    assert_eq!(Vault { keychain: &kc, publisher: &registry, extension: "you.other" }.account("github").unwrap(), None);
    // The key names no token, and says whose it is only as a hash.
    let keys = kc.keys();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].starts_with("codeyard-ext:") && keys[0].ends_with("/you.deploy/account/github"), "{keys:?}");
    assert!(!keys[0].contains("githubusercontent"), "{keys:?}");
    // Signing out removes the entry; signing out again is fine.
    mine.sign_out("github").unwrap();
    assert!(kc.keys().is_empty());
    assert!(!kc.contains_value("gho_registry"));
    mine.sign_out("github").unwrap();
    // The prompt names the publisher.
    assert!(registry.shown.contains("repository 7 of owner 42"), "{}", registry.shown);
    assert!(sideloaded.shown.contains("not verified"), "{}", sideloaded.shown);
}

#[test]
fn secrets_have_their_own_entries_and_limits() {
    let kc = MemoryKeychain::default();
    let p = registry_publisher();
    let vault = Vault { keychain: &kc, publisher: &p, extension: "you.deploy" };
    assert_eq!(vault.secret("api-key").unwrap(), None);
    vault.set_secret("api-key", "s3cr3t").unwrap();
    assert_eq!(vault.secret("api-key").unwrap().as_deref(), Some("s3cr3t"));
    assert!(kc.keys()[0].ends_with("/you.deploy/secret/api-key"));
    // Keys cannot climb into another entry's name; values fit every platform's keychain.
    for bad in ["", "../account/github", "a/b", "with space", &"k".repeat(SECRET_KEY_LEN + 1)] {
        assert!(matches!(vault.set_secret(bad, "x"), Err(v::Error::Invalid(_))), "{bad:?}");
    }
    assert!(matches!(vault.set_secret("big", &"x".repeat(SECRET_VALUE_BYTES + 1)), Err(v::Error::Invalid(_))));
    vault.delete_secret("api-key").unwrap();
    vault.delete_secret("api-key").unwrap();
    assert!(kc.keys().is_empty());
}

#[test]
fn token_answers_are_read_as_json_or_form() {
    let t =
        parse_token_response(200, br#"{"access_token":"at","token_type":"Bearer","expires_in":3600,"refresh_token":"rt","scope":"read write"}"#, 100).unwrap();
    assert_eq!((t.access.as_str(), t.refresh.as_deref(), t.expires, t.scopes.len()), ("at", Some("rt"), Some(3700), 2));
    assert!(!t.stale(3000) && t.stale(3650), "stale a minute before it expires");
    let t = parse_token_response(200, b"access_token=gho_x&scope=repo%2Cgist&token_type=bearer", 0).unwrap();
    assert_eq!((t.access.as_str(), t.expires, t.scopes), ("gho_x", None, vec!["repo".to_string(), "gist".to_string()]));
    let e = parse_token_response(400, br#"{"error":"invalid_grant","error_description":"bad code\u202e"}"#, 0).unwrap_err();
    assert!(e.contains("invalid_grant: bad code") && !e.contains('\u{202e}'), "{e}");
    assert!(parse_token_response(200, br#"{"access_token":"x","token_type":"mac"}"#, 0).is_err());
    assert!(parse_token_response(500, b"oops", 0).is_err());
    assert!(parse_token_response(200, br#"{"token_type":"bearer"}"#, 0).is_err());
    // The token never shows in debug output.
    assert!(!format!("{:?}", token("gho_secret")).contains("gho_secret"));
    assert_eq!(Token::decode(&token("a").encode()), Some(token("a")));
}

#[test]
fn account_names_come_from_the_usual_fields_and_are_cleaned() {
    assert_eq!(account_label(br#"{"login":"ada","name":"Ada"}"#).as_deref(), Some("ada"));
    assert_eq!(account_label(br#"{"user":{"email":"ada@example.com"}}"#).as_deref(), Some("ada@example.com"));
    assert_eq!(account_label(br#"{"id":12345}"#).as_deref(), Some("12345"));
    assert_eq!(account_label(b"{\"login\":\"ad\\u202ea\\n\"}").as_deref(), Some("ada"));
    assert_eq!(account_label(b"not json"), None);
}

#[test]
fn a_second_codeyard_hands_the_callback_to_the_running_one() {
    let dir = std::env::temp_dir().join(format!("codeyard-ext-callback-{}", std::process::id()));
    let file = callback_port_file(&dir);
    let rx = listen_for_callbacks(&file).unwrap();
    let url = "codeyard://ext/you.deploy/callback?code=c&state=s";
    forward_callback(url, &file).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), url);
    assert!(forward_callback("codeyard://other", &file).is_err(), "only sign-in answers are handed over");
    let _ = std::fs::remove_dir_all(dir);
}
