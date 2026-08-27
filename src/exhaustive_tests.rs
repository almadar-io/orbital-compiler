//! Exhaustive State Machine Test Generator
//!
//! Generates comprehensive test cases for state machine validation:
//! 1. Transition Matrix Tests - Every valid (from_state, event) → to_state
//! 2. Guard Enforcement Tests - Guards block/allow correctly
//! 3. Invalid Transition Tests - Events that should NOT cause transitions
//! 4. Journey Tests - Graph traversal visiting all states

use orbital_core::ir::{
    OirBinaryOp, OirBindingRoot, OirExpr, OirModule, OirTrait, OirTransitionFrom,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

/// Result of running exhaustive tests on a trait
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraitTestResult {
    pub trait_name: String,
    pub total_tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub tests: Vec<TestCase>,
}

/// Individual test case
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCase {
    pub name: String,
    pub category: TestCategory,
    pub from_state: String,
    pub event: String,
    pub expected_state: String,
    /// Payload to send with the event (for guard tests)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// Simulated entity data for @entity.* guard bindings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_data: Option<serde_json::Value>,
    /// Path to reach from_state from initial state
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub setup_path: Vec<JourneyStep>,
}

/// Test category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestCategory {
    /// Basic state transition test
    Transition,
    /// Guard should block (invalid payload)
    GuardBlock,
    /// Guard should allow (valid payload)
    GuardAllow,
    /// Event should NOT cause transition from this state
    InvalidTransition,
    /// Full journey visiting all states
    Journey,
}

impl std::fmt::Display for TestCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestCategory::Transition => write!(f, "transition"),
            TestCategory::GuardBlock => write!(f, "guard_block"),
            TestCategory::GuardAllow => write!(f, "guard_allow"),
            TestCategory::InvalidTransition => write!(f, "invalid"),
            TestCategory::Journey => write!(f, "journey"),
        }
    }
}

/// A step in a journey path
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JourneyStep {
    pub event: String,
    pub expected_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// Step the state machine once: given the current state and an event with a
/// payload/entity, return the resulting state — the `to` of the first
/// `(event, from_state)` transition (in declaration order) whose guard passes
/// via the ONE canonical evaluator (`orbital_core::Evaluator::evaluate_guard_oir`),
/// or `None` if no transition fires. This is the single source of "what does this
/// event actually do here", shared by the test generator (to compute the correct
/// expected outcome, accounting for guard priority / shadowing) and the
/// `test --execute` executor — so the two can never disagree. An unconvertible /
/// unevaluable guard is permissive (does not block).
pub fn resolve_transition(
    trait_def: &OirTrait,
    current_state: &str,
    event: &str,
    payload: Option<&serde_json::Value>,
    entity_data: Option<&serde_json::Value>,
) -> Option<String> {
    use orbital_core::{BindingContext, Evaluator, Value};

    let mut ctx = BindingContext::new();
    if let Some(p) = payload {
        if let Ok(v) = serde_json::from_value::<Value>(p.clone()) {
            ctx.set_payload(v);
        }
    }
    if let Some(e) = entity_data {
        if let Ok(v) = serde_json::from_value::<Value>(e.clone()) {
            ctx.set_entity(v);
        }
    }
    let evaluator = Evaluator::new();

    for transition in trait_def
        .transitions
        .iter()
        .filter(|t| t.event == event && t.from.matches(current_state))
    {
        let passes = match &transition.guard {
            Some(g) => evaluator.evaluate_guard_oir(g, &ctx).unwrap_or(true),
            None => true,
        };
        if passes {
            return Some(transition.to.clone());
        }
    }
    None
}

/// Exhaustive test generator for state machines
pub struct ExhaustiveTestGenerator<'a> {
    module: &'a OirModule,
}

impl<'a> ExhaustiveTestGenerator<'a> {
    /// Create a new test generator for the given module
    pub fn new(module: &'a OirModule) -> Self {
        Self { module }
    }

    /// Generate all test cases for all traits in the module
    pub fn generate_all_tests(&self) -> Vec<TraitTestResult> {
        self.module
            .traits
            .iter()
            .map(|t| self.generate_trait_tests(t))
            .collect()
    }

    /// Generate all test cases for a specific trait
    pub fn generate_trait_tests(&self, trait_def: &OirTrait) -> TraitTestResult {
        let mut tests = Vec::new();

        // Build adjacency map for pathfinding
        let adjacency = self.build_adjacency_map(trait_def);
        let states: Vec<&str> = trait_def.states.iter().map(|s| s.name.as_str()).collect();
        let initial_state = trait_def
            .states
            .iter()
            .find(|s| s.is_initial)
            .map(|s| s.name.as_str())
            .unwrap_or(states.first().copied().unwrap_or("idle"));

        // 1. Transition Matrix Tests
        tests.extend(self.generate_transition_tests(trait_def, &adjacency, initial_state));

        // 2. Guard Tests
        tests.extend(self.generate_guard_tests(trait_def, &adjacency, initial_state));

        // 3. Invalid Transition Tests
        tests.extend(self.generate_invalid_tests(trait_def, &adjacency, &states, initial_state));

        // 4. Journey Test
        if let Some(journey_test) =
            self.generate_journey_test(trait_def, &adjacency, &states, initial_state)
        {
            tests.push(journey_test);
        }

        TraitTestResult {
            trait_name: trait_def.name.clone(),
            total_tests: tests.len(),
            passed: 0, // Will be filled in by runner
            failed: 0, // Will be filled in by runner
            tests,
        }
    }

    /// Build adjacency map for state transitions
    fn build_adjacency_map(
        &self,
        trait_def: &OirTrait,
    ) -> HashMap<String, Vec<(String, String, bool)>> {
        let mut adjacency: HashMap<String, Vec<(String, String, bool)>> = HashMap::new();
        let states: Vec<&str> = trait_def.states.iter().map(|s| s.name.as_str()).collect();

        for transition in &trait_def.transitions {
            let from_states: Vec<&str> = match &transition.from {
                OirTransitionFrom::Single(s) => vec![s.as_str()],
                OirTransitionFrom::Multiple(ss) => ss.iter().map(|s| s.as_str()).collect(),
                OirTransitionFrom::Any => states.clone(),
            };

            let has_guard = transition.guard.is_some();

            for from in from_states {
                adjacency.entry(from.to_string()).or_default().push((
                    transition.event.clone(),
                    transition.to.clone(),
                    has_guard,
                ));
            }
        }

        adjacency
    }

    /// Generate transition matrix tests
    fn generate_transition_tests(
        &self,
        trait_def: &OirTrait,
        adjacency: &HashMap<String, Vec<(String, String, bool)>>,
        initial_state: &str,
    ) -> Vec<TestCase> {
        let mut tests = Vec::new();

        for transition in &trait_def.transitions {
            let from_states = self.get_from_states(trait_def, transition);

            // Synthesize a payload/entity that satisfies THIS transition's guard,
            // then ask the shared stepper what the event actually does from here.
            let synth_payload = self.generate_valid_guard_payload(transition.guard.as_ref());
            let synth_entity = self.generate_valid_guard_entity_data(transition.guard.as_ref());
            let payload_opt =
                (synth_payload != serde_json::json!({})).then(|| synth_payload.clone());

            for from_state in from_states {
                // Only assert this transition when it is the one that ACTUALLY
                // fires for the satisfying input. If a higher-priority sibling
                // wins (shadowing) or the guard can't be satisfied at all (a
                // config-folded-off flow), the declared `→ to` is unreachable in
                // this compiled module — skip rather than assert a state the
                // machine never reaches.
                if resolve_transition(
                    trait_def,
                    &from_state,
                    &transition.event,
                    payload_opt.as_ref(),
                    synth_entity.as_ref(),
                )
                .as_deref()
                    != Some(transition.to.as_str())
                {
                    continue;
                }

                let setup_path = self.find_path_to_state(adjacency, initial_state, &from_state);
                tests.push(TestCase {
                    name: format!("{} + {} → {}", from_state, transition.event, transition.to),
                    category: TestCategory::Transition,
                    from_state: from_state.clone(),
                    event: transition.event.clone(),
                    expected_state: transition.to.clone(),
                    payload: payload_opt.clone(),
                    entity_data: synth_entity.clone(),
                    setup_path,
                });
            }
        }

        tests
    }

    /// Generate guard enforcement tests
    fn generate_guard_tests(
        &self,
        trait_def: &OirTrait,
        adjacency: &HashMap<String, Vec<(String, String, bool)>>,
        initial_state: &str,
    ) -> Vec<TestCase> {
        let mut tests = Vec::new();

        for transition in &trait_def.transitions {
            if transition.guard.is_none() {
                continue;
            }

            let from_states = self.get_from_states(trait_def, transition);
            let from_state = from_states
                .first()
                .cloned()
                .unwrap_or_else(|| initial_state.to_string());
            let setup_path = self.find_path_to_state(adjacency, initial_state, &from_state);
            let empty = serde_json::json!({});

            // Test 1: Guard should BLOCK with invalid/empty payload — but only a
            // CLEAN block (the event genuinely leaves the machine in place). If an
            // unguarded/satisfiable sibling catches the same event with the empty
            // payload, the event isn't "blocked to stay"; skip rather than assert
            // a stay that doesn't happen.
            if resolve_transition(
                trait_def,
                &from_state,
                &transition.event,
                Some(&empty),
                None,
            )
            .map_or(true, |to| to == from_state)
            {
                tests.push(TestCase {
                    name: format!("guard blocks {} (invalid payload)", transition.event),
                    category: TestCategory::GuardBlock,
                    from_state: from_state.clone(),
                    event: transition.event.clone(),
                    expected_state: from_state.clone(), // Should stay in same state
                    payload: Some(empty.clone()),
                    entity_data: None, // Deliberately empty to test guard blocking
                    setup_path: setup_path.clone(),
                });
            }

            // Test 2: Guard should ALLOW with valid payload — but only when THIS
            // transition is the one that actually fires for the satisfying input
            // (an unsatisfiable config-folded guard, or a transition shadowed by a
            // higher-priority sibling, has nothing to allow; skip it).
            let valid_payload = self.generate_valid_guard_payload(transition.guard.as_ref());
            let valid_entity = self.generate_valid_guard_entity_data(transition.guard.as_ref());
            let payload_opt = (valid_payload != empty).then(|| valid_payload.clone());
            if resolve_transition(
                trait_def,
                &from_state,
                &transition.event,
                payload_opt.as_ref(),
                valid_entity.as_ref(),
            )
            .as_deref()
                != Some(transition.to.as_str())
            {
                continue;
            }
            tests.push(TestCase {
                name: format!("guard allows {} (valid payload)", transition.event),
                category: TestCategory::GuardAllow,
                from_state: from_state.clone(),
                event: transition.event.clone(),
                expected_state: transition.to.clone(),
                payload: payload_opt,
                entity_data: valid_entity,
                setup_path,
            });
        }

        tests
    }

    /// Generate invalid transition tests
    fn generate_invalid_tests(
        &self,
        trait_def: &OirTrait,
        adjacency: &HashMap<String, Vec<(String, String, bool)>>,
        states: &[&str],
        initial_state: &str,
    ) -> Vec<TestCase> {
        let mut tests = Vec::new();

        for state in states {
            // Find events that ARE valid from this state
            let valid_events: HashSet<&str> = trait_def
                .transitions
                .iter()
                .filter(|t| match &t.from {
                    OirTransitionFrom::Single(s) => s == *state,
                    OirTransitionFrom::Multiple(ss) => ss.iter().any(|s| s == *state),
                    OirTransitionFrom::Any => true,
                })
                .map(|t| t.event.as_str())
                .collect();

            // Test events that should NOT cause transitions
            for event in &trait_def.events {
                if !valid_events.contains(event.key.as_str()) {
                    let setup_path = self.find_path_to_state(adjacency, initial_state, state);

                    tests.push(TestCase {
                        name: format!("{} + {} (invalid)", state, event.key),
                        category: TestCategory::InvalidTransition,
                        from_state: state.to_string(),
                        event: event.key.clone(),
                        expected_state: state.to_string(), // Should stay in same state
                        payload: None,
                        entity_data: None,
                        setup_path,
                    });
                }
            }
        }

        tests
    }

    /// Generate journey test that visits all states
    fn generate_journey_test(
        &self,
        trait_def: &OirTrait,
        adjacency: &HashMap<String, Vec<(String, String, bool)>>,
        states: &[&str],
        initial_state: &str,
    ) -> Option<TestCase> {
        let journey = self.build_journey(adjacency, states, initial_state);

        if journey.is_empty() {
            return None;
        }

        let final_state = journey
            .last()
            .map(|s| s.expected_state.clone())
            .unwrap_or_else(|| initial_state.to_string());

        // Check if journey has guarded transitions
        let has_guards = journey.iter().any(|step| {
            trait_def
                .transitions
                .iter()
                .find(|t| t.event == step.event)
                .map(|t| t.guard.is_some())
                .unwrap_or(false)
        });

        Some(TestCase {
            name: if has_guards {
                "full journey (skipped: guards present)".to_string()
            } else {
                "full journey".to_string()
            },
            category: TestCategory::Journey,
            from_state: initial_state.to_string(),
            event: journey
                .iter()
                .map(|s| s.event.as_str())
                .collect::<Vec<_>>()
                .join(" → "),
            expected_state: final_state,
            payload: None,
            entity_data: None,
            setup_path: journey,
        })
    }

    /// Get from states for a transition
    fn get_from_states(
        &self,
        trait_def: &OirTrait,
        transition: &orbital_core::ir::OirTransition,
    ) -> Vec<String> {
        let states: Vec<String> = trait_def.states.iter().map(|s| s.name.clone()).collect();

        match &transition.from {
            OirTransitionFrom::Single(s) => vec![s.clone()],
            OirTransitionFrom::Multiple(ss) => ss.clone(),
            OirTransitionFrom::Any => states,
        }
    }

    /// Find shortest path from start state to target state using BFS
    fn find_path_to_state(
        &self,
        adjacency: &HashMap<String, Vec<(String, String, bool)>>,
        start: &str,
        target: &str,
    ) -> Vec<JourneyStep> {
        if start == target {
            return vec![];
        }

        let mut visited: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<(&str, Vec<JourneyStep>)> = VecDeque::new();

        queue.push_back((start, vec![]));
        visited.insert(start);

        while let Some((current, path)) = queue.pop_front() {
            if current == target {
                return path;
            }

            if let Some(edges) = adjacency.get(current) {
                for (event, to_state, _has_guard) in edges {
                    if !visited.contains(to_state.as_str()) {
                        visited.insert(to_state.as_str());
                        let mut new_path = path.clone();
                        new_path.push(JourneyStep {
                            event: event.clone(),
                            expected_state: to_state.clone(),
                            payload: None,
                        });
                        queue.push_back((to_state.as_str(), new_path));
                    }
                }
            }
        }

        vec![] // No path found
    }

    /// Build a journey that visits all states (or as many as possible)
    fn build_journey(
        &self,
        adjacency: &HashMap<String, Vec<(String, String, bool)>>,
        states: &[&str],
        initial_state: &str,
    ) -> Vec<JourneyStep> {
        let mut journey: Vec<JourneyStep> = vec![];
        let mut visited: HashSet<&str> = HashSet::new();
        let mut current = initial_state;

        visited.insert(current);

        let max_iterations = states.len() * 2;
        let mut iterations = 0;

        while visited.len() < states.len() && iterations < max_iterations {
            iterations += 1;

            let mut found_unvisited = false;

            if let Some(edges) = adjacency.get(current) {
                // First try unvisited states
                for (event, to_state, _) in edges {
                    if !visited.contains(to_state.as_str()) {
                        journey.push(JourneyStep {
                            event: event.clone(),
                            expected_state: to_state.clone(),
                            payload: None,
                        });
                        current = to_state.as_str();
                        visited.insert(current);
                        found_unvisited = true;
                        break;
                    }
                }

                // If no unvisited found, take any valid transition
                if !found_unvisited {
                    if let Some((event, to_state, _)) = edges.first() {
                        journey.push(JourneyStep {
                            event: event.clone(),
                            expected_state: to_state.clone(),
                            payload: None,
                        });
                        current = to_state.as_str();
                    }
                }
            } else {
                break;
            }
        }

        journey
    }

    /// Generate a valid payload that satisfies guard conditions
    fn generate_valid_guard_payload(&self, guard: Option<&OirExpr>) -> serde_json::Value {
        let Some(guard) = guard else {
            return serde_json::json!({});
        };

        let mut payload = serde_json::Map::new();
        self.extract_guard_requirements(guard, &mut payload);

        if payload.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::Value::Object(payload)
        }
    }

    /// Extract field requirements from a guard expression (payload bindings)
    fn extract_guard_requirements(
        &self,
        expr: &OirExpr,
        payload: &mut serde_json::Map<String, serde_json::Value>,
    ) {
        // Ratio (`current/max >= 0.85`) and length (`len(?data) > 0`)
        // comparisons: neither operand is a bare field, so the direct path below
        // can't see them. Synthesize the payload-side binding(s).
        let synth = {
            let mut s = self.ratio_assignments(expr);
            s.extend(self.len_assignments(expr));
            s
        };
        if !synth.is_empty() {
            for (root, path, val) in synth {
                if matches!(root, OirBindingRoot::Payload) {
                    Self::insert_nested(payload, &path, val);
                }
            }
            return;
        }
        match expr {
            OirExpr::BinaryOp { op, left, right } => match op {
                OirBinaryOp::And | OirBinaryOp::Or => {
                    // For Or, we satisfy the first branch (sufficient for guard to pass)
                    self.extract_guard_requirements(left, payload);
                    self.extract_guard_requirements(right, payload);
                }
                OirBinaryOp::Eq => {
                    if let (Some(path), Some(value)) = (
                        self.extract_payload_path(left),
                        self.extract_literal_value(right),
                    ) {
                        Self::insert_nested(payload, &path, value);
                    } else if let (Some(path), Some(value)) = (
                        self.extract_payload_path(right),
                        self.extract_literal_value(left),
                    ) {
                        Self::insert_nested(payload, &path, value);
                    }
                }
                OirBinaryOp::Ne => {
                    // For !=, generate a value that is DIFFERENT from the literal
                    if let (Some(path), Some(literal)) = (
                        self.extract_payload_path(left),
                        self.extract_literal_value(right),
                    ) {
                        Self::insert_nested(
                            payload,
                            &path,
                            self.generate_different_value(&literal),
                        );
                    } else if let (Some(path), Some(literal)) = (
                        self.extract_payload_path(right),
                        self.extract_literal_value(left),
                    ) {
                        Self::insert_nested(
                            payload,
                            &path,
                            self.generate_different_value(&literal),
                        );
                    }
                }
                OirBinaryOp::Gt => {
                    if let Some(path) = self.extract_payload_path(left) {
                        // Use literal+1 if right side is a known number, else 100
                        let val = self
                            .extract_literal_number(right)
                            .map(|n| n + 1.0)
                            .unwrap_or(100.0);
                        Self::insert_nested(payload, &path, serde_json::json!(val));
                    }
                }
                OirBinaryOp::Ge => {
                    if let Some(path) = self.extract_payload_path(left) {
                        let val = self.extract_literal_number(right).unwrap_or(100.0);
                        Self::insert_nested(payload, &path, serde_json::json!(val));
                    }
                }
                OirBinaryOp::Lt => {
                    if let Some(path) = self.extract_payload_path(left) {
                        let val = self
                            .extract_literal_number(right)
                            .map(|n| n - 1.0)
                            .unwrap_or(0.0);
                        Self::insert_nested(payload, &path, serde_json::json!(val));
                    }
                }
                OirBinaryOp::Le => {
                    if let Some(path) = self.extract_payload_path(left) {
                        let val = self.extract_literal_number(right).unwrap_or(0.0);
                        Self::insert_nested(payload, &path, serde_json::json!(val));
                    }
                }
                _ => {}
            },
            OirExpr::Call { function, args } => {
                if (function == "!=" || function == "isNotEmpty") && !args.is_empty() {
                    if let Some(path) = self.extract_payload_path(&args[0]) {
                        Self::insert_nested(payload, &path, serde_json::json!("test-value"));
                    }
                }
            }
            _ => {}
        }
    }

    /// Generate simulated entity data that satisfies @entity.* guard conditions
    fn generate_valid_guard_entity_data(
        &self,
        guard: Option<&OirExpr>,
    ) -> Option<serde_json::Value> {
        let guard = guard?;

        let mut entity = serde_json::Map::new();
        self.extract_entity_requirements(guard, &mut entity);

        if entity.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(entity))
        }
    }

    /// Extract entity field requirements from a guard expression
    fn extract_entity_requirements(
        &self,
        expr: &OirExpr,
        entity: &mut serde_json::Map<String, serde_json::Value>,
    ) {
        // Ratio (`current/max >= 0.85`) and length (`len(@entity.items) > 0`)
        // comparisons: synthesize the entity-side binding(s) (e.g. the `max`
        // denominator, or both for an entity/entity ratio).
        let synth = {
            let mut s = self.ratio_assignments(expr);
            s.extend(self.len_assignments(expr));
            s
        };
        if !synth.is_empty() {
            for (root, path, val) in synth {
                if matches!(root, OirBindingRoot::Entity) {
                    entity.insert(path.join("."), val);
                }
            }
            return;
        }
        if let OirExpr::BinaryOp { op, left, right } = expr {
            match op {
                OirBinaryOp::And => {
                    self.extract_entity_requirements(left, entity);
                    self.extract_entity_requirements(right, entity);
                }
                OirBinaryOp::Or => {
                    // For Or, satisfy just the first branch
                    self.extract_entity_requirements(left, entity);
                }
                OirBinaryOp::Eq => {
                    if let (Some(field), Some(value)) = (
                        self.extract_entity_field(left),
                        self.extract_literal_value(right),
                    ) {
                        entity.insert(field, value);
                    } else if let (Some(field), Some(value)) = (
                        self.extract_entity_field(right),
                        self.extract_literal_value(left),
                    ) {
                        entity.insert(field, value);
                    }
                }
                OirBinaryOp::Ne => {
                    // For != comparisons, generate a different value
                    if let (Some(field), Some(literal)) = (
                        self.extract_entity_field(left),
                        self.extract_literal_value(right),
                    ) {
                        entity.insert(field, self.generate_different_value(&literal));
                    } else if let (Some(field), Some(literal)) = (
                        self.extract_entity_field(right),
                        self.extract_literal_value(left),
                    ) {
                        entity.insert(field, self.generate_different_value(&literal));
                    }
                }
                OirBinaryOp::Gt => {
                    if let Some(field) = self.extract_entity_field(left) {
                        let val = self
                            .extract_literal_number(right)
                            .map(|n| n + 1.0)
                            .unwrap_or(100.0);
                        entity.insert(field, serde_json::json!(val));
                    }
                }
                OirBinaryOp::Ge => {
                    if let Some(field) = self.extract_entity_field(left) {
                        let val = self.extract_literal_number(right).unwrap_or(100.0);
                        entity.insert(field, serde_json::json!(val));
                    }
                }
                OirBinaryOp::Lt => {
                    if let Some(field) = self.extract_entity_field(left) {
                        let val = self
                            .extract_literal_number(right)
                            .map(|n| n - 1.0)
                            .unwrap_or(0.0);
                        entity.insert(field, serde_json::json!(val));
                    }
                }
                OirBinaryOp::Le => {
                    if let Some(field) = self.extract_entity_field(left) {
                        let val = self.extract_literal_number(right).unwrap_or(0.0);
                        entity.insert(field, serde_json::json!(val));
                    }
                }
                _ => {}
            }
        }
    }

    /// Extract entity field name from @entity.* binding expression
    fn extract_entity_field(&self, expr: &OirExpr) -> Option<String> {
        match expr {
            OirExpr::Binding { binding } => {
                if matches!(binding.root, OirBindingRoot::Entity) && !binding.path.is_empty() {
                    Some(binding.path.join("."))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Generate a value that is different from the given literal (for != tests)
    fn generate_different_value(&self, value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::String(s) => {
                serde_json::json!(format!("{}_different", s))
            }
            serde_json::Value::Number(n) => {
                let v = n.as_f64().unwrap_or(0.0);
                serde_json::json!(v + 999.0)
            }
            serde_json::Value::Bool(b) => serde_json::json!(!b),
            _ => serde_json::json!("_different"),
        }
    }

    /// Extract payload field path from binding expression (returns path segments)
    fn extract_payload_path(&self, expr: &OirExpr) -> Option<Vec<String>> {
        match expr {
            OirExpr::Binding { binding } => {
                if matches!(binding.root, OirBindingRoot::Payload) && !binding.path.is_empty() {
                    Some(binding.path.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Insert a value at a nested path in a JSON map (e.g. ["data", "name"] → { "data": { "name": value } })
    fn insert_nested(
        map: &mut serde_json::Map<String, serde_json::Value>,
        path: &[String],
        value: serde_json::Value,
    ) {
        if path.is_empty() {
            return;
        }
        if path.len() == 1 {
            map.insert(path[0].clone(), value);
            return;
        }
        // Ensure intermediate object exists
        let child = map
            .entry(path[0].clone())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let serde_json::Value::Object(ref mut child_map) = child {
            Self::insert_nested(child_map, &path[1..], value);
        }
    }

    /// Extract value from literal expression
    fn extract_literal_value(&self, expr: &OirExpr) -> Option<serde_json::Value> {
        match expr {
            OirExpr::String { value } => Some(serde_json::json!(value)),
            OirExpr::Number { value } => Some(serde_json::json!(value)),
            OirExpr::Bool { value } => Some(serde_json::json!(value)),
            _ => None,
        }
    }

    /// Extract a numeric literal from an expression
    fn extract_literal_number(&self, expr: &OirExpr) -> Option<f64> {
        match expr {
            OirExpr::Number { value } => Some(*value),
            _ => None,
        }
    }

    /// An `array/len`-family call `["array/len", binding]`.
    fn as_array_len(expr: &OirExpr) -> Option<&OirExpr> {
        match expr {
            OirExpr::Call { function, args }
                if matches!(
                    function.as_str(),
                    "array/len" | "array/length" | "length" | "count" | "len" | "size"
                ) && args.len() == 1 =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }

    /// Satisfying assignment for a *length* comparison
    /// `[cmp, ["array/len", binding], threshold]` (e.g. `len(?data) > 0`): an
    /// array of the length that makes the comparison hold, routed to the
    /// binding's payload/entity bucket.
    fn len_assignments(
        &self,
        expr: &OirExpr,
    ) -> Vec<(OirBindingRoot, Vec<String>, serde_json::Value)> {
        let OirExpr::BinaryOp { op, left, right } = expr else {
            return vec![];
        };
        let Some(threshold) = self.extract_literal_number(right) else {
            return vec![];
        };
        let Some(arg) = Self::as_array_len(left) else {
            return vec![];
        };
        let Some((root, path)) = Self::as_binding(arg) else {
            return vec![];
        };
        let n = threshold.max(0.0) as i64;
        let len = match op {
            OirBinaryOp::Gt => n + 1,
            OirBinaryOp::Ge | OirBinaryOp::Eq | OirBinaryOp::Le => n,
            OirBinaryOp::Lt => (n - 1).max(0),
            _ => return vec![],
        };
        let arr: Vec<serde_json::Value> = (0..len).map(|_| serde_json::json!({})).collect();
        vec![(root, path, serde_json::Value::Array(arr))]
    }

    /// A division `["/", num, den]` in either `BinaryOp` or operator-`Call` form.
    fn as_division(expr: &OirExpr) -> Option<(&OirExpr, &OirExpr)> {
        match expr {
            OirExpr::BinaryOp {
                op: OirBinaryOp::Div,
                left,
                right,
            } => Some((left, right)),
            OirExpr::Call { function, args } if function == "/" && args.len() == 2 => {
                Some((&args[0], &args[1]))
            }
            _ => None,
        }
    }

    /// `(root, path)` for a bare binding expression.
    fn as_binding(expr: &OirExpr) -> Option<(OirBindingRoot, Vec<String>)> {
        match expr {
            OirExpr::Binding { binding } if !binding.path.is_empty() => {
                Some((binding.root.clone(), binding.path.clone()))
            }
            _ => None,
        }
    }

    /// Satisfying assignments for a *ratio* comparison
    /// `[cmp, ["/", numBinding, denBinding], threshold]` (e.g. a gauge guard
    /// `current/max >= 0.85`). The direct-binding path can't synthesize these
    /// because neither comparison operand is itself a field. We fix the
    /// denominator to a base (100) and solve the numerator for the threshold
    /// with a unit margin, so a band `(and [>= r 0.85] [< r 0.95])` lands in
    /// range via last-wins overwrite of the numerator (the tighter `<` bound
    /// wins and still satisfies the `>=`). Each binding is returned with its
    /// root so the payload- and entity-side synthesizers take their own.
    fn ratio_assignments(
        &self,
        expr: &OirExpr,
    ) -> Vec<(OirBindingRoot, Vec<String>, serde_json::Value)> {
        let OirExpr::BinaryOp { op, left, right } = expr else {
            return vec![];
        };
        let Some(threshold) = self.extract_literal_number(right) else {
            return vec![];
        };
        let Some((num, den)) = Self::as_division(left) else {
            return vec![];
        };
        let (Some(num_b), Some(den_b)) = (Self::as_binding(num), Self::as_binding(den)) else {
            return vec![];
        };
        let den_val = 100.0_f64;
        let num_val = match op {
            OirBinaryOp::Ge | OirBinaryOp::Le | OirBinaryOp::Eq => threshold * den_val,
            OirBinaryOp::Gt => threshold * den_val + 1.0,
            OirBinaryOp::Lt => threshold * den_val - 1.0,
            _ => return vec![],
        };
        vec![
            (num_b.0, num_b.1, serde_json::json!(num_val)),
            (den_b.0, den_b.1, serde_json::json!(den_val)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::ir::{OirEvent, OirMetadata, OirState, OirTransition};

    fn create_simple_trait() -> OirTrait {
        OirTrait {
            name: "TestTrait".to_string(),
            id: None,
            orbital_name: String::new(),
            description: None,
            source: Default::default(),
            category: None,
            capabilities: vec![],
            linked_entity: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
            config: None,
            states: vec![
                OirState {
                    name: "idle".to_string(),
                    is_initial: true,
                    is_final: false,
                    description: None,
                    time_bound_ms: None,
                },
                OirState {
                    name: "active".to_string(),
                    is_initial: false,
                    is_final: false,
                    description: None,
                    time_bound_ms: None,
                },
            ],
            events: vec![
                OirEvent {
                    key: "START".to_string(),
                    name: None,
                    description: None,
                    payload_schema: None,
                    payload_required_fields: std::collections::BTreeSet::new(),
                },
                OirEvent {
                    key: "STOP".to_string(),
                    name: None,
                    description: None,
                    payload_schema: None,
                    payload_required_fields: std::collections::BTreeSet::new(),
                },
            ],
            transitions: vec![
                OirTransition::new("idle", "active", "START"),
                OirTransition::new("active", "idle", "STOP"),
            ],
            guards: vec![],
            ticks: vec![],
            listeners: vec![],
            emits: vec![],
            data_entities: vec![],
            slots_used: vec![],
            declared_config_schema: None,
            declared_config_defaults: None,
        }
    }

    fn create_test_module(traits: Vec<OirTrait>) -> OirModule {
        OirModule {
            name: "Test".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            entities: vec![],
            traits,
            pages: vec![],
            themes: vec![],
            services: vec![],
            expected_entities: vec![],
            metadata: OirMetadata::default(),
        }
    }

    #[test]
    fn test_generate_transition_tests() {
        let module = create_test_module(vec![create_simple_trait()]);

        let generator = ExhaustiveTestGenerator::new(&module);
        let results = generator.generate_all_tests();

        assert_eq!(results.len(), 1);
        let trait_result = &results[0];
        assert_eq!(trait_result.trait_name, "TestTrait");

        // Should have transition tests + invalid tests + journey
        assert!(trait_result.tests.len() >= 2);

        // Check transition tests exist
        let transition_tests: Vec<_> = trait_result
            .tests
            .iter()
            .filter(|t| t.category == TestCategory::Transition)
            .collect();
        assert_eq!(transition_tests.len(), 2);
    }

    #[test]
    fn test_find_path_to_state() {
        let module = create_test_module(vec![create_simple_trait()]);

        let generator = ExhaustiveTestGenerator::new(&module);
        let trait_def = &module.traits[0];
        let adjacency = generator.build_adjacency_map(trait_def);

        let path = generator.find_path_to_state(&adjacency, "idle", "active");
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].event, "START");
        assert_eq!(path[0].expected_state, "active");
    }

    #[test]
    fn test_build_journey() {
        let module = create_test_module(vec![create_simple_trait()]);

        let generator = ExhaustiveTestGenerator::new(&module);
        let trait_def = &module.traits[0];
        let adjacency = generator.build_adjacency_map(trait_def);
        let states: Vec<&str> = vec!["idle", "active"];

        let journey = generator.build_journey(&adjacency, &states, "idle");
        assert!(!journey.is_empty());

        // Journey should visit active state
        assert!(journey.iter().any(|s| s.expected_state == "active"));
    }
}
