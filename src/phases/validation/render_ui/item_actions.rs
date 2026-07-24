//! itemActions validation plus effect-shape extraction helpers (fetch entity,
//! render-ui entity, render-ui detection, slot extraction).

use super::sync::patterns::{is_valid_item_action_prop, VALID_ITEM_ACTION_PROPS};
use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::SExpression;

/// Validate itemActions array
pub(super) fn validate_item_actions(item_actions: &SExpression, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    if let SExpression::Array(actions) = item_actions {
        for (i, action) in actions.iter().enumerate() {
            let action_path = format!("{}.itemActions[{}]", path, i);

            if let SExpression::Object(obj) = action {
                // Check for unknown props (hallucinated by LLM) using synced definition
                for key in obj.keys() {
                    if !is_valid_item_action_prop(key) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::RenderUIUnknownItemActionProp,
                                &action_path,
                                format!("Unknown itemAction prop: '{}'", key),
                            )
                            .with_suggestion("Remove the unknown prop")
                            .with_valid_values(
                                VALID_ITEM_ACTION_PROPS
                                    .iter()
                                    .map(|s| s.to_string())
                                    .collect(),
                            ),
                        );
                    }
                }

                // Check for required event prop
                if !obj.contains_key("event") && !obj.contains_key("navigatesTo") {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::RenderUIInvalidItemAction,
                            &action_path,
                            "itemAction must have 'event' or 'navigatesTo' property",
                        )
                        .with_suggestion(
                            "Add event: 'EVENT_NAME' to emit an event, or navigatesTo: '/route/{{row.id}}' to navigate",
                        ),
                    );
                }
            }
        }
    }

    result
}

/// Extract entity name from a fetch effect: ["fetch", "EntityName", ...]
pub(super) fn extract_fetch_entity(effect: &SExpression) -> Option<String> {
    match effect {
        SExpression::Array(arr) if arr.len() >= 2 => {
            if let SExpression::String(op) = &arr[0] {
                if op == "fetch" {
                    if let SExpression::String(entity) = &arr[1] {
                        return Some(entity.clone());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract entity name from a render-ui pattern's `entity` prop
pub(super) fn extract_render_ui_entity(effect: &SExpression) -> Option<String> {
    // The entity prop accepts EITHER an entity name literal (which needs a
    // sibling fetch) OR a binding ref like `@entity.cards` / `@payload.data`
    // / `@config.foo` (populated by `(set ...)` or carried in a payload).
    // Only the literal form needs the fetch warning; binding refs skip.
    fn is_binding_ref(s: &str) -> bool {
        s.starts_with('@')
    }
    match effect {
        SExpression::Array(arr) if arr.len() >= 3 => {
            if let SExpression::String(op) = &arr[0] {
                if op != "render-ui" && op != "render_ui" {
                    return None;
                }
            }
            // Pattern is the third element
            if let SExpression::Object(obj) = &arr[2] {
                if let Some(SExpression::String(entity)) = obj.get("entity") {
                    if is_binding_ref(entity) {
                        return None;
                    }
                    return Some(entity.clone());
                }
            }
            None
        }
        SExpression::Object(obj) => {
            let render_ui = obj.get("render_ui").or_else(|| obj.get("render-ui"))?;
            if let SExpression::Object(rui_obj) = render_ui {
                if let Some(SExpression::Object(pattern)) = rui_obj.get("pattern") {
                    if let Some(SExpression::String(entity)) = pattern.get("entity") {
                        if is_binding_ref(entity) {
                            return None;
                        }
                        return Some(entity.clone());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Check if an S-expression is a render_ui effect
pub fn is_render_ui_effect(expr: &SExpression) -> bool {
    match expr {
        SExpression::Array(arr) if !arr.is_empty() => {
            if let SExpression::String(op) = &arr[0] {
                op == "render-ui" || op == "render_ui"
            } else {
                false
            }
        }
        SExpression::Object(obj) => obj.contains_key("render_ui") || obj.contains_key("render-ui"),
        _ => false,
    }
}

/// Extract slot from a render_ui effect
pub fn extract_slot(expr: &SExpression) -> Option<String> {
    match expr {
        SExpression::Array(arr) if arr.len() >= 2 => {
            if let SExpression::String(slot) = &arr[1] {
                Some(slot.clone())
            } else {
                None
            }
        }
        SExpression::Object(obj) => {
            let render_ui = obj.get("render_ui").or_else(|| obj.get("render-ui"))?;
            if let SExpression::Object(rui_obj) = render_ui {
                if let Some(SExpression::String(slot)) = rui_obj.get("slot") {
                    return Some(slot.clone());
                }
            }
            None
        }
        _ => None,
    }
}
