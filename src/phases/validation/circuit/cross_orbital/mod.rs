//! Cross-Orbital Validation
//!
//! Cross-orbital helpers shared by the circuit checks.
//! Ported from `builder/packages/shared/src/orbitals/validation/orbital-validator.ts`.

mod event_wiring;

pub(crate) use event_wiring::collect_async_result_events;
