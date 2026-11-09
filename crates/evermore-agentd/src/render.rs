use serde_json::Value;

pub fn resolve_path<'a>(context: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = context;
    for part in path.split('.') {
        if part.is_empty() {
            return None;
        }
        current = match current {
            Value::Object(object) => object.get(part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

pub fn render_value(value: &Value, context: &Value) -> Result<Value, String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if let Some(reference) = whole_reference(trimmed) {
                return resolve_path(context, reference)
                    .cloned()
                    .ok_or_else(|| format!("template reference '{reference}' has no value"));
            }
            Ok(Value::String(render_text(text, context)?))
        }
        Value::Array(items) => items
            .iter()
            .map(|item| render_value(item, context))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(object) => {
            let mut rendered = serde_json::Map::new();
            for (key, item) in object {
                rendered.insert(key.clone(), render_value(item, context)?);
            }
            Ok(Value::Object(rendered))
        }
        other => Ok(other.clone()),
    }
}

pub fn render_text(template: &str, context: &Value) -> Result<String, String> {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err("unclosed template reference".into());
        };
        let reference = after[..end].trim();
        let value = resolve_path(context, reference)
            .ok_or_else(|| format!("template reference '{reference}' has no value"))?;
        output.push_str(&display_value(value));
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

fn whole_reference(text: &str) -> Option<&str> {
    let inner = text.strip_prefix("{{")?.strip_suffix("}}")?;
    let reference = inner.trim();
    if reference.is_empty() || reference.contains("{{") || reference.contains("}}") {
        return None;
    }
    Some(reference)
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub fn validate_inputs(schema: &Value, inputs: &Value) -> Result<(), String> {
    let object = inputs
        .as_object()
        .ok_or_else(|| "recipe inputs must be a JSON object".to_string())?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| "recipe inputs schema has no properties".to_string())?;
    for key in object.keys() {
        if !properties.contains_key(key) {
            return Err(format!("unknown recipe input '{key}'"));
        }
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required {
            let name = name
                .as_str()
                .ok_or_else(|| "recipe required list is invalid".to_string())?;
            if !object.contains_key(name) {
                return Err(format!("missing required recipe input '{name}'"));
            }
        }
    }
    for (name, property) in properties {
        if let Some(value) = object.get(name) {
            validate_value(name, property, value)?;
        }
    }
    Ok(())
}

fn validate_value(name: &str, schema: &Value, value: &Value) -> Result<(), String> {
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("input '{name}' schema has no type"))?;
    match kind {
        "string" => {
            let text = value
                .as_str()
                .ok_or_else(|| format!("input '{name}' must be a string"))?;
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
                if text.chars().count() < min as usize {
                    return Err(format!("input '{name}' is shorter than {min} characters"));
                }
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
                if text.chars().count() > max as usize {
                    return Err(format!("input '{name}' is longer than {max} characters"));
                }
            }
        }
        "integer" => {
            let number = value
                .as_i64()
                .ok_or_else(|| format!("input '{name}' must be an integer"))?;
            if let Some(min) = schema.get("minimum").and_then(Value::as_i64) {
                if number < min {
                    return Err(format!("input '{name}' must be at least {min}"));
                }
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_i64) {
                if number > max {
                    return Err(format!("input '{name}' must be at most {max}"));
                }
            }
        }
        "boolean" => {
            value
                .as_bool()
                .ok_or_else(|| format!("input '{name}' must be a boolean"))?;
        }
        other => return Err(format!("input '{name}' has unsupported type '{other}'")),
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if !values.contains(value) {
            return Err(format!("input '{name}' is not one of the allowed values"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_embedded_and_typed_references() {
        let context = json!({ "choose": { "workspace": 3, "focus": true } });
        assert_eq!(
            render_text("workspace {{choose.workspace}}", &context).unwrap(),
            "workspace 3"
        );
        assert_eq!(
            render_value(&json!({"workspace": "{{choose.workspace}}"}), &context).unwrap(),
            json!({"workspace": 3})
        );
    }

    #[test]
    fn rejects_unknown_reference() {
        let error = render_text("{{choose.missing}}", &json!({"choose": {}})).unwrap_err();
        assert!(error.contains("no value"));
    }

    #[test]
    fn validates_input_types_and_bounds() {
        let schema = json!({
            "type": "object",
            "properties": {
                "workspace": {"type": "integer", "minimum": 1, "maximum": 9},
                "focus": {"type": "boolean"}
            },
            "required": ["workspace"]
        });
        validate_inputs(&schema, &json!({"workspace": 3, "focus": true})).unwrap();
        assert!(validate_inputs(&schema, &json!({"focus": true})).is_err());
        assert!(validate_inputs(&schema, &json!({"workspace": 12})).is_err());
        assert!(validate_inputs(&schema, &json!({"workspace": "3"})).is_err());
        assert!(validate_inputs(&schema, &json!({"workspace": 3, "extra": 1})).is_err());
    }
}
