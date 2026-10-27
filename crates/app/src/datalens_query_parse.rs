//! The datalens query language's parser: query text to an `Expr` tree, with the nesting and size limits that keep the
//! evaluator's recursion bounded.

use super::*;

struct Parser<'a> {
    src: &'a str,
    i: usize,
    nesting: usize,
    nodes: usize,
}

type Parsed = Result<Expr, QueryError>;

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
