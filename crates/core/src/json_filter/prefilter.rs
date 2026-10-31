//! Necessary byte patterns for scanning source JSON before exact evaluation.
//! A missing pattern means scan every file. False positives are harmless; false
//! negatives are not. ASCII runs account for JSON's optional \u escapes.

use super::{
    Expr, FieldOperand, JsonFilter, JsonPath, JsonPathStep, Modifier, Op, Operand, pattern::Like,
};
use crate::Result;
use grep_regex::RegexMatcher;

fn longest_run(text: &str) -> String {
    let (mut best, mut current) = (String::new(), String::new());
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            current.push(c);
        } else if current.len() > best.len() {
            best = std::mem::take(&mut current);
        } else {
            current.clear();
        }
    }
    if current.len() > best.len() {
        current
    } else {
        best
    }
}

fn ascii_runs(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            current.push(c);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

fn ordered_runs_regex(runs: Vec<String>, insensitive: bool) -> Option<String> {
    if runs.is_empty() {
        return None;
    }
    let body = runs
        .iter()
        .map(|run| regex::escape(run))
        .collect::<Vec<_>>()
        .join(".*");
    Some(if insensitive {
        format!("(?i:{body})")
    } else {
        body
    })
}

fn path_run(path: &JsonPath) -> String {
    path.iter()
        .filter_map(|step| match step {
            JsonPathStep::Key(key) => Some(longest_run(key)),
            _ => None,
        })
        .max_by_key(String::len)
        .unwrap_or_default()
}

fn field_run(operand: &Operand) -> String {
    match operand {
        Operand::Field(FieldOperand { path, .. }) => path_run(path),
        _ => String::new(),
    }
}

fn rendered(operand: &Operand) -> bool {
    matches!(
        operand,
        Operand::Field(FieldOperand {
            modifier: Some(Modifier::Rendered(_)),
            ..
        })
    )
}

fn atom_pattern(left: &Operand, op: Op, right: &Operand, ignore_case: bool) -> Option<String> {
    if rendered(left) || rendered(right) {
        // Markup can split a literal into fragments absent from source JSON.
        return None;
    }
    let field = [field_run(left), field_run(right)]
        .into_iter()
        .max_by_key(String::len)
        .unwrap_or_default();
    let runs = match (op, left, right) {
        (Op::Eq, Operand::Field(_), Operand::Literal(serde_json::Value::String(s)))
        | (Op::Eq, Operand::Literal(serde_json::Value::String(s)), Operand::Field(_)) => {
            ascii_runs(s)
        }
        // Full folding can expand a source character (ß -> ss), so a folded
        // literal is not necessarily present in the raw JSON source bytes.
        (Op::Contains, Operand::Field(_), Operand::Literal(serde_json::Value::String(s)))
            if !ignore_case =>
        {
            Like::new(s, false, ignore_case).required_ascii_runs()
        }
        (Op::Eq, Operand::Field(_), Operand::Literal(serde_json::Value::Bool(v)))
        | (Op::Eq, Operand::Literal(serde_json::Value::Bool(v)), Operand::Field(_)) => {
            vec![v.to_string()]
        }
        _ => Vec::new(),
    };
    encoded_runs(runs).or_else(|| (!field.is_empty()).then(|| encoded_run(&field)))
}

// JSON can encode every ASCII character literally or as a Unicode escape.
// Case-insensitive matching also permits either letter code point in escapes.
fn encoded_run(run: &str) -> String {
    let mut out = String::from("(?i:");
    for c in run.chars() {
        let lower = c.to_ascii_lowercase();
        let upper = c.to_ascii_uppercase();
        out.push_str(&format!(
            r"(?:{c}|\\u{:04x}|\\u{:04x})",
            lower as u32, upper as u32
        ));
    }
    out.push(')');
    out
}

fn encoded_runs(runs: Vec<String>) -> Option<String> {
    (!runs.is_empty()).then(|| {
        runs.iter()
            .map(|run| encoded_run(run))
            .collect::<Vec<_>>()
            .join(".*")
    })
}

fn necessary(expr: &Expr) -> Option<String> {
    match expr {
        Expr::And(parts) => parts.iter().filter_map(necessary).max_by_key(String::len),
        Expr::Or(parts) => {
            let patterns = parts.iter().map(necessary).collect::<Option<Vec<_>>>()?;
            if patterns.is_empty() {
                None
            } else {
                Some(format!("(?:{})", patterns.join("|")))
            }
        }
        Expr::Exists(path) => {
            let run = path_run(path);
            (!run.is_empty()).then(|| encoded_run(&run))
        }
        Expr::Missing(_) => None,
        Expr::Compare {
            left,
            op,
            right,
            ignore_case,
            ..
        } => atom_pattern(left, *op, right, *ignore_case),
    }
}


impl JsonFilter {
    pub(crate) fn source_prefilter(&self) -> Result<Option<(RegexMatcher, String)>> {
        let Some(pattern) = necessary(&self.expr) else {
            return Ok(None);
        };
        // A very large OR expression may exceed the regex compiler's size limit.
        // Scanning every file is always a correct fallback.
        Ok(RegexMatcher::new(&pattern)
            .ok()
            .map(|matcher| (matcher, pattern)))
    }

    /// Necessary text in original or Unicode-folded scalar values. Field
    /// names and JSON quoting are deliberately absent from the values cache.
    pub(crate) fn values_prefilter(&self) -> Option<(RegexMatcher, String)> {
        fn pattern(expr: &Expr) -> Option<String> {
            match expr {
                Expr::And(parts) => parts.iter().filter_map(pattern).max_by_key(String::len),
                Expr::Or(parts) => {
                    let parts = parts.iter().map(pattern).collect::<Option<Vec<_>>>()?;
                    (!parts.is_empty()).then(|| format!("(?:{})", parts.join("|")))
                }
                Expr::Exists(_) | Expr::Missing(_) => None,
                Expr::Compare { left, right, .. } if rendered(left) || rendered(right) => None,
                Expr::Compare {
                    left,
                    op,
                    right,
                    ignore_case,
                    ..
                } => match (op, left, right) {
                    (Op::Eq, Operand::Field(_), Operand::Literal(serde_json::Value::String(s)))
                    | (Op::Eq, Operand::Literal(serde_json::Value::String(s)), Operand::Field(_)) => {
                        ordered_runs_regex(ascii_runs(s), false)
                    }
                    (
                        Op::Contains,
                        Operand::Field(_),
                        Operand::Literal(serde_json::Value::String(s)),
                    ) => ordered_runs_regex(
                        Like::new(s, false, *ignore_case).required_ascii_runs(),
                        *ignore_case,
                    ),
                    (Op::Eq, Operand::Field(_), Operand::Literal(serde_json::Value::Bool(v)))
                    | (Op::Eq, Operand::Literal(serde_json::Value::Bool(v)), Operand::Field(_)) => {
                        Some(v.to_string())
                    }
                    _ => None,
                },
            }
        }
        let pattern = pattern(&self.expr)?;
        RegexMatcher::new(&pattern)
            .ok()
            .map(|matcher| (matcher, pattern))
    }
}
