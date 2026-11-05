//! Separate literal FTS and structured FILTER parsing.

use crate::model::{FieldPath, Number};
use crate::workload::{ReturnMode, SortField, WorkloadCase};

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    String(String),
    Number(Number),
    Bool(bool),
    Null,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Quantifier {
    Default,
    Any,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Modifier {
    Lower,
    Length,
    Each,
    Null,
    Exists,
    Custom(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub path: FieldPath,
    pub modifiers: Vec<Modifier>,
    pub quantifier: Quantifier,
    pub operator: Operator,
    pub literal: Literal,
    pub span: (usize, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum FilterExpr {
    Term(Term),
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
    Not(Box<FilterExpr>),
}

#[derive(Clone, Debug)]
pub struct FtsField {
    pub path: FieldPath,
    pub binding: Option<String>,
}
#[derive(Clone, Debug)]
pub struct FtsQuery {
    pub literal: String,
    pub fields: Vec<FtsField>,
}
#[derive(Clone, Debug)]
pub struct QueryOptions {
    pub sort: Vec<SortField>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
    pub return_mode: ReturnMode,
    pub evidence: bool,
}
#[derive(Clone, Debug)]
pub struct Query {
    pub collections: Vec<String>,
    pub fts: Option<FtsQuery>,
    pub filter: Option<FilterExpr>,
    pub options: QueryOptions,
}

impl Query {
    pub fn from_workload(case: &WorkloadCase) -> Result<Self, String> {
        let fts = case
            .fts
            .as_ref()
            .map(|literal| {
                let fields = case
                    .fts_fields
                    .iter()
                    .map(|name| {
                        let (path, binding) = name
                            .rsplit_once(':')
                            .map_or((name.as_str(), None), |(a, b)| (a, Some(b.to_owned())));
                        Ok(FtsField {
                            path: FieldPath::parse(path)?,
                            binding,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok::<_, String>(FtsQuery {
                    literal: literal.clone(),
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
            filter: case.filter.as_ref().map(|s| parse_filter(s)).transpose()?,
            options: QueryOptions {
                sort: case.sort.clone(),
                limit: case.limit,
                offset: case.offset,
                count: case.count,
                return_mode: case.return_mode.clone(),
                evidence: case.evidence,
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Word(String),
    String(String),
    Colon,
    LParen,
    RParen,
    And,
    Or,
    Not,
    Op(Operator, Quantifier),
    End,
}
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
                            } else if input[pos..].starts_with('*') {
                                pos += 1;
                            }
                            if !input[pos..].starts_with(']') {
                                return Err(format!("unclosed path bracket at byte {open}"));
                            }
                            pos += 1;
                        } else if c.is_whitespace() || ":()!<>=~?&|".contains(c) {
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
        let path = FieldPath::parse(path_name)?;
        let mut modifiers = Vec::new();
        while matches!(self.peek(), TokenKind::Colon) {
            self.take();
            let token = self.take();
            let name = match token.kind {
                TokenKind::Word(s) => s,
                _ => return Err(format!("expected modifier at byte {}", token.start)),
            };
            modifiers.push(match name.as_str() {
                "lower" => Modifier::Lower,
                "length" => Modifier::Length,
                "each" => Modifier::Each,
                "null" => Modifier::Null,
                "exists" => Modifier::Exists,
                _ => Modifier::Custom(name),
            });
        }
        let op = self.take();
        let (operator, quantifier) = match op.kind {
            TokenKind::Op(o, q) => (o, q),
            _ => return Err(format!("expected comparison operator at byte {}", op.start)),
        };
        let value = self.take();
        let literal = match value.kind {
            TokenKind::String(s) => Literal::String(s),
            TokenKind::Word(s) if s == "true" => Literal::Bool(true),
            TokenKind::Word(s) if s == "false" => Literal::Bool(false),
            TokenKind::Word(s) if s == "null" => Literal::Null,
            TokenKind::Word(s) => {
                let number =
                    if let Ok(n) = s.parse::<i128>() {
                        Number::integer(n)
                    } else {
                        Number::float(s.parse::<f64>().map_err(|_| {
                            format!("invalid literal {s:?} at byte {}", value.start)
                        })?)?
                    };
                Literal::Number(number)
            }
            _ => return Err(format!("expected literal at byte {}", value.start)),
        };
        Ok(Term {
            path,
            modifiers,
            quantifier,
            operator,
            literal,
            span: (path_token.start, value.end),
        })
    }
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
mod tests {
    use super::*;

    #[test]
    fn precedence_quantifiers_and_separate_fts() {
        let ast = parse_filter("a = 1 || b = 2 && !(c ?!= 'x')").unwrap();
        assert!(matches!(ast, FilterExpr::Or(parts) if parts.len() == 2));
        let ast = parse_filter("data.gnd[\"@context\"] ?= 'x' && @file.size >= 1").unwrap();
        assert!(matches!(ast, FilterExpr::And(parts) if parts.len() == 2));
        assert!(parse_filter("a => 1").is_err());
        assert!(parse_filter("a =< 1").is_err());
        assert!(parse_filter("a ?=! 1").is_err());
        let case = WorkloadCase {
            id: "Q000".into(),
            collection: "*".into(),
            fts: Some("a && b".into()),
            filter: Some("a = 1".into()),
            fts_fields: vec![],
            sort: vec![],
            limit: 50,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        };
        let query = Query::from_workload(&case).unwrap();
        assert_eq!(query.fts.unwrap().literal, "a && b");
    }

    #[test]
    fn every_fixed_filter_parses() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
        for case in crate::workload::load_manifest(&path).unwrap() {
            Query::from_workload(&case).unwrap_or_else(|e| panic!("{}: {e}", case.id));
        }
    }
}
