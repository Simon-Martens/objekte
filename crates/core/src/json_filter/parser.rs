//! Parser for the bounded JSON filter expression language.
use super::*;

const MAX_LENGTH: usize = 16 * 1024;
const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 32;

pub(super) fn parse(text: &str, parameters: &BTreeMap<String, Value>) -> Result<Expr> {
    if text.len() > MAX_LENGTH {
        return Err(Error::Request("filter exceeds 16384 bytes".into()));
    }
    let mut p = Parser {
        text,
        pos: 0,
        parameters,
        nodes: 0,
    };
    let expr = p.or(0)?;
    p.ws();
    if p.pos != text.len() {
        return p.error("unexpected token");
    }
    Ok(expr)
}


// INFO: Filter text cursor, parameter bindings, and expression node count.
struct Parser<'a> {
    text: &'a str,
    pos: usize,
    parameters: &'a BTreeMap<String, Value>,
    nodes: usize,
}


impl Parser<'_> {
    fn error<T>(&self, message: &str) -> Result<T> {
        Err(Error::Request(format!(
            "filter at byte {}: {message}",
            self.pos
        )))
    }

    fn ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn take(&mut self, s: &str) -> bool {
        self.ws();
        if self.text[self.pos..].starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn need(&mut self, s: &str) -> Result<()> {
        if self.take(s) {
            Ok(())
        } else {
            self.error(&format!("expected {s}"))
        }
    }

    fn or(&mut self, depth: usize) -> Result<Expr> {
        let mut es = vec![self.and(depth)?];
        while self.take("||") {
            es.push(self.and(depth)?);
        }
        Ok(if es.len() == 1 {
            es.pop().unwrap()
        } else {
            Expr::Or(es)
        })
    }

    fn and(&mut self, depth: usize) -> Result<Expr> {
        let mut es = vec![self.atom(depth)?];
        while self.take("&&") {
            es.push(self.atom(depth)?);
        }
        Ok(if es.len() == 1 {
            es.pop().unwrap()
        } else {
            Expr::And(es)
        })
    }

    fn atom(&mut self, depth: usize) -> Result<Expr> {
        self.nodes += 1;
        if depth >= MAX_DEPTH || self.nodes > MAX_NODES {
            return self.error("filter nesting/expression limit exceeded");
        }
        if self.take("(") {
            let e = self.or(depth + 1)?;
            self.need(")")?;
            return Ok(e);
        }
        self.ws();
        let saved = self.pos;
        if self.text[self.pos..].starts_with("exists") {
            self.pos += 6;
            if self.take("(") {
                let path = self.path()?;
                self.need(")")?;
                return Ok(Expr::Exists(path));
            }
            self.pos = saved;
        }
        let left = self.operand()?;
        let any = self.take("?");
        let op = [
            ("!=", Op::Ne),
            ("!~", Op::NotContains),
            (">=", Op::Ge),
            ("<=", Op::Le),
            ("==", Op::Eq),
            ("=", Op::Eq),
            (">", Op::Gt),
            ("<", Op::Lt),
            ("~", Op::Contains),
        ]
        .into_iter()
        .find_map(|(s, op)| self.take(s).then_some(op));
        let Some(op) = op else {
            return self.error("expected comparison operator");
        };
        let right = self.operand()?;
        for operand in [&left, &right] {
            if matches!(op, Op::Contains | Op::NotContains)
                && matches!(operand, Operand::Literal(v) if !v.is_string())
            {
                return self.error("pattern comparison requires strings");
            }
        }
        Ok(Expr::Compare {
            left,
            op,
            any,
            right,
        })
    }

    fn ident(&mut self) -> Result<String> {
        self.ws();
        let start = self.pos;
        if !self.peek().is_some_and(|c| c == '_' || c.is_alphabetic()) {
            return self.error(
                "expected a JSON field or parameter name (metadata fields are not supported)",
            );
        }
        self.bump();
        while self.peek().is_some_and(|c| c == '_' || c.is_alphanumeric()) {
            self.bump();
        }
        Ok(self.text[start..self.pos].into())
    }

    fn path(&mut self) -> Result<JsonPath> {
        self.ws();
        let mut out = Vec::new();
        if self.peek() != Some('[') {
            out.push(JsonPathStep::Key(self.ident()?));
        }
        loop {
            if self.take("[") {
                self.ws();
                if matches!(self.peek(), Some('\'' | '"')) {
                    out.push(JsonPathStep::Key(self.string()?));
                } else if self.take("*") {
                    out.push(JsonPathStep::Each);
                } else {
                    let start = self.pos;
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.bump();
                    }
                    let i = self.text[start..self.pos].parse::<usize>().map_err(|_| {
                        Error::Request(format!(
                            "filter at byte {start}: expected quoted key, array index, or *"
                        ))
                    })?;
                    out.push(JsonPathStep::Index(i));
                }
                self.need("]")?;
            } else if self.take(".") {
                out.push(JsonPathStep::Key(self.ident()?));
            } else {
                break;
            }
            if out.len() > MAX_DEPTH {
                return self.error("path exceeds 32 steps");
            }
        }
        if out.is_empty() {
            return self.error("expected JSON field path");
        }
        Ok(out)
    }

    fn string(&mut self) -> Result<String> {
        self.ws();
        let quote = self.bump().unwrap();
        let mut json = String::from("\"");
        loop {
            let Some(c) = self.bump() else {
                return self.error("unterminated string");
            };
            if c == quote {
                break;
            }
            if c == '\\' {
                let Some(next) = self.bump() else {
                    return self.error("unterminated escape");
                };
                if next == '\'' {
                    json.push('\'');
                } else {
                    json.push('\\');
                    json.push(next);
                }
            } else {
                if c == '"' {
                    json.push('\\');
                }
                json.push(c);
            }
        }
        json.push('"');
        let value = crate::json::parse(&json)
            .map_err(|e| Error::Request(format!("filter at byte {}: {e}", self.pos)))?;
        Ok(value.as_str().unwrap().into())
    }

    fn operand(&mut self) -> Result<Operand> {
        self.ws();
        if self.take("$") {
            let key = self.ident()?;
            let value = self
                .parameters
                .get(&key)
                .cloned()
                .ok_or_else(|| Error::Request(format!("missing filter parameter ${key}")))?;
            return self.literal(value);
        }
        if matches!(self.peek(), Some('\'' | '"')) {
            return self.string().map(|s| Operand::Literal(Value::String(s)));
        }
        for (literal, value) in [
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("null", Value::Null),
        ] {
            if self.text[self.pos..].starts_with(literal)
                && self.text[self.pos + literal.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric() && !matches!(c, '_' | '.' | '[' | ':'))
            {
                self.pos += literal.len();
                return Ok(Operand::Literal(value));
            }
        }
        if self
            .peek()
            .is_some_and(|c| c == '[' || c == '_' || c.is_alphabetic())
        {
            let path = self.path()?;
            let modifier = if self.take(":") {
                Some(match self.ident()?.as_str() {
                    "length" => Modifier::Length,
                    "each" => Modifier::Each,
                    "lower" => Modifier::Lower,
                    _ => return self.error("unknown modifier; expected :length, :each, or :lower"),
                })
            } else {
                None
            };
            return Ok(Operand::Field(FieldOperand { path, modifier }));
        }
        let start = self.pos;
        while self.peek().is_some_and(|c| {
            !c.is_whitespace() && !matches!(c, ')' | '&' | '|' | '<' | '>' | '=' | '!' | '~' | '?')
        }) {
            self.bump();
        }
        let raw = &self.text[start..self.pos];
        let value = crate::json::parse(raw).map_err(|_| Error::Request(format!("filter at byte {start}: expected quoted string, number, true, false, null, or $parameter")))?;
        self.literal(value)
    }

    fn literal(&self, value: Value) -> Result<Operand> {
        if matches!(value, Value::Array(_) | Value::Object(_)) {
            self.error("comparison value must be a JSON scalar")
        } else {
            Ok(Operand::Literal(value))
        }
    }
}
