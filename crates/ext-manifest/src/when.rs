//! `when` and `enablement` expressions: `editor.language == 'rust' && !terminal`.
//!
//! Grammar: `or := and ('||' and)*`, `and := not ('&&' not)*`, `not := '!' not | cmp`,
//! `cmp := atom (('==' | '!=') atom)?`, `atom := key | 'string' | "string" | number | true | false | '(' or ')'`.
//! Keys are the public context keys (`PUBLIC_KEYS`) or `ext.<key>` for the extension's own context.

use crate::limits;

/// Context keys an extension may test. They map to the app's own contexts inside the app, so internal names stay private.
pub const PUBLIC_KEYS: &[&str] = &["editor", "terminal", "file-tree", "scm", "search", "panel", "workspace.trusted", "editor.language"];

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Str(String),
    Num(f64),
}

impl Value {
    fn truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Str(s) => !s.is_empty(),
            Value::Num(n) => *n != 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Key(String),
    Lit(Value),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Evaluate with `lookup` giving each key's value; an unknown or unset key is false.
    pub fn eval(&self, lookup: &dyn Fn(&str) -> Option<Value>) -> bool {
        self.value(lookup).truthy()
    }

    fn value(&self, lookup: &dyn Fn(&str) -> Option<Value>) -> Value {
        match self {
            Expr::Key(k) => lookup(k).unwrap_or(Value::Bool(false)),
            Expr::Lit(v) => v.clone(),
            Expr::Not(e) => Value::Bool(!e.eval(lookup)),
            Expr::And(a, b) => Value::Bool(a.eval(lookup) && b.eval(lookup)),
            Expr::Or(a, b) => Value::Bool(a.eval(lookup) || b.eval(lookup)),
            Expr::Eq(a, b) => Value::Bool(a.value(lookup) == b.value(lookup)),
            Expr::Ne(a, b) => Value::Bool(a.value(lookup) != b.value(lookup)),
        }
    }

    /// Every key the expression reads.
    pub fn keys(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_keys(&mut out);
        out
    }

    fn collect_keys<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Expr::Key(k) => out.push(k),
            Expr::Lit(_) => {}
            Expr::Not(e) => e.collect_keys(out),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Eq(a, b) | Expr::Ne(a, b) => {
                a.collect_keys(out);
                b.collect_keys(out);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f64),
    Not,
    And,
    Or,
    Eq,
    Ne,
    Open,
    Close,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut it = src.char_indices().peekable();
    while let Some(&(i, c)) = it.peek() {
        match c {
            c if c.is_whitespace() => {
                it.next();
            }
            '(' | ')' => {
                it.next();
                out.push(if c == '(' { Tok::Open } else { Tok::Close });
            }
            '!' | '=' | '&' | '|' => {
                it.next();
                let next = it.peek().map(|&(_, n)| n);
                let tok = match (c, next) {
                    ('!', Some('=')) => Tok::Ne,
                    ('=', Some('=')) => Tok::Eq,
                    ('&', Some('&')) => Tok::And,
                    ('|', Some('|')) => Tok::Or,
                    ('!', _) => {
                        out.push(Tok::Not);
                        continue;
                    }
                    _ => return Err(format!("unexpected '{c}' at column {}; use ==, !=, && or ||", i + 1)),
                };
                it.next();
                out.push(tok);
            }
            '\'' | '"' => {
                it.next();
                let mut s = String::new();
                loop {
                    match it.next() {
                        Some((_, q)) if q == c => break,
                        Some((_, ch)) => s.push(ch),
                        None => return Err(format!("the string at column {} is not closed", i + 1)),
                    }
                }
                out.push(Tok::Str(s));
            }
            c if c.is_ascii_digit() => {
                let mut s = String::new();
                while let Some(&(_, d)) = it.peek() {
                    if d.is_ascii_digit() || d == '.' {
                        s.push(d);
                        it.next();
                    } else {
                        break;
                    }
                }
                out.push(Tok::Num(s.parse().map_err(|_| format!("'{s}' is not a number"))?));
            }
            c if c.is_ascii_alphabetic() => {
                let mut s = String::new();
                while let Some(&(_, d)) = it.peek() {
                    if d.is_ascii_alphanumeric() || matches!(d, '.' | '-' | '_') {
                        s.push(d);
                        it.next();
                    } else {
                        break;
                    }
                }
                out.push(Tok::Ident(s));
            }
            _ => return Err(format!("unexpected '{c}' at column {}", i + 1)),
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > limits::WHEN_DEPTH { Err(format!("the expression is nested deeper than {} levels", limits::WHEN_DEPTH)) } else { Ok(()) }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut e = self.and()?;
        while self.eat(&Tok::Or) {
            e = Expr::Or(Box::new(e), Box::new(self.and()?));
        }
        Ok(e)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut e = self.not()?;
        while self.eat(&Tok::And) {
            e = Expr::And(Box::new(e), Box::new(self.not()?));
        }
        Ok(e)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.eat(&Tok::Not) {
            self.deeper()?;
            let e = Expr::Not(Box::new(self.not()?));
            self.depth -= 1;
            return Ok(e);
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let a = self.atom()?;
        if self.eat(&Tok::Eq) {
            return Ok(Expr::Eq(Box::new(a), Box::new(self.atom()?)));
        }
        if self.eat(&Tok::Ne) {
            return Ok(Expr::Ne(Box::new(a), Box::new(self.atom()?)));
        }
        Ok(a)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        let tok = self.peek().cloned().ok_or("the expression ends too early")?;
        self.pos += 1;
        match tok {
            Tok::Ident(s) if s == "true" => Ok(Expr::Lit(Value::Bool(true))),
            Tok::Ident(s) if s == "false" => Ok(Expr::Lit(Value::Bool(false))),
            Tok::Ident(s) => {
                let public = PUBLIC_KEYS.contains(&s.as_str());
                let own = s.strip_prefix("ext.").is_some_and(|k| !k.is_empty());
                if !public && !own {
                    return Err(format!("unknown key '{s}'; use one of {} or ext.<your-key>", PUBLIC_KEYS.join(", ")));
                }
                Ok(Expr::Key(s))
            }
            Tok::Str(s) => Ok(Expr::Lit(Value::Str(s))),
            Tok::Num(n) => Ok(Expr::Lit(Value::Num(n))),
            Tok::Open => {
                self.deeper()?;
                let e = self.or()?;
                if !self.eat(&Tok::Close) {
                    return Err("a '(' is not closed".into());
                }
                self.depth -= 1;
                Ok(e)
            }
            t => Err(format!("expected a key or a value, found {}", describe(&t))),
        }
    }
}

fn describe(t: &Tok) -> &'static str {
    match t {
        Tok::Not => "'!'",
        Tok::And => "'&&'",
        Tok::Or => "'||'",
        Tok::Eq => "'=='",
        Tok::Ne => "'!='",
        Tok::Close => "')'",
        Tok::Open => "'('",
        Tok::Ident(_) | Tok::Str(_) | Tok::Num(_) => "a value",
    }
}

/// Parse an expression; the error is one sentence for the author.
pub fn parse(src: &str) -> Result<Expr, String> {
    if src.len() > limits::WHEN_LEN {
        return Err(format!("the expression is longer than {} characters", limits::WHEN_LEN));
    }
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err("the expression is empty".into());
    }
    let mut p = Parser { toks, pos: 0, depth: 0 };
    let e = p.or()?;
    match p.peek() {
        None => Ok(e),
        Some(t) => Err(format!("unexpected {} after a complete expression", describe(t))),
    }
}
