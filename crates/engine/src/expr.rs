use cel::{Context, Program};
use serde_json::Value;

pub const MAX_EXPRESSION_LEN: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum ExprError {
    #[error("expression trop longue (max {MAX_EXPRESSION_LEN} caractères)")]
    TooLong,
    #[error("expression invalide: {0}")]
    Parse(String),
    #[error("évaluation impossible: {0}")]
    Eval(String),
    #[error("l'expression doit produire un booléen")]
    NotBoolean,
    #[error("gabarit invalide: {0}")]
    Template(String),
}

pub fn evaluate(expression: &str, vars: &Value) -> Result<Value, ExprError> {
    if expression.len() > MAX_EXPRESSION_LEN {
        return Err(ExprError::TooLong);
    }
    let program = Program::compile(&rewrite_step_references(expression))
        .map_err(|err| ExprError::Parse(err.to_string()))?;
    let mut context = Context::default();
    if let Value::Object(map) = vars {
        for (name, value) in map {
            context.add_variable_from_value(name.as_str(), to_cel(value));
        }
    }
    let result = program
        .execute(&context)
        .map_err(|err| ExprError::Eval(err.to_string()))?;
    result
        .json()
        .map_err(|err| ExprError::Eval(err.to_string()))
}

// Les entiers JSON deviennent des `int` : CEL refuse `uint + int`, et serde produit des `uint`.
fn to_cel(value: &Value) -> cel::Value {
    match value {
        Value::Null => cel::Value::Null,
        Value::Bool(b) => cel::Value::Bool(*b),
        Value::Number(n) => n
            .as_i64()
            .map(cel::Value::Int)
            .or_else(|| n.as_u64().map(cel::Value::UInt))
            .unwrap_or_else(|| cel::Value::Float(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(text) => cel::Value::from(text.as_str()),
        Value::Array(items) => cel::Value::from(items.iter().map(to_cel).collect::<Vec<_>>()),
        Value::Object(map) => cel::Value::from(
            map.iter()
                .map(|(key, item)| (key.clone(), to_cel(item)))
                .collect::<std::collections::HashMap<String, cel::Value>>(),
        ),
    }
}

pub fn evaluate_bool(expression: &str, vars: &Value) -> Result<bool, ExprError> {
    match evaluate(expression, vars)? {
        Value::Bool(value) => Ok(value),
        _ => Err(ExprError::NotBoolean),
    }
}

// Un identifiant d'étape peut contenir `-`, que CEL lirait comme une soustraction : `steps.a-b` devient `steps["a-b"]`.
fn rewrite_step_references(expression: &str) -> String {
    let mut out = String::with_capacity(expression.len());
    let mut chars = expression.char_indices().peekable();
    let mut quote: Option<char> = None;
    let mut previous: Option<char> = None;
    while let Some((index, c)) = chars.next() {
        if let Some(open) = quote {
            out.push(c);
            if c == '\\' {
                if let Some((_, escaped)) = chars.next() {
                    out.push(escaped);
                }
            } else if c == open {
                quote = None;
            }
            previous = Some(c);
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            out.push(c);
            previous = Some(c);
            continue;
        }
        let rest = &expression[index..];
        let boundary = previous.is_none_or(|p| !(p.is_alphanumeric() || p == '_' || p == '.'));
        if boundary && rest.starts_with("steps.") {
            let id_start = index + "steps.".len();
            let id_len = expression[id_start..]
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
                .unwrap_or(expression.len() - id_start);
            if id_len > 0 {
                out.push_str("steps[\"");
                out.push_str(&expression[id_start..id_start + id_len]);
                out.push_str("\"]");
                for _ in 0.."steps.".len() + id_len - 1 {
                    chars.next();
                }
                previous = Some(']');
                continue;
            }
        }
        out.push(c);
        previous = Some(c);
    }
    out
}

pub fn render_str(template: &str, vars: &Value) -> Result<Value, ExprError> {
    let trimmed = template.trim();
    let single = trimmed
        .strip_prefix("${{")
        .and_then(|t| t.strip_suffix("}}"))
        .filter(|inner| !inner.contains("${{") && !inner.contains("}}"));
    if let Some(inner) = single {
        return evaluate(inner.trim(), vars);
    }
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let end = after
            .find("}}")
            .ok_or_else(|| ExprError::Template("`${{` sans `}}`".into()))?;
        match evaluate(after[..end].trim(), vars)? {
            Value::String(text) => out.push_str(&text),
            other => out.push_str(&other.to_string()),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(Value::String(out))
}

pub fn render(value: &Value, vars: &Value) -> Result<Value, ExprError> {
    Ok(match value {
        Value::String(text) => render_str(text, vars)?,
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| render(item, vars))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| Ok((key.clone(), render(item, vars)?)))
                .collect::<Result<_, ExprError>>()?,
        ),
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn vars() -> serde_json::Value {
        json!({
            "event": { "payload": { "status": "success", "service": "api", "count": 3, "tags": ["a", "b"] } },
            "steps": { "check-status": { "result": true }, "fetch": { "output": { "id": 42 } } }
        })
    }

    #[test]
    fn evaluates_a_comparison_on_nested_data() {
        assert_eq!(
            evaluate("event.payload.status == \"success\"", &vars()).unwrap(),
            json!(true)
        );
        assert_eq!(
            evaluate("event.payload.count > 5", &vars()).unwrap(),
            json!(false)
        );
    }

    #[test]
    fn step_ids_with_hyphens_resolve() {
        assert_eq!(
            evaluate("steps.check-status.result == true", &vars()).unwrap(),
            json!(true)
        );
        assert_eq!(
            evaluate("steps.fetch.output.id + 1", &vars()).unwrap(),
            json!(43)
        );
    }

    #[test]
    fn steps_inside_string_literals_are_left_alone() {
        assert_eq!(
            evaluate("\"steps.a-b\"", &vars()).unwrap(),
            json!("steps.a-b")
        );
    }

    #[test]
    fn supports_standard_functions_and_macros() {
        assert_eq!(
            evaluate("size(event.payload.tags)", &vars()).unwrap(),
            json!(2)
        );
        assert_eq!(
            evaluate("event.payload.tags.exists(t, t == \"b\")", &vars()).unwrap(),
            json!(true)
        );
        assert_eq!(
            evaluate("has(event.payload.missing)", &vars()).unwrap(),
            json!(false)
        );
    }

    #[test]
    fn boolean_helper_requires_a_boolean() {
        assert!(evaluate_bool("event.payload.count == 3", &vars()).unwrap());
        assert!(matches!(
            evaluate_bool("event.payload.count", &vars()),
            Err(ExprError::NotBoolean)
        ));
    }

    #[test]
    fn syntax_errors_and_unknown_variables_are_reported() {
        assert!(matches!(
            evaluate("event.payload.", &vars()),
            Err(ExprError::Parse(_))
        ));
        assert!(matches!(
            evaluate("nope.field", &vars()),
            Err(ExprError::Eval(_))
        ));
    }

    #[test]
    fn overlong_expressions_are_refused() {
        let long = format!("\"{}\" == \"\"", "x".repeat(MAX_EXPRESSION_LEN));
        assert!(matches!(evaluate(&long, &vars()), Err(ExprError::TooLong)));
    }

    #[test]
    fn unknown_functions_are_refused() {
        assert!(evaluate("getenv(\"HOME\")", &vars()).is_err());
        assert!(evaluate("event.payload.exec(\"ls\")", &vars()).is_err());
    }

    #[test]
    fn template_with_one_expression_keeps_its_type() {
        assert_eq!(
            render_str("${{ event.payload.count }}", &vars()).unwrap(),
            json!(3)
        );
        assert_eq!(
            render_str("  ${{ event.payload.tags }}  ", &vars()).unwrap(),
            json!(["a", "b"])
        );
    }

    #[test]
    fn template_with_text_renders_a_string() {
        assert_eq!(
            render_str(
                "svc=${{ event.payload.service }} n=${{ event.payload.count }}",
                &vars()
            )
            .unwrap(),
            json!("svc=api n=3")
        );
    }

    #[test]
    fn plain_strings_pass_through_untouched() {
        assert_eq!(
            render_str("rien à faire", &vars()).unwrap(),
            json!("rien à faire")
        );
        assert_eq!(
            render_str("$ {{ pas ça }}", &vars()).unwrap(),
            json!("$ {{ pas ça }}")
        );
    }

    #[test]
    fn unclosed_template_is_an_error() {
        assert!(matches!(
            render_str("a ${{ event.payload.count", &vars()),
            Err(ExprError::Template(_))
        ));
    }

    #[test]
    fn render_walks_objects_and_arrays_and_keeps_other_scalars() {
        let template = json!({
            "service": "${{ event.payload.service }}",
            "list": ["${{ event.payload.count }}", 7, true, null],
            "nested": { "id": "id-${{ steps.fetch.output.id }}" }
        });
        assert_eq!(
            render(&template, &vars()).unwrap(),
            json!({
                "service": "api",
                "list": [3, 7, true, null],
                "nested": { "id": "id-42" }
            })
        );
    }
}
