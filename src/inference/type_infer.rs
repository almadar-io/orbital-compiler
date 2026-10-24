//! Canonical `OirExpr → OirType` inference — the single derivation of "what type does this expression
//! evaluate to", shared by the validator (the `(set @field <expr>)` type check, the operator argument check) and
//! the TS codegen.
//!
//! An operator's result type is its signature's (`@almadar/std` `returnType`, built from the type variables of
//! its `params`), resolved by `orbital_core::signature` — the same resolver the runtime check drives. Every
//! argument of a call is bound against its declared type in order; the call's result is the signature's return
//! type under those bindings, so `(array/first @entity.tags)` is `string` when `tags` is `[string]`. A lambda
//! argument is inferred with its parameters bound to the types the signature gives them. There is no per-operator
//! code here.
//!
//! `None` means no concrete type is determined: a binding nothing resolves, an effect, a call with a type
//! variable no argument bound. It is never a type: handed to a type variable it is an error
//! (`ORB_X_OPERATOR_ARG_UNDETERMINED`), because a variable never stands for an unknown.

use std::cell::RefCell;

use orbital_core_base::ir::{
    OirBinaryOp, OirBinding, OirBindingRoot, OirExpr, OirField, OirModule, OirType, OirUnaryOp,
};
use orbital_core_base::language::registry::{get_operator, OperatorTypeRef, OperatorTypeRefKind};
use orbital_core_base::signature::{
    type_at_path, Actual, CallBindings, Known, Mismatch, Mode, Signature,
};
use orbital_core_eval::evaluator::operator_signature;

/// Binding-resolution context the inferencer needs (and only that). Each consumer supplies its own — the
/// validator from the resolved entity/payload field types, the codegen from its `CodegenContext` catalog — so the
/// inferencer itself is layer-agnostic.
pub trait TypeCtx {
    /// The declared type of a binding (`@entity.f` / `@payload.f` / `@config.f`), or `None` when it can't be
    /// resolved.
    fn binding_type(&self, binding: &OirBinding) -> Option<OirType>;

    /// The declared type a name stands for (a named struct), when the context knows it.
    fn named_type(&self, _name: &str) -> Option<OirType> {
        None
    }

    /// The row type of the entity `name` (its declared fields and the system fields every row carries), when the
    /// context knows the declarations. A field read through a reference to a row is typed by it.
    fn entity_type(&self, _name: &str) -> Option<OirType> {
        None
    }

    /// The type of `@event.payload` in the transition being checked: the payload of the event that triggers it.
    fn delivery_payload(&self) -> Option<OirType> {
        None
    }

    /// The type of `@prevEvents[i].payload`: the payload every event this trait listens to declares, when they
    /// agree. Without one the earlier deliveries' payload has no type.
    fn prev_delivery_payload(&self) -> Option<OirType> {
        None
    }

    /// Whether a row or payload field can be missing: declared without `!` and without a default. A missing value
    /// is below any number to both evaluators and fails every ordering comparison in every push-down store, so an
    /// ordering comparison over one is an error unless the program guards it (ORB_X_ORDERING_OPTIONAL_OPERAND).
    fn may_be_absent(&self, _binding: &OirBinding) -> bool {
        false
    }

    /// Whether an object literal is exactly the record it writes (its keys, each typed by its value). A consumer
    /// comparing a literal with a declared type (a default that omits optional keys) keeps it a record of unknown
    /// extent; one that reads the literal's own fields needs the exact record.
    fn object_literals_are_exact(&self) -> bool {
        false
    }

    /// Whether `name` is a declared type: `None` when the context does not know the declarations at all.
    fn declared_type(&self, _name: &str) -> Option<bool> {
        None
    }
}

/// A call whose argument contradicts the operator's signature.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// The canonical operator name.
    pub function: String,
    /// The zero-based argument position.
    pub index: usize,
    pub mismatch: Mismatch,
    /// The call, printed.
    pub expression: String,
    /// The argument at `index`, printed.
    pub argument: String,
}

/// The signature of an operator that evaluates to a value: the evaluator's (alias-aware) first, then the
/// installed table's for operators the evaluator does not run (the Python-target ones).
pub(crate) fn signature_of(name: &str) -> Option<Signature<'static>> {
    operator_signature(name).or_else(|| {
        let meta = get_operator(name)?;
        (!meta.has_side_effects).then(|| Signature::of(meta))
    })
}

/// A closed `shape` parameter, whose literal argument is checked field by field.
fn is_closed_shape(formal: &OperatorTypeRef) -> bool {
    matches!(formal, OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, open, .. }) if *open != Some(true) && !fields.is_empty())
}

/// Whether `name` is an effect: it runs for what it does and produces no value.
pub(crate) fn is_effect(name: &str) -> bool {
    orbital_core_base::ir::is_substrate_operator(name)
        || get_operator(name).is_some_and(|meta| meta.has_side_effects)
}

/// Whether `name` is an operator that evaluates to a value (not an effect).
pub(crate) fn is_value_operator(name: &str) -> bool {
    signature_of(name).is_some()
}

struct Inferer<'c> {
    ctx: &'c dyn TypeCtx,
    scopes: RefCell<Vec<(String, Option<OirType>)>>,
    diagnostics: RefCell<Vec<Diagnostic>>,
    /// Set by the call (or `let`) just inferred when its result is void.
    void: std::cell::Cell<bool>,
    /// The bindings a presence check on the way in has shown present.
    present: RefCell<Vec<OirBinding>>,
    /// The `let` locals bound to an operand that can be missing.
    absent_locals: RefCell<Vec<String>>,
    refinements: RefCell<Vec<TypeFact>>,
    /// `let` locals bound to one member of a value (`(let ((k (object/get @n "kind" ""))) ...)`): a tag test on the
    /// local narrows that value.
    tag_aliases: RefCell<Vec<(String, OirBinding, String)>>,
    children: RefCell<Vec<ChildrenDependency>>,
    named_types: RefCell<std::sync::Arc<std::collections::BTreeMap<String, OirType>>>,
    /// The type inferred for each `["typed", value, "#slot:<n>"]` the typed-arguments pass tagged.
    slots: RefCell<Vec<(String, Option<OirType>)>>,
}

struct ScopeState {
    depth: usize,
    refinements: Vec<TypeFact>,
    presence: Vec<OirBinding>,
    children: Vec<ChildrenDependency>,
    absent: Vec<String>,
}

impl<'c> Inferer<'c> {
    fn new(ctx: &'c dyn TypeCtx) -> Self {
        Self {
            ctx,
            scopes: RefCell::new(Vec::new()),
            diagnostics: RefCell::new(Vec::new()),
            void: std::cell::Cell::new(false),
            present: RefCell::new(Vec::new()),
            absent_locals: RefCell::new(Vec::new()),
            refinements: RefCell::new(Vec::new()),
            tag_aliases: RefCell::new(Vec::new()),
            children: RefCell::new(Vec::new()),
            named_types: RefCell::new(std::sync::Arc::default()),
            slots: RefCell::new(Vec::new()),
        }
    }

    /// What the checker knows of `expr` as an argument: a type, the null value, an effect (void), an empty list, or
    /// nothing (undetermined).
    fn known(&self, expr: &OirExpr) -> Option<Known> {
        if let Some((live, dead)) = live_branch(expr) {
            let mark = self.diagnostics.borrow().len();
            let dead_known = self.known(dead);
            self.diagnostics.borrow_mut().truncate(mark);
            return match (self.known(live), dead_known) {
                (Some(Known::EmptyList), Some(Known::Type(ty))) => Some(Known::Type(ty)),
                (live_known, _) => live_known,
            };
        }
        match expr {
            OirExpr::Null => Some(Known::Absent),
            OirExpr::Array { items } if items.is_empty() => Some(Known::EmptyList),
            OirExpr::Effect { effect } => {
                // An `os/` effect's arguments are checked against the operation's declared parameters.
                if let orbital_core_base::ir::OirEffect::Os { op, args, key, .. } = effect.as_ref()
                {
                    if let Some(meta) = get_operator(op.name()) {
                        let refs: Vec<&OirExpr> = args.iter().collect();
                        self.call_signed(op.name(), &Signature::of(meta), &refs);
                        if let Some(key) = key {
                            self.known(key);
                        }
                        return Some(Known::Void);
                    }
                }
                if let orbital_core_base::ir::OirEffect::Set { target, value, .. } = effect.as_ref() {
                    self.infer_set_value(target, value);
                    return Some(Known::Void);
                }
                for child in effect.child_exprs() {
                    self.known(child);
                }
                Some(Known::Void)
            }
            // An effect produces no value, but the expressions it is handed are checked where they stand, in the
            // scope of every `let` and lambda around them.
            OirExpr::Call { function, args } if is_effect(function) => {
                // An effect from the operator table is checked against its own declared parameters; the arguments
                // past them (the `{ emit, key }` envelope) are checked where they stand.
                let declared = (!orbital_core_base::ir::is_substrate_operator(function))
                    .then(|| get_operator(function))
                    .flatten()
                    .map(|meta| (Signature::of(meta), meta.params.len()));
                let checked = match declared {
                    Some((signature, count)) => {
                        let count = count.min(args.len());
                        let refs: Vec<&OirExpr> = args[..count].iter().collect();
                        self.call_signed(function, &signature, &refs);
                        count
                    }
                    None => 0,
                };
                for arg in &args[checked..] {
                    self.known(arg);
                }
                Some(Known::Void)
            }
            _ => {
                let reported = self.diagnostics.borrow().len();
                self.void.set(false);
                let inferred = self.infer(expr);
                let through_call = matches!(
                    expr,
                    OirExpr::Call { .. }
                        | OirExpr::Conditional { .. }
                        | OirExpr::BinaryOp { .. }
                        | OirExpr::UnaryOp { .. }
                );
                match inferred {
                    Some(ty) => Some(Known::Type(ty)),
                    None if through_call && self.void.get() => Some(Known::Void),
                    // Its own error is already reported; the enclosing call does not report it again.
                    None if self.diagnostics.borrow().len() > reported => Some(Known::Absent),
                    None => None,
                }
            }
        }
    }

    /// The printed operand when `arg` can be missing at this point of the program: a field declared without `!` and
    /// without a default, or a `let` local bound to one, that no enclosing presence check shows present.
    fn absent_operand(&self, arg: &OirExpr) -> Option<String> {
        let binding = operand_binding(arg)?;
        if self.present.borrow().contains(&binding) {
            return None;
        }
        let mut absent = match &binding.root {
            OirBindingRoot::Local(name) => {
                binding.path.is_empty() && self.absent_locals.borrow().contains(name)
            }
            _ => self.ctx.may_be_absent(&binding),
        };
        for depth in 1..=binding.path.len() {
            let prefix = OirBinding {
                root: binding.root.clone(),
                path: binding.path[..depth].to_vec(),
            };
            if self.present.borrow().contains(&prefix) {
                continue;
            }
            absent |= self.ctx.may_be_absent(&prefix);
            let parent = OirBinding {
                root: binding.root.clone(),
                path: binding.path[..depth - 1].to_vec(),
            };
            let mut parent_type = self.binding_unrefined(&parent);
            while let Some(OirType::Optional { inner }) = parent_type {
                parent_type = Some(*inner);
            }
            if let Some(OirType::Struct { fields, .. }) = parent_type {
                absent |= fields.iter().any(|field| {
                    field.name == binding.path[depth - 1]
                        && field.default.is_none()
                        && (!field.required || field.field_type.is_optional())
                });
            }
        }
        absent.then(|| printed_arg(Some(&arg)))
    }

    fn binding(&self, binding: &OirBinding) -> Option<OirType> {
        let mut ty = self.binding_unrefined(binding)?;
        if let (OirBindingRoot::Local(_), false) = (&binding.root, binding.path.is_empty()) {
            let base = OirBinding { root: binding.root.clone(), path: Vec::new() };
            let refined = self
                .binding(&base)
                .and_then(|base| type_at_path(&self.unfold_rows(self.unfold(base)), &binding.path));
            ty = refined.unwrap_or(ty);
        }
        for dependency in self
            .children
            .borrow()
            .iter()
            .filter(|dependency| dependency.children == *binding)
        {
            let declared = self.unfold(self.binding_unrefined(&dependency.node)?);
            let refined = self.unfold(self.binding(&dependency.node)?);
            let projected =
                orbital_core_base::signature::project_children_type(&declared, &ty, &refined);
            ty = projected.unwrap_or(ty);
        }
        for fact in self
            .refinements
            .borrow()
            .iter()
            .filter(|fact| fact.projection.is_none() && fact.operand == *binding)
        {
            ty = self.narrow(&ty, &fact.predicate, fact.truth).unwrap_or(ty);
        }
        Some(ty)
    }

    fn binding_unrefined(&self, binding: &OirBinding) -> Option<OirType> {
        if let OirBindingRoot::Local(name) = &binding.root {
            let scoped = self
                .scopes
                .borrow()
                .iter()
                .rev()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone());
            if let Some(local) = scoped {
                let mut ty = local?;
                for segment in &binding.path {
                    ty = type_at_path(&self.unfold_rows(ty), std::slice::from_ref(segment))?;
                }
                return Some(ty);
            }
        }
        if let Some(builtin) = language_binding(
            binding,
            self.ctx.delivery_payload(),
            self.ctx.prev_delivery_payload(),
        ) {
            return type_at_path(&builtin, &binding.path);
        }
        // The context types the first segment (`@payload.row`); a reference to a row is read through its entity.
        let split = binding.path.len().min(1);
        let head = OirBinding {
            root: binding.root.clone(),
            path: binding.path[..split].to_vec(),
        };
        let mut ty = self.ctx.binding_type(&head)?;
        for segment in &binding.path[split..] {
            ty = type_at_path(&self.unfold_rows(ty), std::slice::from_ref(segment))?;
        }
        Some(self.unfold_rows(ty))
    }

    /// A reference to a row (or a list of them) as the row it stands for, one level deep.
    fn unfold_rows(&self, ty: OirType) -> OirType {
        match &ty {
            OirType::TypeRef { .. } => self.unfold(ty),
            OirType::Reference { entity } | OirType::Row { entity } => self
                .ctx
                .entity_type(entity)
                .or_else(|| self.ctx.named_type(entity))
                .unwrap_or(ty),
            OirType::Array { inner } => OirType::array(self.unfold_rows((**inner).clone())),
            OirType::Optional { inner } => OirType::optional(self.unfold_rows((**inner).clone())),
            _ => ty,
        }
    }

    fn infer(&self, expr: &OirExpr) -> Option<OirType> {
        let mut inferred = self.unfold_rows(self.infer_bare(expr)?);
        if let Some((operand, projection)) = guarded_operand(expr) {
            for fact in self
                .refinements
                .borrow()
                .iter()
                .filter(|fact| fact.operand == operand && fact.projection == projection)
            {
                inferred = self.narrow(&inferred, &fact.predicate, fact.truth).unwrap_or(inferred);
            }
        }
        Some(inferred)
    }

    fn infer_bare(&self, expr: &OirExpr) -> Option<OirType> {
        match expr {
            OirExpr::Null => None,
            OirExpr::Bool { .. } => Some(OirType::Boolean),
            OirExpr::Number { .. } => Some(OirType::Number),
            OirExpr::String { .. } => Some(OirType::String),
            OirExpr::Array { items } if items.is_empty() => {
                Some(OirType::Tuple { items: Vec::new() })
            }
            OirExpr::Array { items } => self.call("list", &items.iter().collect::<Vec<_>>()),
            // The fields an object literal writes are not all the fields of the type it stands for (a default that
            // omits optional keys): a record of unknown extent. The field values are still checked.
            OirExpr::Object { fields } => {
                let typed: Vec<(String, Option<OirType>)> = fields
                    .iter()
                    .map(|(name, value)| (name.clone(), self.infer(value)))
                    .collect();
                if !self.ctx.object_literals_are_exact() {
                    return Some(OirType::Row {
                        entity: "{ … }".to_string(),
                    });
                }
                let mut record: Vec<OirField> = Vec::new();
                for (name, ty) in typed {
                    record.push(OirField::new(name, ty?));
                }
                record.sort_by(|a, b| a.name.cmp(&b.name));
                Some(OirType::Struct { additional: None, fields: record })
            }
            OirExpr::Binding { binding } => self.binding(binding),
            OirExpr::BinaryOp { op, left, right } => {
                let name = binary_name(op);
                self.call(name, &[left, right])
            }
            OirExpr::UnaryOp { op, operand } => match op {
                OirUnaryOp::Not => self.call("not", &[operand]),
                OirUnaryOp::Neg => self.call("-", &[operand]),
            },
            OirExpr::Conditional {
                condition,
                then_branch,
                else_branch,
            } => match live_branch(expr) {
                Some((live, dead)) => {
                    // The dead branch still says what type the other one stands for (an empty list); its own
                    // findings are dropped, because it never runs.
                    let mark = self.diagnostics.borrow().len();
                    let dead_known = self.known(dead);
                    self.diagnostics.borrow_mut().truncate(mark);
                    match (self.known(live), dead_known) {
                        (Some(Known::Type(ty)), _) => Some(ty),
                        (Some(Known::EmptyList), Some(Known::Type(ty))) => Some(ty),
                        (Some(Known::EmptyList), _) => Some(OirType::Tuple { items: Vec::new() }),
                        (Some(Known::Void), _) => {
                            self.void.set(true);
                            None
                        }
                        _ => None,
                    }
                }
                None => self.call("if", &[condition, then_branch, else_branch]),
            },
            OirExpr::Call { function, args } if function == "typed" && args.len() == 2 => {
                let ty = self.infer(&args[0]);
                let OirExpr::String { value } = &args[1] else {
                    return ty;
                };
                // A value the program declared a type for (a substituted config knob, a recorded argument) is read
                // as that type; a slot tag only records what was inferred.
                if let Ok(declared) = serde_json::from_str::<OirType>(value) {
                    return Some(declared);
                }
                self.slots.borrow_mut().push((value.clone(), ty.clone()));
                ty
            }
            OirExpr::Call { function, args } if function == "let" => self.infer_let(args),
            OirExpr::Call { function, args } if quoted_scalar(function, args).is_some() => {
                quoted_scalar(function, args)
            }
            OirExpr::Call { function, args } => {
                self.call(function, &args.iter().collect::<Vec<_>>())
            }
            OirExpr::Map {
                source,
                variable,
                body,
            } => {
                let element = element_of(&self.infer(source)?);
                let result =
                    self.with_scope(&[(variable.clone(), element)], || self.infer(body))?;
                Some(OirType::array(result))
            }
            OirExpr::Filter {
                source,
                variable,
                predicate,
            } => {
                let source_ty = self.infer(source);
                let element = source_ty.as_ref().and_then(element_of);
                self.with_scope(&[(variable.clone(), element)], || self.infer(predicate));
                source_ty
            }
            OirExpr::Reduce {
                source,
                variable,
                accumulator,
                initial,
                body,
            } => {
                let element = self.infer(source).as_ref().and_then(element_of);
                let accumulated = self.infer(initial);
                self.with_scope(
                    &[
                        (variable.clone(), element),
                        (accumulator.clone(), accumulated.clone()),
                    ],
                    || self.infer(body),
                );
                accumulated
            }
            OirExpr::Lambda { .. } => None,
            // An `os/` effect has no value; its arguments are checked against the operation's parameters where it stands.
            OirExpr::Effect { effect }
                if matches!(effect.as_ref(), orbital_core_base::ir::OirEffect::Os { .. }) =>
            {
                self.known(expr);
                None
            }
            // A `set` value is a value expression: its reads are checked exactly as they are in a guard.
            OirExpr::Effect { effect }
                if matches!(
                    effect.as_ref(),
                    orbital_core_base::ir::OirEffect::Set { .. }
                        | orbital_core_base::ir::OirEffect::SetDynamic { .. }
                ) =>
            {
                if let orbital_core_base::ir::OirEffect::Set { target, value, .. } = effect.as_ref() {
                    self.infer_set_value(target, value);
                    return None;
                }
                for child in effect.child_exprs() {
                    self.infer(child);
                }
                None
            }
            OirExpr::Effect { .. } => None,
        }
    }

    fn enter_scope(&self, names: &[(String, Option<OirType>)]) -> ScopeState {
        let depth = self.scopes.borrow().len();
        let captured: Vec<_> = self
            .children
            .borrow()
            .iter()
            .filter_map(|dependency| {
                let (OirBindingRoot::Local(node), OirBindingRoot::Local(child)) =
                    (&dependency.node.root, &dependency.children.root)
                else {
                    return None;
                };
                (names.iter().any(|(local, _)| local == node)
                    && !names.iter().any(|(local, _)| local == child))
                .then(|| (child.clone(), self.binding(&dependency.children)))
            })
            .collect();
        self.scopes.borrow_mut().extend(captured);
        self.scopes.borrow_mut().extend(names.iter().cloned());
        let visible = self
            .refinements
            .borrow()
            .iter()
            .filter(|fact| !names.iter().any(|(name, _)| fact.mentions_local(name)))
            .cloned()
            .collect();
        let previous = self.refinements.replace(visible);
        let visible = self.present.borrow().iter().filter(|binding| {
            !matches!(&binding.root, OirBindingRoot::Local(name) if names.iter().any(|(local, _)| local == name))
        }).cloned().collect();
        let previous_presence = self.present.replace(visible);
        let visible = self.children.borrow().iter().filter(|dependency| {
            ![&dependency.node, &dependency.children].into_iter().any(|binding|
                matches!(&binding.root, OirBindingRoot::Local(name) if names.iter().any(|(local, _)| local == name)))
        }).cloned().collect();
        let previous_children = self.children.replace(visible);
        let mut absent: Vec<_> = self
            .absent_locals
            .borrow()
            .iter()
            .filter(|name| !names.iter().any(|(local, _)| local == *name))
            .cloned()
            .collect();
        absent.extend(
            names
                .iter()
                .filter(|(_, ty)| matches!(ty, Some(OirType::Optional { .. })))
                .map(|(name, _)| name.clone()),
        );
        let previous_absent = self.absent_locals.replace(absent);
        ScopeState {
            depth,
            refinements: previous,
            presence: previous_presence,
            children: previous_children,
            absent: previous_absent,
        }
    }

    fn leave_scope(&self, state: ScopeState) {
        self.refinements.replace(state.refinements);
        self.present.replace(state.presence);
        self.children.replace(state.children);
        self.absent_locals.replace(state.absent);
        self.scopes.borrow_mut().truncate(state.depth);
    }

    fn with_scope<R>(&self, names: &[(String, Option<OirType>)], f: impl FnOnce() -> R) -> R {
        let state = self.enter_scope(names);
        let out = f();
        self.leave_scope(state);
        out
    }

    /// `(let [[name value] …] body)`: each name is in scope, typed by its value, for the bindings after it and the body.
    fn infer_let(&self, args: &[OirExpr]) -> Option<OirType> {
        let (OirExpr::Array { items: bindings }, Some(body)) = (args.first()?, args.get(1)) else {
            return None;
        };
        let mut scopes = Vec::new();
        let mut aliased = 0usize;
        for pair in bindings {
            if let OirExpr::Array { items } = pair {
                if let (Some(OirExpr::String { value: name }), Some(value)) =
                    (items.first(), items.get(1))
                {
                    let ty = self.infer_local_initializer(value);
                    let absent = self.absent_operand(value).is_some();
                    let alias = member_read(value);
                    scopes.push(self.enter_scope(&[(name.clone(), ty)]));
                    if let Some((owner, member)) = alias {
                        self.tag_aliases.borrow_mut().push((name.clone(), owner, member));
                        aliased += 1;
                    }
                    if absent {
                        self.absent_locals.borrow_mut().push(name.clone());
                    }
                }
            }
        }
        let body_known = self.known(body);
        {
            let mut aliases = self.tag_aliases.borrow_mut();
            let kept = aliases.len() - aliased;
            aliases.truncate(kept);
        }
        for scope in scopes.into_iter().rev() {
            self.leave_scope(scope);
        }
        match body_known {
            Some(Known::Type(ty)) => Some(ty),
            Some(Known::Void) => {
                self.void.set(true);
                None
            }
            _ => None,
        }
    }

    fn infer_local_initializer(&self, value: &OirExpr) -> Option<OirType> {
        let OirExpr::Call { function, args } = value else {
            return self.infer(value);
        };
        let [initializer, OirExpr::String { value: declaration }] = args.as_slice() else {
            return self.infer(value);
        };
        let Some(shape) = (function == "typed")
            .then(|| orbital_core_base::ir::types::DeclaredShape::parse(declaration))
            .flatten()
        else {
            return self.infer(value);
        };
        let mark = self.diagnostics.borrow().len();
        let resolve = |name: &str| {
            shape
                .named
                .get(name)
                .cloned()
                .or_else(|| self.ctx.named_type(name))
        };
        if orbital_core_base::type_compat::checked_sub_with_named(&shape.ty, &shape.ty, &resolve)
            .is_some()
            && self
                .infer_contextual(initializer, &shape.ty, &shape.named, &mut Vec::new())
                .is_some()
            && self.diagnostics.borrow().len() == mark
        {
            return Some(shape.ty);
        }
        self.diagnostics.borrow_mut().push(Diagnostic {
            function: "let".into(),
            index: 0,
            mismatch: Mismatch::Type {
                expected: shape.name.unwrap_or_else(|| format!("{:?}", shape.ty)),
                actual: "an initializer whose declared type is not proved".into(),
            },
            expression: orbital_core_base::ir::pretty_print_expr(value),
            argument: orbital_core_base::ir::pretty_print_expr(initializer),
        });
        None
    }

    /// A `set` value is read against the declared type of the field it writes: branches and list items whose types
    /// differ are one value when that type admits each of them. Findings of the plain reading stand whenever the
    /// declared type does not admit the value.
    fn infer_set_value(&self, target: &OirBinding, value: &OirExpr) {
        let mark = self.diagnostics.borrow().len();
        self.infer(value);
        if self.diagnostics.borrow().len() == mark {
            return;
        }
        let Some(declared) = self.ctx.binding_type(target) else {
            return;
        };
        let found = self.diagnostics.borrow_mut().split_off(mark);
        let admitted = self
            .infer_contextual(value, &declared, &Default::default(), &mut Vec::new())
            .is_some()
            && self.diagnostics.borrow().len() == mark;
        if !admitted {
            let mut diagnostics = self.diagnostics.borrow_mut();
            diagnostics.truncate(mark);
            diagnostics.extend(found);
        }
    }

    fn infer_contextual(
        &self,
        expr: &OirExpr,
        expected: &OirType,
        named: &std::collections::BTreeMap<String, OirType>,
        expanding: &mut Vec<String>,
    ) -> Option<OirType> {
        let resolve = |name: &str| {
            named
                .get(name)
                .cloned()
                .or_else(|| self.ctx.named_type(name))
        };
        if let OirType::TypeRef { name } | OirType::Reference { entity: name } = expected {
            if let Some(shape) = resolve(name) {
                if expanding.contains(name) {
                    return None;
                }
                expanding.push(name.clone());
                let result = self.infer_contextual(expr, &shape, named, expanding);
                expanding.pop();
                return result.map(|_| expected.clone());
            }
            if matches!(expected, OirType::TypeRef { .. }) {
                return None;
            }
        }
        if let OirType::Optional { inner } = expected {
            if matches!(expr, OirExpr::Null) {
                return Some(expected.clone());
            }
            if expr.is_literal() {
                return self
                    .infer_contextual(expr, inner, named, expanding)
                    .map(|_| expected.clone());
            }
        }
        if let Some((_, then_branch, else_branch)) = conditional_parts(expr) {
            for branch in [then_branch, else_branch] {
                self.infer_contextual(branch, expected, named, expanding)?;
            }
            return Some(expected.clone());
        }
        let literal_container = matches!(expr, OirExpr::Array { .. } | OirExpr::Object { .. });
        if literal_container {
            if let OirType::Union { variants } = expected {
                for variant in variants {
                    let mark = self.diagnostics.borrow().len();
                    if self
                        .infer_contextual(expr, &variant.field_type, named, expanding)
                        .is_some()
                        && self.diagnostics.borrow().len() == mark
                    {
                        return Some(expected.clone());
                    }
                    self.diagnostics.borrow_mut().truncate(mark);
                }
                return None;
            }
        }
        match (expr, expected) {
            (OirExpr::String { value }, OirType::Enum { name }) => {
                orbital_core_base::type_compat::literal_union_members(name)
                    .filter(|members| members.contains(value))
                    .map(|_| expected.clone())
            }
            (OirExpr::Array { items }, OirType::Array { inner }) => {
                for item in items {
                    self.infer_contextual(item, inner, named, &mut Vec::new())?;
                }
                Some(expected.clone())
            }
            (OirExpr::Array { items }, OirType::Tuple { items: slots })
                if items.len() == slots.len() =>
            {
                for (item, slot) in items.iter().zip(slots) {
                    self.infer_contextual(item, slot, named, &mut Vec::new())?;
                }
                Some(expected.clone())
            }
            (OirExpr::Object { fields }, OirType::Map { key, value }) => {
                for (name, item) in fields {
                    self.infer_contextual(
                        &OirExpr::String {
                            value: name.clone(),
                        },
                        key,
                        named,
                        &mut Vec::new(),
                    )?;
                    self.infer_contextual(item, value, named, &mut Vec::new())?;
                }
                Some(expected.clone())
            }
            (OirExpr::Object { fields }, OirType::Struct { fields: members, additional }) => {
                let mut actual = Vec::new();
                for (name, item) in fields {
                    let ty =
                        if let Some(member) = members.iter().find(|member| member.name == *name) {
                            self.infer_contextual(item, &member.field_type, named, &mut Vec::new())?
                        } else if let Some(additional) = additional {
                            self.infer_contextual(item, additional, named, &mut Vec::new())?
                        } else {
                            self.infer(item)?
                        };
                    actual.push(OirField::new(name.clone(), ty));
                }
                actual.extend(
                    members
                        .iter()
                        .filter(|member| !member.required && !fields.contains_key(&member.name))
                        .cloned(),
                );
                let source = OirType::Struct { additional: additional.clone(), fields: actual };
                orbital_core_base::type_compat::checked_sub_with_named(
                    &source, expected, &resolve,
                )?;
                Some(expected.clone())
            }
            _ => {
                let actual = self.infer(expr)?;
                orbital_core_base::type_compat::checked_sub_with_named(
                    &actual, expected, &resolve,
                )?;
                Some(expected.clone())
            }
        }
    }

    fn literal_text(expr: &OirExpr) -> Option<String> {
        match expr {
            OirExpr::String { value } => Some(value.clone()),
            OirExpr::Call { function, args } => match quoted_literal(function, args)? {
                orbital_core_base::schema::SExpression::String(text) => Some(text),
                _ => None,
            },
            _ => None,
        }
    }

    fn literal_texts(expr: &OirExpr) -> Option<Vec<String>> {
        let items = match expr {
            OirExpr::Array { items } => items,
            OirExpr::Call { function, args } if function == "list" => args,
            _ => return None,
        };
        items.iter().map(Self::literal_text).collect()
    }

    fn register_named_types(&self, ty: &OirType, bindings: &mut CallBindings) {
        let mut pending = Vec::new();
        orbital_core_base::ir::types::names_in(ty, &mut pending);
        let mut visited = std::collections::HashSet::new();
        let mut known = self.named_types.borrow_mut();
        while let Some(name) = pending.pop() {
            if !visited.insert(name.clone()) || known.contains_key(&name) {
                continue;
            }
            if let Some(declared) = self.ctx.named_type(&name) {
                orbital_core_base::ir::types::names_in(&declared, &mut pending);
                std::sync::Arc::make_mut(&mut known).insert(name, declared);
            }
        }
        bindings.use_named_types(std::sync::Arc::clone(&known));
    }

    fn call(&self, name: &str, args: &[&OirExpr]) -> Option<OirType> {
        let signature = signature_of(name)?;
        self.call_signed(name, &signature, args)
    }

    fn call_signed(
        &self,
        name: &str,
        signature: &Signature<'_>,
        args: &[&OirExpr],
    ) -> Option<OirType> {
        let name_fn = name;
        let mut bindings = CallBindings::new();
        bindings.use_named_types(std::sync::Arc::clone(&self.named_types.borrow()));
        let mut sound = true;
        // An argument whose type is not determined is a finding, but it refutes only what depends on it: a result
        // its variable does not reach (a reduce's accumulator over a list of unknown items) stays determined.
        let mut refuted = false;
        if is_ordering(name) {
            for (index, arg) in args.iter().enumerate() {
                if let Some(operand) = self.absent_operand(arg) {
                    self.diagnostics.borrow_mut().push(Diagnostic {
                        function: name.to_string(),
                        index,
                        mismatch: Mismatch::MaybeAbsent {
                            operator: name.to_string(),
                            operand,
                        },
                        expression: printed(name, args),
                        argument: printed_arg(args.get(index)),
                    });
                }
            }
        }
        for (index, arg) in args.iter().enumerate() {
            if !signature.rejects_null(index) {
                continue;
            }
            if let Some(operand) = self.absent_operand(arg) {
                self.diagnostics.borrow_mut().push(Diagnostic {
                    function: name.to_string(),
                    index,
                    mismatch: Mismatch::NullOperand {
                        operator: name.to_string(),
                        operand,
                    },
                    expression: printed(name, args),
                    argument: printed_arg(args.get(index)),
                });
            }
        }
        // The shape a later argument names types an earlier lambda's childResults (`tree/fold`): bind it first.
        if let Some((at, shape)) = signature
            .shape_position()
            .and_then(|at| Some((at, Self::literal_text(args.get(at)?)?)))
        {
            let actual = Actual {
                known: Some(Known::Type(OirType::String)),
                text: Some(shape),
                texts: None,
                lambda: None,
            };
            let _ = signature.bind(&mut bindings, at, &actual, Mode::Static);
        }
        let declared_node = signature
            .shape_position()
            .and_then(|at| Self::literal_text(args.get(at)?))
            .and_then(|shape| {
                let name = orbital_core_base::ir::types::DeclaredShape::name_of(&shape);
                self.register_named_types(
                    &OirType::TypeRef {
                        name: name.to_string(),
                    },
                    &mut bindings,
                );
                self.ctx.named_type(name.as_ref())
            })
            .map(|ty| self.unfold(ty));
        for (index, arg) in args.iter().enumerate() {
            if let Some(declared) = &declared_node {
                self.register_named_types(declared, &mut bindings);
                signature.bind_declared_node(&mut bindings, index, declared.clone());
            }
            let refined = self.refinements.borrow().len();
            let guards = self.through_aliases(type_guards_for(name, args, index));
            self.refinements.borrow_mut().extend(guards);
            let narrowed = self.present.borrow().len();
            self.present
                .borrow_mut()
                .extend(guards_for(name, args, index));
            let outcome = match arg {
                OirExpr::Lambda { params, body } => {
                    // A callback the operator also hands the element's index: that parameter is a number.
                    let index_at = orbital_core_eval::evaluator::callback_index_position(name);
                    let dependencies = signature.lambda_child_parameters(index);
                    let provisional = std::cell::Cell::new(signature.reads_lambda_twice(index));
                    let run = |types: &[Option<OirType>]| {
                        let dropped = provisional
                            .replace(false)
                            .then(|| self.diagnostics.borrow().len());
                        let mut typed = types.to_vec();
                        if let Some(at) = index_at {
                            typed.resize(at + 1, None);
                            typed[at] = Some(OirType::Number);
                        }
                        // A parameter the call typed with the empty list (a fold's accumulator started at `[]`) has
                        // no element until the body builds one: read the body once for the list it builds, then
                        // check it with the parameter typed as that list. The first reading's findings are dropped;
                        // they were made without the element.
                        let empty = |t: &Option<OirType>| t.as_ref().is_some_and(is_empty_list);
                        if typed.iter().any(empty) {
                            let mark = self.diagnostics.borrow().len();
                            let built = self
                                .infer_lambda(
                                    params,
                                    body,
                                    &typed,
                                    index_at.is_some(),
                                    &dependencies,
                                )
                                .filter(|t| !is_empty_list(t));
                            self.diagnostics.borrow_mut().truncate(mark);
                            if let Some(built) = built {
                                for slot in typed.iter_mut().filter(|t| empty(t)) {
                                    *slot = Some(built.clone());
                                }
                            }
                        }
                        let result = self.infer_lambda(
                            params,
                            body,
                            &typed,
                            index_at.is_some(),
                            &dependencies,
                        );
                        if let Some(mark) = dropped {
                            self.diagnostics.borrow_mut().truncate(mark);
                        }
                        result
                    };
                    let actual = Actual {
                        known: None,
                        text: None,
                        texts: None,
                        lambda: Some(&run),
                    };
                    signature.bind(&mut bindings, index, &actual, Mode::Static)
                }
                OirExpr::Object { fields }
                    if signature.formal(index).is_some_and(is_closed_shape) =>
                {
                    let actuals: Vec<(String, Actual<'_>)> = fields
                        .iter()
                        .map(|(field, value)| {
                            let actual = Actual {
                                known: self.known(value),
                                text: Self::literal_text(value),
                                texts: Self::literal_texts(value),
                                lambda: None,
                            };
                            (field.clone(), actual)
                        })
                        .collect();
                    signature
                        .bind_shape_literal(&mut bindings, index, &actuals, Mode::Static)
                        .unwrap_or(Ok(()))
                }
                other => {
                    let known = self.known(other);
                    if let Some(Known::Type(ty)) = &known {
                        self.register_named_types(ty, &mut bindings);
                    }
                    let actual = Actual {
                        known,
                        text: Self::literal_text(other),
                        texts: Self::literal_texts(other),
                        lambda: None,
                    };
                    signature.bind(&mut bindings, index, &actual, Mode::Static)
                }
            };
            self.present.borrow_mut().truncate(narrowed);
            self.refinements.borrow_mut().truncate(refined);
            if let Err(mismatch) = outcome {
                sound = false;
                refuted |= !matches!(mismatch, Mismatch::Undetermined { .. });
                self.diagnostics.borrow_mut().push(Diagnostic {
                    function: name.to_string(),
                    index,
                    mismatch,
                    expression: printed(name, args),
                    argument: printed_arg(args.get(index)),
                });
            }
        }
        // What the whole call decides is only asked of a call whose arguments each fit: a rejected argument is the
        // finding, and a variable it left unbound is not a second one.
        let finished = if sound {
            signature.finish(&bindings, args.len())
        } else {
            Ok(())
        };
        if let Err((index, mismatch)) = finished {
            sound = false;
            refuted |= !matches!(mismatch, Mismatch::Undetermined { .. });
            self.diagnostics.borrow_mut().push(Diagnostic {
                function: name.to_string(),
                index,
                mismatch,
                expression: printed(name, args),
                argument: printed_arg(args.get(index)),
            });
        }
        self.void
            .set(sound && (signature.returns_void() || signature.result_is_void(&bindings)));
        if refuted {
            return None;
        }
        let result =
            signature.result_with_options(&bindings, |index, field| match args.get(index) {
                None => Some(false),
                Some(OirExpr::Object { fields }) => match fields
                    .iter()
                    .find(|(name, _)| name.as_str() == field)
                    .map(|(_, value)| value)
                {
                    None => Some(false),
                    Some(OirExpr::Bool { value }) => Some(*value),
                    _ => None,
                },
                _ => None,
            })?;
        if let (OperatorTypeRef::Tagged(OperatorTypeRefKind::Shape), OirType::TypeRef { name }) =
            (signature.returns(), &result)
        {
            if self.ctx.declared_type(name) == Some(false) {
                let index = signature.shape_position().unwrap_or_default();
                self.diagnostics.borrow_mut().push(Diagnostic {
                    function: name_fn.to_string(),
                    index,
                    mismatch: Mismatch::Type {
                        expected: "the name of a declared type".to_string(),
                        actual: format!("`{name}`, which no type declares"),
                    },
                    expression: printed(name_fn, args),
                    argument: printed_arg(args.get(index)),
                });
                return None;
            }
        }
        let defaulted = signature
            .field_projection()
            .and_then(|projection| projection.default_arg)
            .and_then(|index| args.get(index))
            .is_some_and(|default| !matches!(default, OirExpr::Null));
        let result = match result {
            OirType::Optional { inner } if defaulted => *inner,
            other => other,
        };
        Some(self.unfold(result))
    }

    /// `names_the_element` is true for a callback that is handed the element's index: both evaluators then also bind
    /// the element as `@item` and its position as `@index`, whatever the lambda names its own parameters.
    fn infer_lambda(
        &self,
        params: &[String],
        body: &OirExpr,
        types: &[Option<OirType>],
        names_the_element: bool,
        dependencies: &[(usize, usize)],
    ) -> Option<OirType> {
        let mut names: Vec<(String, Option<OirType>)> = Vec::new();
        if names_the_element {
            names.push(("item".to_string(), types.first().cloned().flatten()));
            names.push(("index".to_string(), Some(OirType::Number)));
        }
        names.extend(
            params
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), types.get(i).cloned().flatten())),
        );
        self.with_scope(&names, || {
            let depth = self.children.borrow().len();
            for (node, child) in dependencies {
                if types.get(*child).is_none_or(Option::is_none) {
                    continue;
                }
                let (Some(node), Some(child)) = (params.get(*node), params.get(*child)) else {
                    continue;
                };
                if node == child {
                    continue;
                }
                self.children.borrow_mut().push(ChildrenDependency {
                    node: OirBinding {
                        root: OirBindingRoot::Local(node.clone()),
                        path: Vec::new(),
                    },
                    children: OirBinding {
                        root: OirBindingRoot::Local(child.clone()),
                        path: Vec::new(),
                    },
                });
            }
            let result = self.infer(body);
            self.children.borrow_mut().truncate(depth);
            result
        })
    }

    /// A by-name result (the shape `json/parse` was given) is the declared type it names, when the context knows it.
    /// Facts with a tag test on a `let` local bound to one member of a value, restated on that value.
    fn through_aliases(&self, facts: Vec<TypeFact>) -> Vec<TypeFact> {
        let aliases = self.tag_aliases.borrow();
        facts
            .into_iter()
            .filter_map(|fact| match &fact.predicate {
                Narrowing::Tag { member, literals } if member.is_empty() => {
                    let OirBindingRoot::Local(local) = &fact.operand.root else {
                        return None;
                    };
                    let (_, owner, member) = aliases.iter().rev().find(|(name, _, _)| name == local)?;
                    Some(TypeFact {
                        operand: owner.clone(),
                        projection: None,
                        predicate: Narrowing::Tag { member: member.clone(), literals: literals.clone() },
                        truth: fact.truth,
                    })
                }
                _ => Some(fact),
            })
            .collect()
    }

    /// `ty` as a fact narrows it: a type test keeps the matching types, a tag test the matching tagged-union variants.
    fn narrow(&self, ty: &OirType, narrowing: &Narrowing, truth: bool) -> Option<OirType> {
        match narrowing {
            Narrowing::Type(predicate) => refine_type(&self.unfold(ty.clone()), *predicate, truth),
            Narrowing::Tag { member, literals } => self.narrow_tag(&self.unfold(ty.clone()), member, literals, truth),
        }
    }

    /// The variants of a tagged union whose `member` tag is (`truth`) or is not one of `literals`; a variant without
    /// that tag stays either way. A comparison that holds also shows an optional value present.
    fn narrow_tag(&self, ty: &OirType, member: &str, literals: &[String], truth: bool) -> Option<OirType> {
        match ty {
            OirType::Optional { inner } => {
                let narrowed = self.narrow_tag(&self.unfold((**inner).clone()), member, literals, truth)?;
                Some(if truth { narrowed } else { OirType::optional(narrowed) })
            }
            OirType::Union { variants } => {
                let kept: Vec<OirField> = variants
                    .iter()
                    .filter(|variant| match self.variant_tag(&variant.field_type, member) {
                        Some(tag) => literals.contains(&tag) == truth,
                        None => true,
                    })
                    .cloned()
                    .collect();
                match kept.as_slice() {
                    [] => None,
                    _ if kept.len() == variants.len() => Some(ty.clone()),
                    [only] => Some(only.field_type.clone()),
                    _ => Some(OirType::Union { variants: kept }),
                }
            }
            _ => Some(ty.clone()),
        }
    }

    /// The single literal a struct variant's `member` field holds (its tag), when it has one.
    fn variant_tag(&self, variant: &OirType, member: &str) -> Option<String> {
        let OirType::Struct { fields, .. } = self.unfold(variant.clone()) else {
            return None;
        };
        let field = fields.into_iter().find(|field| field.name == member)?;
        let members = match (&field.field_type, field.enum_values) {
            (OirType::Enum { name }, _) => orbital_core_base::type_compat::literal_union_members(name)?,
            (_, Some(values)) => values,
            _ => return None,
        };
        match members.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        }
    }

    fn unfold(&self, mut ty: OirType) -> OirType {
        let mut seen = std::collections::HashSet::new();
        while let OirType::TypeRef { name } | OirType::Reference { entity: name } = &ty {
            if !seen.insert(name.clone()) {
                break;
            }
            let Some(next) = self.ctx.named_type(name) else {
                break;
            };
            ty = next;
        }
        ty
    }
}

/// The type of a binding the language itself defines, whatever the program: the dispatch's timestamp, the
/// state names, the locale and theme keys, the delivery being processed, a sibling trait's render output.
fn language_binding(
    binding: &OirBinding,
    payload: Option<OirType>,
    prev_payload: Option<OirType>,
) -> Option<OirType> {
    let field = |name: &str, ty: OirType| OirField::new(name.to_string(), ty);
    // The emitter's envelope: the ten `BusEventSource` fields, each present only when the delivery carries it.
    let source = || OirType::Struct {
        additional: None,
        fields: vec![
            field("orbital", OirType::String),
            field("orbitalId", OirType::String),
            field("trait", OirType::String),
            field("traitId", OirType::String),
            field("eventId", OirType::String),
            field("transition", OirType::String),
            field("tick", OirType::String),
            field("fromBridge", OirType::Boolean),
            field("dispatched", OirType::Boolean),
            field("originClientId", OirType::String),
        ],
    };
    Some(match &binding.root {
        OirBindingRoot::Local(name) if name == "now" => OirType::Number,
        OirBindingRoot::State
        | OirBindingRoot::FromState
        | OirBindingRoot::ToState
        | OirBindingRoot::Locale
        | OirBindingRoot::CurrentTheme => OirType::String,
        OirBindingRoot::PrevStates => OirType::array(OirType::String),
        OirBindingRoot::Event => {
            let mut fields = vec![field("event", OirType::String), field("source", source())];
            if let Some(payload) = payload {
                fields.push(field("payload", payload));
            }
            OirType::Struct { additional: None, fields }
        }
        OirBindingRoot::PrevEvents => {
            let mut fields = vec![field("event", OirType::String), field("source", source())];
            if let Some(payload) = prev_payload {
                fields.push(field("payload", payload));
            }
            OirType::array(OirType::Struct { additional: None, fields })
        }
        OirBindingRoot::Trait(_) => OirType::Node,
        _ => return None,
    })
}

fn binary_name(op: &OirBinaryOp) -> &'static str {
    match op {
        OirBinaryOp::Add => "+",
        OirBinaryOp::Sub => "-",
        OirBinaryOp::Mul => "*",
        OirBinaryOp::Div => "/",
        OirBinaryOp::Mod => "%",
        OirBinaryOp::Eq => "==",
        OirBinaryOp::Ne => "!=",
        OirBinaryOp::Lt => "<",
        OirBinaryOp::Gt => ">",
        OirBinaryOp::Le => "<=",
        OirBinaryOp::Ge => ">=",
        OirBinaryOp::And => "and",
        OirBinaryOp::Or => "or",
        OirBinaryOp::Concat => "str/concat",
    }
}

/// A call's name and arguments, however the lowering spelled it.
/// The literal a `(quote <literal>)` holds, decoded from its IR body.
fn quoted_literal(
    function: &str,
    args: &[OirExpr],
) -> Option<orbital_core_base::schema::SExpression> {
    match args {
        [OirExpr::String { value }] if function == orbital_core_base::schema::quote::QUOTE_HEAD => {
            orbital_core_base::schema::quote::decode_quote_body(value)
        }
        _ => None,
    }
}

/// `(quote <string|number|boolean literal>)` is that scalar; a quote of anything else stays `SExpr`.
fn quoted_scalar(function: &str, args: &[OirExpr]) -> Option<OirType> {
    use orbital_core_base::schema::SExpression as S;
    match quoted_literal(function, args)? {
        S::String(_) => Some(OirType::String),
        S::Number(_) => Some(OirType::Number),
        S::Bool(_) => Some(OirType::Boolean),
        _ => None,
    }
}

fn as_call(expr: &OirExpr) -> Option<(&str, Vec<&OirExpr>)> {
    match expr {
        OirExpr::Call { function, args } => Some((function.as_str(), args.iter().collect())),
        OirExpr::BinaryOp { op, left, right } => {
            Some((binary_name(op), vec![left.as_ref(), right.as_ref()]))
        }
        OirExpr::UnaryOp {
            op: OirUnaryOp::Not,
            operand,
        } => Some(("not", vec![operand.as_ref()])),
        OirExpr::Conditional {
            condition,
            then_branch,
            else_branch,
        } => Some((
            "if",
            vec![
                condition.as_ref(),
                then_branch.as_ref(),
                else_branch.as_ref(),
            ],
        )),
        _ => None,
    }
}

/// The field an operand reads, spelled as its binding: `@entity.p` and `(object/get @entity p)` are one operand.
fn operand_binding(expr: &OirExpr) -> Option<OirBinding> {
    match expr {
        OirExpr::Binding { binding } => Some(binding.clone()),
        OirExpr::Call { function, args } => {
            let projection = signature_of(function)?.field_projection()?;
            if projection
                .default_arg
                .is_some_and(|i| args.get(i).is_some())
            {
                return None;
            }
            let OirExpr::Binding { binding } = args.get(projection.source_arg)? else {
                return None;
            };
            let OirExpr::String { value } = args.get(projection.key_arg)? else {
                return None;
            };
            let mut read = binding.clone();
            read.path.push(value.clone());
            Some(read)
        }
        _ => None,
    }
}

fn operand_key(expr: &OirExpr) -> Option<OirBinding> {
    operand_binding(expr)
}

fn union_of(lists: impl Iterator<Item = Vec<OirBinding>>) -> Vec<OirBinding> {
    lists.flatten().collect()
}

fn intersection_of(mut lists: impl Iterator<Item = Vec<OirBinding>>) -> Vec<OirBinding> {
    let Some(first) = lists.next() else {
        return Vec::new();
    };
    lists.fold(first, |acc, next| {
        acc.into_iter().filter(|k| next.contains(k)).collect()
    })
}

/// The operands `expr` shows present when it evaluates to `truth`: `(validate/required x)`, `(!= x null)`, a bare
/// `x`, and `and` / `or` / `not` over them. Nothing else narrows.
fn presence(expr: &OirExpr, truth: bool) -> Vec<OirBinding> {
    if truth {
        if let Some(key) = operand_key(expr) {
            return vec![key];
        }
    }
    let Some((name, args)) = as_call(expr) else {
        return Vec::new();
    };
    if truth
        && orbital_core_eval::evaluator::operators::get_operator(name)
            .and_then(|op| op.type_predicate())
            .is_some()
    {
        return args
            .first()
            .and_then(|arg| operand_key(arg))
            .into_iter()
            .collect();
    }
    let null_compared = |args: &[&OirExpr]| -> Option<OirBinding> {
        match args {
            [OirExpr::Null, other] | [other, OirExpr::Null] => operand_key(other),
            _ => None,
        }
    };
    match name {
        "validate/required" | "required" if truth => args
            .first()
            .and_then(|a| operand_key(a))
            .into_iter()
            .collect(),
        "not" => args
            .first()
            .map(|a| presence(a, !truth))
            .unwrap_or_default(),
        "and" if truth => union_of(args.iter().map(|a| presence(a, true))),
        "and" => intersection_of(args.iter().map(|a| presence(a, false))),
        "or" if truth => intersection_of(args.iter().map(|a| presence(a, true))),
        "or" => union_of(args.iter().map(|a| presence(a, false))),
        "!=" if truth => null_compared(&args).into_iter().collect(),
        "=" | "==" if !truth => null_compared(&args).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// The operands the earlier arguments of a call show present when argument `index` is evaluated.
fn guards_for(name: &str, args: &[&OirExpr], index: usize) -> Vec<OirBinding> {
    match name {
        "and" => union_of(args[..index].iter().map(|a| presence(a, true))),
        "or" => union_of(args[..index].iter().map(|a| presence(a, false))),
        "if" | "when" if index >= 1 => match args.first() {
            Some(condition) => presence(condition, name == "when" || index == 1),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

#[derive(Clone)]
struct ChildrenDependency {
    node: OirBinding,
    children: OirBinding,
}

#[derive(Clone, PartialEq)]
struct TypeFact {
    operand: OirBinding,
    projection: Option<ReadProjection>,
    predicate: Narrowing,
    truth: bool,
}

/// What a fact shows of its operand: a type test (`validate/string`), or a tagged union's tag compared with a literal.
#[derive(Clone, PartialEq)]
enum Narrowing {
    Type(orbital_core_eval::evaluator::operators::TypePredicate),
    /// The member's tag is one of `literals` (an `or` of tag tests on one member joins them).
    Tag { member: String, literals: Vec<String> },
}

#[derive(Clone, PartialEq)]
struct ReadProjection {
    key: OirExpr,
    default: Option<OirExpr>,
}

impl TypeFact {
    fn mentions_local(&self, name: &str) -> bool {
        let local = |binding: &OirBinding| matches!(&binding.root, OirBindingRoot::Local(found) if found == name);
        local(&self.operand)
            || self.projection.as_ref().is_some_and(
                |read| matches!(&read.key, OirExpr::Binding { binding } if local(binding)),
            )
    }
}

fn guarded_operand(expr: &OirExpr) -> Option<(OirBinding, Option<ReadProjection>)> {
    if let Some(binding) = operand_binding(expr) {
        return Some((binding, None));
    }
    let OirExpr::Call { function, args } = expr else {
        return None;
    };
    let projection = signature_of(function)?.field_projection()?;
    let OirExpr::Binding { binding } = args.get(projection.source_arg)? else {
        return None;
    };
    let key = args.get(projection.key_arg)?;
    if !matches!(key, OirExpr::String { .. } | OirExpr::Binding { .. }) {
        return None;
    }
    let default = projection.default_arg.and_then(|index| args.get(index));
    if default.is_some_and(|value| {
        !matches!(
            value,
            OirExpr::String { .. } | OirExpr::Number { .. } | OirExpr::Bool { .. } | OirExpr::Null
        )
    }) {
        return None;
    }
    Some((
        binding.clone(),
        Some(ReadProjection {
            key: key.clone(),
            default: default.cloned(),
        }),
    ))
}

fn type_facts(expr: &OirExpr, truth: bool) -> Vec<TypeFact> {
    let Some((name, args)) = as_call(expr) else {
        return Vec::new();
    };
    if let Some(predicate) = orbital_core_eval::evaluator::operators::get_operator(name)
        .and_then(|op| op.type_predicate())
    {
        return args
            .first()
            .and_then(|arg| guarded_operand(arg))
            .map(|(operand, projection)| TypeFact {
                operand,
                projection,
                predicate: Narrowing::Type(predicate),
                truth,
            })
            .into_iter()
            .collect();
    }
    let combine = |parts: Vec<Vec<TypeFact>>, all: bool| {
        if all {
            return parts.into_iter().flatten().collect();
        }
        let mut iter = parts.into_iter();
        let first = iter.next().unwrap_or_default();
        iter.fold(first, |facts, next| {
            facts
                .into_iter()
                .filter(|fact| next.contains(fact))
                .collect()
        })
    };
    match name {
        "=" | "==" | "!=" => {
            let equal = name != "!=";
            match args.as_slice() {
                [left, right] => tag_fact(left, right)
                    .or_else(|| tag_fact(right, left))
                    .map(|(operand, member, literal)| TypeFact {
                        operand,
                        projection: None,
                        predicate: Narrowing::Tag { member, literals: vec![literal] },
                        truth: truth == equal,
                    })
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            }
        }
        "not" => args
            .first()
            .map(|arg| type_facts(arg, !truth))
            .unwrap_or_default(),
        "and" => combine(
            args.iter().map(|arg| type_facts(arg, truth)).collect(),
            truth,
        ),
        "or" if truth => {
            let parts: Vec<Vec<TypeFact>> = args.iter().map(|arg| type_facts(arg, true)).collect();
            joined_tags(&parts).unwrap_or_else(|| combine(parts, false))
        }
        "or" => combine(
            args.iter().map(|arg| type_facts(arg, truth)).collect(),
            !truth,
        ),
        _ => Vec::new(),
    }
}

/// An `or` whose every part is one tag test on the same value and member: one test of the joined literals.
fn joined_tags(parts: &[Vec<TypeFact>]) -> Option<Vec<TypeFact>> {
    let mut joined: Option<TypeFact> = None;
    for part in parts {
        let [fact] = part.as_slice() else {
            return None;
        };
        let Narrowing::Tag { member, literals } = &fact.predicate else {
            return None;
        };
        if !fact.truth {
            return None;
        }
        match &mut joined {
            None => joined = Some(fact.clone()),
            Some(TypeFact { operand, predicate: Narrowing::Tag { member: seen, literals: all }, .. })
                if *operand == fact.operand && seen == member =>
            {
                all.extend(literals.iter().cloned());
            }
            Some(_) => return None,
        }
    }
    joined.map(|fact| vec![fact])
}

/// `side` read as one member of a value (`@n.kind`, or `(object/get @n "kind" d)` with a text key) compared with the
/// text literal `other`: the value, the member and the literal. Which variants carry that member as a tag is the
/// value's type to say; a member that is no tag narrows nothing.
fn tag_fact(side: &OirExpr, other: &OirExpr) -> Option<(OirBinding, String, String)> {
    let OirExpr::String { value: literal } = other else {
        return None;
    };
    if let Some(binding) = operand_binding(side) {
        if binding.path.is_empty() && matches!(binding.root, OirBindingRoot::Local(_)) {
            return Some((binding, String::new(), literal.clone()));
        }
        let (member, owner) = binding.path.split_last()?;
        return Some((
            OirBinding { root: binding.root.clone(), path: owner.to_vec() },
            member.clone(),
            literal.clone(),
        ));
    }
    let (operand, projection) = guarded_operand(side)?;
    let OirExpr::String { value: member } = projection?.key else {
        return None;
    };
    Some((operand, member, literal.clone()))
}

/// A read of one member of a value: `@n.kind`, or `(object/get @n "kind" d)` with a text key.
fn member_read(expr: &OirExpr) -> Option<(OirBinding, String)> {
    if let Some(binding) = operand_binding(expr) {
        let (member, owner) = binding.path.split_last()?;
        return Some((OirBinding { root: binding.root.clone(), path: owner.to_vec() }, member.clone()));
    }
    let (operand, projection) = guarded_operand(expr)?;
    let OirExpr::String { value: member } = projection?.key else {
        return None;
    };
    Some((operand, member))
}

fn type_guards_for(name: &str, args: &[&OirExpr], index: usize) -> Vec<TypeFact> {
    match name {
        "and" => args[..index]
            .iter()
            .flat_map(|arg| type_facts(arg, true))
            .collect(),
        "or" => args[..index]
            .iter()
            .flat_map(|arg| type_facts(arg, false))
            .collect(),
        "if" | "when" if index >= 1 => args
            .first()
            .map(|condition| type_facts(condition, name == "when" || index == 1))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn refine_type(
    ty: &OirType,
    predicate: orbital_core_eval::evaluator::operators::TypePredicate,
    truth: bool,
) -> Option<OirType> {
    match ty {
        OirType::Union { variants } => {
            let variants: Vec<_> = variants
                .iter()
                .filter_map(|variant| {
                    Some(OirField {
                        field_type: refine_type(&variant.field_type, predicate, truth)?,
                        ..variant.clone()
                    })
                })
                .collect();
            match variants.as_slice() {
                [] => None,
                [only] => Some(only.field_type.clone()),
                _ => Some(OirType::Union { variants }),
            }
        }
        OirType::Optional { inner } => match (truth, refine_type(inner, predicate, truth)) {
            (true, found) => found,
            (false, Some(found)) => Some(OirType::optional(found)),
            (false, None) => None,
        },
        _ if predicate
            .matches_type(ty)
            .is_none_or(|matches| matches == truth) =>
        {
            Some(ty.clone())
        }
        _ => None,
    }
}

fn is_ordering(name: &str) -> bool {
    matches!(name, "<" | "<=" | ">" | ">=")
}

/// The condition and branches of an `if`, however it was lowered.
fn conditional_parts(expr: &OirExpr) -> Option<(&OirExpr, &OirExpr, &OirExpr)> {
    match expr {
        OirExpr::Conditional { condition, then_branch, else_branch } => {
            Some((&**condition, &**then_branch, &**else_branch))
        }
        OirExpr::Call { function, args } if function == "if" => match args.as_slice() {
            [condition, then_branch, else_branch] => Some((condition, then_branch, else_branch)),
            _ => None,
        },
        _ => None,
    }
}

/// The branch of an `if` that can run when its condition is provably decided without running the program (a
/// `@config` knob substituted to its call-site literal), with the dead branch that cannot.
fn live_branch(expr: &OirExpr) -> Option<(&OirExpr, &OirExpr)> {
    let OirExpr::Conditional {
        condition,
        then_branch,
        else_branch,
    } = expr
    else {
        return None;
    };
    let decided = condition.to_sexpression()?;
    if decided.is_statically_true() {
        Some((then_branch, else_branch))
    } else if decided.is_statically_false() {
        Some((else_branch, then_branch))
    } else {
        None
    }
}

fn printed_arg(arg: Option<&&OirExpr>) -> String {
    arg.map(|a| orbital_core_base::ir::pretty_print_expr(a))
        .unwrap_or_default()
}

fn printed(name: &str, args: &[&OirExpr]) -> String {
    orbital_core_base::ir::pretty_print_expr(&OirExpr::call(
        name,
        args.iter().map(|a| (*a).clone()).collect(),
    ))
}

fn is_empty_list(ty: &OirType) -> bool {
    matches!(ty, OirType::Tuple { items } if items.is_empty())
}

fn element_of(ty: &OirType) -> Option<OirType> {
    match ty {
        OirType::Array { inner } => Some((**inner).clone()),
        OirType::Optional { inner } => element_of(inner),
        _ => None,
    }
}

/// The `OirType` an expression evaluates to, or `None` when no concrete type is determined.
pub fn infer_oir_expr_type<C: TypeCtx>(expr: &OirExpr, ctx: &C) -> Option<OirType> {
    Inferer::new(ctx).infer(expr)
}

/// Every call in `expr` (lambda bodies included, with their parameters typed) whose argument contradicts the
/// operator's signature.
/// The type inferred for each slot tag (`["typed", value, "<tag>"]`) in `expr`, by tag; an undetermined slot is absent.
pub fn typed_slot_types<C: TypeCtx>(
    expr: &OirExpr,
    ctx: &C,
) -> std::collections::HashMap<String, OirType> {
    let inferer = Inferer::new(ctx);
    inferer.known(expr);
    inferer
        .slots
        .into_inner()
        .into_iter()
        .filter_map(|(tag, ty)| ty.map(|ty| (tag, ty)))
        .collect()
}

pub fn check_oir_expr<C: TypeCtx>(expr: &OirExpr, ctx: &C) -> Vec<Diagnostic> {
    let inferer = Inferer::new(ctx);
    inferer.infer(expr);
    inferer.diagnostics.into_inner()
}

/// A `TypeCtx` that resolves no bindings — every `@entity.x` / `@payload.x` / `@config.x` is undetermined. Used
/// by the guard check so bare binding guards (intentional truthy checks, e.g. `@entity.running`) are never
/// false-positive-flagged.
pub(crate) struct NoBindingCtx;
impl TypeCtx for NoBindingCtx {
    fn binding_type(&self, _binding: &OirBinding) -> Option<OirType> {
        None
    }
}

/// Phase 4 (C-GUARD-NO-BOOLEAN-CHECK): collect warnings for `when` guard expressions that resolve to a
/// definitively non-boolean type (`Number` / `String` / `Array`), which would otherwise run with silent JS
/// truthiness coercion. Conservative by design — with `NoBindingCtx`, bindings are undetermined and are NOT
/// flagged; only structural non-boolean exprs (arithmetic, literals, number/string operator calls like
/// `math/cos`) produce a warning. One warning string per offending guard, each naming the trait + guard site.
pub fn collect_guard_type_warnings(module: &OirModule) -> Vec<String> {
    fn check_guard(guard: &OirExpr, trait_name: &str, site: &str, out: &mut Vec<String>) {
        match infer_oir_expr_type(guard, &NoBindingCtx) {
            Some(OirType::Number | OirType::String | OirType::Array { .. }) => out.push(format!(
                "{trait_name}: {site} guard does not return boolean \
                 (resolves to a non-boolean type) — it will run with JS truthiness coercion"
            )),
            _ => {}
        }
    }

    let mut warnings = Vec::new();
    for tr in &module.traits {
        for t in &tr.transitions {
            if let Some(g) = &t.guard {
                check_guard(g, &tr.name, "transition", &mut warnings);
            }
        }
        for tk in &tr.ticks {
            if let Some(g) = &tk.guard {
                check_guard(g, &tr.name, "tick", &mut warnings);
            }
        }
        for l in &tr.listeners {
            if let Some(g) = &l.guard {
                check_guard(g, &tr.name, "listener", &mut warnings);
            }
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::ir::OirBindingRoot;

    struct NoCtx;
    impl TypeCtx for NoCtx {
        fn binding_type(&self, _: &OirBinding) -> Option<OirType> {
            None
        }
    }

    fn call(name: &str, args: Vec<OirExpr>) -> OirExpr {
        OirExpr::Call {
            function: name.to_string(),
            args,
        }
    }

    fn num(v: f64) -> OirExpr {
        OirExpr::Number { value: v }
    }

    fn text(v: &str) -> OirExpr {
        OirExpr::String { value: v.into() }
    }

    fn list_of(items: Vec<OirExpr>) -> OirExpr {
        OirExpr::Array { items }
    }

    fn checked_local(value: OirExpr, ty: OirType) -> OirExpr {
        call(
            "let",
            vec![
                list_of(vec![list_of(vec![
                    text("checked"),
                    call(
                        "typed",
                        vec![value, text(&serde_json::to_string(&ty).unwrap())],
                    ),
                ])]),
                OirExpr::Binding {
                    binding: OirBinding {
                        root: OirBindingRoot::Local("checked".into()),
                        path: Vec::new(),
                    },
                },
            ],
        )
    }

    #[test]
    fn native_contract_optional_nonnull_member_read_can_be_absent() {
        for required in [false, true] {
            let mut label = OirField::new("label", OirType::String);
            label.required = required;
            let context = Row(vec![("record", OirType::Struct {
                fields: vec![label],
                additional: None,
            })]);
            assert_eq!(
                infer_oir_expr_type(&entity(&["record", "label"]), &context),
                Some(if required { OirType::String } else { OirType::optional(OirType::String) }),
                "required={required}: missing-member reads may be null; admission still excludes null"
            );
        }
    }

    #[test]
    fn native_contract_optional_nonnull_member_admits_missing_but_refuses_null() {
        let mut label = OirField::new("label", OirType::String);
        label.required = false;
        let expected = OirType::Struct { fields: vec![label], additional: None };
        for (fields, accepted) in [
            (Vec::new(), true),
            (vec![("label".into(), text("present"))], true),
            (vec![("label".into(), OirExpr::Null)], false),
        ] {
            let expr = checked_local(OirExpr::Object { fields: fields.into_iter().collect() }, expected.clone());
            assert_eq!(infer_oir_expr_type(&expr, &NoCtx).is_some(), accepted, "{expr:?}");
        }
    }

    #[test]
    fn native_contract_contextual_literal_checks_every_additional_member() {
        let expected = OirType::Struct {
            fields: vec![OirField::new("label", OirType::String)],
            additional: Some(Box::new(OirType::Number)),
        };
        for (fields, accepted) in [
            (vec![("label".into(), text("present"))], true),
            (vec![("label".into(), text("present")), ("rate".into(), num(2.0))], true),
            (vec![("label".into(), text("present")), ("rate".into(), text("wrong"))], false),
            (vec![("label".into(), num(2.0))], false),
            (Vec::new(), false),
        ] {
            let expr = checked_local(OirExpr::Object { fields: fields.into_iter().collect() }, expected.clone());
            assert_eq!(infer_oir_expr_type(&expr, &NoCtx).is_some(), accepted, "{expr:?}");
        }
    }

    #[test]
    fn native_contract_contextual_empty_literal_does_not_narrow_an_opaque_record() {
        let expected = OirType::Struct {
            fields: Vec::new(),
            additional: Some(Box::new(OirType::Number)),
        };
        let literal = checked_local(OirExpr::Object { fields: Default::default() }, expected.clone());
        assert_eq!(infer_oir_expr_type(&literal, &NoCtx), Some(expected.clone()));
        let opaque = Row(vec![("opaque", OirType::Struct { fields: Vec::new(), additional: None })]);
        let expr = checked_local(entity(&["opaque"]), expected);
        assert_eq!(infer_oir_expr_type(&expr, &opaque), None);
    }

    #[test]
    fn checked_local_annotations_refuse_wrong_or_undetermined_initializers() {
        for value in [
            text("wrong"),
            OirExpr::Binding {
                binding: OirBinding {
                    root: OirBindingRoot::Local("missing".into()),
                    path: Vec::new(),
                },
            },
        ] {
            let expr = checked_local(value, OirType::Number);
            assert!(!check_oir_expr(&expr, &NoCtx).is_empty(), "{expr:?}");
            assert_eq!(infer_oir_expr_type(&expr, &NoCtx), None);
        }
        let expr = checked_local(text("key"), OirType::CallKey);
        assert!(!check_oir_expr(&expr, &NoCtx).is_empty());
    }

    #[test]
    fn checked_local_annotations_contextualize_literal_containers_without_casts() {
        let map = OirType::Map {
            key: Box::new(OirType::String),
            value: Box::new(OirType::Number),
        };
        let expr = checked_local(
            OirExpr::Object {
                fields: Default::default(),
            },
            map.clone(),
        );
        assert_eq!(infer_oir_expr_type(&expr, &NoCtx), Some(map));
        assert!(check_oir_expr(&expr, &NoCtx).is_empty());
        let array = OirType::array(OirType::Union {
            variants: vec![
                OirField::new("number", OirType::Number),
                OirField::new("string", OirType::String),
            ],
        });
        let expr = checked_local(list_of(vec![num(1.0), text("x")]), array.clone());
        assert_eq!(infer_oir_expr_type(&expr, &NoCtx), Some(array));
        assert!(check_oir_expr(&expr, &NoCtx).is_empty());
        let required = OirType::Struct {
            additional: None,
            fields: vec![OirField::new("name", OirType::String)],
        };
        let expr = checked_local(
            OirExpr::Object {
                fields: Default::default(),
            },
            required,
        );
        assert!(!check_oir_expr(&expr, &NoCtx).is_empty());
    }

    #[test]
    fn checked_local_annotations_preserve_recursive_aliases_and_plain_list_rules() {
        let ty = OirType::array(OirType::TypeRef {
            name: "Alias".into(),
        });
        let expr = checked_local(
            list_of(vec![num(1.0), text("x"), list_of(vec![text("nested")])]),
            ty.clone(),
        );
        assert!(check_oir_expr(&expr, &DeclaredFold).is_empty());
        assert_eq!(
            infer_oir_expr_type(&expr, &DeclaredFold),
            Some(OirType::array(recursive_value_type()))
        );
        assert!(!check_oir_expr(
            &lowered(serde_json::json!(["array/append", ["list", 1, "x"], 3])),
            &NoCtx
        )
        .is_empty());
        let missing = checked_local(
            num(1.0),
            OirType::TypeRef {
                name: "Missing".into(),
            },
        );
        assert!(!check_oir_expr(&missing, &NoCtx).is_empty());
    }

    #[test]
    fn checked_local_annotations_preserve_already_optional_values_without_unwrapping() {
        let optional = OirType::optional(OirType::Number);
        let context = Row(vec![("maybe", optional.clone())]);
        let expr = checked_local(entity(&["maybe"]), optional.clone());
        assert!(check_oir_expr(&expr, &context).is_empty());
        assert_eq!(infer_oir_expr_type(&expr, &context), Some(optional));
        let narrowed = checked_local(entity(&["maybe"]), OirType::Number);
        assert!(!check_oir_expr(&narrowed, &context).is_empty());
    }

    #[test]
    fn checked_local_annotations_prove_literal_enum_keys_and_refuse_illegal_keys() {
        let map = OirType::map(
            OirType::Enum {
                name: "\"allowed\" | \"other\"".into(),
            },
            OirType::Number,
        );
        for (key, accepted) in [("allowed", true), ("illegal", false)] {
            let initializer = OirExpr::Object {
                fields: [(key.into(), num(1.0))].into_iter().collect(),
            };
            let expr = checked_local(initializer, map.clone());
            assert_eq!(check_oir_expr(&expr, &NoCtx).is_empty(), accepted, "{key}");
        }
        let unknown = OirType::map(
            OirType::TypeRef {
                name: "Missing".into(),
            },
            OirType::Number,
        );
        let empty = checked_local(
            OirExpr::Object {
                fields: Default::default(),
            },
            unknown,
        );
        assert!(
            !check_oir_expr(&empty, &NoCtx).is_empty(),
            "empty maps still require a declared key contract"
        );
    }

    /// Resolves `@entity.<field>` against a fixed field list, whole `@entity` to the struct of them.
    struct Row(Vec<(&'static str, OirType)>);
    impl TypeCtx for Row {
        fn binding_type(&self, binding: &OirBinding) -> Option<OirType> {
            if !matches!(binding.root, OirBindingRoot::Entity) {
                return None;
            }
            let whole = OirType::Struct {
                additional: None,
                fields: self
                    .0
                    .iter()
                    .map(|(n, t)| OirField::new((*n).to_string(), t.clone()))
                    .collect(),
            };
            type_at_path(&whole, &binding.path)
        }
    }

    fn entity(path: &[&str]) -> OirExpr {
        OirExpr::entity_binding(path.iter().map(|s| s.to_string()).collect())
    }

    fn tags_row() -> Row {
        Row(vec![
            ("tags", OirType::array(OirType::String)),
            ("count", OirType::Number),
            ("name", OirType::String),
        ])
    }

    #[test]
    fn literals_and_binops() {
        assert_eq!(
            infer_oir_expr_type(&num(1.0), &NoCtx),
            Some(OirType::Number)
        );
        assert_eq!(
            infer_oir_expr_type(
                &OirExpr::BinaryOp {
                    op: OirBinaryOp::Concat,
                    left: Box::new(text("a")),
                    right: Box::new(text("b")),
                },
                &NoCtx
            ),
            Some(OirType::String)
        );
    }

    #[test]
    fn static_return_type_op() {
        assert_eq!(
            infer_oir_expr_type(&call("str/concat", vec![text("a")]), &NoCtx),
            Some(OirType::String)
        );
    }

    /// G-CROSS-041: `(quote x)` is S-expression data, not an opaque value.
    #[test]
    fn quote_infers_sexpr() {
        assert_eq!(
            infer_oir_expr_type(&call("quote", vec![text("[\">\",1,0]")]), &NoCtx),
            Some(OirType::SExpr)
        );
    }

    fn quoted(value: orbital_core_base::schema::SExpression) -> OirExpr {
        call(
            "quote",
            vec![text(&orbital_core_base::schema::quote::encode_quote_body(
                &value,
            ))],
        )
    }

    fn quoted_text(v: &str) -> OirExpr {
        quoted(orbital_core_base::schema::SExpression::String(
            v.to_string(),
        ))
    }

    fn attribute_row() -> Row {
        Row(vec![
            ("@href", OirType::String),
            ("#text", OirType::Number),
            ("rel", OirType::Boolean),
        ])
    }

    #[test]
    fn quote_of_a_scalar_literal_is_that_scalar() {
        use orbital_core_base::schema::SExpression as S;
        assert_eq!(
            infer_oir_expr_type(&quoted_text("@href"), &NoCtx),
            Some(OirType::String)
        );
        assert_eq!(
            infer_oir_expr_type(&quoted(S::Number(3.0)), &NoCtx),
            Some(OirType::Number)
        );
        assert_eq!(
            infer_oir_expr_type(&quoted(S::Bool(true)), &NoCtx),
            Some(OirType::Boolean)
        );
    }

    #[test]
    fn quote_of_anything_else_stays_sexpr() {
        use orbital_core_base::schema::SExpression as S;
        let list = quoted(S::Array(vec![
            S::String(">".into()),
            S::Number(1.0),
            S::Number(0.0),
        ]));
        assert_eq!(infer_oir_expr_type(&list, &NoCtx), Some(OirType::SExpr));
        assert_eq!(
            infer_oir_expr_type(&quoted(S::Null), &NoCtx),
            Some(OirType::SExpr)
        );
        assert_eq!(
            infer_oir_expr_type(&quoted(S::Object(Default::default())), &NoCtx),
            Some(OirType::SExpr)
        );
    }

    #[test]
    fn object_get_with_a_quoted_key_reads_the_declared_field_type() {
        let row = attribute_row();
        let get = |key: OirExpr| call("object/get", vec![entity(&[]), key]);
        assert_eq!(
            infer_oir_expr_type(&get(quoted_text("@href")), &row),
            Some(OirType::String)
        );
        assert_eq!(
            infer_oir_expr_type(&get(quoted_text("#text")), &row),
            Some(OirType::Number)
        );
        // control: a bare-identifier field reads the same way through a plain key.
        assert_eq!(
            infer_oir_expr_type(&get(text("rel")), &row),
            Some(OirType::Boolean)
        );
    }

    #[test]
    fn a_quoted_key_that_names_no_field_is_reported() {
        let row = attribute_row();
        let found = check_oir_expr(
            &call("object/get", vec![entity(&[]), quoted_text("@nope")]),
            &row,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(matches!(found[0].mismatch, Mismatch::NoField { .. }));
        assert!(check_oir_expr(
            &call("object/get", vec![entity(&[]), quoted_text("@href")]),
            &row
        )
        .is_empty());
    }

    #[test]
    fn a_polymorphic_call_is_the_concrete_type_of_its_arguments() {
        // (array/first @entity.tags) is a string when tags is [string]; the control is a [number].
        let row = tags_row();
        assert_eq!(
            infer_oir_expr_type(&call("array/first", vec![entity(&["tags"])]), &row),
            Some(OirType::String)
        );
        assert_eq!(
            infer_oir_expr_type(&call("array/first", vec![list_of(vec![num(1.0)])]), &NoCtx),
            Some(OirType::Number)
        );
    }

    #[test]
    fn bytes_operators_infer_from_their_signatures() {
        let blob = call("bytes/from-string", vec![text("hi")]);
        assert_eq!(infer_oir_expr_type(&blob, &NoCtx), Some(OirType::Bytes));
        assert_eq!(
            infer_oir_expr_type(&call("bytes/length", vec![blob.clone()]), &NoCtx),
            Some(OirType::Number)
        );
        assert_eq!(
            infer_oir_expr_type(&call("bytes/to-string", vec![blob.clone()]), &NoCtx),
            Some(OirType::String)
        );
        assert_eq!(
            infer_oir_expr_type(
                &call(
                    "bytes/concat",
                    vec![list_of(vec![blob.clone(), blob.clone()])]
                ),
                &NoCtx
            ),
            Some(OirType::Bytes)
        );
        assert_eq!(
            infer_oir_expr_type(&call("bytes/equals", vec![blob.clone(), blob]), &NoCtx),
            Some(OirType::Boolean)
        );
    }

    #[test]
    fn an_undetermined_argument_leaves_the_result_undetermined() {
        assert_eq!(
            infer_oir_expr_type(
                &call("array/first", vec![entity(&["missing"])]),
                &tags_row()
            ),
            None
        );
        assert_eq!(
            infer_oir_expr_type(&call("totally/unknown", vec![]), &NoCtx),
            None
        );
    }

    #[test]
    fn object_get_reads_the_field_type_of_a_known_record() {
        let row = tags_row();
        assert_eq!(
            infer_oir_expr_type(&call("object/get", vec![entity(&[]), text("count")]), &row),
            Some(OirType::Number)
        );
        assert_eq!(
            infer_oir_expr_type(
                &call("object/get", vec![entity(&[]), text("name"), text("anon")]),
                &row
            ),
            Some(OirType::String)
        );
    }

    #[test]
    fn a_lambda_is_inferred_with_its_parameter_typed_by_the_signature() {
        // (array/map @entity.tags (fn t (str/len @t))) is [number]
        let row = tags_row();
        let lambda = OirExpr::Lambda {
            params: vec!["t".into()],
            body: Box::new(call(
                "str/len",
                vec![OirExpr::Binding {
                    binding: OirBinding::new(OirBindingRoot::Local("t".into()), vec![]),
                }],
            )),
        };
        assert_eq!(
            infer_oir_expr_type(&call("array/map", vec![entity(&["tags"]), lambda]), &row),
            Some(OirType::array(OirType::Number))
        );
    }

    #[test]
    fn a_mismatch_inside_a_lambda_body_is_reported_with_the_parameter_typed() {
        // The parameter is a string, so `(+ @t 1)` is a string passed to arithmetic.
        let row = tags_row();
        let lambda = OirExpr::Lambda {
            params: vec!["t".into()],
            body: Box::new(OirExpr::BinaryOp {
                op: OirBinaryOp::Add,
                left: Box::new(OirExpr::Binding {
                    binding: OirBinding::new(OirBindingRoot::Local("t".into()), vec![]),
                }),
                right: Box::new(num(1.0)),
            }),
        };
        let found = check_oir_expr(&call("array/map", vec![entity(&["tags"]), lambda]), &row);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].function, "+");
        assert_eq!(found[0].index, 0);
    }

    #[test]
    fn arguments_bound_to_one_variable_that_disagree_are_reported() {
        let row = tags_row();
        // (array/append @entity.tags 1): a number into a [string]
        let found = check_oir_expr(
            &call("array/append", vec![entity(&["tags"]), num(1.0)]),
            &row,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].index, 1);
        // control: the same call with a string
        assert!(check_oir_expr(
            &call("array/append", vec![entity(&["tags"]), text("x")]),
            &row
        )
        .is_empty());
    }

    #[test]
    fn an_empty_list_is_typed_by_a_sibling_argument_and_by_nothing_else() {
        let row = tags_row();
        let empty = || list_of(vec![]);
        // (array/append [] "x") is a [string]: the sibling item types the empty list.
        assert_eq!(
            infer_oir_expr_type(&call("array/append", vec![empty(), text("x")]), &row),
            Some(OirType::array(OirType::String))
        );
        assert!(check_oir_expr(&call("array/append", vec![empty(), text("x")]), &row).is_empty());
        // (or @entity.tags []) is the tags' type; so is (if c [] @entity.tags).
        assert_eq!(
            infer_oir_expr_type(&call("or", vec![entity(&["tags"]), empty()]), &row),
            Some(OirType::array(OirType::String))
        );
        // control: nothing types (array/first []) or (array/len []).
        for operator in ["array/first", "array/len"] {
            let found = check_oir_expr(&call(operator, vec![empty()]), &row);
            assert_eq!(found.len(), 1, "{operator}: {found:?}");
            assert!(
                matches!(found[0].mismatch, Mismatch::Undetermined { .. }),
                "{found:?}"
            );
        }
        // control: the same operators over a typed list are clean.
        assert!(check_oir_expr(&call("array/len", vec![entity(&["tags"])]), &row).is_empty());
        // control: two empty lists type each other no better.
        assert_eq!(
            check_oir_expr(&call("or", vec![empty(), empty()]), &row).len(),
            1
        );
    }

    #[test]
    fn a_key_that_names_no_field_is_reported() {
        let row = tags_row();
        let found = check_oir_expr(&call("object/get", vec![entity(&[]), text("nope")]), &row);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(matches!(found[0].mismatch, Mismatch::NoField { .. }));
        assert!(
            check_oir_expr(&call("object/get", vec![entity(&[]), text("count")]), &row).is_empty()
        );
    }

    #[test]
    fn an_operand_projection_is_a_binding_only_without_a_supplied_default() {
        assert_eq!(
            operand_binding(&call("object/get", vec![entity(&[]), text("count")])),
            Some(OirBinding::entity(vec!["count".into()]))
        );
        for expression in [
            call("object/get", vec![entity(&[]), text("count"), num(0.0)]),
            call("object/get", vec![entity(&[]), entity(&["name"])]),
            call("object/get", vec![text("not-a-binding"), text("count")]),
        ] {
            assert_eq!(operand_binding(&expression), None);
        }
    }

    #[test]
    fn operand_projection_keeps_the_declared_dot_path_and_literal_bracket_key() {
        for path in ["profile.id", "data[0]"] {
            assert_eq!(
                operand_binding(&call("object/get", vec![entity(&[]), text(path)])),
                Some(OirBinding::entity(vec![path.into()]))
            );
        }
    }

    struct NestedOptionalPayload {
        required: bool,
    }

    impl TypeCtx for NestedOptionalPayload {
        fn binding_type(&self, binding: &OirBinding) -> Option<OirType> {
            if binding.root != OirBindingRoot::Payload {
                return None;
            }
            type_at_path(
                &OirType::Struct {
                    additional: None,
                    fields: vec![OirField::new(
                        "result",
                        OirType::TypeRef {
                            name: "Wire".into(),
                        },
                    )],
                },
                &binding.path,
            )
        }

        fn named_type(&self, name: &str) -> Option<OirType> {
            (name == "Wire").then(|| {
                let mut field = OirField::new("rejections", OirType::array(OirType::Boolean));
                field.required = self.required;
                OirType::Struct {
                    additional: None,
                    fields: vec![field],
                }
            })
        }
    }

    fn nested_optional_array_findings(source: &str, required: bool) -> Vec<Diagnostic> {
        let expr: orbital_core_base::schema::SExpression = serde_json::from_str(source).unwrap();
        let expr = orbital_core_eval::lower::lower_sexpr_keeping_typed(&expr).unwrap();
        check_oir_expr(&expr, &NestedOptionalPayload { required })
    }

    struct OptionalTagsPayload;

    impl TypeCtx for OptionalTagsPayload {
        fn binding_type(&self, binding: &OirBinding) -> Option<OirType> {
            if binding.root != OirBindingRoot::Payload {
                return None;
            }
            let mut tags = OirField::new("tags", OirType::array(OirType::String));
            tags.required = false;
            type_at_path(
                &OirType::Struct {
                    additional: None,
                    fields: vec![OirField::new(
                        "result",
                        OirType::Struct {
                            additional: None,
                            fields: vec![tags],
                        },
                    )],
                },
                &binding.path,
            )
        }
    }

    fn let_bound_read_findings(default: serde_json::Value) -> Vec<Diagnostic> {
        let expr = lowered(serde_json::json!([
            "let",
            [["c", ["object/get", "@payload.result", "tags", default]]],
            ["array/includes", "@c", "a"]
        ]));
        check_oir_expr(&expr, &OptionalTagsPayload)
    }

    #[test]
    fn a_let_bound_read_with_a_non_null_default_is_present() {
        let found = let_bound_read_findings(serde_json::json!([]));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_let_bound_read_with_a_null_default_can_be_missing() {
        let found = let_bound_read_findings(serde_json::Value::Null);
        assert!(!found.is_empty(), "a null default leaves the read optional");
    }

    #[test]
    fn a_let_bound_read_without_a_default_can_be_missing() {
        let expr = lowered(serde_json::json!([
            "let",
            [["c", ["object/get", "@payload.result", "tags"]]],
            ["array/includes", "@c", "a"]
        ]));
        let found = check_oir_expr(&expr, &OptionalTagsPayload);
        assert!(!found.is_empty(), "no default leaves the read optional");
    }

    #[test]
    fn nested_named_optional_collection_is_a_null_operand() {
        let found = nested_optional_array_findings(
            r#"["array/some", "@payload.result.rejections", ["fn", "r", true]]"#,
            false,
        );
        assert!(
            found
                .iter()
                .any(|finding| matches!(finding.mismatch, Mismatch::NullOperand { .. })),
            "{found:?}"
        );
    }

    #[test]
    fn nested_named_required_collection_is_present() {
        let found = nested_optional_array_findings(
            r#"["array/some", "@payload.result.rejections", ["fn", "r", true]]"#,
            true,
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn nested_named_optional_collection_can_be_guarded_or_defaulted() {
        for source in [
            r#"["if", ["!=", "@payload.result.rejections", null], ["array/some", "@payload.result.rejections", ["fn", "r", true]], false]"#,
            r#"["array/some", ["or", "@payload.result.rejections", ["list"]], ["fn", "r", true]]"#,
        ] {
            let found = nested_optional_array_findings(source, false);
            assert!(found.is_empty(), "{source}: {found:?}");
        }
    }

    #[test]
    fn a_let_binding_is_typed_by_its_value() {
        // (let [[n 2]] (+ @n 1)) is a number; (let [[s "a"]] (+ @s 1)) is a string in arithmetic
        let local = |n: &str| OirExpr::Binding {
            binding: OirBinding::new(OirBindingRoot::Local(n.into()), vec![]),
        };
        let ok = call(
            "let",
            vec![
                list_of(vec![list_of(vec![text("n"), num(2.0)])]),
                OirExpr::BinaryOp {
                    op: OirBinaryOp::Add,
                    left: Box::new(local("n")),
                    right: Box::new(num(1.0)),
                },
            ],
        );
        assert_eq!(infer_oir_expr_type(&ok, &NoCtx), Some(OirType::Number));
        assert!(check_oir_expr(&ok, &NoCtx).is_empty());
        let bad = call(
            "let",
            vec![
                list_of(vec![list_of(vec![text("s"), text("a")])]),
                OirExpr::BinaryOp {
                    op: OirBinaryOp::Add,
                    left: Box::new(local("s")),
                    right: Box::new(num(1.0)),
                },
            ],
        );
        assert_eq!(check_oir_expr(&bad, &NoCtx).len(), 1);
    }

    fn lowered(value: serde_json::Value) -> OirExpr {
        let sexpr: orbital_core_base::schema::SExpression =
            serde_json::from_value(value).expect("s-expression");
        orbital_core_eval::lower::lower_sexpr(&sexpr).expect("lowers")
    }

    /// The maps of two reads inside a lambda merge into one map, so the lambda's result is typed.
    #[test]
    fn a_merge_of_maps_read_in_a_lambda_is_a_typed_result() {
        let words = OirType::map(OirType::String, OirType::String);
        let item = OirType::Struct {
            additional: None,
            fields: vec![OirField::new("entities".to_string(), words)],
        };
        let typed = Row(vec![
            ("items", OirType::array(item)),
            ("out", OirType::String),
        ]);
        let body = |entities: serde_json::Value| {
            serde_json::json!([
                "let",
                [["n", 1]],
                [
                    "set",
                    "@entity.out",
                    [
                        "json/stringify",
                        ["array/map", "@entity.items", ["fn", "v", entities]]
                    ]
                ]
            ])
        };
        let merged = body(serde_json::json!([
            "object/merge",
            ["object/get", "@v", "entities", {}],
            ["object/get", "@v", "entities", {}]
        ]));
        let found = check_oir_expr(&lowered(merged), &typed);
        assert!(
            found.is_empty(),
            "{:?}",
            found.iter().map(|d| &d.expression).collect::<Vec<_>>()
        );
        let unlike = body(
            serde_json::json!(["object/merge", ["object/get", "@v", "entities", {}], { "n": 1 }]),
        );
        assert_eq!(
            check_oir_expr(&lowered(unlike), &typed).len(),
            1,
            "control: a map and a number record have no one value type"
        );
    }

    fn recursive_value_type() -> OirType {
        OirType::Union {
            variants: vec![
                OirField::new("text", OirType::String),
                OirField::new("count", OirType::Number),
                OirField::new(
                    "list",
                    OirType::array(OirType::TypeRef {
                        name: "Value".into(),
                    }),
                ),
            ],
        }
    }

    struct DeclaredFold;
    impl TypeCtx for DeclaredFold {
        fn binding_type(&self, _: &OirBinding) -> Option<OirType> {
            Some(OirType::String)
        }
        fn named_type(&self, name: &str) -> Option<OirType> {
            match name {
                "Value" => Some(recursive_value_type()),
                "Alias" => Some(OirType::Reference {
                    entity: "Value".into(),
                }),
                _ => None,
            }
        }
    }

    #[test]
    fn declared_union_fold_nodes_narrow_by_their_exact_type_predicate() {
        for shape in ["Value", "Alias"] {
            let fold = lowered(serde_json::json!([
                "tree/fold",
                "@entity.value",
                [
                    "fn",
                    ["n", "kids"],
                    ["if", ["validate/array", "@n"], ["array/len", "@n"], 0]
                ],
                shape
            ]));
            let found = check_oir_expr(&fold, &DeclaredFold);
            assert!(found.is_empty(), "{shape}: {found:?}");
            assert_eq!(
                infer_oir_expr_type(&fold, &DeclaredFold),
                Some(OirType::Number)
            );
        }
    }

    #[test]
    fn fold_child_dependencies_follow_lexical_node_and_child_bindings() {
        let fold = |arm: serde_json::Value| {
            lowered(serde_json::json!([
                "tree/fold",
                "@entity.value",
                [
                    "fn",
                    ["n", "k"],
                    [
                        "array/concat",
                        ["if", ["validate/string", "@n"], ["list", "@n"], []],
                        arm
                    ]
                ],
                "Value"
            ]))
        };
        let direct = serde_json::json!([
            "tree/fold",
            "@entity.value",
            [
                "fn",
                ["n", "k"],
                [
                    "array/concat",
                    ["if", ["validate/string", "@n"], ["list", "@n"], []],
                    ["if", ["validate/array", "@n"], ["array/flatten", "@k"], []]
                ]
            ],
            "Alias"
        ]);
        assert!(
            check_oir_expr(&lowered(direct), &DeclaredFold).is_empty(),
            "an alias chain preserves the recursive child identity"
        );
        let guarded =
            |body: serde_json::Value| serde_json::json!(["if", ["validate/array", "@n"], body, []]);
        let captured = fold(guarded(serde_json::json!([
            "let",
            [[
                "discard",
                [
                    "array/map",
                    ["list", 1],
                    ["fn", "n", ["array/flatten", "@k"]]
                ]
            ]],
            ["array/flatten", "@k"]
        ])));
        assert!(
            check_oir_expr(&captured, &DeclaredFold).is_empty(),
            "an outer proven child binding survives a shadowed node"
        );
        let sequential_capture = fold(guarded(serde_json::json!([
            "let",
            [["n", "shadow"], ["q", ["array/flatten", "@k"]]],
            "@q"
        ])));
        assert!(
            check_oir_expr(&sequential_capture, &DeclaredFold).is_empty(),
            "sequential let bindings preserve the captured outer child proof"
        );
        let sequential_unrelated = fold(serde_json::json!([
            "let",
            [
                ["n", ["list", 1]],
                [
                    "q",
                    ["if", ["validate/array", "@n"], ["array/flatten", "@k"], []]
                ]
            ],
            "@q"
        ]));
        assert!(
            !check_oir_expr(&sequential_unrelated, &DeclaredFold).is_empty(),
            "a new let node's guard does not prove the outer child shape"
        );
        let sequential_child = fold(guarded(serde_json::json!([
            "let",
            [["k", 1], ["q", ["array/flatten", "@k"]]],
            "@q"
        ])));
        assert!(
            !check_oir_expr(&sequential_child, &DeclaredFold).is_empty(),
            "a new let child binding retains its actual scalar type"
        );
        let unrelated = fold(serde_json::json!([
            "array/flatten",
            [
                "array/map",
                ["list", ["list", 1]],
                [
                    "fn",
                    "n",
                    ["if", ["validate/array", "@n"], ["array/flatten", "@k"], []]
                ]
            ]
        ]));
        assert!(
            !check_oir_expr(&unrelated, &DeclaredFold).is_empty(),
            "a new node's guard cannot prove the outer child shape"
        );
        let shadowed_child = fold(guarded(serde_json::json!([
            "array/flatten",
            [
                "array/map",
                ["list", 1],
                ["fn", "k", ["array/flatten", "@k"]]
            ]
        ])));
        assert!(
            !check_oir_expr(&shadowed_child, &DeclaredFold).is_empty(),
            "a new child binding cannot inherit the outer relation"
        );
        let restored = fold(guarded(serde_json::json!([
            "let",
            [[
                "discard",
                ["array/map", ["list", 1], ["fn", "k", ["+", "@k", 1]]]
            ]],
            ["array/flatten", "@k"]
        ])));
        assert!(
            check_oir_expr(&restored, &DeclaredFold).is_empty(),
            "outer child projection is restored after the nested lambda"
        );
    }

    #[test]
    fn type_predicates_refine_true_false_and_logical_branches_without_leaking() {
        let ctx = Row(vec![
            ("value", recursive_value_type()),
            ("flag", OirType::Boolean),
        ]);
        for body in [
            serde_json::json!([
                "if",
                ["validate/array", "@entity.value"],
                ["array/len", "@entity.value"],
                0
            ]),
            serde_json::json!([
                "if",
                ["not", ["validate/string", "@entity.value"]],
                0,
                ["str/len", "@entity.value"]
            ]),
            serde_json::json!([
                "if",
                ["and", ["validate/array", "@entity.value"], "@entity.flag"],
                ["array/len", "@entity.value"],
                0
            ]),
            serde_json::json!([
                "and",
                ["validate/string", "@entity.value"],
                [">", ["str/len", "@entity.value"], 0]
            ]),
            serde_json::json!([
                "if",
                ["or", ["not", ["validate/array", "@entity.value"]], false],
                0,
                ["array/len", "@entity.value"]
            ]),
        ] {
            let found = check_oir_expr(&lowered(body.clone()), &ctx);
            assert!(found.is_empty(), "{body}: {found:?}");
        }
        let unguarded = lowered(serde_json::json!([
            "+",
            [
                "if",
                ["validate/array", "@entity.value"],
                ["array/len", "@entity.value"],
                0
            ],
            ["array/len", "@entity.value"]
        ]));
        assert!(
            !check_oir_expr(&unguarded, &ctx).is_empty(),
            "the sibling read is still unguarded"
        );
        let disjunction = lowered(serde_json::json!([
            "if",
            ["or", ["validate/array", "@entity.value"], "@entity.flag"],
            ["array/len", "@entity.value"],
            0
        ]));
        assert!(
            !check_oir_expr(&disjunction, &ctx).is_empty(),
            "either disjunct can make the branch run"
        );
        let shadow = Row(vec![("values", OirType::array(recursive_value_type()))]);
        let nested = lowered(serde_json::json!([
            "array/map",
            ["list", "text"],
            [
                "fn",
                "n",
                [
                    "if",
                    ["validate/string", "@n"],
                    [
                        "array/map",
                        "@entity.values",
                        ["fn", "n", ["str/len", "@n"]]
                    ],
                    []
                ]
            ]
        ]));
        assert!(
            !check_oir_expr(&nested, &shadow).is_empty(),
            "a shadowed lambda does not inherit its parent's proof"
        );
    }

    #[test]
    fn predicates_refine_repeated_dynamic_reads_with_their_exact_default() {
        let ctx = Row(vec![
            (
                "values",
                OirType::map(OirType::String, recursive_value_type()),
            ),
            ("key", OirType::String),
        ]);
        let read = serde_json::json!(["object/get", "@entity.values", "@entity.key", ""]);
        let expr = lowered(serde_json::json!([
            "if",
            ["validate/array", read.clone()],
            ["array/len", read.clone()],
            0
        ]));
        let found = check_oir_expr(&expr, &ctx);
        assert!(found.is_empty(), "{found:?}");
        let static_read = serde_json::json!(["object/get", "@entity.values", "fixed"]);
        let static_probe = lowered(serde_json::json!([
            "if",
            ["validate/array", static_read.clone()],
            ["array/len", static_read],
            0
        ]));
        assert!(
            check_oir_expr(&static_probe, &ctx).is_empty(),
            "static projected reads share their canonical binding proof"
        );
        let local_read = serde_json::json!(["object/get", "@entity.values", "@key", ""]);
        let scoped = |body: serde_json::Value| {
            lowered(serde_json::json!([
                "let",
                [["key", "@entity.key"]],
                ["if", ["validate/array", local_read.clone()], body, 0]
            ]))
        };
        let shadowed = scoped(serde_json::json!([
            "array/len",
            [
                "array/map",
                ["list", "other"],
                ["fn", "key", ["array/len", local_read.clone()]]
            ]
        ]));
        assert!(
            !check_oir_expr(&shadowed, &ctx).is_empty(),
            "a shadowed key does not reuse the outer read's proof"
        );
        let restored = scoped(serde_json::json!([
            "let",
            [[
                "discard",
                ["array/map", ["list", "other"], ["fn", "key", "@key"]]
            ]],
            ["array/len", local_read.clone()]
        ]));
        assert!(
            check_oir_expr(&restored, &ctx).is_empty(),
            "the outer key proof is restored after the nested scope"
        );
        let different_default = lowered(serde_json::json!([
            "if",
            [
                "validate/array",
                ["object/get", "@entity.values", "@entity.key", ["list", 1]]
            ],
            ["array/len", read.clone()],
            0
        ]));
        assert!(
            !check_oir_expr(&different_default, &ctx).is_empty(),
            "changing the default changes the operand being proven"
        );
        let different_key = lowered(serde_json::json!([
            "if",
            ["validate/array", read.clone()],
            ["array/len", ["object/get", "@entity.values", "other", ""]],
            0
        ]));
        assert!(
            !check_oir_expr(&different_key, &ctx).is_empty(),
            "another key is not proven"
        );
    }

    #[test]
    fn branch_records_keep_each_known_field_type_and_named_rows_reject_bad_fields() {
        struct Cells;
        impl TypeCtx for Cells {
            fn binding_type(&self, binding: &OirBinding) -> Option<OirType> {
                if binding.path.first().is_some_and(|field| field == "flag") {
                    Some(OirType::Boolean)
                } else {
                    Some(OirType::array(OirType::TypeRef {
                        name: "SampleCell".into(),
                    }))
                }
            }
            fn named_type(&self, name: &str) -> Option<OirType> {
                (name == "SampleCell").then(|| OirType::Struct {
                    additional: None,
                    fields: vec![OirField::new("kind", OirType::String)],
                })
            }
            fn object_literals_are_exact(&self) -> bool {
                true
            }
        }
        let branch = lowered(serde_json::json!(["if", "@entity.flag", {"raw":"x"}, {"raw":3}]));
        let found = check_oir_expr(&branch, &Cells);
        assert!(found.is_empty(), "{found:?}");
        let OirType::Struct { fields, .. } =
            infer_oir_expr_type(&branch, &Cells).expect("record result")
        else {
            panic!("record result")
        };
        let OirType::Union { variants } = &fields[0].field_type else {
            panic!("both raw types remain")
        };
        assert!(variants.iter().any(|v| v.field_type == OirType::String));
        assert!(variants.iter().any(|v| v.field_type == OirType::Number));
        for item in [serde_json::json!({"kind":3}), serde_json::json!(3)] {
            let expr = lowered(serde_json::json!(["array/append", "@entity.cells", item]));
            assert!(
                !check_oir_expr(&expr, &Cells).is_empty(),
                "malformed row must fail"
            );
        }
        let good = lowered(serde_json::json!(["array/append", "@entity.cells", {"kind":"ok"}]));
        assert!(check_oir_expr(&good, &Cells).is_empty());
    }

    #[test]
    fn a_shadowed_nullable_lambda_does_not_inherit_outer_presence() {
        let ctx = Row(vec![(
            "values",
            OirType::array(OirType::optional(OirType::Number)),
        )]);
        let probe = |inner: serde_json::Value| {
            lowered(serde_json::json!([
                "array/map",
                ["list", ["list", 1]],
                [
                    "fn",
                    "x",
                    [
                        "if",
                        ["validate/array", "@x"],
                        ["array/map", "@entity.values", ["fn", "x", inner]],
                        []
                    ]
                ]
            ]))
        };
        let bad = check_oir_expr(&probe(serde_json::json!([">", "@x", 0])), &ctx);
        assert!(
            bad.iter()
                .any(|finding| matches!(finding.mismatch, Mismatch::MaybeAbsent { .. })),
            "{bad:?}"
        );
        let good = check_oir_expr(
            &probe(serde_json::json!([
                "if",
                ["validate/number", "@x"],
                [">", "@x", 0],
                false
            ])),
            &ctx,
        );
        assert!(good.is_empty(), "{good:?}");
    }

    fn tree_row() -> Row {
        let own = || OirType::TypeRef {
            name: "Tree".to_string(),
        };
        let tree = OirType::Struct {
            additional: None,
            fields: vec![
                OirField::new("label".to_string(), OirType::String),
                OirField::new("kids".to_string(), OirType::array(own())),
                OirField::new("parent".to_string(), own()),
                OirField::new("tags".to_string(), OirType::array(OirType::String)),
            ],
        };
        Row(vec![("tree", tree), ("out", OirType::String)])
    }

    fn fold_over_tree(step: serde_json::Value) -> serde_json::Value {
        serde_json::json!([
            "tree/fold",
            "@entity.tree",
            ["fn", ["n", "k"], step],
            "Tree"
        ])
    }

    fn reported(expr: &OirExpr, ctx: &Row) -> Vec<String> {
        check_oir_expr(expr, ctx)
            .iter()
            .map(|d| format!("{}: {}", d.expression, d.mismatch))
            .collect()
    }

    #[test]
    fn a_fold_step_reads_child_results_of_its_own_result_type() {
        let ctx = tree_row();
        let sum = lowered(fold_over_tree(serde_json::json!([
            "+",
            1,
            ["array/len", "@k.kids"]
        ])));
        assert_eq!(
            reported(&sum, &ctx),
            Vec::<String>::new(),
            "the provisional reading's findings are dropped"
        );
        assert_eq!(infer_oir_expr_type(&sum, &ctx), Some(OirType::Number));
        let joined = lowered(fold_over_tree(serde_json::json!([
            "str/concat",
            "@n.label",
            ["str/join", "@k.kids", ","]
        ])));
        assert_eq!(
            reported(&joined, &ctx),
            Vec::<String>::new(),
            "control: string children of a string step"
        );
        assert_eq!(infer_oir_expr_type(&joined, &ctx), Some(OirType::String));
    }

    #[test]
    fn a_fold_step_that_reads_its_child_results_as_another_type_is_a_mismatch() {
        let ctx = tree_row();
        let step = serde_json::json!([
            "+",
            ["array/len", "@n.tags"],
            ["str/len", ["object/get", "@k", "parent", ""]]
        ]);
        let found = reported(&lowered(fold_over_tree(step)), &ctx);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("parent") && found[0].contains("expected number, got string"),
            "the child result is the step's number, read with a string default: {found:?}"
        );
    }

    struct PredCtx;
    impl TypeCtx for PredCtx {
        fn binding_type(&self, binding: &OirBinding) -> Option<OirType> {
            matches!(binding.root, OirBindingRoot::Entity)
                .then(|| OirType::TypeRef { name: "Pred".into() })
        }
        fn named_type(&self, name: &str) -> Option<OirType> {
            let own = || OirType::TypeRef { name: "Pred".into() };
            let node = |of: OirType| OirType::Struct {
                additional: None,
                fields: vec![
                    OirField::new("kind".to_string(), OirType::String),
                    OirField::new("of".to_string(), of),
                ],
            };
            (name == "Pred").then(|| OirType::Union {
                variants: vec![
                    OirField::new("All", node(OirType::array(own()))),
                    OirField::new("Not", node(own())),
                ],
            })
        }
    }

    #[test]
    fn a_fold_step_reading_a_single_child_variants_field_as_a_list_is_rejected_unguarded() {
        let fold = |step: serde_json::Value| {
            lowered(serde_json::json!([
                "tree/fold",
                "@entity.p",
                ["fn", ["n", "k"], step],
                "Pred"
            ]))
        };
        let unguarded = fold(serde_json::json!([
            "if",
            ["=", ["object/get", "@n", "kind", ""], "leaf"],
            1,
            ["array/len", ["object/get", "@k", "of", []]]
        ]));
        assert!(
            !check_oir_expr(&unguarded, &PredCtx).is_empty(),
            "`of` is a list on one variant and one result on the other"
        );
        let guarded = fold(serde_json::json!([
            "let",
            [["o", ["object/get", "@k", "of", 0]]],
            ["if", ["validate/array", "@o"], ["array/len", "@o"], 1]
        ]));
        assert!(
            check_oir_expr(&guarded, &PredCtx).is_empty(),
            "control: a read guarded as a list is accepted: {:?}",
            check_oir_expr(&guarded, &PredCtx)
        );
    }

    #[test]
    fn json_parse_is_the_type_its_shape_names() {
        struct Named;
        impl TypeCtx for Named {
            fn binding_type(&self, _: &OirBinding) -> Option<OirType> {
                None
            }
            fn named_type(&self, name: &str) -> Option<OirType> {
                (name == "Point").then(|| OirType::Struct {
                    additional: None,
                    fields: vec![OirField::new("x".to_string(), OirType::Number)],
                })
            }
        }
        let parse = call("json/parse", vec![text("{}"), text("Point")]);
        assert_eq!(
            infer_oir_expr_type(&parse, &Named),
            Some(OirType::Struct {
                additional: None,
                fields: vec![OirField::new("x".to_string(), OirType::Number)]
            })
        );
        assert_eq!(
            infer_oir_expr_type(&parse, &NoCtx),
            Some(OirType::TypeRef {
                name: "Point".into()
            }),
            "control: a context that does not know the name keeps the by-name reference"
        );
    }

    #[test]
    fn and_or_results_are_the_operand_type() {
        let both_strings = OirExpr::BinaryOp {
            op: OirBinaryOp::Or,
            left: Box::new(entity(&["name"])),
            right: Box::new(text("anon")),
        };
        assert_eq!(
            infer_oir_expr_type(&both_strings, &tags_row()),
            Some(OirType::String)
        );
        let mixed = OirExpr::BinaryOp {
            op: OirBinaryOp::Or,
            left: Box::new(entity(&["name"])),
            right: Box::new(OirExpr::Bool { value: true }),
        };
        assert_eq!(check_oir_expr(&mixed, &tags_row()).len(), 1);
    }

    // ---- Phase 4: collect_guard_type_warnings (C-GUARD-NO-BOOLEAN-CHECK) ----
    use orbital_core_base::ir::{
        OirInterval, OirListener, OirModule, OirTick, OirTrait, OirTransition,
    };

    fn binop(op: OirBinaryOp, l: OirExpr, r: OirExpr) -> OirExpr {
        OirExpr::BinaryOp {
            op,
            left: Box::new(l),
            right: Box::new(r),
        }
    }

    fn module_with_transition_guard(guard: OirExpr) -> OirModule {
        let mut tr = OirTransition::new("s", "s2", "e");
        tr.guard = Some(guard);
        let mut trait_ = OirTrait::new("MyTrait");
        trait_.transitions.push(tr);
        let mut m = OirModule::new("App", "1.0.0");
        m.traits.push(trait_);
        m
    }

    #[test]
    fn guard_warns_on_arithmetic() {
        // (+ 1 2) → Number → 1 warning.
        let m = module_with_transition_guard(binop(
            OirBinaryOp::Add,
            OirExpr::Number { value: 1.0 },
            OirExpr::Number { value: 2.0 },
        ));
        let w = collect_guard_type_warnings(&m);
        assert_eq!(w.len(), 1, "arithmetic guard must warn: {w:?}");
        assert!(w[0].contains("MyTrait") && w[0].contains("transition"));
    }

    #[test]
    fn guard_warns_on_number_literal() {
        // bare number literal as a guard → Number → 1 warning.
        let m = module_with_transition_guard(OirExpr::Number { value: 5.0 });
        assert_eq!(collect_guard_type_warnings(&m).len(), 1);
    }

    #[test]
    fn guard_warns_on_string_returning_call() {
        // (str/concat ...) → String → 1 warning.
        let m = module_with_transition_guard(call(
            "str/concat",
            vec![OirExpr::String { value: "a".into() }],
        ));
        assert_eq!(collect_guard_type_warnings(&m).len(), 1);
    }

    #[test]
    fn guard_passes_comparison() {
        // (= x 5) → Boolean → 0 warnings.
        let m = module_with_transition_guard(binop(
            OirBinaryOp::Eq,
            OirExpr::entity_binding(vec!["x".into()]),
            OirExpr::Number { value: 5.0 },
        ));
        assert!(
            collect_guard_type_warnings(&m).is_empty(),
            "comparison guard must not warn"
        );
    }

    #[test]
    fn guard_passes_and_or() {
        // (and … …) → Json (JS-truthy union) → must NOT warn (no false positive).
        let m = module_with_transition_guard(binop(
            OirBinaryOp::And,
            OirExpr::entity_binding(vec!["a".into()]),
            OirExpr::entity_binding(vec!["b".into()]),
        ));
        assert!(
            collect_guard_type_warnings(&m).is_empty(),
            "and/or guard must not warn (Json truthy semantics)"
        );
    }

    #[test]
    fn guard_passes_bare_binding() {
        // @entity.running → NoBindingCtx → Json → must NOT warn.
        let m = module_with_transition_guard(OirExpr::entity_binding(vec!["running".into()]));
        assert!(
            collect_guard_type_warnings(&m).is_empty(),
            "bare binding guard must not warn (intentional truthy check)"
        );
    }

    #[test]
    fn guard_covers_tick_and_listener() {
        // number-returning guards on a tick AND a listener → 2 warnings.
        let mut trait_ = OirTrait::new("MyTrait");
        let mut tk = OirTick::new("t", OirInterval::Seconds { value: 1 });
        tk.guard = Some(OirExpr::Number { value: 1.0 });
        trait_.ticks.push(tk);
        let mut li = OirListener::new("evt", "trig");
        li.guard = Some(binop(
            OirBinaryOp::Add,
            OirExpr::Number { value: 1.0 },
            OirExpr::Number { value: 2.0 },
        ));
        trait_.listeners.push(li);
        let mut m = OirModule::new("App", "1.0.0");
        m.traits.push(trait_);
        let w = collect_guard_type_warnings(&m);
        assert_eq!(w.len(), 2, "tick + listener guards must each warn: {w:?}");
        assert!(w.iter().any(|s| s.contains("tick")));
        assert!(w.iter().any(|s| s.contains("listener")));
    }

    struct ExactRow(Row);
    impl TypeCtx for ExactRow {
        fn binding_type(&self, binding: &OirBinding) -> Option<OirType> {
            self.0.binding_type(binding)
        }
        fn object_literals_are_exact(&self) -> bool {
            true
        }
    }

    fn shape_row() -> OirType {
        let mut entity = OirField::new("entity", OirType::String);
        entity.required = false;
        let mut address = OirField::new("address", OirType::String);
        address.required = false;
        OirType::Struct {
            additional: None,
            fields: vec![OirField::new("name", OirType::String), entity, address],
        }
    }

    fn record(fields: Vec<(&str, OirExpr)>) -> OirExpr {
        OirExpr::Object { fields: fields.into_iter().map(|(n, v)| (n.to_string(), v)).collect() }
    }

    fn set_rows(value: OirExpr) -> OirExpr {
        OirExpr::effect(orbital_core_base::ir::OirEffect::set(
            OirBinding { root: OirBindingRoot::Entity, path: vec!["rows".into()] },
            value,
        ))
    }

    fn two_shapes() -> OirExpr {
        OirExpr::Conditional {
            condition: Box::new(entity(&["flag"])),
            then_branch: Box::new(list_of(vec![record(vec![("name", text("a")), ("entity", text("x"))])])),
            else_branch: Box::new(list_of(vec![record(vec![("name", text("b")), ("address", text("y"))])])),
        }
    }

    #[test]
    fn set_branches_that_differ_in_optional_fields_fit_the_declared_element_type() {
        let ctx = ExactRow(Row(vec![("flag", OirType::Boolean), ("rows", OirType::array(shape_row()))]));
        assert!(check_oir_expr(&set_rows(two_shapes()), &ctx).is_empty());
        let nested = OirExpr::Conditional {
            condition: Box::new(entity(&["flag"])),
            then_branch: Box::new(two_shapes()),
            else_branch: Box::new(list_of(vec![])),
        };
        assert!(check_oir_expr(&set_rows(nested), &ctx).is_empty());
    }

    #[test]
    fn control_set_branches_the_declared_type_does_not_admit_still_conflict() {
        let ctx = ExactRow(Row(vec![("flag", OirType::Boolean), ("rows", OirType::array(shape_row()))]));
        let wrong = OirExpr::Conditional {
            condition: Box::new(entity(&["flag"])),
            then_branch: Box::new(list_of(vec![record(vec![("name", text("a")), ("entity", text("x"))])])),
            else_branch: Box::new(list_of(vec![record(vec![("name", num(1.0))])])),
        };
        assert!(!check_oir_expr(&set_rows(wrong), &ctx).is_empty());
        let undeclared = ExactRow(Row(vec![("flag", OirType::Boolean)]));
        assert!(!check_oir_expr(&set_rows(two_shapes()), &undeclared).is_empty());
        let scalar = ExactRow(Row(vec![("flag", OirType::Boolean), ("rows", OirType::Number)]));
        assert!(!check_oir_expr(&set_rows(two_shapes()), &scalar).is_empty());
    }

    #[test]
    fn a_checked_local_admits_branches_its_declared_type_admits() {
        let expected = OirType::array(shape_row());
        let ctx = ExactRow(Row(vec![("flag", OirType::Boolean)]));
        assert!(check_oir_expr(&checked_local(two_shapes(), expected), &ctx).is_empty());
    }

    #[test]
    fn a_list_conflict_names_the_item_types_that_differ() {
        let ctx = ExactRow(Row(vec![("flag", OirType::Boolean)]));
        let found = check_oir_expr(&two_shapes(), &ctx);
        let Some(Mismatch::Conflict { expected, actual }) = found.first().map(|d| d.mismatch.clone())
        else {
            panic!("{found:?}");
        };
        assert_ne!(expected, actual);
        assert!(expected.contains("entity") && expected.starts_with('['), "{expected}");
        assert!(actual.contains("address") && actual.starts_with('['), "{actual}");
    }
}
