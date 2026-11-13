//! Separate literal FTS and structured FILTER parsing.

use crate::byte_size_literal::parse_size_literal;
use crate::clock_context::ClockContext;
use crate::model::{FieldPath, Number, Value};
use crate::query_path::{QueryPath, split_path_binding};
use crate::temporal_query::{PeriodLiteral, looks_like_temporal};
use crate::workload::{ReturnMode, SortExpression, SortField, SortReducer, WorkloadCase};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;


/// Exact case behavior for a full-text request. The index alphabet is separate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseSensitivity {
    #[default]
    Sensitive,
    Insensitive,
}


impl CaseSensitivity {
    pub fn is_sensitive(&self) -> bool {
        *self == Self::Sensitive
    }
}


/// Explicit FTS interpretation; literal search remains the default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FtsMode {
    #[default]
    Literal,
    Regex(RegexOptions),
}


impl FtsMode {
    pub fn is_literal(&self) -> bool {
        *self == Self::Literal
    }
}


/// Regex flags supported independently of FTS case sensitivity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RegexOptions {
    pub multi_line: bool,
    pub dot_matches_new_line: bool,
}


#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Literal {
    String(String),
    Number(Number),
    Bool(bool),
    Null,
    Typed(Value),
}


/// Field used as a comparison operand, with its original source span.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct FieldExpression {
    pub path: QueryPath,
    pub modifiers: Vec<Modifier>,
    pub span: (usize, usize),
}


/// Parsed right-hand operand before type checking and execution binding.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Operand {
    Literal(Literal),
    Period(PeriodLiteral),
    Macro { name: String, span: (usize, usize) },
    Field(FieldExpression),
    Parameter { name: String, bound: Option<Value> },
}


#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum Operator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
    NotContains,
}


#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum Quantifier {
    Default,
    Any,
}


#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub enum Modifier {
    Lower,
    Length,
    Each,
    Null,
    Exists,
    NonEmpty,
    StringType,
    NumberType,
    ArrayType,
    ObjectType,
    Custom(String),
}


fn parse_modifier(name: String) -> Modifier {
    match name.as_str() {
        "lower" => Modifier::Lower,
        "length" => Modifier::Length,
        "each" => Modifier::Each,
        "null" => Modifier::Null,
        "exists" => Modifier::Exists,
        "?" => Modifier::NonEmpty,
        "string" => Modifier::StringType,
        "number" => Modifier::NumberType,
        "array" => Modifier::ArrayType,
        "object" => Modifier::ObjectType,
        _ => Modifier::Custom(name),
    }
}


/// Parsed field predicate before schema binding.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Term {
    pub path: QueryPath,
    pub modifiers: Vec<Modifier>,
    pub quantifier: Quantifier,
    pub operator: Operator,
    pub operand: Operand,
    pub span: (usize, usize),
}


#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum FilterExpr {
    Term(Term),
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
    Not(Box<FilterExpr>),
}


/// One selected full-text field and optional binding.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct FtsField {
    pub path: FieldPath,
    pub binding: Option<String>,
    /// Original selected path when it contains an explicit array step.
    #[serde(default)]
    pub query_path: Option<String>,
}


/// Full-text literal or regex pattern with its selected fields. A complete
/// regex pattern may instead come from one execution parameter.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct FtsQuery {
    pub case_sensitivity: CaseSensitivity,
    pub mode: FtsMode,
    pub literal: String,
    #[serde(default)]
    pub pattern_parameter: Option<String>,
    pub fields: Vec<FtsField>,
}


/// Count, sort, page, return, and evidence choices for a search.
#[derive(Clone, Debug)]
pub struct QueryOptions {
    pub sort: Vec<SortField>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
    pub return_mode: ReturnMode,
    pub evidence: bool,
}


/// Parsed search request across selected collections.
#[derive(Clone, Debug)]
pub struct Query {
    pub collections: Vec<String>,
    pub fts: Option<FtsQuery>,
    pub filter: Option<FilterExpr>,
    pub options: QueryOptions,
}


impl Query {
    /// Freeze all clock macros at one caller-supplied instant. Reuse the
    /// returned query for stable pagination across a UTC boundary.
    pub fn with_clock(&self, now: OffsetDateTime) -> Result<Self, String> {
        fn resolve(expr: &mut FilterExpr, clock: &ClockContext) -> Result<(), String> {
            match expr {
                FilterExpr::Term(term) => {
                    if let Operand::Macro { name, span } = &term.operand {
                        term.operand = clock
                            .resolve(name)
                            .map_err(|error| format!("{error} at byte {}", span.0))?;
                    }
                }
                FilterExpr::And(parts) | FilterExpr::Or(parts) => {
                    for part in parts {
                        resolve(part, clock)?;
                    }
                }
                FilterExpr::Not(inner) => resolve(inner, clock)?,
            }
            Ok(())
        }

        let mut query = self.clone();
        if let Some(filter) = &mut query.filter {
            resolve(filter, &ClockContext::new(now))?;
        }
        Ok(query)
    }

    pub(crate) fn resolve_current_clock(&self) -> Result<Cow<'_, Self>, String> {
        fn has_macro(expr: &FilterExpr) -> bool {
            match expr {
                FilterExpr::Term(term) => matches!(term.operand, Operand::Macro { .. }),
                FilterExpr::And(parts) | FilterExpr::Or(parts) => parts.iter().any(has_macro),
                FilterExpr::Not(inner) => has_macro(inner),
            }
        }
        if self.filter.as_ref().is_some_and(has_macro) {
            Ok(Cow::Owned(self.with_clock(OffsetDateTime::now_utc())?))
        } else {
            Ok(Cow::Borrowed(self))
        }
    }

    pub fn from_workload(case: &WorkloadCase) -> Result<Self, String> {
        let structured = case
            .filter
            .as_deref()
            .map(parse_structured_filter)
            .transpose()?;
        let inline_sort = structured.as_ref().and_then(|parsed| parsed.sort.as_ref());
        if inline_sort.is_some() && !case.sort.is_empty() {
            return Err("inline SORT BY conflicts with independent sort options".into());
        }
        let fts = case
            .fts
            .as_ref()
            .map(|literal| {
                let fields = case
                    .fts_fields
                    .iter()
                    .map(|name| {
                        let (path, binding) = split_path_binding(name);
                        let parsed = QueryPath::parse(path)?;
                        Ok(FtsField {
                            path: parsed.field,
                            binding: binding.map(str::to_owned),
                            query_path: parsed.has_explicit_array_step.then(|| path.to_owned()),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok::<_, String>(FtsQuery {
                    case_sensitivity: case.fts_case_sensitivity,
                    mode: case.fts_mode,
                    literal: literal.clone(),
                    pattern_parameter: if matches!(case.fts_mode, FtsMode::Regex(_)) {
                        regex_pattern_parameter(literal).map(str::to_owned)
                    } else {
                        None
                    },
                    fields,
                })
            })
            .transpose()?;
        Ok(Self {
            collections: if case.collection == "*" {
                Vec::new()
            } else {
                vec![case.collection.clone()]
            },
            fts,
            filter: structured.as_ref().and_then(|parsed| parsed.filter.clone()),
            options: QueryOptions {
                sort: inline_sort.cloned().unwrap_or_else(|| case.sort.clone()),
                limit: case.limit,
                offset: case.offset,
                count: case.count,
                return_mode: case.return_mode.clone(),
                evidence: case.evidence,
            },
        })
    }

    /// Bind data values into a parsed template without reparsing query text.
    /// Unused supplied values are intentionally ignored.
    pub fn with_parameters(&self, values: &HashMap<String, Value>) -> Result<Self, String> {
        fn bind(expr: &mut FilterExpr, values: &HashMap<String, Value>) -> Result<(), String> {
            match expr {
                FilterExpr::Term(term) => {
                    if let Operand::Parameter { name, bound } = &mut term.operand {
                        *bound = Some(
                            values
                                .get(name)
                                .cloned()
                                .ok_or_else(|| format!("missing parameter ${name}"))?,
                        );
                    }
                }
                FilterExpr::And(parts) | FilterExpr::Or(parts) => {
                    for part in parts {
                        bind(part, values)?;
                    }
                }
                FilterExpr::Not(inner) => bind(inner, values)?,
            }
            Ok(())
        }

        let mut query = self.clone();
        if let Some(filter) = &mut query.filter {
            bind(filter, values)?;
        }
        if let Some(fts) = &mut query.fts
            && let Some(name) = fts.pattern_parameter.take()
        {
            if !matches!(fts.mode, FtsMode::Regex(_)) {
                return Err("pattern parameter requires regex FTS mode".into());
            }
            let value = values
                .get(&name)
                .ok_or_else(|| format!("missing parameter ${name}"))?;
            let Value::String(pattern) = value else {
                return Err(format!("regex pattern parameter ${name} requires a string"));
            };
            fts.literal = pattern.clone();
        }
        Ok(query)
    }
}


fn regex_pattern_parameter(literal: &str) -> Option<&str> {
    let name = literal.strip_prefix('$')?;
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_alphabetic())
        || !name.chars().all(|ch| ch == '_' || ch.is_alphanumeric())
    {
        return None;
    }
    Some(name)
}


#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Word(String),
    String(String),
    Question,
    Colon,
    Comma,
    LParen,
    RParen,
    And,
    Or,
    Not,
    Op(Operator, Quantifier),
    End,
}


/// Filter token and its source span.
#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
}


fn lex(input: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        let ch = input[pos..].chars().next().unwrap();
        if ch.is_whitespace() {
            pos += ch.len_utf8();
            continue;
        }
        let start = pos;
        let (kind, len) = if let Some((symbol, op, quantifier)) = [
            ("?!=", Operator::Ne, Quantifier::Any),
            ("?!~", Operator::NotContains, Quantifier::Any),
            ("?>=", Operator::Ge, Quantifier::Any),
            ("?<=", Operator::Le, Quantifier::Any),
            ("&&", Operator::Eq, Quantifier::Default),
            ("||", Operator::Eq, Quantifier::Default),
            ("!=", Operator::Ne, Quantifier::Default),
            ("!~", Operator::NotContains, Quantifier::Default),
            (">=", Operator::Ge, Quantifier::Default),
            ("<=", Operator::Le, Quantifier::Default),
            ("?=", Operator::Eq, Quantifier::Any),
            ("?>", Operator::Gt, Quantifier::Any),
            ("?<", Operator::Lt, Quantifier::Any),
            ("?~", Operator::Contains, Quantifier::Any),
            ("=", Operator::Eq, Quantifier::Default),
            (">", Operator::Gt, Quantifier::Default),
            ("<", Operator::Lt, Quantifier::Default),
            ("~", Operator::Contains, Quantifier::Default),
        ]
        .iter()
        .find(|(s, _, _)| input[pos..].starts_with(s))
        {
            let kind = match *symbol {
                "&&" => TokenKind::And,
                "||" => TokenKind::Or,
                _ => TokenKind::Op(*op, *quantifier),
            };
            (kind, symbol.len())
        } else {
            match ch {
                '(' => (TokenKind::LParen, 1),
                ')' => (TokenKind::RParen, 1),
                ':' => (TokenKind::Colon, 1),
                '?' => (TokenKind::Question, 1),
                ',' => (TokenKind::Comma, 1),
                '!' => (TokenKind::Not, 1),
                '\'' | '"' => {
                    let quote = ch;
                    pos += 1;
                    let mut value = String::new();
                    let mut closed = false;
                    while pos < input.len() {
                        let c = input[pos..].chars().next().unwrap();
                        pos += c.len_utf8();
                        if c == quote {
                            closed = true;
                            break;
                        }
                        if c == '\\' {
                            let escaped = input[pos..]
                                .chars()
                                .next()
                                .ok_or("trailing string escape")?;
                            pos += escaped.len_utf8();
                            let decoded = match escaped {
                                'n' => '\n',
                                'r' => '\r',
                                't' => '\t',
                                '\\' => '\\',
                                '\'' => '\'',
                                '"' => '"',
                                '%' => '%',
                                _ => return Err(format!("unknown escape at byte {pos}")),
                            };
                            if escaped == '%' {
                                value.push('\\');
                            }
                            value.push(decoded);
                        } else {
                            value.push(c);
                        }
                    }
                    if !closed {
                        return Err(format!("unterminated string at byte {start}"));
                    }
                    out.push(Token {
                        kind: TokenKind::String(value),
                        start,
                        end: pos,
                    });
                    continue;
                }
                _ => {
                    // A bracket-quoted path component may contain otherwise special
                    // characters; scan until its closing bracket before resuming.
                    while pos < input.len() {
                        let c = input[pos..].chars().next().unwrap();
                        if c == '[' {
                            let open = pos;
                            pos += 1;
                            if let Some(quote) = input[pos..]
                                .chars()
                                .next()
                                .filter(|c| *c == '\'' || *c == '"')
                            {
                                pos += 1;
                                while pos < input.len() {
                                    let current = input[pos..].chars().next().unwrap();
                                    pos += current.len_utf8();
                                    if current == quote {
                                        break;
                                    }
                                    if current == '\\' && pos < input.len() {
                                        pos += input[pos..].chars().next().unwrap().len_utf8();
                                    }
                                }
                            } else {
                                while pos < input.len() && !input[pos..].starts_with(']') {
                                    pos += input[pos..].chars().next().unwrap().len_utf8();
                                }
                            }
                            if !input[pos..].starts_with(']') {
                                return Err(format!("unclosed path bracket at byte {open}"));
                            }
                            pos += 1;
                        } else if c.is_whitespace()
                            || (if looks_like_temporal(&input[start..]) {
                                "(),!<>=~?&|"
                            } else {
                                ":(),!<>=~?&|"
                            })
                            .contains(c)
                        {
                            break;
                        } else {
                            pos += c.len_utf8();
                        }
                    }
                    if pos == start {
                        return Err(format!("unexpected character {ch:?} at byte {pos}"));
                    }
                    out.push(Token {
                        kind: TokenKind::Word(input[start..pos].to_owned()),
                        start,
                        end: pos,
                    });
                    continue;
                }
            }
        };
        pos += len;
        out.push(Token {
            kind,
            start,
            end: pos,
        });
    }
    out.push(Token {
        kind: TokenKind::End,
        start: input.len(),
        end: input.len(),
    });
    Ok(out)
}


/// Cursor and token stream for the filter grammar.
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}


impl Parser {
    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn take(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        self.pos += 1;
        token
    }

    fn expect(&mut self, expected: TokenKind) -> Result<Token, String> {
        if *self.peek() == expected {
            Ok(self.take())
        } else {
            Err(format!(
                "expected {expected:?} at byte {}",
                self.tokens[self.pos].start
            ))
        }
    }

    fn or(&mut self) -> Result<FilterExpr, String> {
        let mut parts = vec![self.and()?];
        while matches!(self.peek(), TokenKind::Or) {
            self.take();
            parts.push(self.and()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            FilterExpr::Or(parts)
        })
    }

    fn and(&mut self) -> Result<FilterExpr, String> {
        let mut parts = vec![self.unary()?];
        while matches!(self.peek(), TokenKind::And) {
            self.take();
            parts.push(self.unary()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            FilterExpr::And(parts)
        })
    }

    fn unary(&mut self) -> Result<FilterExpr, String> {
        if matches!(self.peek(), TokenKind::Not) {
            self.take();
            self.expect(TokenKind::LParen)?;
            let inner = self.or()?;
            self.expect(TokenKind::RParen)?;
            Ok(FilterExpr::Not(Box::new(inner)))
        } else if matches!(self.peek(), TokenKind::LParen) {
            self.take();
            let inner = self.or()?;
            self.expect(TokenKind::RParen)?;
            Ok(inner)
        } else {
            self.term().map(FilterExpr::Term)
        }
    }

    fn term(&mut self) -> Result<Term, String> {
        let path_token = self.take();
        let path_name = match &path_token.kind {
            TokenKind::Word(s) => s,
            _ => return Err(format!("expected field path at byte {}", path_token.start)),
        };
        if path_name.starts_with('$') {
            return Err(format!(
                "parameter cannot select a field at byte {}",
                path_token.start
            ));
        }
        let path = QueryPath::parse(path_name)?;
        let mut modifiers = Vec::new();
        let mut modifier_end = path_token.end;
        while matches!(self.peek(), TokenKind::Colon) {
            self.take();
            let token = self.take();
            let name = match token.kind {
                TokenKind::Word(s) => s,
                TokenKind::Question => "?".to_string(),
                _ => return Err(format!("expected modifier at byte {}", token.start)),
            };
            if name.starts_with('$') {
                return Err(format!(
                    "parameter cannot select a modifier at byte {}",
                    token.start
                ));
            }
            modifiers.push(parse_modifier(name));
            modifier_end = token.end;
        }
        if path_name == "@root"
            && modifiers.last() == Some(&Modifier::Null)
            && matches!(
                self.peek(),
                TokenKind::End | TokenKind::And | TokenKind::Or | TokenKind::RParen
            )
        {
            return Ok(Term {
                path,
                modifiers,
                quantifier: Quantifier::Default,
                operator: Operator::Eq,
                operand: Operand::Literal(Literal::Bool(true)),
                span: (path_token.start, modifier_end),
            });
        }
        let op = self.take();
        let (operator, quantifier) = match op.kind {
            TokenKind::Op(o, q) => (o, q),
            _ => return Err(format!("expected comparison operator at byte {}", op.start)),
        };
        let value = self.take();
        let mut end = value.end;
        let operand = match value.kind {
            TokenKind::String(s) => Operand::Literal(Literal::String(s)),
            TokenKind::Word(s) if s == "true" => Operand::Literal(Literal::Bool(true)),
            TokenKind::Word(s) if s == "false" => Operand::Literal(Literal::Bool(false)),
            TokenKind::Word(s) if s == "null" => Operand::Literal(Literal::Null),
            TokenKind::Word(s) if s.starts_with('$') => {
                let name = &s[1..];
                if name.is_empty()
                    || !name
                        .chars()
                        .next()
                        .is_some_and(|ch| ch == '_' || ch.is_alphabetic())
                    || !name.chars().all(|ch| ch == '_' || ch.is_alphanumeric())
                {
                    return Err(format!("invalid parameter name at byte {}", value.start));
                }
                Operand::Parameter {
                    name: name.to_owned(),
                    bound: None,
                }
            }
            TokenKind::Word(s) => {
                if looks_like_temporal(&s) {
                    Operand::Period(
                        PeriodLiteral::parse(&s)
                            .map_err(|error| format!("{error} at byte {}", value.start))?,
                    )
                } else if let Some(name) = s
                    .strip_prefix('@')
                    .filter(|_| !s.contains('.') && !s.starts_with("@root"))
                {
                    Operand::Macro {
                        name: name.to_owned(),
                        span: (value.start, value.end),
                    }
                } else if let Some(bytes) = parse_size_literal(&s)
                    .map_err(|error| format!("{error} at byte {}", value.start))?
                {
                    Operand::Literal(Literal::Number(Number::integer(bytes)))
                } else if s
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| *byte == b'-' || byte.is_ascii_digit())
                    && s.bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'))
                {
                    Operand::Literal(Literal::Number(Number::parse_json_literal(&s)?))
                } else {
                    let path = QueryPath::parse(&s).map_err(|error| {
                        format!("invalid field operand at byte {}: {error}", value.start)
                    })?;
                    let mut modifiers = Vec::new();
                    while matches!(self.peek(), TokenKind::Colon) {
                        self.take();
                        let token = self.take();
                        let name = match token.kind {
                            TokenKind::Word(name) => name,
                            TokenKind::Question => "?".to_string(),
                            _ => return Err(format!("expected modifier at byte {}", token.start)),
                        };
                        end = token.end;
                        modifiers.push(parse_modifier(name));
                    }
                    Operand::Field(FieldExpression {
                        path,
                        modifiers,
                        span: (value.start, end),
                    })
                }
            }
            _ => return Err(format!("expected operand at byte {}", value.start)),
        };
        Ok(Term {
            path,
            modifiers,
            quantifier,
            operator,
            operand,
            span: (path_token.start, end),
        })
    }

    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), TokenKind::Word(value) if value.eq_ignore_ascii_case(word))
    }

    fn sort_clause_start(&self) -> bool {
        self.keyword("SORT")
            && !matches!(
                self.tokens.get(self.pos + 1).map(|token| &token.kind),
                Some(TokenKind::Op(_, _) | TokenKind::Colon)
            )
    }

    fn sort_clause(&mut self) -> Result<Vec<SortField>, String> {
        let sort = self.take();
        debug_assert!(matches!(sort.kind, TokenKind::Word(_)));
        if !self.keyword("BY") {
            return Err(format!(
                "expected BY after SORT at byte {}",
                self.tokens[self.pos].start
            ));
        }
        self.take();
        let mut fields = Vec::new();
        let mut seen = HashSet::new();
        loop {
            let token = self.take();
            let first = match token.kind {
                TokenKind::Word(name) => name,
                _ => return Err(format!("expected sort field at byte {}", token.start)),
            };
            let reducer = if matches!(self.peek(), TokenKind::LParen) {
                let reducer = match first.to_ascii_uppercase().as_str() {
                    "MIN" => SortReducer::Min,
                    "MAX" => SortReducer::Max,
                    "FIRST" => SortReducer::First,
                    "LAST" => SortReducer::Last,
                    _ => return Err(format!("unknown sort reduction at byte {}", token.start)),
                };
                self.take();
                Some(reducer)
            } else {
                None
            };
            let name = if reducer.is_some() {
                let path = self.take();
                match path.kind {
                    TokenKind::Word(value) => value,
                    _ => return Err(format!("expected sort field at byte {}", path.start)),
                }
            } else {
                first
            };
            if name.bytes().any(|byte| byte == b'$') {
                return Err(format!("unsupported sort field at byte {}", token.start));
            }
            let parsed = QueryPath::parse(&name)
                .map_err(|error| format!("invalid sort field at byte {}: {error}", token.start))?;
            let binding = if matches!(self.peek(), TokenKind::Colon) {
                self.take();
                let modifier = self.take();
                match modifier.kind {
                    TokenKind::Word(value) if !value.is_empty() => Some(value),
                    _ => return Err(format!("expected sort binding at byte {}", modifier.start)),
                }
            } else {
                None
            };
            if reducer.is_some() && !matches!(self.peek(), TokenKind::RParen) {
                return Err(format!(
                    "expected ')' at byte {}",
                    self.tokens[self.pos].start
                ));
            }
            if reducer.is_some() {
                self.take();
            }
            let expression = SortExpression {
                path: parsed.display.clone(),
                binding,
                reducer,
            };
            if !seen.insert(expression.clone()) {
                return Err(format!(
                    "duplicate sort field at byte {}: {}",
                    token.start, expression.path
                ));
            }
            let descending = if self.keyword("ASC") {
                self.take();
                false
            } else if self.keyword("DESC") {
                self.take();
                true
            } else {
                false
            };
            let field = if expression.reducer.is_some() {
                format!("{:?}({})", expression.reducer.unwrap(), expression.path)
            } else if let Some(binding) = &expression.binding {
                format!("{}:{binding}", expression.path)
            } else {
                expression.path.clone()
            };
            let expression = (expression.reducer.is_some()
                || expression.binding.is_some()
                || parsed.has_explicit_array_step)
                .then_some(expression);
            fields.push(SortField {
                field,
                descending,
                expression,
            });
            match self.peek() {
                TokenKind::Comma => {
                    self.take();
                }
                TokenKind::End => return Ok(fields),
                _ => {
                    return Err(format!(
                        "unknown sort direction or trailing token at byte {}",
                        self.tokens[self.pos].start
                    ));
                }
            }
        }
    }
}


/// Top-level structured request split into membership and registered ordering.
#[derive(Clone, Debug, PartialEq)]
pub struct StructuredFilter {
    pub filter: Option<FilterExpr>,
    pub sort: Option<Vec<SortField>>,
}


pub fn parse_structured_filter(input: &str) -> Result<StructuredFilter, String> {
    let mut parser = Parser {
        tokens: lex(input)?,
        pos: 0,
    };
    let filter = if parser.sort_clause_start() {
        None
    } else {
        Some(parser.or()?)
    };
    let sort = if parser.sort_clause_start() {
        Some(parser.sort_clause()?)
    } else {
        None
    };
    if !matches!(parser.peek(), TokenKind::End) {
        return Err(format!(
            "unexpected token at byte {}",
            parser.tokens[parser.pos].start
        ));
    }
    Ok(StructuredFilter { filter, sort })
}

/// Byte offset of the top-level ordering clause, if one exists. This uses the
/// same lexer and parser as execution, so quoted "SORT BY" text and nested
/// expressions cannot be mistaken for a clause during editor transforms.
pub fn structured_sort_start(input: &str) -> Result<Option<usize>, String> {
    let mut parser = Parser {
        tokens: lex(input)?,
        pos: 0,
    };
    if !parser.sort_clause_start() {
        parser.or()?;
    }
    let start = parser
        .sort_clause_start()
        .then(|| parser.tokens[parser.pos].start);
    if start.is_some() {
        parser.sort_clause()?;
    }
    if !matches!(parser.peek(), TokenKind::End) {
        return Err(format!(
            "unexpected token at byte {}",
            parser.tokens[parser.pos].start
        ));
    }
    Ok(start)
}

pub fn parse_filter(input: &str) -> Result<FilterExpr, String> {
    let mut parser = Parser {
        tokens: lex(input)?,
        pos: 0,
    };
    let result = parser.or()?;
    if !matches!(parser.peek(), TokenKind::End) {
        return Err(format!(
            "unexpected token at byte {}",
            parser.tokens[parser.pos].start
        ));
    }
    Ok(result)
}

#[cfg(test)]
#[path = "query.test.rs"]
mod tests;
