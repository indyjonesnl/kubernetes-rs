// CEL (Common Expression Language) evaluation engine
//
// This module provides CEL expression evaluation for ValidatingAdmissionPolicy

use crate::cel_struct::{self, CompiledExpression};
use crate::resources::crd::{JSONSchemaProps, JSONSchemaPropsOrArray, JSONSchemaPropsOrBool};
use anyhow::{anyhow, Result};
use cel::{
    objects::{Key, Map, OptionalValue},
    Value,
};
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;

/// CELEvaluator evaluates CEL expressions with given context
pub struct CELEvaluator {
    // Cache compiled programs for performance
    program_cache: HashMap<String, CompiledExpression>,
}

impl CELEvaluator {
    /// Create a new CEL evaluator
    pub fn new() -> Self {
        Self {
            program_cache: HashMap::new(),
        }
    }

    /// Compile (or fetch from the cache) `expression`. `Object{}` /
    /// `JSONPatch{}` initializers are checked and their struct types
    /// collected; see [`cel_struct`].
    fn compiled(&mut self, expression: &str) -> Result<&CompiledExpression> {
        if !self.program_cache.contains_key(expression) {
            // `ObserveCompilation` after `env.Compile` (admission/plugin/cel/compile.go).
            let started = std::time::Instant::now();
            let compiled = cel_struct::compile(expression);
            crate::cel_metrics::observe_compilation(started.elapsed());
            let prog = compiled
                .map_err(|e| anyhow!("Failed to compile CEL expression '{}': {}", expression, e))?;
            self.program_cache.insert(expression.to_string(), prog);
        }
        Ok(&self.program_cache[expression])
    }

    /// Evaluate a CEL expression with the given context
    ///
    /// # Arguments
    /// * `expression` - The CEL expression to evaluate
    /// * `context` - Variables to make available in the CEL environment
    ///
    /// # Returns
    /// The result of the evaluation as a boolean, or an error
    pub fn evaluate(&mut self, expression: &str, context: &CELContext) -> Result<bool> {
        // Get or compile the program
        let program = self.compiled(expression)?;

        // Create CEL context
        let mut cel_context = cel_struct::new_context_for(program);

        // Add variables to context
        for (key, value) in &context.variables {
            let _ = cel_context.add_variable(key.clone(), value.clone());
        }

        // Execute the program
        let result = observed_eval(|| cel_struct::execute(program, &cel_context))
            .map_err(|e| anyhow!("Failed to execute CEL expression '{}': {}", expression, e))?;

        // Convert result to boolean
        match result {
            Value::Bool(b) => Ok(b),
            _ => Err(anyhow!(
                "CEL expression did not return a boolean: {:?}",
                result
            )),
        }
    }

    /// Evaluate a CEL expression that returns a string (for messages, audit annotations, etc.)
    pub fn evaluate_string(&mut self, expression: &str, context: &CELContext) -> Result<String> {
        // Get or compile the program
        let program = self.compiled(expression)?;

        // Create CEL context
        let mut cel_context = cel_struct::new_context_for(program);

        // Add variables to context
        for (key, value) in &context.variables {
            let _ = cel_context.add_variable(key.clone(), value.clone());
        }

        // Execute the program
        let result = observed_eval(|| cel_struct::execute(program, &cel_context))
            .map_err(|e| anyhow!("Failed to execute CEL expression '{}': {}", expression, e))?;

        // Convert result to string
        match result {
            Value::String(s) => Ok(s.to_string()),
            Value::Int(i) => Ok(i.to_string()),
            Value::UInt(u) => Ok(u.to_string()),
            Value::Float(f) => Ok(f.to_string()),
            Value::Bool(b) => Ok(b.to_string()),
            _ => Ok(format!("{:?}", result)),
        }
    }

    /// Evaluate a CEL expression and return the raw Value (for VAP variables)
    pub fn evaluate_to_value(&mut self, expression: &str, context: &CELContext) -> Result<Value> {
        let program = self.compiled(expression)?;

        let mut cel_context = cel_struct::new_context_for(program);
        for (key, value) in &context.variables {
            let _ = cel_context.add_variable(key.clone(), value.clone());
        }

        observed_eval(|| cel_struct::execute(program, &cel_context))
            .map_err(|e| anyhow!("Failed to execute CEL expression '{}': {}", expression, e))
    }

    /// Like [`Self::evaluate_to_value`] but keeps the struct values (with
    /// their type names), for values that are bound into another evaluation.
    pub fn evaluate_to_value_raw(
        &mut self,
        expression: &str,
        context: &CELContext,
    ) -> Result<Value> {
        let program = self.compiled(expression)?;
        let mut cel_context = cel_struct::new_context_for(program);
        for (key, value) in &context.variables {
            let _ = cel_context.add_variable(key.clone(), value.clone());
        }
        observed_eval(|| cel_struct::execute_raw(program, &cel_context))
            .map_err(|e| anyhow!("Failed to execute CEL expression '{}': {}", expression, e))
    }

    /// Evaluate with the cel-go runtime cost tracker (`cel.CostTracking`),
    /// returning the result and `EvalDetails.ActualCost()` (#3323). See
    /// [`crate::cel_cost`].
    pub fn evaluate_with_cost(
        &mut self,
        expression: &str,
        context: &CELContext,
        opts: crate::cel_cost::CostOptions,
    ) -> (
        std::result::Result<Value, crate::cel_cost::CostEvalError>,
        u64,
    ) {
        let (r, cost) = self.evaluate_with_cost_raw(expression, context, opts);
        (r.map(|v| cel_struct::strip_type_markers(&v)), cost)
    }

    /// [`Self::evaluate_with_cost`] keeping the struct values (for values
    /// bound into another evaluation, or checked by the caller).
    pub fn evaluate_with_cost_raw(
        &mut self,
        expression: &str,
        context: &CELContext,
        opts: crate::cel_cost::CostOptions,
    ) -> (
        std::result::Result<Value, crate::cel_cost::CostEvalError>,
        u64,
    ) {
        let program = match self.compiled(expression) {
            Ok(p) => p,
            Err(e) => {
                return (
                    Err(crate::cel_cost::CostEvalError::Exec(
                        cel::ExecutionError::function_error("compile", e.to_string()),
                    )),
                    0,
                )
            }
        };
        let mut cel_context = cel_struct::new_context_for(program);
        for (key, value) in &context.variables {
            let _ = cel_context.add_variable(key.clone(), value.clone());
        }
        observed_eval(|| crate::cel_cost::eval_with_cost(program.expression(), &cel_context, opts))
    }

    /// [`Self::evaluate_with_cost`] with `variables` resolved lazily through
    /// `lazy` (`composition.go`, see [`crate::cel_cost::LazyVariables`]): the
    /// context must not bind `variables` itself. The cost of the variables the
    /// expression read is in `lazy` (`GetAndResetCost`), not in the returned
    /// cost.
    pub fn evaluate_with_cost_lazy(
        &mut self,
        expression: &str,
        context: &CELContext,
        opts: crate::cel_cost::CostOptions,
        lazy: &crate::cel_cost::LazyVariables,
    ) -> (
        std::result::Result<Value, crate::cel_cost::CostEvalError>,
        u64,
    ) {
        let program = match self.compiled(expression) {
            Ok(p) => p,
            Err(e) => {
                return (
                    Err(crate::cel_cost::CostEvalError::Exec(
                        cel::ExecutionError::function_error("compile", e.to_string()),
                    )),
                    0,
                )
            }
        };
        let mut cel_context = cel_struct::new_context_for(program);
        for (key, value) in &context.variables {
            let _ = cel_context.add_variable(key.clone(), value.clone());
        }
        let (r, cost) = observed_eval(|| {
            crate::cel_cost::eval_with_cost_lazy(
                program.expression(),
                &cel_context,
                opts,
                Some(lazy),
            )
        });
        (r.map(|v| cel_struct::strip_type_markers(&v)), cost)
    }

    /// Evaluate a MAP patch expression and run upstream's
    /// `CheckTypeNamesMatchFieldPathNames` on the result; a mismatch is
    /// `unexpected type name ...` (the caller adds `type mismatch: `).
    pub fn evaluate_to_value_checked(
        &mut self,
        expression: &str,
        context: &CELContext,
        mode: cel_struct::TypeCheck,
    ) -> Result<Value> {
        let program = self.compiled(expression)?;
        let mut cel_context = cel_struct::new_context_for(program);
        for (key, value) in &context.variables {
            let _ = cel_context.add_variable(key.clone(), value.clone());
        }
        observed_eval(|| cel_struct::execute_checked(program, &cel_context, mode)).map_err(|e| {
            if e.starts_with("unexpected type name") {
                anyhow!("type mismatch: {e}")
            } else {
                anyhow!("Failed to execute CEL expression '{}': {}", expression, e)
            }
        })
    }

    /// Type-check a CEL expression without executing it
    pub fn type_check(&mut self, expression: &str) -> Result<()> {
        cel_struct::compile(expression)
            .map_err(|e| anyhow!("Failed to compile CEL expression '{}': {}", expression, e))?;
        Ok(())
    }
}

impl Default for CELEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// A CEL error that comes from a declaration this server does not supply,
/// rather than from the expression being malformed.
/// `ObserveEvaluation` around `prog.ContextEval`
/// (admission/plugin/cel/validator.go, cel/common).
fn observed_eval<T>(f: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let r = f();
    crate::cel_metrics::observe_evaluation(started.elapsed());
    r
}

fn is_missing_declaration(err: &str) -> bool {
    let err = err.to_lowercase();
    err.contains("no such key")
        || err.contains("not found")
        || err.contains("undeclared")
        || err.contains("undefined")
        || err.contains("no matching overload")
}

/// Why `expression` does not compile, if it does not.
///
/// Upstream compiles against a typed CEL environment
/// (`validateMatchConditionsExpression`, validation.go:1100). kubernetes-rs has
/// no typed environment, so this compiles with the plain `cel` crate and
/// tolerates the errors that come from the missing declarations rather than
/// from the expression itself.
pub fn compile_failure(expression: &str) -> Option<String> {
    compile_failure_in(expression, true)
}

/// As [`compile_failure`], but only parses. The MutatingAdmissionPolicy
/// expressions use `Object{...}` / `JSONPatch{...}` struct initializers, which
/// the `cel` crate parses but panics on when executed ("Support structs!"),
/// so the execute step that catches webhook match-condition errors cannot be
/// applied to them.
pub fn parse_failure(expression: &str) -> Option<String> {
    compile_failure_in(expression, false)
}

fn compile_failure_in(expression: &str, execute: bool) -> Option<String> {
    // The antlr4rust parser panics on some invalid expressions instead of
    // returning `Err`.
    let source = expression.to_string();
    let program = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cel::Program::compile(&source)
    })) {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            // The CEL crate type-checks at compile time and rejects references
            // to declarations we do not supply (`object.metadata`), which are
            // valid at admission time.
            return (!is_missing_declaration(&e.to_string()))
                .then(|| format!("compilation failed: {e}"));
        }
        Err(_) => {
            return Some(format!(
                "compilation failed: invalid CEL expression '{expression}'"
            ))
        }
    };

    if !execute {
        return None;
    }

    // The CEL crate's parser accepts some expressions Kubernetes rejects.
    // Executing with an empty context catches the genuinely invalid ones.
    let ctx = cel::Context::default();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| program.execute(&ctx))) {
        Ok(Ok(_)) => None,
        Ok(Err(e)) => {
            (!is_missing_declaration(&e.to_string())).then(|| format!("compilation failed: {e}"))
        }
        Err(_) => Some(format!(
            "compilation failed: invalid CEL expression '{expression}'"
        )),
    }
}

/// CELContext holds variables available to CEL expressions
#[derive(Debug, Clone)]
pub struct CELContext {
    pub variables: HashMap<String, Value>,
}

impl CELContext {
    /// Create a new empty CEL context
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }

    /// Create a CEL context for admission control
    ///
    /// Provides standard variables:
    /// - `object`: The object being created/updated/deleted
    /// - `oldObject`: The existing object (for UPDATE operations)
    /// - `request`: The admission request
    /// - `params`: Parameters from the binding
    pub fn for_admission(
        object: &serde_json::Value,
        old_object: Option<&serde_json::Value>,
        params: Option<&serde_json::Value>,
    ) -> Result<Self> {
        let mut context = Self::new();

        // Add object
        context.add_json_variable("object", object)?;

        // Add oldObject if present
        if let Some(old) = old_object {
            context.add_json_variable("oldObject", old)?;
        }

        // Add params if present
        if let Some(p) = params {
            context.add_json_variable("params", p)?;
        }

        // Add request metadata (simplified for now)
        let request = serde_json::json!({
            "operation": "CREATE", // This should be dynamic
            "userInfo": {},
            "namespace": "",
        });
        context.add_json_variable("request", &request)?;

        Ok(context)
    }

    /// Add a variable to the context
    pub fn add_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }

    /// Bind `name` to a CEL optional: `optional.of(value)` when `value` is
    /// `Some`, else `optional.none()`. Port of
    /// `validationActivationWithOldSelf`'s `optionalOldSelfActivation`
    /// (`apiextensions-apiserver/pkg/apiserver/schema/cel/validation.go:776-793`,
    /// `types.OptionalNone` / `types.OptionalOf`).
    pub fn add_optional_json_variable(
        &mut self,
        name: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<()> {
        let opt = match value {
            Some(v) => OptionalValue::of(json_to_cel_value(v)?),
            None => OptionalValue::none(),
        };
        self.variables
            .insert(name.to_string(), Value::Opaque(Arc::new(opt)));
        Ok(())
    }

    /// Like [`Self::add_json_variable`] but converts `value` with its
    /// structural schema ([`json_to_cel_value_with_schema`]).
    pub fn add_json_variable_with_schema(
        &mut self,
        name: &str,
        value: &serde_json::Value,
        schema: &JSONSchemaProps,
    ) -> Result<()> {
        let cel_value = json_to_cel_value_with_schema(value, schema)?;
        self.variables.insert(name.to_string(), cel_value);
        Ok(())
    }

    /// Like [`Self::add_optional_json_variable`] with schema-aware conversion.
    pub fn add_optional_json_variable_with_schema(
        &mut self,
        name: &str,
        value: Option<&serde_json::Value>,
        schema: &JSONSchemaProps,
    ) -> Result<()> {
        let opt = match value {
            Some(v) => OptionalValue::of(json_to_cel_value_with_schema(v, schema)?),
            None => OptionalValue::none(),
        };
        self.variables
            .insert(name.to_string(), Value::Opaque(Arc::new(opt)));
        Ok(())
    }

    /// Add a JSON value as a variable (converts to CEL Value)
    pub fn add_json_variable(&mut self, name: &str, value: &serde_json::Value) -> Result<()> {
        let cel_value = json_to_cel_value(value)?;
        self.variables.insert(name.to_string(), cel_value);
        Ok(())
    }
}

impl Default for CELContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a serde_json::Value to a CEL Value
pub fn json_to_cel_value(value: &serde_json::Value) -> Result<Value> {
    match value {
        serde_json::Value::Null => Ok(Value::Null),
        serde_json::Value::Bool(b) => Ok(Value::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else if let Some(u) = n.as_u64() {
                Ok(Value::UInt(u))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Float(f))
            } else {
                Err(anyhow!("Invalid number: {}", n))
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(std::sync::Arc::new(s.clone()))),
        serde_json::Value::Array(arr) => {
            let cel_values: Result<Vec<Value>> = arr.iter().map(json_to_cel_value).collect();
            Ok(Value::List(Arc::new(cel_values?)))
        }
        serde_json::Value::Object(obj) => {
            let mut map = HashMap::new();
            for (k, v) in obj {
                let key = Key::String(Arc::new(k.clone()));
                map.insert(key, json_to_cel_value(v)?);
            }
            Ok(Value::Map(Map { map: Arc::new(map) }))
        }
    }
}

/// `strfmt.ParseDuration` (`k8s.io/kube-openapi/pkg/validation/strfmt/duration.go`):
/// Go's `time.ParseDuration` first, else scala-style `<int><unit>` groups
/// (`ns|nano`, `us|µs|micro`, `ms|milli`, `s|sec`, `m|min`, `h|hr|hour`,
/// `d|day`, `w|wk|week`; the last variant of a unit matches as a prefix).
/// Returns nanoseconds.
fn strfmt_parse_duration(cand: &str) -> std::result::Result<i64, String> {
    if let Ok(n) = crate::go_duration::parse_go_duration(cand) {
        return Ok(n);
    }
    const UNITS: [(&[&str], i64); 8] = [
        (&["ns", "nano"], 1),
        (&["us", "µs", "micro"], 1_000),
        (&["ms", "milli"], 1_000_000),
        (&["s", "sec"], 1_000_000_000),
        (&["m", "min"], 60_000_000_000),
        (&["h", "hr", "hour"], 3_600_000_000_000),
        (&["d", "day"], 86_400_000_000_000),
        (&["w", "wk", "week"], 604_800_000_000_000),
    ];
    static MATCHER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = MATCHER.get_or_init(|| regex::Regex::new(r"((\d+)\s*([A-Za-zµ]+))").expect("valid"));
    let mut dur: i64 = 0;
    let mut ok = false;
    for m in re.captures_iter(cand) {
        let factor: i64 = m[2].parse().map_err(|e| format!("{e}"))?;
        let unit = m[3].trim().to_lowercase();
        for (variants, multiplier) in UNITS {
            let last = variants.len() - 1;
            for (i, variant) in variants.iter().enumerate() {
                if (i == last && unit.starts_with(variant)) || variant.to_lowercase() == unit {
                    ok = true;
                    dur = dur.wrapping_add(factor.wrapping_mul(multiplier));
                }
            }
        }
    }
    if ok {
        Ok(dur)
    } else {
        Err(format!("unable to parse {cand} as duration"))
    }
}

/// `strfmt.ParseDateTime` (`.../strfmt/time.go:67-81`): "" is the Unix epoch;
/// otherwise RFC3339 (with or without fractional seconds) or
/// `2006-01-02T15:04:05` (no zone, UTC).
fn strfmt_parse_date_time(
    s: &str,
) -> std::result::Result<chrono::DateTime<chrono::FixedOffset>, String> {
    if s.is_empty() {
        return Ok(chrono::DateTime::UNIX_EPOCH.fixed_offset());
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .or_else(|e| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                .map(|n| n.and_utc().fixed_offset())
                .map_err(|_| e)
        })
        .map_err(|e| e.to_string())
}

/// Schema-aware conversion of an unstructured value to a CEL value. Port of
/// `UnstructuredToVal`
/// (`staging/src/k8s.io/apiserver/pkg/cel/common/valuesunstructured.go:38-210`):
/// `x-kubernetes-int-or-string` is a string or an int; `string` with format
/// `duration` / `date` / `date-time` / `byte` becomes a CEL duration /
/// timestamp / timestamp / bytes; `number` is a double (whole numbers
/// included); objects and arrays are converted member by member with their
/// property / `additionalProperties` / `items` schema. Type mismatches are
/// errors, with upstream's messages.
///
/// Deliberate differences, each leniency rather than new behaviour:
/// - `null` converts to CEL null regardless of `nullable` (upstream errors
///   for `nullable=false`; nulls are pruned before validation here).
/// - Members with no schema (unknown keys, schemaless nodes, nodes marked
///   `x-kubernetes-preserve-unknown-fields`) convert schema-blind, where
///   upstream wraps them in an opaque `unknownPreserved` value.
/// - `x-kubernetes-list-type` map/set list equality semantics are not
///   modelled; lists are plain CEL lists.
pub fn json_to_cel_value_with_schema(
    value: &serde_json::Value,
    schema: &JSONSchemaProps,
) -> Result<Value> {
    use serde_json::Value as J;
    if value.is_null() {
        return Ok(Value::Null);
    }
    if schema.x_kubernetes_int_or_string == Some(true) {
        return match value {
            J::String(s) => Ok(Value::String(Arc::new(s.clone()))),
            J::Number(n) if n.as_i64().is_some() => Ok(Value::Int(n.as_i64().unwrap())),
            _ => Err(anyhow!(
                "invalid data, expected XIntOrString value to be either a string or integer"
            )),
        };
    }
    let type_name = |v: &J| match v {
        J::Null => "<nil>",
        J::Bool(_) => "bool",
        J::Number(n) if n.is_f64() => "float64",
        J::Number(_) => "int64",
        J::String(_) => "string",
        J::Array(_) => "[]interface {}",
        J::Object(_) => "map[string]interface {}",
    };
    match schema.type_.as_deref().unwrap_or("") {
        "object" => {
            let obj = value.as_object().ok_or_else(|| {
                anyhow!("invalid data, expected a map for the provided schema with type=object")
            })?;
            let addl = match schema.additional_properties.as_deref() {
                Some(JSONSchemaPropsOrBool::Schema(s)) => Some(s),
                _ => None,
            };
            let mut map = HashMap::new();
            for (k, v) in obj {
                let child = schema.properties.as_ref().and_then(|p| p.get(k)).or(
                    if schema.properties.is_none() {
                        addl
                    } else {
                        None
                    },
                );
                let cv = match child {
                    Some(cs) => json_to_cel_value_with_schema(v, cs)?,
                    None => json_to_cel_value(v)?,
                };
                map.insert(Key::String(Arc::new(k.clone())), cv);
            }
            Ok(Value::Map(Map { map: Arc::new(map) }))
        }
        "array" => {
            let arr = value.as_array().ok_or_else(|| {
                anyhow!("invalid data, expected an array for the provided schema with type=array")
            })?;
            let items = match schema.items.as_deref() {
                Some(JSONSchemaPropsOrArray::Schema(s)) => s,
                _ => {
                    return Err(anyhow!(
                        "invalid array type, expected Items with a non-empty Schema"
                    ))
                }
            };
            let vals: Result<Vec<Value>> = arr
                .iter()
                .map(|v| json_to_cel_value_with_schema(v, items))
                .collect();
            Ok(Value::List(Arc::new(vals?)))
        }
        "string" => {
            let s = value.as_str().ok_or_else(|| {
                anyhow!("invalid data, expected string, got {}", type_name(value))
            })?;
            match schema.format.as_deref().unwrap_or("") {
                "duration" => {
                    let nanos = strfmt_parse_duration(s)
                        .map_err(|e| anyhow!("Invalid duration {s}: {e}"))?;
                    Ok(Value::Duration(chrono::Duration::nanoseconds(nanos)))
                }
                "date" => {
                    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .map_err(|e| anyhow!("Invalid date formatted string {s}: {e}"))?;
                    let dt = d.and_hms_opt(0, 0, 0).expect("midnight is valid").and_utc();
                    Ok(Value::Timestamp(dt.fixed_offset()))
                }
                "date-time" => {
                    let dt = strfmt_parse_date_time(s)
                        .map_err(|e| anyhow!("Invalid date-time formatted string {s}: {e}"))?;
                    Ok(Value::Timestamp(dt))
                }
                "byte" => {
                    use base64::Engine as _;
                    let b = base64::engine::general_purpose::STANDARD
                        .decode(s)
                        .map_err(|e| anyhow!("Invalid byte formatted string {s}: {e}"))?;
                    Ok(Value::Bytes(Arc::new(b)))
                }
                _ => Ok(Value::String(Arc::new(s.to_string()))),
            }
        }
        "number" => match value {
            J::Number(n) => Ok(Value::Float(n.as_f64().unwrap_or(f64::NAN))),
            _ => Err(anyhow!(
                "invalid data, expected float, got {}",
                type_name(value)
            )),
        },
        "integer" => match value {
            J::Number(n) if n.is_i64() => Ok(Value::Int(n.as_i64().unwrap())),
            J::Number(n) if n.is_u64() => Ok(Value::UInt(n.as_u64().unwrap())),
            _ => Err(anyhow!(
                "invalid data, expected int, got {}",
                type_name(value)
            )),
        },
        "boolean" => match value {
            J::Bool(b) => Ok(Value::Bool(*b)),
            _ => Err(anyhow!(
                "invalid data, expected bool, got {}",
                type_name(value)
            )),
        },
        // No type: x-kubernetes-preserve-unknown-fields or an untyped node.
        _ => json_to_cel_value(value),
    }
}
