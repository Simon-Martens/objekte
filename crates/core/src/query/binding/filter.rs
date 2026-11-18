#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Derived {
    None,
    Length,
    Null,
    Exists,
    IsNonEmpty,
    IsString,
    IsNumber,
    IsArray,
    IsObject,
}

impl Derived {
    pub(crate) fn state_predicate(self) -> Option<StatePredicate> {
        Some(match self {
            Self::Null => StatePredicate::Null,
            Self::Exists => StatePredicate::Exists,
            Self::IsNonEmpty => StatePredicate::NonEmpty,
            Self::IsString => StatePredicate::String,
            Self::IsNumber => StatePredicate::Number,
            Self::IsArray => StatePredicate::Array,
            Self::IsObject => StatePredicate::Object,
            Self::None | Self::Length => return None,
        })
    }
}

/// Substring pattern parsed once for a bound literal.
#[derive(Clone, Debug)]
pub(crate) enum PreparedContains {
    Literal(Finder<'static>),
    Unescaped(Finder<'static>),
    Wildcards(Vec<Finder<'static>>),
}

#[cfg(test)]
thread_local! {
    pub(crate) static PREPARED_CONTAINS_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn split_contains_wildcards(pattern: &str) -> (bool, Vec<String>) {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut chars = pattern.chars().peekable();
    let mut wildcard = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'%') {
            current.push(chars.next().unwrap());
        } else if ch == '%' {
            pieces.push(std::mem::take(&mut current));
            wildcard = true;
        } else {
            current.push(ch);
        }
    }
    pieces.push(current);
    (wildcard, pieces)
}

impl PreparedContains {
    pub(crate) fn new(pattern: &str) -> Self {
        #[cfg(test)]
        PREPARED_CONTAINS_BUILDS.with(|count| count.set(count.get() + 1));
        if !pattern.as_bytes().contains(&b'%') {
            return Self::Literal(Finder::new(pattern).into_owned());
        }
        let (wildcard, mut pieces) = split_contains_wildcards(pattern);
        if wildcard {
            Self::Wildcards(
                pieces
                    .into_iter()
                    .filter(|piece| !piece.is_empty())
                    .map(|piece| Finder::new(&piece).into_owned())
                    .collect(),
            )
        } else {
            Self::Unescaped(Finder::new(&pieces.pop().unwrap()).into_owned())
        }
    }

    pub(crate) fn matches_bytes(&self, text: &[u8]) -> bool {
        match self {
            Self::Literal(finder) | Self::Unescaped(finder) => finder.find(text).is_some(),
            Self::Wildcards(pieces) => {
                let mut rest = text;
                for finder in pieces {
                    let Some(index) = finder.find(rest) else {
                        return false;
                    };
                    rest = &rest[index + finder.needle().len()..];
                }
                true
            }
        }
    }

    pub(crate) fn matches(&self, text: &str, _literal: &str) -> bool {
        self.matches_bytes(text.as_bytes())
    }
}

/// Typed predicate with its resolved path, literal, and binding.
#[derive(Clone, Debug)]
pub struct BoundTerm {
    pub path: FieldPath,
    pub query_path: QueryPath,
    pub rhs: Option<Box<BoundFieldOperand>>,
    pub related: Option<Box<crate::query::binding::relation::BoundRelationPath>>,
    pub kind: LogicalType,
    pub source_kind: LogicalType,
    pub operator: Operator,
    pub quantifier: Quantifier,
    pub literal: Value,
    pub period: Option<TemporalBounds>,
    pub(crate) prepared_contains: Option<PreparedContains>,
    pub(crate) gram_request: Option<BigramRequest>,
    pub derived: Derived,
    pub transform: Transform,
    pub operation: Option<Arc<dyn BindingOperation>>,
    pub matcher: Option<Arc<dyn PreparedFilterMatcher>>,
    pub binding_name: String,
    pub binding_version: u32,
    pub indexed: bool,
    pub multi: bool,
    pub implicit_array: bool,
    pub span: (usize, usize),
}

/// Prepared right-hand field for one dynamic scalar comparison.
#[derive(Clone, Debug)]
pub struct BoundFieldOperand {
    pub path: FieldPath,
    pub query_path: QueryPath,
    pub kind: LogicalType,
    pub derived: Derived,
    pub transform: Transform,
    pub operation: Option<Arc<dyn BindingOperation>>,
    pub multi: bool,
}

/// Compare the exact semantics of two predicates within one collection plan.
/// Source spans and gram requests locate/accelerate a predicate; neither is
/// its identity. The enclosing plan supplies the collection-generation scope.
pub(crate) fn same_bound_term(a: &BoundTerm, b: &BoundTerm) -> bool {
    fn same_arc<T: ?Sized>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    let same_rhs = match (&a.rhs, &b.rhs) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.path == b.path
                && a.query_path == b.query_path
                && a.kind == b.kind
                && a.derived == b.derived
                && a.transform == b.transform
                && a.multi == b.multi
                && same_arc(&a.operation, &b.operation)
        }
        _ => false,
    };
    let same_related = match (&a.related, &b.related) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.hops == b.hops
                && a.target_is_lhs == b.target_is_lhs
                && a.evidence_path == b.evidence_path
                && same_bound_term(&a.terminal, &b.terminal)
                && match (&a.source_operand, &b.source_operand) {
                    (None, None) => true,
                    (Some(a), Some(b)) => same_bound_term(a, b),
                    _ => false,
                }
        }
        _ => false,
    };
    a.path == b.path
        && a.query_path == b.query_path
        && a.kind == b.kind
        && a.source_kind == b.source_kind
        && a.operator == b.operator
        && a.quantifier == b.quantifier
        && a.literal == b.literal
        && a.period.as_ref().map(|period| (&period.start, &period.end))
            == b.period.as_ref().map(|period| (&period.start, &period.end))
        && a.derived == b.derived
        && a.transform == b.transform
        && a.binding_name == b.binding_name
        && a.binding_version == b.binding_version
        && a.multi == b.multi
        && a.implicit_array == b.implicit_array
        && same_arc(&a.operation, &b.operation)
        && same_arc(&a.matcher, &b.matcher)
        && same_rhs
        && same_related
}

#[derive(Debug, Clone)]
pub enum BoundExpr {
    Term(BoundTerm),
    And(Vec<BoundExpr>),
    Or(Vec<BoundExpr>),
    Not(Box<BoundExpr>),
}

pub fn bind_filter(expr: &FilterExpr, schema: &Schema) -> Result<BoundExpr, String> {
    bind_filter_with_parameters(expr, schema, &mut HashMap::new())
}

/// Bind one FILTER while sharing parameter type constraints across collections.
pub(crate) fn bind_filter_with_parameters(
    expr: &FilterExpr,
    schema: &Schema,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<BoundExpr, String> {
    Ok(match expr {
        FilterExpr::Term(term) => BoundExpr::Term(bind_term(term, schema, parameter_types)?),
        FilterExpr::And(parts) => BoundExpr::And(
            parts
                .iter()
                .map(|p| bind_filter_with_parameters(p, schema, parameter_types))
                .collect::<Result<_, _>>()?,
        ),
        FilterExpr::Or(parts) => BoundExpr::Or(
            parts
                .iter()
                .map(|p| bind_filter_with_parameters(p, schema, parameter_types))
                .collect::<Result<_, _>>()?,
        ),
        FilterExpr::Not(inner) => BoundExpr::Not(Box::new(bind_filter_with_parameters(
            inner,
            schema,
            parameter_types,
        )?)),
    })
}

fn query_path_type(
    schema: &Schema,
    path: &QueryPath,
    field: &FieldDefinition,
) -> Result<(LogicalType, bool, bool), String> {
    if path.field.root != PathRoot::Data {
        return Ok((field.kind.clone(), field.multi, false));
    }
    let mut prefix = Vec::new();
    let mut current = LogicalType::Unknown;
    let mut implicit_array = false;
    let mut each = false;
    for step in &path.steps {
        match step {
            PathStep::Field(name) => {
                if matches!(current, LogicalType::Array(_)) {
                    implicit_array = true;
                }
                prefix.push(name.clone());
                current = schema
                    .fields
                    .get(&prefix)
                    .map_or(LogicalType::Unknown, |definition| definition.kind.clone());
            }
            PathStep::ArrayIndex(_) | PathStep::EachElement => {
                current = match current {
                    LogicalType::Array(inner) => *inner,
                    LogicalType::Unknown => LogicalType::Unknown,
                    _ => return Err(format!("array selector on scalar field: {}", path.display)),
                };
                each |= matches!(step, PathStep::EachElement);
            }
        }
    }
    if current == LogicalType::Unknown {
        current = field.kind.clone();
    }
    let multi = if path.has_explicit_array_step {
        each || implicit_array || matches!(current, LogicalType::Array(_))
    } else {
        field.multi || matches!(current, LogicalType::Array(_))
    };
    Ok((current, multi, implicit_array))
}

fn bind_term(
    term: &Term,
    schema: &Schema,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<BoundTerm, String> {
    if let Operand::Field(field) = &term.operand {
        let mut left = term.clone();
        left.operand = Operand::Literal(Literal::Null);
        left.quantifier = Quantifier::Default;
        let mut lhs = bind_literal_term(&left, schema, parameter_types, true)?;
        let rhs_term = Term {
            path: field.path.clone(),
            modifiers: field.modifiers.clone(),
            quantifier: Quantifier::Default,
            operator: term.operator,
            operand: Operand::Literal(Literal::Null),
            span: field.span,
        };
        let rhs = bind_literal_term(&rhs_term, schema, parameter_types, true)?;
        if matches!(lhs.derived, Derived::Exists | Derived::Null)
            || matches!(rhs.derived, Derived::Exists | Derived::Null)
        {
            return Err("exists/null modifiers cannot compare to another field".into());
        }
        if lhs.derived.state_predicate().is_some() || rhs.derived.state_predicate().is_some() {
            return Err("Boolean state modifiers cannot compare to another field".into());
        }
        if lhs.kind != LogicalType::Unknown
            && rhs.kind != LogicalType::Unknown
            && !comparable_logical_types(&lhs.kind, &rhs.kind)
        {
            return Err(format!(
                "incompatible field operand types: {:?} and {:?}",
                lhs.kind, rhs.kind
            ));
        }
        for operation in [lhs.operation.as_ref(), rhs.operation.as_ref()]
            .into_iter()
            .flatten()
        {
            if !operation.supports_dynamic_comparison() {
                return Err("custom binding does not support dynamic field comparison".into());
            }
        }
        let left_scope = explicit_scope(&lhs.query_path);
        let right_scope = explicit_scope(&rhs.query_path);
        if lhs.multi
            && rhs.multi
            && (left_scope.is_empty()
                || left_scope != right_scope
                || lhs.implicit_array
                || rhs.implicit_array
                || matches!(lhs.source_kind, LogicalType::Array(_))
                || matches!(rhs.source_kind, LogicalType::Array(_)))
        {
            return Err("independent multivalued field operands need a shared [*] scope or explicit indexes".into());
        }
        if term.quantifier == Quantifier::Any && term.modifiers.contains(&Modifier::Each) {
            return Err("each and existential operator conflict".into());
        }
        if term.quantifier == Quantifier::Any && !lhs.multi && !rhs.multi {
            return Err("existential comparison requires a multi-valued field".into());
        }
        lhs.quantifier = term.quantifier;
        lhs.rhs = Some(Box::new(BoundFieldOperand {
            path: rhs.path,
            query_path: rhs.query_path,
            kind: rhs.kind,
            derived: rhs.derived,
            transform: rhs.transform,
            operation: rhs.operation,
            multi: rhs.multi,
        }));
        return Ok(lhs);
    }
    bind_literal_term(term, schema, parameter_types, false)
}

pub(crate) fn explicit_scope(path: &QueryPath) -> Vec<Vec<PathStep>> {
    let mut prefix = Vec::new();
    let mut scopes = Vec::new();
    for step in &path.steps {
        prefix.push(step.clone());
        if matches!(step, PathStep::EachElement) {
            scopes.push(prefix.clone());
        }
    }
    scopes
}

fn bind_literal_term(
    term: &Term,
    schema: &Schema,
    parameter_types: &mut HashMap<String, LogicalType>,
    field_probe: bool,
) -> Result<BoundTerm, String> {
    let parameter_literal;
    let period_operand = match &term.operand {
        Operand::Period(period) => Some(period),
        _ => None,
    };
    let parsed_literal = match &term.operand {
        Operand::Literal(literal) => literal,
        Operand::Period(_) => {
            parameter_literal = Literal::Null;
            &parameter_literal
        }
        Operand::Macro { name, .. } => {
            return Err(format!("unresolved time macro @{name}"));
        }
        Operand::Field(field) => {
            return Err(format!(
                "field operand at byte {} is not yet supported by the binder",
                field.span.0
            ));
        }
        Operand::Parameter { name, bound } => {
            let value = bound
                .as_ref()
                .ok_or_else(|| format!("parameter ${name} requires execution bindings"))?;
            parameter_literal = match value {
                Value::Null => Literal::Null,
                Value::String(text) => Literal::String(text.clone()),
                Value::Number(number) => Literal::Number(number.clone()),
                Value::Bool(value) => Literal::Bool(*value),
                other => Literal::Typed(other.clone()),
            };
            &parameter_literal
        }
    };
    let parameter_name = if let Operand::Parameter { name, .. } = &term.operand {
        Some(name.as_str())
    } else {
        None
    };
    let field = schema.field_or_dynamic(&term.path.field)?;
    let (source_kind, path_multi, implicit_array) = query_path_type(schema, &term.path, &field)?;
    let mut derived = Derived::None;
    let mut transform = Transform::Identity;
    let mut operation = None;
    let mut binding_name = "default".to_string();
    let mut binding_version = 1;
    let mut each = false;
    for modifier in &term.modifiers {
        let type_name = match modifier {
            Modifier::StringType => Some("string"),
            Modifier::NumberType => Some("number"),
            Modifier::ArrayType => Some("array"),
            Modifier::ObjectType => Some("object"),
            _ => None,
        };
        // Existing registered conversions keep their old spelling for their
        // ordinary comparison forms. A terminal Boolean comparison selects
        // the new primitive predicate instead.
        let named_conversion = match modifier {
            Modifier::Custom(name) => Some(name.as_str()),
            _ => type_name.filter(|name| {
                let key = (term.path.field.segments.clone(), (*name).to_string());
                (schema.operations.contains_key(&key) || schema.custom.contains_key(&key))
                    && !matches!(parsed_literal, Literal::Bool(_))
            }),
        };
        if let Some(name) = named_conversion {
            if binding_name != "default" {
                return Err("multiple string bindings".into());
            }
            let key = (term.path.field.segments.clone(), name.to_string());
            if let Some(binding) = schema.operations.get(&key) {
                operation = Some(binding.operation.clone());
                binding_version = binding.version;
            } else {
                let binding = schema
                    .custom
                    .get(&key)
                    .ok_or_else(|| format!("unknown binding {name}"))?;
                transform = binding.transform;
                binding_version = binding.version;
            }
            binding_name = name.to_string();
            continue;
        }
        match modifier {
            Modifier::Length
            | Modifier::Null
            | Modifier::Exists
            | Modifier::NonEmpty
            | Modifier::StringType
            | Modifier::NumberType
            | Modifier::ArrayType
            | Modifier::ObjectType => {
                if derived != Derived::None {
                    return Err("incompatible derived modifiers".into());
                }
                derived = match modifier {
                    Modifier::Length => Derived::Length,
                    Modifier::Null => Derived::Null,
                    Modifier::Exists => Derived::Exists,
                    Modifier::NonEmpty => Derived::IsNonEmpty,
                    Modifier::StringType => Derived::IsString,
                    Modifier::NumberType => Derived::IsNumber,
                    Modifier::ArrayType => Derived::IsArray,
                    Modifier::ObjectType => Derived::IsObject,
                    _ => unreachable!(),
                };
            }
            Modifier::Each => {
                if each {
                    return Err("duplicate each modifier".into());
                }
                each = true;
            }
            Modifier::Lower => {
                if binding_name != "default" {
                    return Err("multiple string bindings".into());
                }
                transform = Transform::CaseFold;
                binding_name = "lower".into();
            }
            Modifier::Custom(_) => unreachable!("named conversion handled above"),
        }
    }
    if binding_name == "default" && derived == Derived::None {
        if let Some(name) = schema.defaults.get(&term.path.field.segments) {
            let key = (term.path.field.segments.clone(), name.clone());
            if name == "lower" {
                transform = Transform::CaseFold;
            } else if let Some(binding) = schema.operations.get(&key) {
                operation = Some(binding.operation.clone());
                binding_version = binding.version;
            } else if let Some(binding) = schema.custom.get(&key) {
                transform = binding.transform;
                binding_version = binding.version;
            }
            binding_name = name.clone();
        }
    }
    if derived != Derived::None
        && (transform != Transform::Identity
            || operation.is_some()
            || (derived != Derived::Length && (each || term.quantifier == Quantifier::Any)))
    {
        return Err("derived modifier cannot be combined with quantifier or text binding".into());
    }
    if each && term.quantifier == Quantifier::Any {
        return Err("each and existential operator conflict".into());
    }
    let multi = path_multi;
    if each && !multi {
        return Err("each requires a multi-valued field".into());
    }
    if term.quantifier == Quantifier::Any && !multi {
        return Err("existential comparison requires a multi-valued field".into());
    }
    let kind = match derived {
        Derived::Length => {
            if !matches!(source_kind, LogicalType::Array(_) | LogicalType::Unknown) {
                return Err("length requires an array field".into());
            }
            LogicalType::Number
        }
        Derived::Null
        | Derived::Exists
        | Derived::IsNonEmpty
        | Derived::IsString
        | Derived::IsNumber
        | Derived::IsArray
        | Derived::IsObject => LogicalType::Bool,
        Derived::None => match &source_kind {
            LogicalType::Array(inner) => *inner.clone(),
            other => other.clone(),
        },
    };
    let kind = if term.path.field.root == PathRoot::Metadata(MetadataField::RecordId)
        && transform == Transform::CaseFold
        && derived == Derived::None
    {
        LogicalType::String
    } else {
        kind
    };
    if let Some(name) = parameter_name {
        let required = if kind == LogicalType::Unknown {
            match parsed_literal {
                Literal::Null => LogicalType::Unknown,
                Literal::String(_) => LogicalType::String,
                Literal::Number(_) => LogicalType::Number,
                Literal::Bool(_) => LogicalType::Bool,
                Literal::Typed(value) => value.logical_type(),
            }
        } else {
            kind.clone()
        };
        if let Some(previous) = parameter_types.get(name) {
            if *previous != LogicalType::Unknown
                && required != LogicalType::Unknown
                && *previous != required
            {
                return Err(format!(
                    "parameter ${name} has incompatible uses: {previous:?} and {required:?}"
                ));
            }
        }
        if required != LogicalType::Unknown {
            parameter_types.insert(name.to_owned(), required);
        }
    }
    if transform != Transform::Identity
        && !matches!(kind, LogicalType::String | LogicalType::Unknown)
    {
        return Err("text binding requires string values".into());
    }
    if let Some(op) = &operation {
        if !op.accepts(&kind, term.operator, multi) {
            return Err(format!("unsupported operation for binding {binding_name}"));
        }
    }
    if derived.state_predicate().is_some() && !matches!(term.operator, Operator::Eq | Operator::Ne)
    {
        return Err("Boolean state predicate permits only equality or inequality".into());
    }
    if kind == LogicalType::Id && !matches!(term.operator, Operator::Eq | Operator::Ne) {
        return Err("ID permits only equality operators".into());
    }
    if matches!(term.operator, Operator::Contains | Operator::NotContains)
        && !matches!(kind, LogicalType::String | LogicalType::Unknown)
    {
        return Err("contains requires string values".into());
    }
    if matches!(
        term.operator,
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
    ) && matches!(kind, LogicalType::Object | LogicalType::Array(_))
    {
        return Err("ordered comparison requires scalar values".into());
    }
    if field_probe {
        return Ok(BoundTerm {
            path: term.path.field.clone(),
            query_path: term.path.clone(),
            rhs: None,
            related: None,
            kind,
            source_kind,
            operator: term.operator,
            quantifier: term.quantifier,
            literal: Value::Null,
            period: None,
            prepared_contains: None,
            gram_request: None,
            derived,
            transform,
            operation,
            matcher: None,
            binding_name,
            binding_version,
            indexed: field.indexed,
            multi,
            implicit_array,
            span: term.span,
        });
    }
    if transform != Transform::Identity
        && !matches!(parsed_literal, Literal::String(_) | Literal::Null)
    {
        return Err("text binding requires a string literal".into());
    }
    if matches!(term.operator, Operator::Contains | Operator::NotContains)
        && !matches!(parsed_literal, Literal::String(_))
    {
        return Err("contains requires a string literal".into());
    }
    let period = if let Some(source) = period_operand {
        if derived != Derived::None || transform != Transform::Identity || operation.is_some() {
            return Err("temporal period cannot use derived or text bindings".into());
        }
        Some(source.for_type(&kind)?)
    } else {
        None
    };
    let literal = if let Some(bounds) = &period {
        bounds.start.clone()
    } else if let Some(op) = &operation {
        op.prepare_literal(parsed_literal, &kind)?
    } else {
        prepare_literal(parsed_literal, &kind, transform)?
    };
    if matches!(literal, Value::Null) && !matches!(term.operator, Operator::Eq | Operator::Ne) {
        return Err("null permits only equality operators".into());
    }
    if derived.state_predicate().is_some() && !matches!(literal, Value::Bool(_)) {
        return Err("Boolean state predicate requires a Boolean literal".into());
    }
    if derived == Derived::Length && !matches!(literal, Value::Number(_)) {
        return Err("invalid derived literal".into());
    }
    let matcher = operation
        .as_ref()
        .map(|op| {
            op.prepare_filter_matcher(&literal, term.operator)
                .map(|prepared| {
                    prepared.unwrap_or_else(|| {
                        Arc::new(MappedFilterMatcher {
                            operation: op.clone(),
                            literal: literal.clone(),
                            operator: term.operator,
                            text_equivalent: op.value_text_equivalent(),
                        })
                    })
                })
        })
        .transpose()?;
    let gram_request = if derived == Derived::None
        && matches!(term.operator, Operator::Eq | Operator::Contains)
        && let Value::String(text) = &literal
        && !text.contains(['%', '\\'])
    {
        Some(BigramRequest::new(text))
    } else {
        None
    };
    let prepared_contains = if matcher.is_none()
        && matches!(term.operator, Operator::Contains | Operator::NotContains)
    {
        match &literal {
            Value::String(pattern) => Some(PreparedContains::new(pattern)),
            _ => None,
        }
    } else {
        None
    };
    Ok(BoundTerm {
        path: term.path.field.clone(),
        query_path: term.path.clone(),
        rhs: None,
        related: None,
        kind,
        source_kind,
        operator: term.operator,
        quantifier: term.quantifier,
        literal,
        period,
        prepared_contains,
        gram_request,
        derived,
        transform,
        operation,
        matcher,
        binding_name,
        binding_version,
        indexed: field.indexed,
        multi,
        implicit_array,
        span: term.span,
    })
}

pub(crate) fn bind_field_probe(
    term: &Term,
    schema: &Schema,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<BoundTerm, String> {
    bind_literal_term(term, schema, parameter_types, true)
}

fn prepare_literal(
    literal: &Literal,
    kind: &LogicalType,
    transform: Transform,
) -> Result<Value, String> {
    let value = match (literal, kind) {
        (Literal::Null, _) => Value::Null,
        (Literal::String(s), LogicalType::String) => Value::String(s.clone()),
        (Literal::String(s), LogicalType::Id) => Value::parse_typed_string(s, kind)?,
        (Literal::String(s), LogicalType::Date | LogicalType::Datetime) => {
            Value::parse_typed_string(s, kind)?
        }
        (Literal::Number(n), LogicalType::Number) => Value::Number(n.clone()),
        (Literal::Number(Number::Integer(n)), LogicalType::Unixtime) => {
            Value::Unixtime(i64::try_from(*n).map_err(|_| "unixtime out of range")?)
        }
        (Literal::Bool(b), LogicalType::Bool) => Value::Bool(*b),
        (Literal::String(s), LogicalType::Unknown) => Value::String(s.clone()),
        (Literal::Number(n), LogicalType::Unknown) => Value::Number(n.clone()),
        (Literal::Bool(b), LogicalType::Unknown) => Value::Bool(*b),
        (Literal::Typed(value), LogicalType::Unknown)
            if !matches!(value, Value::Object(_) | Value::Array(_)) =>
        {
            value.clone()
        }
        (Literal::Typed(value), expected)
            if !matches!(value, Value::Null | Value::Object(_) | Value::Array(_))
                && value.logical_type() == *expected =>
        {
            value.clone()
        }
        _ => return Err(format!("literal is incompatible with {kind:?}")),
    };
    Ok(match value {
        Value::String(s) if transform == Transform::CaseFold => Value::String(fold(&s)),
        other => other,
    })
}

/// Record data and inventory metadata available during evaluation.
use super::*;
