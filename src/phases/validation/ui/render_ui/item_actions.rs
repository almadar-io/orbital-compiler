//! itemActions validation plus effect-shape extraction helpers (fetch entity,
//! render-ui entity, render-ui detection, slot extraction).

use super::sync::patterns::{is_valid_item_action_prop, VALID_ITEM_ACTION_PROPS};
use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::SExpression;

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

/// Check if an S-expression is a render_ui effect
pub fn is_render_ui_effect(expr: &SExpression) -> bool {
    match expr {
        SExpression::Array(arr) if !arr.is_empty() => {
            if let SExpression::String(op) = &arr[0] {
                op == "render-ui"
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Extract slot from a render_ui effect
pub fn extract_slot(expr: &SExpression) -> Option<String> {
    match expr {
        SExpression::Array(arr) if arr.len() >= 2 => {
            if let SExpression::String(slot) = &arr[1] {
                Some(slot.clone().into_inner())
            } else {
                None
            }
        }
        _ => None,
    }
}
