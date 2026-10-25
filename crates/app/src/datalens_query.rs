//! A small jq: paths (`.a.b[0]`, `.[]`, `..`, slices), pipes, comma, comparison, boolean and arithmetic operators,
//! array and object construction, `if`, `//` and about forty built-in functions. There is no `input`, `env`, `$ENV`,
//! `import`, `include` or any other way out of the document: a query can only read the value it is given.
//!
//! Evaluation is eager (each filter returns all its outputs) and borrows from the input where it can, so a path
//! through a big document does not copy it. Work is bounded by a step budget, and a cancel callback is polled so a
//! newer query can stop an older one that is still running.

use std::borrow::Cow;
use std::cmp::Ordering;

use serde_json::{Map, Number, Value};

use crate::i18n::{t, tf};

/// Filters and operators evaluated before the query is given up on. A node of a 20 MB document costs one step, so real
/// queries stay far below it; a cross product like `.[] + .[]` on a long list ends in a message instead of eating memory
/// (every step can hold a value).
const STEP_BUDGET: u64 = 5_000_000;
/// Nodes in a parsed query. Bounds recursion depth when evaluating (and when dropping the tree).
const MAX_NODES: usize = 400;
/// Nested brackets, parentheses and call arguments in the query text; bounds the parser's recursion.
const MAX_NESTING: usize = 24;
/// Longest string `*` may build.
const MAX_REPEAT: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct QueryError {
    pub message: String,
    /// Byte offset in the query text, for syntax errors; runtime errors have none.
    pub offset: Option<usize>,
}

impl QueryError {
    fn at(offset: usize, message: String) -> Self {
        QueryError { message, offset: Some(offset) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone)]
enum Expr {
    Identity,
    Recurse,
    Literal(Value),
    Index(Box<Expr>, Box<Expr>),
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>),
    Iterate(Box<Expr>),
    Try(Box<Expr>),
    Pipe(Box<Expr>, Box<Expr>),
    Comma(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Alt(Box<Expr>, Box<Expr>),
    Array(Option<Box<Expr>>),
    Object(Vec<(Expr, Expr)>),
    If(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Call(&'static str, Vec<Expr>),
}

/// Built-in functions: name and the argument counts they accept.
const FUNCTIONS: &[(&str, &[usize])] = &[
    ("length", &[0]),
    ("keys", &[0]),
    ("keys_unsorted", &[0]),
    ("values", &[0]),
    ("not", &[0]),
    ("empty", &[0]),
    ("type", &[0]),
    ("map", &[1]),
    ("map_values", &[1]),
    ("select", &[1]),
    ("has", &[1]),
    ("contains", &[1]),
    ("add", &[0]),
    ("any", &[0, 1]),
    ("all", &[0, 1]),
    ("first", &[0, 1]),
    ("last", &[0]),
    ("limit", &[2]),
    ("sort", &[0]),
    ("sort_by", &[1]),
    ("group_by", &[1]),
    ("unique", &[0]),
    ("unique_by", &[1]),
    ("reverse", &[0]),
    ("min", &[0]),
    ("max", &[0]),
    ("flatten", &[0, 1]),
    ("to_entries", &[0]),
    ("from_entries", &[0]),
    ("with_entries", &[1]),
    ("tostring", &[0]),
    ("tonumber", &[0]),
    ("tojson", &[0]),
    ("fromjson", &[0]),
    ("ascii_downcase", &[0]),
    ("ascii_upcase", &[0]),
    ("startswith", &[1]),
    ("endswith", &[1]),
    ("ltrimstr", &[1]),
    ("rtrimstr", &[1]),
    ("join", &[1]),
    ("split", &[1]),
    ("floor", &[0]),
    ("recurse", &[0]),
];

// --- Parser ---

struct Parser<'a> {
    src: &'a str,
    i: usize,
    nesting: usize,
    nodes: usize,
}

type Parsed = Result<Expr, QueryError>;

/// A parsed query, ready to run against any number of documents.
pub struct Query {
    expr: Expr,
}

pub fn parse(src: &str) -> Result<Query, QueryError> {
    let mut p = Parser { src, i: 0, nesting: 0, nodes: 0 };
    // jq treats an empty program as `.`.
    if src.trim().is_empty() {
        return Ok(Query { expr: Expr::Identity });
    }
    let expr = p.pipe()?;
    p.ws();
    if p.i < src.len() {
        return Err(p.unexpected());
    }
    Ok(Query { expr })
}

impl Parser<'_> {
    fn ws(&mut self) {
        let b = self.src.as_bytes();
        while self.i < b.len() {
            match b[self.i] {
                b' ' | b'\t' | b'\r' | b'\n' => self.i += 1,
                b'#' => {
                    while self.i < b.len() && b[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.i).copied()
    }

    fn rest(&self) -> &str {
        &self.src[self.i..]
    }

    fn eat(&mut self, token: &str) -> bool {
        self.ws();
        if self.rest().starts_with(token) {
            self.i += token.len();
            true
        } else {
            false
        }
    }

    /// A keyword or operator word: must not run into a longer identifier (`order` is not `or`).
    fn eat_word(&mut self, word: &str) -> bool {
        self.ws();
        let rest = self.rest();
        if rest.starts_with(word) && !rest[word.len()..].starts_with(is_ident_char) {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<(), QueryError> {
        if self.eat(token) {
            Ok(())
        } else {
            self.ws();
            Err(QueryError::at(self.i, tf("Expected “{}”", &[&token])))
        }
    }

    fn unexpected(&self) -> QueryError {
        match self.rest().chars().next() {
            Some(c) => QueryError::at(self.i, tf("Unexpected “{}”", &[&c])),
            None => QueryError::at(self.i, t("The query ends too early").to_string()),
        }
    }

    fn node(&mut self, e: Expr) -> Result<Expr, QueryError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(QueryError::at(self.i, t("The query is too long").to_string()));
        }
        Ok(e)
    }

    fn enter(&mut self) -> Result<(), QueryError> {
        self.nesting += 1;
        if self.nesting > MAX_NESTING {
            return Err(QueryError::at(self.i, t("The query is nested too deeply").to_string()));
        }
        Ok(())
    }

    /// `a | b | c` folds to the right, so evaluation order and tree shape agree.
    fn pipe(&mut self) -> Parsed {
        self.enter()?;
        let mut parts = vec![self.comma()?];
        while self.peek_after_ws() == Some(b'|') && !self.rest_after_ws().starts_with("|=") {
            self.eat("|");
            parts.push(self.comma()?);
        }
        let mut out = parts.pop().expect("one part");
        while let Some(left) = parts.pop() {
            out = self.node(Expr::Pipe(Box::new(left), Box::new(out)))?;
        }
        self.nesting -= 1;
        Ok(out)
    }

    fn peek_after_ws(&mut self) -> Option<u8> {
        self.ws();
        self.peek()
    }

    fn rest_after_ws(&mut self) -> &str {
        self.ws();
        self.rest()
    }

    fn comma(&mut self) -> Parsed {
        let mut left = self.alt()?;
        while self.eat(",") {
            let right = self.alt()?;
            left = self.node(Expr::Comma(Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }

    /// `a // b`, right-associative, binds looser than `or`.
    fn alt(&mut self) -> Parsed {
        let left = self.or()?;
        if self.rest_after_ws().starts_with("//") && !self.rest().starts_with("//=") {
            self.i += 2;
            let right = self.alt()?;
            return self.node(Expr::Alt(Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn or(&mut self) -> Parsed {
        let mut left = self.and()?;
        while self.eat_word("or") {
            let right = self.and()?;
            left = self.node(Expr::Or(Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }

    fn and(&mut self) -> Parsed {
        let mut left = self.compare()?;
        while self.eat_word("and") {
            let right = self.compare()?;
            left = self.node(Expr::And(Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }

    fn compare(&mut self) -> Parsed {
        let left = self.additive()?;
        self.ws();
        let op = [("==", BinOp::Eq), ("!=", BinOp::Ne), ("<=", BinOp::Le), (">=", BinOp::Ge), ("<", BinOp::Lt), (">", BinOp::Gt)]
            .into_iter()
            .find(|(s, _)| self.rest().starts_with(s));
        let Some((token, op)) = op else { return Ok(left) };
        self.i += token.len();
        let right = self.additive()?;
        self.node(Expr::Binary(op, Box::new(left), Box::new(right)))
    }

    fn additive(&mut self) -> Parsed {
        let mut left = self.multiplicative()?;
        loop {
            self.ws();
            let op = match self.peek() {
                Some(b'+') if !self.rest().starts_with("+=") => BinOp::Add,
                Some(b'-') if !self.rest().starts_with("-=") => BinOp::Sub,
                _ => return Ok(left),
            };
            self.i += 1;
            let right = self.multiplicative()?;
            left = self.node(Expr::Binary(op, Box::new(left), Box::new(right)))?;
        }
    }

    fn multiplicative(&mut self) -> Parsed {
        let mut left = self.unary()?;
        loop {
            self.ws();
            let op = match self.peek() {
                Some(b'*') if !self.rest().starts_with("*=") => BinOp::Mul,
                Some(b'/') if !self.rest().starts_with("//") && !self.rest().starts_with("/=") => BinOp::Div,
                Some(b'%') if !self.rest().starts_with("%=") => BinOp::Rem,
                _ => return Ok(left),
            };
            self.i += 1;
            let right = self.unary()?;
            left = self.node(Expr::Binary(op, Box::new(left), Box::new(right)))?;
        }
    }

    fn unary(&mut self) -> Parsed {
        self.ws();
        if self.peek() == Some(b'-') {
            self.i += 1;
            self.enter()?;
            let inner = self.unary()?;
            self.nesting -= 1;
            return self.node(Expr::Neg(Box::new(inner)));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Parsed {
        let mut e = self.term()?;
        loop {
            self.ws();
            let rest = self.rest();
            if rest.starts_with('?') {
                self.i += 1;
                e = self.node(Expr::Try(Box::new(e)))?;
            } else if rest.starts_with('[') {
                e = self.bracket(e)?;
            } else if rest.starts_with(".[") {
                self.i += 1;
                e = self.bracket(e)?;
            } else if rest.starts_with('.') && !rest.starts_with("..") {
                let next = rest[1..].chars().next();
                if next == Some('"') {
                    self.i += 1;
                    let key = self.string()?;
                    e = self.index_with(e, Value::String(key))?;
                } else if next.is_some_and(is_ident_start) {
                    self.i += 1;
                    let name = self.ident();
                    e = self.index_with(e, Value::String(name))?;
                } else {
                    return Ok(e);
                }
            } else {
                return Ok(e);
            }
        }
    }

    fn index_with(&mut self, target: Expr, key: Value) -> Parsed {
        let key = self.node(Expr::Literal(key))?;
        self.node(Expr::Index(Box::new(target), Box::new(key)))
    }

    /// `[...]` after a term: iterate, index or slice.
    fn bracket(&mut self, target: Expr) -> Parsed {
        self.expect("[")?;
        self.enter()?;
        let out = if self.eat("]") {
            self.node(Expr::Iterate(Box::new(target)))?
        } else if self.eat(":") {
            let to = self.pipe()?;
            self.expect("]")?;
            self.node(Expr::Slice(Box::new(target), None, Some(Box::new(to))))?
        } else {
            let first = self.pipe()?;
            if self.eat(":") {
                let to = if self.rest_after_ws().starts_with(']') { None } else { Some(Box::new(self.pipe()?)) };
                self.expect("]")?;
                self.node(Expr::Slice(Box::new(target), Some(Box::new(first)), to))?
            } else {
                self.expect("]")?;
                self.node(Expr::Index(Box::new(target), Box::new(first)))?
            }
        };
        self.nesting -= 1;
        Ok(out)
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while self.rest().starts_with(is_ident_char) {
            self.i += 1;
        }
        self.src[start..self.i].to_string()
    }

    fn string(&mut self) -> Result<String, QueryError> {
        let start = self.i;
        let b = self.src.as_bytes();
        debug_assert_eq!(b.get(start), Some(&b'"'));
        let mut i = start + 1;
        while i < b.len() {
            match b[i] {
                b'\\' if b.get(i + 1) == Some(&b'(') => {
                    return Err(QueryError::at(i, t("String interpolation is not supported").to_string()));
                }
                b'\\' => i += 2,
                b'"' => {
                    self.i = i + 1;
                    return serde_json::from_str::<String>(&self.src[start..=i]).map_err(|_| QueryError::at(start, t("Invalid string").to_string()));
                }
                _ => i += 1,
            }
        }
        Err(QueryError::at(start, t("The string is not closed").to_string()))
    }

    fn number(&mut self) -> Parsed {
        let start = self.i;
        let b = self.src.as_bytes();
        while self.i < b.len() && (b[self.i].is_ascii_digit() || b[self.i] == b'.') {
            self.i += 1;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.i += 1;
                }
            } else {
                self.i = save;
            }
        }
        let text = &self.src[start..self.i];
        let value = text
            .parse::<i64>()
            .map(Number::from)
            .or_else(|_| text.parse::<u64>().map(Number::from))
            .ok()
            .or_else(|| text.parse::<f64>().ok().and_then(Number::from_f64))
            .ok_or_else(|| QueryError::at(start, tf("“{}” is not a number", &[&text])))?;
        self.node(Expr::Literal(Value::Number(value)))
    }

    fn term(&mut self) -> Parsed {
        self.ws();
        let at = self.i;
        let Some(c) = self.peek() else { return Err(self.unexpected()) };
        match c {
            b'.' if self.rest().starts_with("..") => {
                self.i += 2;
                self.node(Expr::Recurse)
            }
            b'.' => {
                self.i += 1;
                match self.peek() {
                    Some(b'"') => {
                        let key = self.string()?;
                        let id = self.node(Expr::Identity)?;
                        self.index_with(id, Value::String(key))
                    }
                    Some(c) if is_ident_start(c as char) => {
                        let name = self.ident();
                        let id = self.node(Expr::Identity)?;
                        self.index_with(id, Value::String(name))
                    }
                    _ => self.node(Expr::Identity),
                }
            }
            b'"' => {
                let s = self.string()?;
                self.node(Expr::Literal(Value::String(s)))
            }
            b'0'..=b'9' => self.number(),
            b'(' => {
                self.i += 1;
                let inner = self.pipe()?;
                self.expect(")")?;
                Ok(inner)
            }
            b'[' => {
                self.i += 1;
                if self.eat("]") {
                    return self.node(Expr::Array(None));
                }
                let inner = self.pipe()?;
                self.expect("]")?;
                self.node(Expr::Array(Some(Box::new(inner))))
            }
            b'{' => self.object(),
            b'$' => Err(QueryError::at(at, t("Variables are not supported").to_string())),
            b'@' => Err(QueryError::at(at, t("Format strings are not supported").to_string())),
            c if is_ident_start(c as char) => self.word(at),
            _ => Err(self.unexpected()),
        }
    }

    fn object(&mut self) -> Parsed {
        self.expect("{")?;
        self.enter()?;
        let mut entries = Vec::new();
        if !self.eat("}") {
            loop {
                self.ws();
                let at = self.i;
                let (key, shorthand) = match self.peek() {
                    Some(b'(') => {
                        self.i += 1;
                        let k = self.pipe()?;
                        self.expect(")")?;
                        (k, None)
                    }
                    Some(b'"') => {
                        let s = self.string()?;
                        (self.node(Expr::Literal(Value::String(s.clone())))?, Some(s))
                    }
                    Some(c) if is_ident_start(c as char) => {
                        let s = self.ident();
                        (self.node(Expr::Literal(Value::String(s.clone())))?, Some(s))
                    }
                    Some(b'$') => return Err(QueryError::at(at, t("Variables are not supported").to_string())),
                    _ => return Err(self.unexpected()),
                };
                let value = if self.eat(":") {
                    // Like jq, a value is a term chain with pipes, but no comma (the comma separates entries).
                    let mut parts = vec![self.alt()?];
                    while self.peek_after_ws() == Some(b'|') {
                        self.i += 1;
                        parts.push(self.alt()?);
                    }
                    let mut out = parts.pop().expect("one part");
                    while let Some(left) = parts.pop() {
                        out = self.node(Expr::Pipe(Box::new(left), Box::new(out)))?;
                    }
                    out
                } else if let Some(name) = shorthand {
                    let id = self.node(Expr::Identity)?;
                    self.index_with(id, Value::String(name))?
                } else {
                    return Err(QueryError::at(self.i, tf("Expected “{}”", &[&":"])));
                };
                entries.push((key, value));
                if self.eat(",") {
                    continue;
                }
                self.expect("}")?;
                break;
            }
        }
        self.nesting -= 1;
        self.node(Expr::Object(entries))
    }

    fn word(&mut self, at: usize) -> Parsed {
        let name = self.ident();
        match name.as_str() {
            "true" => return self.node(Expr::Literal(Value::Bool(true))),
            "false" => return self.node(Expr::Literal(Value::Bool(false))),
            "null" => return self.node(Expr::Literal(Value::Null)),
            "if" => return self.conditional(),
            "try" => {
                let body = self.postfix()?;
                return self.node(Expr::Try(Box::new(body)));
            }
            "def" | "reduce" | "foreach" | "as" | "catch" | "label" | "import" | "include" | "then" | "elif" | "else" | "end" | "and" | "or" | "input"
            | "inputs" | "env" | "input_filename" | "debug" | "stderr" | "now" | "halt" | "halt_error" => {
                return Err(QueryError::at(at, tf("“{}” is not supported", &[&name])));
            }
            _ => {}
        }
        let mut args = Vec::new();
        if self.peek_after_ws() == Some(b'(') {
            self.i += 1;
            self.enter()?;
            loop {
                args.push(self.pipe()?);
                if self.eat(";") {
                    continue;
                }
                self.expect(")")?;
                break;
            }
            self.nesting -= 1;
        }
        let Some(&(fname, arities)) = FUNCTIONS.iter().find(|(n, _)| *n == name) else {
            return Err(QueryError::at(at, tf("“{}” is not a function this query language has", &[&name])));
        };
        if !arities.contains(&args.len()) {
            return Err(QueryError::at(at, tf("“{}” is called with the wrong number of arguments", &[&fname])));
        }
        self.node(Expr::Call(fname, args))
    }

    fn conditional(&mut self) -> Parsed {
        self.enter()?;
        let cond = self.pipe()?;
        if !self.eat_word("then") {
            return Err(QueryError::at(self.i, tf("Expected “{}”", &[&"then"])));
        }
        let then = self.pipe()?;
        let otherwise = if self.eat_word("elif") {
            // `elif` is `else if` that shares the final `end`.
            Some(Box::new(self.conditional()?))
        } else if self.eat_word("else") {
            let e = self.pipe()?;
            if !self.eat_word("end") {
                return Err(QueryError::at(self.i, tf("Expected “{}”", &[&"end"])));
            }
            Some(Box::new(e))
        } else if self.eat_word("end") {
            None
        } else {
            return Err(QueryError::at(self.i, tf("Expected “{}”", &[&"end"])));
        };
        self.nesting -= 1;
        self.node(Expr::If(Box::new(cond), Box::new(then), otherwise))
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

// --- Evaluation ---

enum Stop {
    Failed(String),
    Cancelled,
    OutOfBudget,
}

type Out<'a> = Result<Vec<Cow<'a, Value>>, Stop>;

fn fail<T>(message: String) -> Result<T, Stop> {
    Err(Stop::Failed(message))
}

struct Run<'c> {
    steps: u64,
    cancel: &'c dyn Fn() -> bool,
}

impl Query {
    /// All outputs of the query for `input`. `cancel` is polled now and then; when it returns true the run stops with an
    /// empty error (the caller has already moved on to a newer query).
    pub fn run(&self, input: &Value, cancel: &dyn Fn() -> bool) -> Result<Vec<Value>, QueryError> {
        let mut run = Run { steps: 0, cancel };
        match run.eval(&self.expr, Cow::Borrowed(input)) {
            Ok(out) => Ok(out.into_iter().map(Cow::into_owned).collect()),
            Err(Stop::Failed(message)) => Err(QueryError { message, offset: None }),
            Err(Stop::Cancelled) => Err(QueryError { message: String::new(), offset: None }),
            Err(Stop::OutOfBudget) => Err(QueryError { message: t("The query needs more work than allowed; narrow it down").to_string(), offset: None }),
        }
    }
}

fn owned<'a>(v: Value) -> Cow<'a, Value> {
    Cow::Owned(v)
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Null | Value::Bool(false))
}

pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A value for an error message: compact JSON, shortened like jq does.
fn show(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 11 {
        let head: String = s.chars().take(10).collect();
        format!("{head}...")
    } else {
        s
    }
}

fn typed(v: &Value) -> String {
    format!("{} ({})", type_name(v), show(v))
}

/// Numbers that are whole print as integers (`3`, not `3.0`), like jq.
fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 {
        return Value::from(f as i64);
    }
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

fn as_f64(n: &Number) -> f64 {
    n.as_f64().unwrap_or(f64::NAN)
}

/// jq's order across types: null, false, true, numbers, strings, arrays, objects.
pub fn compare(a: &Value, b: &Value) -> Ordering {
    fn rank(v: &Value) -> u8 {
        match v {
            Value::Null => 0,
            Value::Bool(false) => 1,
            Value::Bool(true) => 2,
            Value::Number(_) => 3,
            Value::String(_) => 4,
            Value::Array(_) => 5,
            Value::Object(_) => 6,
        }
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => x.cmp(&y),
            _ => match (x.as_u64(), y.as_u64()) {
                (Some(x), Some(y)) => x.cmp(&y),
                _ => as_f64(x).partial_cmp(&as_f64(y)).unwrap_or(Ordering::Equal),
            },
        },
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Array(x), Value::Array(y)) => {
            for (p, q) in x.iter().zip(y) {
                let o = compare(p, q);
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        (Value::Object(x), Value::Object(y)) => {
            let mut kx: Vec<&String> = x.keys().collect();
            let mut ky: Vec<&String> = y.keys().collect();
            kx.sort();
            ky.sort();
            match kx.cmp(&ky) {
                Ordering::Equal => {
                    for k in kx {
                        let o = compare(&x[k], &y[k]);
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    Ordering::Equal
                }
                o => o,
            }
        }
        _ => rank(a).cmp(&rank(b)),
    }
}

impl<'c> Run<'c> {
    fn tick(&mut self, n: u64) -> Result<(), Stop> {
        self.steps += n;
        if self.steps > STEP_BUDGET {
            return Err(Stop::OutOfBudget);
        }
        if self.steps & 0x3ff < n && (self.cancel)() {
            return Err(Stop::Cancelled);
        }
        Ok(())
    }

    fn eval<'a>(&mut self, e: &Expr, input: Cow<'a, Value>) -> Out<'a> {
        self.tick(1)?;
        match e {
            Expr::Identity => Ok(vec![input]),
            Expr::Literal(v) => Ok(vec![owned(v.clone())]),
            Expr::Recurse => {
                let mut out = Vec::new();
                match input {
                    Cow::Borrowed(v) => self.descend_borrowed(v, &mut out)?,
                    Cow::Owned(v) => self.descend_owned(&v, &mut out)?,
                }
                Ok(out)
            }
            Expr::Index(target, index) => {
                // A literal index (`.a`, `.[0]`) does not depend on the input, which saves cloning an owned input.
                if let Expr::Literal(key) = &**index {
                    let mut out = Vec::new();
                    for t in self.eval(target, input)? {
                        out.push(index_value(t, key)?);
                    }
                    return Ok(out);
                }
                let mut out = Vec::new();
                for key in self.eval(index, input.clone())? {
                    for t in self.eval(target, input.clone())? {
                        out.push(index_value(t, &key)?);
                    }
                }
                Ok(out)
            }
            Expr::Slice(target, from, to) => {
                let mut out = Vec::new();
                let bounds = |this: &mut Self, e: &Option<Box<Expr>>| -> Result<Vec<Option<Value>>, Stop> {
                    match e {
                        None => Ok(vec![None]),
                        Some(e) => Ok(this.eval(e, input.clone())?.into_iter().map(|v| Some(v.into_owned())).collect()),
                    }
                };
                let (froms, tos) = (bounds(self, from)?, bounds(self, to)?);
                for t in self.eval(target, input.clone())? {
                    for f in &froms {
                        for u in &tos {
                            out.push(owned(slice(&t, f.as_ref(), u.as_ref())?));
                        }
                    }
                }
                Ok(out)
            }
            Expr::Iterate(target) => {
                let mut out = Vec::new();
                for t in self.eval(target, input)? {
                    match t {
                        Cow::Borrowed(Value::Array(a)) => out.extend(a.iter().map(Cow::Borrowed)),
                        Cow::Borrowed(Value::Object(m)) => out.extend(m.values().map(Cow::Borrowed)),
                        Cow::Owned(Value::Array(a)) => out.extend(a.into_iter().map(owned)),
                        Cow::Owned(Value::Object(m)) => out.extend(m.into_iter().map(|(_, v)| owned(v))),
                        other => return fail(tf("Cannot iterate over {}", &[&typed(&other)])),
                    }
                    self.tick(out.len() as u64 / 64)?;
                }
                Ok(out)
            }
            Expr::Try(inner) => match self.eval(inner, input) {
                Err(Stop::Failed(_)) => Ok(Vec::new()),
                other => other,
            },
            Expr::Pipe(a, b) => {
                let mut out = Vec::new();
                for v in self.eval(a, input)? {
                    out.extend(self.eval(b, v)?);
                }
                Ok(out)
            }
            Expr::Comma(a, b) => {
                let mut out = self.eval(a, input.clone())?;
                out.extend(self.eval(b, input)?);
                Ok(out)
            }
            Expr::Neg(inner) => {
                let mut out = Vec::new();
                for v in self.eval(inner, input)? {
                    match &*v {
                        Value::Number(n) => out.push(owned(match n.as_i64().and_then(i64::checked_neg) {
                            Some(i) => Value::from(i),
                            None => num(-as_f64(n)),
                        })),
                        other => return fail(tf("{} cannot be negated", &[&typed(other)])),
                    }
                }
                Ok(out)
            }
            Expr::Binary(op, l, r) => {
                // jq runs the right operand as the outer loop: `(1,2) + (10,20)` is 11, 12, 21, 22.
                let mut out = Vec::new();
                for rv in self.eval(r, input.clone())? {
                    for lv in self.eval(l, input.clone())? {
                        self.tick(1)?;
                        out.push(owned(binary(*op, &lv, &rv)?));
                    }
                }
                Ok(out)
            }
            Expr::And(l, r) | Expr::Or(l, r) => {
                let is_and = matches!(e, Expr::And(..));
                let mut out = Vec::new();
                for lv in self.eval(l, input.clone())? {
                    if truthy(&lv) != is_and {
                        out.push(owned(Value::Bool(!is_and)));
                        continue;
                    }
                    for rv in self.eval(r, input.clone())? {
                        out.push(owned(Value::Bool(truthy(&rv))));
                    }
                }
                Ok(out)
            }
            Expr::Alt(l, r) => {
                // Errors on the left count as "no value", as in jq.
                let found: Vec<_> = match self.eval(l, input.clone()) {
                    Ok(v) => v.into_iter().filter(|v| truthy(v)).collect(),
                    Err(Stop::Failed(_)) => Vec::new(),
                    Err(other) => return Err(other),
                };
                if found.is_empty() { self.eval(r, input) } else { Ok(found) }
            }
            Expr::Array(None) => Ok(vec![owned(Value::Array(Vec::new()))]),
            Expr::Array(Some(inner)) => {
                let items = self.eval(inner, input)?.into_iter().map(Cow::into_owned).collect();
                Ok(vec![owned(Value::Array(items))])
            }
            Expr::Object(entries) => {
                let mut partial: Vec<Map<String, Value>> = vec![Map::new()];
                for (k, v) in entries {
                    let keys = self.eval(k, input.clone())?;
                    let values = self.eval(v, input.clone())?;
                    let mut next = Vec::with_capacity(partial.len() * keys.len() * values.len());
                    for m in &partial {
                        for key in &keys {
                            let Value::String(key) = &**key else {
                                return fail(tf("Object keys must be strings, not {}", &[&type_name(key)]));
                            };
                            for value in &values {
                                self.tick(1)?;
                                let mut m = m.clone();
                                m.insert(key.clone(), (**value).clone());
                                next.push(m);
                            }
                        }
                    }
                    partial = next;
                }
                Ok(partial.into_iter().map(|m| owned(Value::Object(m))).collect())
            }
            Expr::If(cond, then, otherwise) => {
                let mut out = Vec::new();
                for c in self.eval(cond, input.clone())? {
                    if truthy(&c) {
                        out.extend(self.eval(then, input.clone())?);
                    } else if let Some(o) = otherwise {
                        out.extend(self.eval(o, input.clone())?);
                    } else {
                        out.push(input.clone());
                    }
                }
                Ok(out)
            }
            Expr::Call(name, args) => self.call(name, args, input),
        }
    }

    fn descend_borrowed<'a>(&mut self, v: &'a Value, out: &mut Vec<Cow<'a, Value>>) -> Result<(), Stop> {
        // An explicit stack: documents nest deeper than a thread's stack is willing to recurse.
        let mut todo = vec![v];
        while let Some(v) = todo.pop() {
            self.tick(1)?;
            out.push(Cow::Borrowed(v));
            match v {
                Value::Array(a) => todo.extend(a.iter().rev()),
                Value::Object(m) => todo.extend(m.values().rev()),
                _ => {}
            }
        }
        Ok(())
    }

    fn descend_owned<'a>(&mut self, v: &Value, out: &mut Vec<Cow<'a, Value>>) -> Result<(), Stop> {
        let mut todo = vec![v];
        while let Some(v) = todo.pop() {
            self.tick(1)?;
            out.push(owned(v.clone()));
            match v {
                Value::Array(a) => todo.extend(a.iter().rev()),
                Value::Object(m) => todo.extend(m.values().rev()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Outputs of `e` as plain values.
    fn values(&mut self, e: &Expr, input: &Value) -> Result<Vec<Value>, Stop> {
        Ok(self.eval(e, Cow::Borrowed(input))?.into_iter().map(Cow::into_owned).collect())
    }

    fn call<'a>(&mut self, name: &str, args: &[Expr], input: Cow<'a, Value>) -> Out<'a> {
        let one = |v: Value| -> Out<'a> { Ok(vec![owned(v)]) };
        match (name, args) {
            ("empty", _) => Ok(Vec::new()),
            ("not", _) => one(Value::Bool(!truthy(&input))),
            ("type", _) => one(Value::String(type_name(&input).into())),
            ("length", _) => one(length(&input)?),
            ("keys" | "keys_unsorted", _) => match &*input {
                Value::Object(m) => {
                    let mut keys: Vec<&String> = m.keys().collect();
                    if name == "keys" {
                        keys.sort();
                    }
                    one(Value::Array(keys.into_iter().map(|k| Value::String(k.clone())).collect()))
                }
                Value::Array(a) => one(Value::Array((0..a.len()).map(Value::from).collect())),
                other => fail(tf("{} has no keys", &[&typed(other)])),
            },
            ("values", _) => Ok(if matches!(*input, Value::Null) { Vec::new() } else { vec![input] }),
            ("select", [f]) => {
                let mut out = Vec::new();
                for c in self.eval(f, input.clone())? {
                    if truthy(&c) {
                        out.push(input.clone());
                    }
                }
                Ok(out)
            }
            ("map", [f]) => {
                let mut items = Vec::new();
                for item in iterate(&input)? {
                    items.extend(self.eval(f, item)?.into_iter().map(Cow::into_owned));
                }
                one(Value::Array(items))
            }
            ("map_values", [f]) => match &*input {
                Value::Array(a) => {
                    let mut items = Vec::new();
                    for item in a {
                        if let Some(v) = self.eval(f, Cow::Borrowed(item))?.into_iter().next() {
                            items.push(v.into_owned());
                        }
                    }
                    one(Value::Array(items))
                }
                Value::Object(m) => {
                    let mut out = Map::new();
                    for (k, item) in m {
                        if let Some(v) = self.eval(f, Cow::Borrowed(item))?.into_iter().next() {
                            out.insert(k.clone(), v.into_owned());
                        }
                    }
                    one(Value::Object(out))
                }
                other => fail(tf("Cannot iterate over {}", &[&typed(other)])),
            },
            ("has", [k]) => {
                let mut out = Vec::new();
                for key in self.values(k, &input)? {
                    out.push(owned(Value::Bool(match (&*input, &key) {
                        (Value::Object(m), Value::String(s)) => m.contains_key(s),
                        (Value::Array(a), Value::Number(n)) => n.as_u64().is_some_and(|i| (i as usize) < a.len()),
                        (i, k) => return fail(tf("Cannot check whether {0} has a key of type {1}", &[&type_name(i), &type_name(k)])),
                    })));
                }
                Ok(out)
            }
            ("contains", [x]) => {
                let mut out = Vec::new();
                for needle in self.values(x, &input)? {
                    out.push(owned(Value::Bool(contains(&input, &needle)?)));
                }
                Ok(out)
            }
            ("add", _) => {
                let mut acc = Value::Null;
                for item in iterate(&input)? {
                    acc = binary(BinOp::Add, &acc, &item)?;
                    self.tick(1)?;
                }
                one(acc)
            }
            ("any" | "all", _) => {
                let want_any = name == "any";
                let mut result = !want_any;
                for item in iterate(&input)? {
                    let hits = match args.first() {
                        Some(f) => self.eval(f, item)?.iter().any(|v| truthy(v)),
                        None => truthy(&item),
                    };
                    if hits == want_any {
                        result = want_any;
                        break;
                    }
                }
                one(Value::Bool(result))
            }
            ("first", []) => one(index_value(input, &Value::from(0))?.into_owned()),
            ("last", _) => one(index_value(input, &Value::from(-1))?.into_owned()),
            ("first", [f]) => Ok(self.eval(f, input)?.into_iter().take(1).collect()),
            ("limit", [n, f]) => {
                let mut out = Vec::new();
                for n in self.values(n, &input)? {
                    let Some(n) = n.as_i64() else { return fail(t("limit needs a whole number").to_string()) };
                    out.extend(self.eval(f, input.clone())?.into_iter().take(n.max(0) as usize));
                }
                Ok(out)
            }
            ("sort", _) => {
                let mut items = array_of(&input)?;
                items.sort_by(compare);
                one(Value::Array(items))
            }
            ("sort_by" | "group_by" | "unique_by", [f]) => {
                let items = array_of(&input)?;
                let mut keyed = Vec::with_capacity(items.len());
                for item in items {
                    let key = Value::Array(self.eval(f, Cow::Borrowed(&item))?.into_iter().map(Cow::into_owned).collect());
                    keyed.push((key, item));
                }
                keyed.sort_by(|a, b| compare(&a.0, &b.0));
                match name {
                    "sort_by" => one(Value::Array(keyed.into_iter().map(|(_, v)| v).collect())),
                    "unique_by" => {
                        keyed.dedup_by(|a, b| compare(&a.0, &b.0) == Ordering::Equal);
                        one(Value::Array(keyed.into_iter().map(|(_, v)| v).collect()))
                    }
                    _ => {
                        let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
                        for (k, v) in keyed {
                            match groups.last_mut() {
                                Some((last, items)) if compare(last, &k) == Ordering::Equal => items.push(v),
                                _ => groups.push((k, vec![v])),
                            }
                        }
                        one(Value::Array(groups.into_iter().map(|(_, items)| Value::Array(items)).collect()))
                    }
                }
            }
            ("unique", _) => {
                let mut items = array_of(&input)?;
                items.sort_by(compare);
                items.dedup_by(|a, b| compare(a, b) == Ordering::Equal);
                one(Value::Array(items))
            }
            ("reverse", _) => match &*input {
                Value::Null => one(Value::Array(Vec::new())),
                Value::String(s) => one(Value::String(s.chars().rev().collect())),
                _ => {
                    let mut items = array_of(&input)?;
                    items.reverse();
                    one(Value::Array(items))
                }
            },
            ("min" | "max", _) => {
                let items = array_of(&input)?;
                let best = if name == "min" { items.into_iter().min_by(compare) } else { items.into_iter().max_by(compare) };
                one(best.unwrap_or(Value::Null))
            }
            ("flatten", _) => {
                let depth = match args.first() {
                    Some(d) => match self.values(d, &input)?.first().and_then(Value::as_i64) {
                        Some(d) if d >= 0 => d as usize,
                        _ => return fail(t("flatten depth must not be negative").to_string()),
                    },
                    None => usize::MAX,
                };
                let items = array_of(&input)?;
                one(Value::Array(flatten(items, depth)))
            }
            ("to_entries", _) => match &*input {
                Value::Object(m) => {
                    let entries = m
                        .iter()
                        .map(|(k, v)| {
                            let mut e = Map::new();
                            e.insert("key".into(), Value::String(k.clone()));
                            e.insert("value".into(), v.clone());
                            Value::Object(e)
                        })
                        .collect();
                    one(Value::Array(entries))
                }
                other => fail(tf("{} has no keys", &[&typed(other)])),
            },
            ("from_entries", _) => one(from_entries(&input)?),
            ("with_entries", [f]) => {
                let Value::Object(m) = &*input else { return fail(tf("{} has no keys", &[&typed(&input)])) };
                let mut entries = Vec::new();
                for (k, v) in m {
                    let mut e = Map::new();
                    e.insert("key".into(), Value::String(k.clone()));
                    e.insert("value".into(), v.clone());
                    entries.extend(self.eval(f, owned(Value::Object(e)))?.into_iter().map(Cow::into_owned));
                }
                one(from_entries(&Value::Array(entries))?)
            }
            ("tostring", _) => match &*input {
                Value::String(_) => Ok(vec![input]),
                other => one(Value::String(other.to_string())),
            },
            ("tojson", _) => one(Value::String(input.to_string())),
            ("fromjson", _) => match &*input {
                Value::String(s) => match serde_json::from_str::<Value>(s) {
                    Ok(v) => one(v),
                    Err(e) => fail(tf("{0} (while parsing {1})", &[&e, &show(&input)])),
                },
                other => fail(tf("{} cannot be parsed as JSON", &[&typed(other)])),
            },
            ("tonumber", _) => match &*input {
                Value::Number(_) => Ok(vec![input]),
                Value::String(s) => {
                    match s.trim().parse::<i64>().map(Value::from).ok().or_else(|| s.trim().parse::<f64>().ok().filter(|f| f.is_finite()).map(num)) {
                        Some(v) => one(v),
                        None => fail(tf("Cannot parse {} as a number", &[&show(&input)])),
                    }
                }
                other => fail(tf("{} cannot be parsed as a number", &[&typed(other)])),
            },
            ("ascii_downcase" | "ascii_upcase", _) => match &*input {
                Value::String(s) => one(Value::String(if name == "ascii_downcase" { s.to_ascii_lowercase() } else { s.to_ascii_uppercase() })),
                other => fail(tf("{0} cannot be used with {1}", &[&name, &typed(other)])),
            },
            ("startswith" | "endswith" | "ltrimstr" | "rtrimstr", [a]) => {
                let mut out = Vec::new();
                for arg in self.values(a, &input)? {
                    out.push(owned(match (&*input, &arg) {
                        (Value::String(s), Value::String(p)) => match name {
                            "startswith" => Value::Bool(s.starts_with(p.as_str())),
                            "endswith" => Value::Bool(s.ends_with(p.as_str())),
                            "ltrimstr" => Value::String(s.strip_prefix(p.as_str()).unwrap_or(s).to_string()),
                            _ => Value::String(s.strip_suffix(p.as_str()).unwrap_or(s).to_string()),
                        },
                        // jq leaves other inputs alone for the trim functions and complains for the tests.
                        (other, _) if name.ends_with("trimstr") => other.clone(),
                        _ => return fail(tf("{0} needs strings, got {1} and {2}", &[&name, &type_name(&input), &type_name(&arg)])),
                    }));
                }
                Ok(out)
            }
            ("join", [sep]) => {
                let mut out = Vec::new();
                for sep in self.values(sep, &input)? {
                    let Value::String(sep) = &sep else { return fail(tf("join needs a string separator, got {}", &[&type_name(&sep)])) };
                    let mut parts = Vec::new();
                    for item in iterate(&input)? {
                        parts.push(match &*item {
                            Value::Null => String::new(),
                            Value::String(s) => s.clone(),
                            v @ (Value::Bool(_) | Value::Number(_)) => v.to_string(),
                            other => return fail(tf("Cannot join {}", &[&typed(other)])),
                        });
                    }
                    out.push(owned(Value::String(parts.join(sep))));
                }
                Ok(out)
            }
            ("split", [sep]) => {
                let mut out = Vec::new();
                for sep in self.values(sep, &input)? {
                    match (&*input, &sep) {
                        (Value::String(s), Value::String(sep)) => {
                            let parts: Vec<Value> = if sep.is_empty() {
                                s.chars().map(|c| Value::String(c.to_string())).collect()
                            } else {
                                s.split(sep.as_str()).map(|p| Value::String(p.to_string())).collect()
                            };
                            out.push(owned(Value::Array(parts)));
                        }
                        _ => return fail(t("split input and separator must be strings").to_string()),
                    }
                }
                Ok(out)
            }
            ("floor", _) => match &*input {
                Value::Number(n) => one(num(as_f64(n).floor())),
                other => fail(tf("{} cannot be rounded", &[&typed(other)])),
            },
            ("recurse", _) => self.eval(&Expr::Recurse, input),
            _ => fail(tf("“{}” is not a function this query language has", &[&name])),
        }
    }
}

/// Elements of an array or values of an object, borrowed from `v`.
fn iterate(v: &Value) -> Result<Vec<Cow<'_, Value>>, Stop> {
    match v {
        Value::Array(a) => Ok(a.iter().map(Cow::Borrowed).collect()),
        Value::Object(m) => Ok(m.values().map(Cow::Borrowed).collect()),
        other => fail(tf("Cannot iterate over {}", &[&typed(other)])),
    }
}

fn array_of(v: &Value) -> Result<Vec<Value>, Stop> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        other => fail(tf("{} is not an array", &[&typed(other)])),
    }
}

fn flatten(items: Vec<Value>, depth: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::Array(inner) if depth > 0 => out.extend(flatten(inner, depth - 1)),
            other => out.push(other),
        }
    }
    out
}

fn from_entries(v: &Value) -> Result<Value, Stop> {
    let Value::Array(items) = v else { return fail(tf("Cannot iterate over {}", &[&typed(v)])) };
    let mut out = Map::new();
    for item in items {
        let Value::Object(e) = item else { return fail(tf("Cannot use {} as an entry", &[&typed(item)])) };
        let pick = |names: &[&str]| names.iter().find_map(|n| e.get(*n)).cloned().unwrap_or(Value::Null);
        let key = match pick(&["key", "k", "name"]) {
            Value::String(s) => s,
            Value::Null => "null".into(),
            v @ (Value::Bool(_) | Value::Number(_)) => v.to_string(),
            other => return fail(tf("Cannot use {} as an object key", &[&typed(&other)])),
        };
        out.insert(key, pick(&["value", "v"]));
    }
    Ok(Value::Object(out))
}

fn length(v: &Value) -> Result<Value, Stop> {
    Ok(match v {
        Value::Null => Value::from(0),
        Value::Bool(_) => return fail(tf("{} has no length", &[&typed(v)])),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Value::from(i.unsigned_abs()),
            None => num(as_f64(n).abs()),
        },
        Value::String(s) => Value::from(s.chars().count()),
        Value::Array(a) => Value::from(a.len()),
        Value::Object(m) => Value::from(m.len()),
    })
}

fn contains(a: &Value, b: &Value) -> Result<bool, Stop> {
    Ok(match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut all = true;
            for (k, bv) in y {
                all &= match x.get(k) {
                    Some(av) => contains(av, bv)?,
                    None => false,
                };
            }
            all
        }
        (Value::Array(x), Value::Array(y)) => {
            let mut all = true;
            for bv in y {
                let mut any = false;
                for av in x {
                    any |= contains(av, bv).unwrap_or(false);
                }
                all &= any;
            }
            all
        }
        (Value::String(x), Value::String(y)) => x.contains(y.as_str()),
        _ if type_name(a) == type_name(b) => a == b,
        _ => return fail(tf("{0} and {1} cannot have their containment checked", &[&typed(a), &typed(b)])),
    })
}

/// Position of `n` in an array of `len` elements (negative counts from the end), if it is inside.
fn array_slot(len: usize, n: &Number) -> Option<usize> {
    let mut i = n.as_f64()?.floor() as i64;
    if i < 0 {
        i += len as i64;
    }
    usize::try_from(i).ok().filter(|i| *i < len)
}

/// `t[key]`: object member, array element or null where jq says null.
fn index_value<'a>(t: Cow<'a, Value>, key: &Value) -> Result<Cow<'a, Value>, Stop> {
    match (t, key) {
        (Cow::Borrowed(Value::Object(m)), Value::String(k)) => Ok(m.get(k).map_or_else(|| owned(Value::Null), Cow::Borrowed)),
        (Cow::Owned(Value::Object(m)), Value::String(k)) => Ok(owned(m.get(k).cloned().unwrap_or(Value::Null))),
        (Cow::Borrowed(Value::Array(a)), Value::Number(n)) => Ok(array_slot(a.len(), n).map_or_else(|| owned(Value::Null), |i| Cow::Borrowed(&a[i]))),
        (Cow::Owned(Value::Array(mut a)), Value::Number(n)) => Ok(owned(array_slot(a.len(), n).map_or(Value::Null, |i| a.swap_remove(i)))),
        (t, key) => match (&*t, key) {
            (Value::Null, Value::String(_) | Value::Number(_) | Value::Null) => Ok(owned(Value::Null)),
            (v @ (Value::Object(_) | Value::Array(_)), other) => fail(tf("Cannot index {0} with {1}", &[&type_name(v), &type_name(other)])),
            (v, Value::String(k)) => fail(tf("Cannot index {0} with “{1}”", &[&type_name(v), &k])),
            (v, other) => fail(tf("Cannot index {0} with {1}", &[&type_name(v), &type_name(other)])),
        },
    }
}

fn slice(v: &Value, from: Option<&Value>, to: Option<&Value>) -> Result<Value, Stop> {
    let bound = |b: Option<&Value>, default: i64, len: i64| -> Result<i64, Stop> {
        match b {
            None | Some(Value::Null) => Ok(default),
            Some(Value::Number(n)) => {
                let f = as_f64(n);
                let i = if f < 0.0 { f + len as f64 } else { f };
                Ok(i.clamp(0.0, len as f64) as i64)
            }
            Some(_) => fail(t("Start and end of a slice must be numbers").to_string()),
        }
    };
    match v {
        Value::Null => Ok(Value::Null),
        Value::Array(a) => {
            let len = a.len() as i64;
            let (s, e) = (bound(from, 0, len)?, bound(to, len, len)?);
            Ok(Value::Array(if s < e { a[s as usize..e as usize].to_vec() } else { Vec::new() }))
        }
        Value::String(s) => {
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let (a, b) = (bound(from, 0, len)?, bound(to, len, len)?);
            Ok(Value::String(if a < b { chars[a as usize..b as usize].iter().collect() } else { String::new() }))
        }
        other => fail(tf("Cannot index {} with object", &[&type_name(other)])),
    }
}

fn binary(op: BinOp, l: &Value, r: &Value) -> Result<Value, Stop> {
    match op {
        BinOp::Eq => return Ok(Value::Bool(compare(l, r) == Ordering::Equal)),
        BinOp::Ne => return Ok(Value::Bool(compare(l, r) != Ordering::Equal)),
        BinOp::Lt => return Ok(Value::Bool(compare(l, r) == Ordering::Less)),
        BinOp::Le => return Ok(Value::Bool(compare(l, r) != Ordering::Greater)),
        BinOp::Gt => return Ok(Value::Bool(compare(l, r) == Ordering::Greater)),
        BinOp::Ge => return Ok(Value::Bool(compare(l, r) != Ordering::Less)),
        _ => {}
    }
    match (op, l, r) {
        (BinOp::Add, Value::Null, x) | (BinOp::Add, x, Value::Null) => Ok(x.clone()),
        (BinOp::Add, Value::Number(a), Value::Number(b)) => Ok(match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) => a.checked_add(b).map_or_else(|| num(a as f64 + b as f64), Value::from),
            _ => num(as_f64(a) + as_f64(b)),
        }),
        (BinOp::Add, Value::String(a), Value::String(b)) => Ok(Value::String(format!("{a}{b}"))),
        (BinOp::Add, Value::Array(a), Value::Array(b)) => Ok(Value::Array(a.iter().chain(b).cloned().collect())),
        (BinOp::Add, Value::Object(a), Value::Object(b)) => {
            let mut m = a.clone();
            for (k, v) in b {
                m.insert(k.clone(), v.clone());
            }
            Ok(Value::Object(m))
        }
        (BinOp::Add, ..) => fail(tf("{0} and {1} cannot be added", &[&typed(l), &typed(r)])),
        (BinOp::Sub, Value::Number(a), Value::Number(b)) => Ok(match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) => a.checked_sub(b).map_or_else(|| num(a as f64 - b as f64), Value::from),
            _ => num(as_f64(a) - as_f64(b)),
        }),
        (BinOp::Sub, Value::Array(a), Value::Array(b)) => {
            Ok(Value::Array(a.iter().filter(|x| !b.iter().any(|y| compare(x, y) == Ordering::Equal)).cloned().collect()))
        }
        (BinOp::Sub, ..) => fail(tf("{0} and {1} cannot be subtracted", &[&typed(l), &typed(r)])),
        (BinOp::Mul, Value::Number(a), Value::Number(b)) => Ok(match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) => a.checked_mul(b).map_or_else(|| num(a as f64 * b as f64), Value::from),
            _ => num(as_f64(a) * as_f64(b)),
        }),
        (BinOp::Mul, Value::String(s), Value::Number(n)) | (BinOp::Mul, Value::Number(n), Value::String(s)) => {
            let times = as_f64(n);
            if times <= 0.0 {
                return Ok(Value::Null);
            }
            let times = times.ceil() as usize;
            if s.len().saturating_mul(times) > MAX_REPEAT {
                return fail(t("The repeated string would be too long").to_string());
            }
            Ok(Value::String(s.repeat(times)))
        }
        (BinOp::Mul, Value::Object(a), Value::Object(b)) => Ok(deep_merge(a, b)),
        (BinOp::Mul, ..) => fail(tf("{0} and {1} cannot be multiplied", &[&typed(l), &typed(r)])),
        (BinOp::Div, Value::Number(a), Value::Number(b)) => {
            if as_f64(b) == 0.0 {
                return fail(tf("{0} and {1} cannot be divided because the divisor is zero", &[&typed(l), &typed(r)]));
            }
            Ok(num(as_f64(a) / as_f64(b)))
        }
        (BinOp::Div, Value::String(a), Value::String(b)) => {
            let parts: Vec<Value> = if b.is_empty() {
                a.chars().map(|c| Value::String(c.to_string())).collect()
            } else {
                a.split(b.as_str()).map(|p| Value::String(p.into())).collect()
            };
            Ok(Value::Array(parts))
        }
        (BinOp::Div, ..) => fail(tf("{0} and {1} cannot be divided", &[&typed(l), &typed(r)])),
        (BinOp::Rem, Value::Number(a), Value::Number(b)) => {
            let (x, y) = (as_f64(a) as i64, as_f64(b) as i64);
            if y == 0 {
                return fail(tf("{0} and {1} cannot be divided because the divisor is zero", &[&typed(l), &typed(r)]));
            }
            // Like jq the result takes the sign of the dividend; `wrapping_rem` keeps `i64::MIN % -1` from panicking.
            Ok(Value::from(x.wrapping_rem(y)))
        }
        (BinOp::Rem, ..) => fail(tf("{0} and {1} cannot be divided", &[&typed(l), &typed(r)])),
        _ => unreachable!("comparison operators return above"),
    }
}

fn deep_merge(a: &Map<String, Value>, b: &Map<String, Value>) -> Value {
    let mut m = a.clone();
    for (k, v) in b {
        let merged = match (m.get(k), v) {
            (Some(Value::Object(x)), Value::Object(y)) => deep_merge(x, y),
            _ => v.clone(),
        };
        m.insert(k.clone(), merged);
    }
    Value::Object(m)
}
