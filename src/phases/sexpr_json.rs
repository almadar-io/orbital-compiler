//! SExpression → serde_json::Value bridge
//!
//! Shared helper used by the compose phase and the closed-circuit validator to
//! convert an `SExpression` subtree into a `serde_json::Value`.

use orbital_core_base::schema::SExpression;

/// Convert an `SExpression` object map into a JSON object map.
pub fn sexpr_obj_to_json_map(
    obj: &orbital_core_base::schema::SObject,
) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for (key, value) in obj {
        map.insert(key.to_string(), sexpr_to_json(value));
    }
    map
}

/// Convert an `SExpression` to a `serde_json::Value`.
pub fn sexpr_to_json(expr: &SExpression) -> serde_json::Value {
    match expr {
        SExpression::String(s) => serde_json::Value::String(s.clone().into_inner()),
        SExpression::Number(n) => {
            serde_json::Value::Number(serde_json::Number::from_f64(*n).unwrap_or(0.into()))
        }
        SExpression::Bool(b) => serde_json::Value::Bool(*b),
        SExpression::Null => serde_json::Value::Null,
        SExpression::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sexpr_to_json).collect())
        }
        SExpression::Object(obj) => serde_json::Value::Object(sexpr_obj_to_json_map(obj)),
    }
}
