//! Composition Module
//!
//! Native Rust port of the TypeScript behavior composition logic from
//! `@almadar/core/src/builders/`. Used by the `compose` phase to evaluate
//! `behavior/compose`, `behavior/wire`, `behavior/detect-layout`, and
//! `behavior/pipe` calls embedded in `.orb` programs.
//!
//! The TypeScript implementation in `packages/almadar-core/src/builders/` is
//! the canonical reference. This Rust port must produce byte-identical output
//! for the same input. Algorithmic drift between Rust and TS is a blocker.


use orbital_core_base::value::{Shared, Value, ValueMap};

/// What a `json!` field or element may be: a core value built straight from it, no JSON tree between.
pub(crate) trait ToValue {
    fn to_value(self) -> Value;
}

impl ToValue for Value {
    fn to_value(self) -> Value {
        self
    }
}
impl ToValue for &Value {
    fn to_value(self) -> Value {
        self.clone()
    }
}
impl ToValue for &str {
    fn to_value(self) -> Value {
        Value::string(self.to_string())
    }
}
impl ToValue for String {
    fn to_value(self) -> Value {
        Value::string(self)
    }
}
impl ToValue for Shared<String> {
    fn to_value(self) -> Value {
        Value::String(self)
    }
}
impl ToValue for &String {
    fn to_value(self) -> Value {
        Value::string(self.clone())
    }
}
impl ToValue for bool {
    fn to_value(self) -> Value {
        Value::Bool(self)
    }
}
impl ToValue for f64 {
    fn to_value(self) -> Value {
        Value::Number(self)
    }
}
impl ToValue for i32 {
    fn to_value(self) -> Value {
        Value::Int(self.into())
    }
}
impl ToValue for usize {
    fn to_value(self) -> Value {
        i64::try_from(self).map_or(Value::Number(self as f64), Value::Int)
    }
}
impl<T: ToValue> ToValue for Option<T> {
    fn to_value(self) -> Value {
        self.map_or(Value::Null, ToValue::to_value)
    }
}
impl<T: ToValue> ToValue for Vec<T> {
    fn to_value(self) -> Value {
        Value::array(self.into_iter().map(ToValue::to_value).collect::<Vec<_>>())
    }
}
impl<T: ToValue + Clone> ToValue for &Vec<T> {
    fn to_value(self) -> Value {
        Value::array(self.iter().cloned().map(ToValue::to_value).collect::<Vec<_>>())
    }
}
impl<T: ToValue + Clone> ToValue for &[T] {
    fn to_value(self) -> Value {
        Value::array(self.iter().cloned().map(ToValue::to_value).collect::<Vec<_>>())
    }
}
impl<T: ToValue, const N: usize> ToValue for [T; N] {
    fn to_value(self) -> Value {
        Value::array(self.into_iter().map(ToValue::to_value).collect::<Vec<_>>())
    }
}
impl ToValue for ValueMap {
    fn to_value(self) -> Value {
        Value::object(self)
    }
}

/// A core value from a JSON-shaped literal; fields and elements are expressions of a [`ToValue`] type.
macro_rules! json {
    ({}) => { $crate::composition::object_value(Vec::new()) };
    ({ $($k:literal : $v:expr),+ $(,)? }) => {
        $crate::composition::object_value(vec![$(($k, $crate::composition::ToValue::to_value(($v).clone()))),+])
    };
    ([ $($v:expr),* $(,)? ]) => {
        $crate::composition::ToValue::to_value(vec![$($crate::composition::ToValue::to_value(($v).clone())),*] as Vec<$crate::composition::Value>)
    };
    ($v:expr) => { $crate::composition::ToValue::to_value(($v).clone()) };
}

pub(crate) fn object_value(fields: Vec<(&str, Value)>) -> Value {
    let mut map = ValueMap::default();
    for (key, value) in fields {
        map.insert(key.into(), value);
    }
    Value::object(map)
}

pub mod compose_app;
pub mod compose_behaviors;
pub mod event_wiring;
pub mod layout_strategy;
pub mod pipe;

pub use compose_app::{
    compose_app_from_files, compose_orbital_surface, compose_programs, dedupe_composed_identity,
    dedupe_composed_surface, identity_vocabulary_delta, union_os_grants, ComposeAppOptions,
    ComposeLookups,
};
pub use compose_behaviors::{
    compose_behaviors, ComposeBehaviorsInput, ComposeBehaviorsResult, ComposeError, LayoutMetadata,
    WiringMetadata,
};
pub use event_wiring::{apply_event_wiring, EventWiringEntry};
pub use layout_strategy::{detect_layout_strategy, LayoutStrategy};
pub use pipe::pipe_behaviors;
