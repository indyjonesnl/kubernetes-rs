// Field Selector implementation for server-side filtering
//
// Enables filtering resources by field values like:
// - status.phase=Running
// - spec.nodeName=node-1
// - metadata.namespace=default

use serde_json::Value;
use std::fmt;

/// Field selector for filtering resources by field values
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSelector {
    /// Individual field requirements
    pub requirements: Vec<FieldRequirement>,
}

impl FieldSelector {
    /// Parse a field selector string
    ///
    /// Format: field1=value1,field2!=value2
    /// Operators: =, ==, !=
    ///
    /// Examples:
    /// - "status.phase=Running"
    /// - "spec.nodeName=node-1,status.phase!=Failed"
    /// - "metadata.namespace=default"
    pub fn parse(selector: &str) -> Result<Self, FieldSelectorError> {
        // `parseSelector` (apimachinery/pkg/fields/selector.go): split on
        // unescaped commas, sort the terms, skip empty ones.
        let mut parts = split_terms(selector);
        parts.sort();
        let mut requirements = Vec::new();
        for part in parts {
            if part.is_empty() {
                continue;
            }
            let Some((lhs, op, rhs)) = split_term(&part) else {
                return Err(FieldSelectorError::InvalidSelector {
                    selector: selector.to_string(),
                    part,
                });
            };
            let value = unescape_value(rhs)?;
            requirements.push(FieldRequirement {
                field: lhs.to_string(),
                operator: if op == "!=" {
                    FieldOperator::NotEquals
                } else {
                    FieldOperator::Equals
                },
                value,
            });
        }
        Ok(Self { requirements })
    }

    /// Check if a resource matches this field selector
    pub fn matches(&self, resource: &Value) -> bool {
        // Empty selector matches everything
        if self.requirements.is_empty() {
            return true;
        }

        // All requirements must match
        self.requirements.iter().all(|req| req.matches(resource))
    }

    /// Get the individual requirements
    pub fn requirements(&self) -> &[FieldRequirement] {
        &self.requirements
    }

    /// Check if this is an empty selector
    pub fn is_empty(&self) -> bool {
        self.requirements.is_empty()
    }
}

/// A single field requirement
#[derive(Debug, Clone, PartialEq)]
pub struct FieldRequirement {
    /// Field path (e.g., "status.phase", "spec.nodeName")
    pub field: String,
    /// Operator (=, ==, !=)
    pub operator: FieldOperator,
    /// Expected value
    pub value: String,
}

impl FieldRequirement {
    /// Check if a resource matches this requirement
    fn matches(&self, resource: &Value) -> bool {
        let field_value = extract_field_value(resource, &self.field);

        match &self.operator {
            FieldOperator::Equals => {
                if let Some(ref fv) = field_value {
                    fv == &self.value
                } else {
                    // Field is missing/null. In Kubernetes, missing boolean fields
                    // default to false, missing string fields default to "".
                    self.value == "false" || self.value.is_empty()
                }
            }
            FieldOperator::NotEquals => {
                if let Some(ref fv) = field_value {
                    fv != &self.value
                } else {
                    // Missing field: treat as "false" or "" for comparison
                    self.value != "false" && !self.value.is_empty()
                }
            }
        }
    }

    /// Get the field path
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Get the operator
    pub fn operator(&self) -> FieldOperator {
        self.operator
    }

    /// Get the expected value
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// `EscapeValue` (selector.go): escape `\\`, `,` and `=` in a value.
pub fn escape_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | ',' | '=') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `UnescapeValue` (selector.go): reverse of [`escape_value`]; unescaped `,`
/// or `=`, an unknown escape or a trailing backslash is an error.
pub fn unescape_value(s: &str) -> Result<String, FieldSelectorError> {
    if !s.contains(['\\', ',', '=']) {
        return Ok(s.to_string());
    }
    let mut v = String::with_capacity(s.len());
    let mut in_slash = false;
    for c in s.chars() {
        if in_slash {
            match c {
                '\\' | ',' | '=' => v.push(c),
                _ => return Err(FieldSelectorError::InvalidEscapeSequence(format!("\\{c}"))),
            }
            in_slash = false;
            continue;
        }
        match c {
            '\\' => in_slash = true,
            ',' | '=' => return Err(FieldSelectorError::UnescapedRune((c, c as u32))),
            _ => v.push(c),
        }
    }
    if in_slash {
        return Err(FieldSelectorError::InvalidEscapeSequence("\\".to_string()));
    }
    Ok(v)
}

/// `splitTerms` (selector.go): split on commas that are not backslash-escaped.
pub fn split_terms(field_selector: &str) -> Vec<String> {
    if field_selector.is_empty() {
        return Vec::new();
    }
    let mut terms = Vec::new();
    let mut start = 0;
    let mut in_slash = false;
    for (i, c) in field_selector.char_indices() {
        if in_slash {
            in_slash = false;
        } else if c == '\\' {
            in_slash = true;
        } else if c == ',' {
            terms.push(field_selector[start..i].to_string());
            start = i + 1;
        }
    }
    terms.push(field_selector[start..].to_string());
    terms
}

/// `splitTerm` (selector.go): lhs, operator and rhs at the first occurrence of
/// `!=`, `==` or `=` (checked in that order at each position).
pub fn split_term(term: &str) -> Option<(&str, &'static str, &str)> {
    for (i, _) in term.char_indices() {
        let remaining = &term[i..];
        for op in ["!=", "==", "="] {
            if remaining.starts_with(op) {
                return Some((&term[..i], op, &term[i + op.len()..]));
            }
        }
    }
    None
}

/// Field selector operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldOperator {
    /// Equals (= or ==)
    Equals,
    /// Not equals (!=)
    NotEquals,
}

impl fmt::Display for FieldOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldOperator::Equals => write!(f, "="),
            FieldOperator::NotEquals => write!(f, "!="),
        }
    }
}

/// Extract a field value from a JSON object using a path
///
/// Supports nested fields like "status.phase" or "metadata.namespace"
pub fn extract_field_value(resource: &Value, field_path: &str) -> Option<String> {
    // K8s field selector aliases — some fields are shorthands for nested paths
    // Event `source` falls back to the reporting controller at selection time
    // and never mutates the object: `ToSelectableFields`,
    // pkg/registry/core/event/strategy.go:117-121 (`source := Source.Component;
    // if source == "" { source = ReportingController }`). The core/v1 JSON name
    // of ReportingController is `reportingComponent`.
    if field_path == "source" {
        let component = resource
            .get("source")
            .and_then(|s| s.get("component"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if component.is_empty() {
            // The stored core Event serializes it as `reportingComponent`
            // (resources/event.rs `reporting_component`); events.k8s.io
            // bodies carry `reportingController`.
            return Some(
                resource
                    .get("reportingComponent")
                    .or_else(|| resource.get("reportingController"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            );
        }
        return Some(component.to_string());
    }
    // strategy.go:117-121: `"reportingComponent": event.ReportingController`
    // ("use the core/v1 field name").
    // events.k8s.io/v1 selects the same value as `reportingController`
    // (events strategy ToSelectableFields); stored objects use the core name.
    if field_path == "reportingComponent" || field_path == "reportingController" {
        return Some(
            resource
                .get("reportingComponent")
                .or_else(|| resource.get("reportingController"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
    }
    // Every other key (`type`, `reason`, `involvedObject.*`, the events.k8s.io/v1
    // `reportingController`, ...) is a plain dotted path into the object.
    let resolved_path = field_path;

    let parts: Vec<&str> = resolved_path.split('.').collect();
    let mut current = resource;

    for part in parts {
        current = current.get(part)?;
    }

    // Convert the final value to string
    match current {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".to_string()),
        _ => None, // Objects and arrays don't convert to simple strings
    }
}

/// Errors that can occur during field selector operations
#[derive(Debug, thiserror::Error)]
pub enum FieldSelectorError {
    #[error("Invalid field selector requirement: {0}")]
    InvalidRequirement(String),

    /// `fmt.Errorf("invalid selector: '%s'; can't understand '%s'", ...)`.
    #[error("invalid selector: '{selector}'; can't understand '{part}'")]
    InvalidSelector { selector: String, part: String },

    /// `InvalidEscapeSequence.Error`.
    #[error("invalid field selector: invalid escape sequence: {0}")]
    InvalidEscapeSequence(String),

    /// `UnescapedRune.Error`.
    /// Go prints the rune with `%v`, i.e. its code point.
    #[error("invalid field selector: unescaped character in value: {}", .0.1)]
    UnescapedRune((char, u32)),

    #[error("Invalid operator in field selector: {0}")]
    InvalidOperator(String),

    #[error("Field not found: {0}")]
    FieldNotFound(String),
}

/// Common field selector patterns for Kubernetes resources
impl FieldSelector {
    /// Match pods by status phase
    pub fn pod_phase(phase: &str) -> Self {
        Self {
            requirements: vec![FieldRequirement {
                field: "status.phase".to_string(),
                operator: FieldOperator::Equals,
                value: phase.to_string(),
            }],
        }
    }

    /// Match pods by node name
    pub fn pod_node(node_name: &str) -> Self {
        Self {
            requirements: vec![FieldRequirement {
                field: "spec.nodeName".to_string(),
                operator: FieldOperator::Equals,
                value: node_name.to_string(),
            }],
        }
    }

    /// Match resources by namespace
    pub fn namespace(namespace: &str) -> Self {
        Self {
            requirements: vec![FieldRequirement {
                field: "metadata.namespace".to_string(),
                operator: FieldOperator::Equals,
                value: namespace.to_string(),
            }],
        }
    }

    /// Match resources by name
    pub fn name(name: &str) -> Self {
        Self {
            requirements: vec![FieldRequirement {
                field: "metadata.name".to_string(),
                operator: FieldOperator::Equals,
                value: name.to_string(),
            }],
        }
    }
}

impl fmt::Display for FieldSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .requirements
            .iter()
            .map(|req| format!("{}{}{}", req.field, req.operator, escape_value(&req.value)))
            .collect();
        write!(f, "{}", parts.join(","))
    }
}
