use super::*;

fn lang(id: &str) -> &'static Language {
    family(id).unwrap().language()
}

fn pat(id: &str, p: &str) -> Pattern {
    compile(lang(id), p).unwrap_or_else(|e| panic!("{id} pattern {p:?}: {e:?}"))
}

/// Matched source texts, in order.
fn found<'t>(id: &str, p: &str, text: &'t str) -> Vec<&'t str> {
    let pattern = pat(id, p);
    find_all(&pattern, text).into_iter().map(|m| &text[m.range]).collect()
}

fn capture<'a>(m: &'a Match, name: &str) -> &'a str {
    &m.captures.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no capture {name}")).text
}

#[test]
fn a_metavariable_matches_one_node() {
    let src = "fn f(v: Vec<String>, w: Option<String>) {\n    let a = w.clone().unwrap_or(v[0].clone());\n    let b = w.unwrap_or(String::new());\n}\n";
    let p = pat("Rust", "$R.unwrap_or($X.clone())");
    let ms = find_all(&p, src);
    assert_eq!(ms.len(), 1);
    assert_eq!(&src[ms[0].range.clone()], "w.clone().unwrap_or(v[0].clone())");
    assert_eq!(capture(&ms[0], "X"), "v[0]");
    assert_eq!(found("Rust", "$X.unwrap_or($Y)", src).len(), 2);
}

#[test]
fn the_pattern_is_matched_by_structure_not_by_spacing() {
    let src = "fn f() { foo(  1 ,\n 2 ); foo(1,2); foo(1, 2, 3); }";
    assert_eq!(found("Rust", "foo($A, $B)", src), ["foo(  1 ,\n 2 )", "foo(1,2)"]);
}

#[test]
fn a_repeated_metavariable_must_match_the_same_text() {
    let src = "fn f(a: i32, b: i32) { let _ = a == a; let _ = a == b; let _ = (a + 1) == (a + 1); }";
    assert_eq!(found("Rust", "$X == $X", src), ["a == a", "(a + 1) == (a + 1)"]);
}

#[test]
fn triple_dollar_takes_any_number_of_siblings() {
    let src = "fn f() { log(); log(1); log(1, 2, 3); other(1); }";
    assert_eq!(found("Rust", "log($$$ARGS)", src), ["log()", "log(1)", "log(1, 2, 3)"]);
    let p = pat("Rust", "log($$$ARGS)");
    let ms = find_all(&p, src);
    assert_eq!(capture(&ms[0], "ARGS"), "");
    assert_eq!(capture(&ms[2], "ARGS"), "1, 2, 3");
    // Siblings on both sides: the first and the last argument stay fixed.
    assert_eq!(found("Rust", "log(1, $$$MID, 3)", src), ["log(1, 2, 3)"]);
    let ms = find_all(&pat("Rust", "log($FIRST, $$$REST)"), src);
    assert_eq!((capture(&ms[0], "FIRST"), capture(&ms[0], "REST")), ("1", "2, 3"));
}

#[test]
fn a_block_of_statements_is_captured_whole() {
    let src = "fn f() { if ok { a(); b(); } if ok { } if other { c(); } }";
    let p = pat("Rust", "if ok { $$$BODY }");
    let ms = find_all(&p, src);
    assert_eq!(ms.len(), 2);
    assert_eq!(capture(&ms[0], "BODY"), "a(); b();");
    assert_eq!(capture(&ms[1], "BODY"), "");
}

#[test]
fn comments_are_skipped_unless_the_pattern_has_one() {
    let src = "fn f() { foo(1, /* why */ 2); foo(1, 2); }";
    assert_eq!(found("Rust", "foo($A, $B)", src).len(), 2);
    // With a comment in the pattern, comments take part.
    let src = "fn f() { foo(1, /* why */ 2); foo(1, 2); }";
    assert_eq!(found("Rust", "foo($A, /* why */ $B)", src), ["foo(1, /* why */ 2)"]);
    // A comment is never matched as a node of its own.
    assert!(found("Rust", "foo(1)", "fn f() { // foo(1)\n}").is_empty());
}

#[test]
fn nested_matches_are_all_found_and_the_outermost_are_kept_for_replacing() {
    let src = "fn f() { id(id(1)); id(2); }";
    let p = pat("Rust", "id($X)");
    let ms = find_all(&p, src);
    assert_eq!(ms.iter().map(|m| &src[m.range.clone()]).collect::<Vec<_>>(), ["id(id(1))", "id(1)", "id(2)"]);
    let top = outermost(&ms);
    assert_eq!(top.iter().map(|m| &src[m.range.clone()]).collect::<Vec<_>>(), ["id(id(1))", "id(2)"]);
}

#[test]
fn identifiers_may_be_non_ascii() {
    let src = "fn f() { çağır(değer); çağır(1); çagir(2); }";
    assert_eq!(found("Rust", "çağır($X)", src), ["çağır(değer)", "çağır(1)"]);
    let ms = find_all(&pat("Rust", "çağır($X)"), src);
    assert_eq!(&src[ms[0].captures[0].range.clone()], "değer");
    assert_eq!(found("Python", "yazdır($X)", "yazdır('é')\nprint(1)\n"), ["yazdır('é')"]);
    assert_eq!(found("Go", "çağır($X)", "package p\nfunc f() {\n\tçağır(1)\n\tçagir(2)\n}\n"), ["çağır(1)"]);
}

#[test]
fn rust_patterns_cover_statements_types_and_items() {
    let src = "fn a() -> Vec<String> { let x = Vec::<String>::new(); let y: Vec<i32> = vec![]; x }\nfn b() {}\n";
    assert_eq!(found("Rust", "let $X: Vec<$T> = $V;", src), ["let y: Vec<i32> = vec![];"]);
    assert_eq!(found("Rust", "Vec<$T>", src), ["Vec<String>", "Vec<i32>"]);
    assert_eq!(found("Rust", "fn $F() {}", src), ["fn b() {}"]);
}

#[test]
fn typescript_patterns() {
    let src = "const a = arr.map(x => x + 1);\nconst b = arr.map((x) => { return x; });\nlet c = arr.filter(f);\nconsole.log(a, b);\nconsole.log(c)\n";
    assert_eq!(found("TypeScript", "console.log($$$ARGS)", src), ["console.log(a, b)", "console.log(c)"]);
    assert_eq!(found("TypeScript", "$A.map($F)", src).len(), 2);
    assert_eq!(found("TypeScript", "const $X = $Y;", src).len(), 2);
    // A semicolon the pattern writes, or leaves out, does not decide the match.
    assert_eq!(found("TypeScript", "let c = arr.filter(f)", src).len(), 1);
    assert_eq!(found("TypeScript", "type $T = Array<$E>;", "type L = Array<string>;\ntype M = Set<string>;\n"), ["type L = Array<string>;"]);
    // A `.tsx` file goes through the TSX grammar; the pattern was compiled for TypeScript.
    let tsx = family("TypeScript").unwrap().language_of("a.tsx").unwrap();
    let p = pat("TypeScript", "useState($X)");
    let m = find_all_in(&p, tsx, "const C = () => { const [a] = useState(0); return <div>{a}</div>; };\n").unwrap();
    assert_eq!(m.len(), 1);
}

#[test]
fn python_patterns() {
    let src = "x = foo(1)\nif x == None:\n    print(x)\nif y == None:\n    pass\nprint('a', 'b')\n";
    assert_eq!(found("Python", "$X == None", src), ["x == None", "y == None"]);
    assert_eq!(found("Python", "print($$$A)", src), ["print(x)", "print('a', 'b')"]);
    assert_eq!(found("Python", "if $C:\n    $$$BODY", src).len(), 2);
    assert_eq!(found("Python", "$A = $B", src), ["x = foo(1)"]);
}

#[test]
fn go_patterns() {
    let src = "package p\n\nfunc f() error {\n\tif err != nil {\n\t\treturn err\n\t}\n\tfmt.Println(\"a\", 1)\n\tx := g(2)\n\treturn nil\n}\n";
    assert_eq!(found("Go", "fmt.Println($$$A)", src), ["fmt.Println(\"a\", 1)"]);
    assert_eq!(found("Go", "$X := g($Y)", src), ["x := g(2)"]);
    assert_eq!(found("Go", "if $E != nil { return $E }", src).len(), 1);
    assert_eq!(found("Go", "func $F() error { $$$B }", src).len(), 1);
}

#[test]
fn the_mutated_text_is_never_found_inside_strings_by_accident() {
    let src = "fn f() { let s = \"unwrap_or(1)\"; unwrap_or(1); }";
    assert_eq!(found("Rust", "unwrap_or($X)", src), ["unwrap_or(1)"]);
    // A variable that is the whole content of a string stands for any content; next to other text a dollar is literal.
    assert_eq!(found("Rust", "price(\"$X\")", "fn f() { price(\"$X\"); price(\"1\"); }").len(), 2);
    assert_eq!(found("Rust", "price(\"a $X\")", "fn f() { price(\"a $X\"); price(\"a 1\"); }"), ["price(\"a $X\")"]);
}

#[test]
fn text_with_syntax_errors_is_refused() {
    let p = pat("Rust", "foo($X)");
    assert_eq!(find_all_in(&p, p.language(), "fn f( { foo(1) "), Err(ParseFailed));
    assert!(find_all(&p, "fn f( { foo(1) ").is_empty());
}

#[test]
fn a_pattern_that_does_not_parse_says_where() {
    let e = compile(lang("Rust"), "foo($X").err().unwrap();
    assert!(matches!(e.kind, PatternErrorKind::Syntax | PatternErrorKind::Missing(_)), "{e:?}");
    assert!(e.span.end <= "foo($X".len() && e.span.start <= e.span.end, "{e:?}");
    let e = compile(lang("Rust"), "let = = 1;").err().unwrap();
    assert!(e.span.end <= 10, "{e:?}");
    assert_eq!(compile(lang("Python"), "   ").err().unwrap().kind, PatternErrorKind::Empty);
    assert_eq!(compile(lang("Rust"), "$X").err().unwrap().kind, PatternErrorKind::OnlyMetavariable);
    assert_eq!(compile(lang("Rust"), "$$$X").err().unwrap().kind, PatternErrorKind::OnlyMetavariable);
    assert_eq!(compile(lang("Rust"), "a(); b();").err().unwrap().kind, PatternErrorKind::NotOneNode);
}

#[test]
fn error_spans_count_bytes_of_the_pattern_as_typed() {
    // The metavariable is longer in the parsed text than in the pattern; the span must not drift.
    let pattern = "foo($LONG_NAME, ) )";
    let e = compile(lang("Rust"), pattern).err().unwrap();
    assert!(e.span.end <= pattern.len() && pattern.is_char_boundary(e.span.start) && pattern.is_char_boundary(e.span.end), "{e:?}");
    let before_error = &pattern[..e.span.start];
    assert!(!before_error.contains("__cymv"));
}

#[test]
fn expand_fills_in_captures_and_leaves_other_dollars_alone() {
    let p = pat("Rust", "log($LEVEL, $$$REST)");
    let src = "fn f() { log(3, \"a\", 1); }";
    let m = &find_all(&p, src)[0];
    assert_eq!(expand("emit($LEVEL, [$$$REST])", &m.captures), "emit(3, [\"a\", 1])");
    assert_eq!(expand("emit($REST)", &m.captures), "emit(\"a\", 1)");
    assert_eq!(expand("${NOT} $other $ $$ $UNKNOWN", &m.captures), "${NOT} $other $ $$ $UNKNOWN");
    assert_eq!(expand("$LEVEL$LEVEL", &m.captures), "33");
}

#[test]
fn the_literal_hint_is_a_token_every_match_contains() {
    assert_eq!(pat("Rust", "$R.unwrap_or($X.clone())").literal_hint().as_deref(), Some("unwrap_or"));
    assert_eq!(pat("Python", "print($A, 'yes')").literal_hint().as_deref(), Some("print"));
    let hint = pat("Rust", "$A + $B").literal_hint();
    assert_eq!(hint.as_deref(), Some("+"));
    assert_eq!(pat("Rust", "let $X = $Y;").literal_hint().as_deref(), Some("let"));
}

#[test]
fn variables_are_listed_in_order_of_first_use() {
    assert_eq!(pat("Rust", "f($B, $A, $B, $_, $$$C)").variables(), ["B", "A", "C"]);
}

#[test]
fn every_family_compiles_a_simple_call_pattern() {
    for f in FAMILIES {
        let sample = match f.id {
            "Shell" => "echo $X",
            "Lua" | "Swift" | "Ruby" => "foo($X)",
            "PHP" => "foo($X)",
            _ => "foo($X)",
        };
        let p = compile(f.language(), sample);
        assert!(p.is_ok(), "{}: {:?}", f.id, p.err());
    }
}

#[test]
fn a_file_belongs_to_one_family() {
    assert_eq!(family_for_path("/a/b/lib.rs").map(|f| f.id), Some("Rust"));
    assert_eq!(family_for_path("x.TSX").map(|f| f.id), Some("TypeScript"));
    assert_eq!(family_for_path("README.md").map(|f| f.id), None);
    assert!(family("Rust").unwrap().language_of("a.py").is_none());
}

/// Cheap deterministic generator: the fuzz test has to give the same run everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

#[test]
fn random_patterns_never_panic_or_hang() {
    const BITS: &[&str] = &[
        "$X",
        "$$$X",
        "$$$",
        "$_",
        "$",
        "$$",
        "$$$$",
        "$x",
        "foo",
        "(",
        ")",
        "{",
        "}",
        "[",
        "]",
        ",",
        ";",
        ".",
        "=",
        "==",
        "let ",
        "fn ",
        "if ",
        "def ",
        "func ",
        "return ",
        "çağır",
        "é",
        "ü",
        "\"",
        "'",
        "/*",
        "*/",
        "//",
        "\n",
        " ",
        "1",
        "->",
        "=>",
        ":",
        "::",
        "<",
        ">",
        "\\",
        "__cymv0__",
        "😀",
    ];
    let texts = [
        ("Rust", "fn f(a: i32) -> i32 { let x = foo(a, 1); if x == a { return foo(x, x, x); } x }\n"),
        ("TypeScript", "const f = (a: number) => { const x = foo(a, 1); if (x === a) { return foo(x, x); } return x; };\n"),
        ("Python", "def f(a):\n    x = foo(a, 1)\n    if x == a:\n        return foo(x, x, x)\n    return x\n"),
        ("Go", "package p\nfunc f(a int) int {\n\tx := foo(a, 1)\n\tif x == a {\n\t\treturn foo(x, x)\n\t}\n\treturn x\n}\n"),
    ];
    let start = std::time::Instant::now();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut compiled = 0;
    for round in 0..1500 {
        let (id, text) = texts[round % texts.len()];
        let n = 1 + (rng.next() % 9) as usize;
        let pattern: String = (0..n).map(|_| rng.pick(BITS)).collect();
        if let Ok(p) = compile(lang(id), &pattern) {
            compiled += 1;
            let _ = find_all(&p, text);
            let _ = p.literal_hint();
            for m in find_all(&p, text) {
                let _ = expand(&pattern, &m.captures);
            }
        }
    }
    assert!(compiled > 20, "the generator should produce some valid patterns ({compiled})");
    assert!(start.elapsed().as_secs() < 60, "fuzz run took {:?}", start.elapsed());
}

#[test]
fn many_triple_dollar_variables_cannot_run_away() {
    let args = (0..40).map(|i| format!("a{i}")).collect::<Vec<_>>().join(", ");
    let text = format!("fn f() {{ foo({args}); }}");
    let p = pat("Rust", "foo($$$A, $$$B, $$$C, $$$D, $$$E, nomatch)");
    let start = std::time::Instant::now();
    assert!(find_all(&p, &text).is_empty());
    assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
}

/// 1 MB of Rust source is matched in tens of milliseconds in a release build; this only guards against a quadratic walk.
#[test]
fn a_megabyte_of_source_matches_in_reasonable_time() {
    let unit = "fn item(a: Option<String>, b: Vec<u8>) -> String {\n    let x = a.unwrap_or(b.len().to_string().clone());\n    x // trailing\n}\n\n";
    let text = unit.repeat(1_000_000 / unit.len());
    let p = pat("Rust", "$R.unwrap_or($X.clone())");
    let start = std::time::Instant::now();
    let n = find_all(&p, &text).len();
    let took = start.elapsed();
    assert_eq!(n, 1_000_000 / unit.len());
    let limit = if cfg!(debug_assertions) { 3000 } else { 150 };
    assert!(took.as_millis() < limit, "1 MB took {took:?}");
    let t = std::time::Instant::now();
    for _ in 0..100 {
        let _ = compile(lang("Rust"), "unwrap_or($X.clone())").unwrap();
    }
    assert!(t.elapsed().as_millis() / 100 < if cfg!(debug_assertions) { 20 } else { 5 }, "compile took {:?} each", t.elapsed() / 100);
}
