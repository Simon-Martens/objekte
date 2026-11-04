//! Parser for the bounded JSON filter expression language.
use super::*;

const MAX_LENGTH: usize = 16 * 1024;
const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 32;

pub(super) fn parse(text: &str, parameters: &BTreeMap<String, Value>) -> Result<Expr> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as i128);
    parse_at(text, parameters, now)
}

/// Parse with `@now` macros evaluated at `now` (Unix nanoseconds).
pub(super) fn parse_at(
    text: &str,
    parameters: &BTreeMap<String, Value>,
    now: i128,
) -> Result<Expr> {
    if text.len() > MAX_LENGTH {
        return Err(Error::Request("filter exceeds 16384 bytes".into()));
    }
    let mut p = Parser {
        text,
        pos: 0,
        parameters,
        nodes: 0,
        now,
    };
    let expr = p.or(0)?;
    p.ws();
    if p.pos != text.len() {
        return p.error("unexpected token");
    }
    Ok(expr)
}

pub(super) fn parse_path(text: &str) -> Result<JsonPath> {
    if text.len() > MAX_LENGTH {
        return Err(Error::Request("path exceeds 16384 bytes".into()));
    }
    let parameters = BTreeMap::new();
    let mut parser = Parser {
        text,
        pos: 0,
        parameters: &parameters,
        nodes: 0,
        now: 0,
    };
    let path = parser.path()?;
    parser.ws();
    if parser.pos != text.len() {
        return parser.error("unexpected token in path");
    }
    Ok(path)
}


// INFO: Filter text cursor, parameter bindings, and expression node count.
struct Parser<'a> {
    text: &'a str,
    pos: usize,
    parameters: &'a BTreeMap<String, Value>,
    nodes: usize,
    /// Unix nanoseconds that `@now` macros resolve against.
    now: i128,
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
        if self.text[self.pos..].starts_with("missing") {
            self.pos += 7;
            if self.take("(") {
                let path = self.path()?;
                self.need(")")?;
                return Ok(Expr::Missing(path));
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
        let ignore_case = self.take(":");
        let right = if ignore_case {
            if !matches!(op, Op::Contains | Op::NotContains) {
                return self.error(":ignorecase is only valid with ~ and !~");
            }
            if self.ident()? != "ignorecase" {
                return self.error("expected :ignorecase(pattern)");
            }
            self.need("(")?;
            let operand = self.operand()?;
            self.need(")")?;
            operand
        } else {
            self.operand()?
        };
        for operand in [&left, &right] {
            if matches!(op, Op::Contains | Op::NotContains)
                && matches!(operand, Operand::Literal(v) if !v.is_string())
            {
                return self.error("pattern comparison requires strings");
            }
        }
        for (special, other) in [(&left, &right), (&right, &left)] {
            match special {
                Operand::Temporal(_)
                    if !matches!(
                        other,
                        Operand::Field(FieldOperand {
                            metadata: None | Some(MetadataField::Modified | MetadataField::Created),
                            ..
                        })
                    ) =>
                {
                    return self
                        .error("date/time literals require a JSON field, @modified, or @created");
                }
                Operand::Temporal(_) if matches!(op, Op::Contains | Op::NotContains) => {
                    return self.error("~ cannot compare date/time literals");
                }
                Operand::SizeBytes(_)
                    if !matches!(
                        other,
                        Operand::Field(FieldOperand {
                            metadata: Some(MetadataField::Size),
                            ..
                        })
                    ) =>
                {
                    return self.error("byte-size units require @size");
                }
                _ => {}
            }
        }
        if matches!(op, Op::Contains | Op::NotContains)
            && [&left, &right].iter().any(|operand| {
                matches!(
                    operand,
                    Operand::Field(FieldOperand {
                        metadata: Some(
                            MetadataField::Size | MetadataField::Modified | MetadataField::Created
                        ),
                        ..
                    })
                )
            })
        {
            return self.error("~ is only valid for string metadata fields");
        }
        // A rendered (`:fts`) field reads without punctuation, so a string
        // compared with it is normalized the same way.
        let right = match (&left, right) {
            (
                Operand::Field(FieldOperand {
                    modifier: Some(Modifier::Fts | Modifier::Rendered(_) | Modifier::Normalized),
                    ..
                }),
                Operand::Literal(Value::String(text)),
            ) => Operand::Literal(Value::String(
                if matches!(op, Op::Contains | Op::NotContains) {
                    super::pattern::normalize_rendered_like(&text)
                } else {
                    crate::text::render_text(&text, crate::TextFormat::Plain).unwrap_or(text)
                },
            )),
            (_, right) => right,
        };
        Ok(Expr::Compare {
            left,
            op,
            any,
            right,
            ignore_case,
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
        if self.take("@") {
            let name = self.ident()?;
            if let Some(operand) = super::temporal::macro_operand(&name, self.now) {
                return Ok(operand);
            }
            let Some(field) = MetadataField::parse(&name) else {
                return self.error("unknown metadata field or @now macro");
            };
            return Ok(Operand::Field(FieldOperand {
                path: Vec::new(),
                modifier: None,
                metadata: Some(field),
            }));
        }
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
                    "fts" => Modifier::Fts,
                    _ => {
                        return self
                            .error("unknown modifier; expected :length, :each, :lower, or :fts");
                    }
                })
            } else {
                None
            };
            return Ok(Operand::Field(FieldOperand {
                path,
                modifier,
                metadata: None,
            }));
        }
        let start = self.pos;
        while self.peek().is_some_and(|c| {
            !c.is_whitespace() && !matches!(c, ')' | '&' | '|' | '<' | '>' | '=' | '!' | '~' | '?')
        }) {
            self.bump();
        }
        let raw = &self.text[start..self.pos];
        if raw.len() >= 7 && raw.as_bytes().get(4) == Some(&b'-') {
            return Ok(Operand::Temporal(super::temporal::parse(raw)?));
        }
        if let Ok(value) = crate::json::parse(raw) {
            return self.literal(value);
        }
        if let Some(bytes) = size_literal(raw)? {
            return Ok(Operand::SizeBytes(bytes));
        }
        Err(Error::Request(format!(
            "filter at byte {start}: expected quoted string, number, date/time, byte size, true, false, null, or $parameter"
        )))
    }

    fn literal(&self, value: Value) -> Result<Operand> {
        if matches!(value, Value::Array(_) | Value::Object(_)) {
            self.error("comparison value must be a JSON scalar")
        } else {
            Ok(Operand::Literal(value))
        }
    }
}


fn size_literal(raw: &str) -> Result<Option<u64>> {
    let Some(split) = raw.find(|ch: char| ch.is_ascii_alphabetic()) else {
        return Ok(None);
    };
    let (number, unit) = raw.split_at(split);
    let factor: u128 = match unit {
        "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1_024,
        "MiB" => 1_048_576,
        "GiB" => 1_073_741_824,
        "TiB" => 1_099_511_627_776,
        _ => return Err(Error::Request(format!("unknown byte-size unit {unit}"))),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 18
        || (number.contains('.') && fraction.is_empty())
    {
        return Err(Error::Request("invalid byte-size literal".into()));
    }
    let whole = whole
        .parse::<u128>()
        .map_err(|_| Error::Request("byte-size literal is too large".into()))?;
    let divisor = 10_u128.pow(fraction.len() as u32);
    let fractional = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u128>()
            .map_err(|_| Error::Request("invalid byte-size literal".into()))?
    };
    let bytes = whole
        .checked_mul(factor)
        .and_then(|value| {
            fractional.checked_mul(factor).and_then(|extra| {
                extra
                    .checked_div(divisor)
                    .and_then(|extra| value.checked_add(extra))
            })
        })
        .ok_or_else(|| Error::Request("byte-size literal is too large".into()))?;
    if !(fractional * factor).is_multiple_of(divisor) {
        return Err(Error::Request(
            "byte-size literal must resolve to whole bytes".into(),
        ));
    }
    Ok(Some(u64::try_from(bytes).map_err(|_| {
        Error::Request("byte-size literal exceeds u64 bytes".into())
    })?))
}
