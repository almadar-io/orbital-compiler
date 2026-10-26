//! Content-grade classification of render-ui payloads — the compiled-path
//! mirror of `@almadar/core`'s `patterns/content-grade.ts` (the JS single
//! owner of "is this pattern a content body"). Hand-written (unlike the
//! sibling `patterns.rs` in this module, which pattern-sync regenerates):
//! it reads the SAME registry data (`crate::embedded::get_pattern`'s
//! `props_schema[].kind`) rather than duplicating it, so it never drifts
//! from a sync.
//!
//! A pattern TYPE is a content body when its registry entry declares a data
//! inlet — any `propsSchema` prop stamped `kind: "entity"` (pattern-sync's
//! mark for an `EntityRecord<T>`/`EntityCollection<T>` prop). A pattern
//! INSTANCE is a content body when it is a string payload, a composite with
//! non-empty `children`, or of a content-body TYPE. Consumed today by
//! `steady-state-no-init-reentry` (`validation::async_results`); the L2 port
//! map documents `viewer-stranded` needing the same facts when it ports.

use crate::embedded::get_pattern;
use orbital_core_base::schema::SExpression;
use orbital_core_base::slots::MAIN_SLOTS;

/// True when the pattern type declares a data inlet — any `propsSchema`
/// prop stamped `kind: "entity"`. Mirrors `isContentBodyPatternType`
/// (`content-grade.ts`) exactly; registry-derived, no name matching.
pub fn is_content_body_pattern_type(pattern_type: &str) -> bool {
    get_pattern(pattern_type).is_some_and(|p| {
        p.props_schema
            .values()
            .any(|prop| prop.kind.as_deref() == Some("entity"))
    })
}

/// True for a render-ui payload that is content-body-shaped: a string
/// literal, an object with a non-empty `children` array or a single child
/// object, or an object whose
/// `type` is a content-body pattern type. Mirrors `isContentBodyPattern`.
pub fn is_content_body_pattern(payload: &SExpression) -> bool {
    match payload {
        SExpression::String(_) => true,
        SExpression::Object(map) => {
            match map.get("children") {
                Some(SExpression::Array(children)) if !children.is_empty() => return true,
                Some(SExpression::Object(_)) => return true,
                _ => {}
            }
            matches!(map.get("type"), Some(SExpression::String(t)) if is_content_body_pattern_type(t))
        }
        _ => false,
    }
}

/// True iff `effect` is a `["render-ui", <slot>, payload]` node whose slot
/// is one of `orbital_core::slots::MAIN_SLOTS` (`main`/`content`/`screen`).
/// Widened from the JS wiring lint's `isMainSlotRenderUi`, which checks the
/// literal `'main'` only — `MAIN_SLOTS` is the declared vocabulary both
/// paths share (CLAUDE.md's L2 port convention: the JS literal was the
/// narrow one, not the other way around).
pub fn is_main_slot_render_ui(effect: &SExpression) -> bool {
    let SExpression::Array(arr) = effect else {
        return false;
    };
    arr.len() >= 3
        && matches!(&arr[0], SExpression::String(op) if op == "render-ui")
        && matches!(&arr[1], SExpression::String(slot) if MAIN_SLOTS.contains(&slot.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: &[(&str, SExpression)]) -> SExpression {
        SExpression::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn content_body_type_true_for_entity_grid() {
        // entity-table/data-grid declare an `entity` prop stamped kind: "entity".
        assert!(is_content_body_pattern_type("data-grid"));
    }

    #[test]
    fn content_body_type_false_for_chrome() {
        assert!(!is_content_body_pattern_type("button"));
    }

    #[test]
    fn content_body_pattern_true_for_string_payload() {
        assert!(is_content_body_pattern(&SExpression::String(
            ("@trait.X".to_string()).into()
        )));
    }

    #[test]
    fn content_body_pattern_true_for_nonempty_children() {
        let payload = obj(&[
            ("type", SExpression::String(("stack".to_string()).into())),
            (
                "children",
                SExpression::Array(vec![SExpression::String(("x".to_string()).into())]),
            ),
        ]);
        assert!(is_content_body_pattern(&payload));
    }

    #[test]
    fn content_body_pattern_true_for_a_single_child_object() {
        // `{ type: container, children: { type: stack, … } }` — one child node.
        let payload = obj(&[
            ("type", SExpression::String(("container".to_string()).into())),
            (
                "children",
                obj(&[("type", SExpression::String(("stack".to_string()).into()))]),
            ),
        ]);
        assert!(is_content_body_pattern(&payload));
    }

    #[test]
    fn content_body_pattern_false_for_empty_children() {
        let payload = obj(&[
            ("type", SExpression::String(("container".to_string()).into())),
            ("children", SExpression::Array(vec![])),
        ]);
        assert!(!is_content_body_pattern(&payload));
    }

    #[test]
    fn content_body_pattern_false_for_chrome_with_no_children() {
        let payload = obj(&[("type", SExpression::String(("button".to_string()).into()))]);
        assert!(!is_content_body_pattern(&payload));
    }

    #[test]
    fn main_slot_render_ui_accepts_all_three_main_slots() {
        for slot in MAIN_SLOTS {
            let effect = SExpression::Array(vec![
                SExpression::String(("render-ui".to_string()).into()),
                SExpression::String(((*slot).to_string()).into()),
                SExpression::Null,
            ]);
            assert!(
                is_main_slot_render_ui(&effect),
                "slot {slot} should count as main"
            );
        }
    }

    #[test]
    fn main_slot_render_ui_false_for_overlay_slot() {
        let effect = SExpression::Array(vec![
            SExpression::String(("render-ui".to_string()).into()),
            SExpression::String(("modal".to_string()).into()),
            SExpression::Null,
        ]);
        assert!(!is_main_slot_render_ui(&effect));
    }
}
