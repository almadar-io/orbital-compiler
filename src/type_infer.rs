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

use orbital_core_eval::evaluator::operator_signature;
use orbital_core_base::ir::{OirBinaryOp, OirBinding, OirBindingRoot, OirExpr, OirField, OirModule, OirType, OirUnaryOp};
use orbital_core_base::language::registry::{get_operator, OperatorTypeRef, OperatorTypeRefKind};
use orbital_core_base::signature::{type_at_path, Actual, CallBindings, Known, Mismatch, Mode, Signature};

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
    matches!(formal, OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, open }) if *open != Some(true) && !fields.is_empty())
}

/// Whether `name` is an effect: it runs for what it does and produces no value.
pub(crate) fn is_effect(name: &str) -> bool {
    orbital_core_base::ir::is_substrate_operator(name) || get_operator(name).is_some_and(|meta| meta.has_side_effects)
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
    /// The operands (printed) a presence check on the way in has shown present.
    present: RefCell<Vec<String>>,
    /// The `let` locals bound to an operand that can be missing.
    absent_locals: RefCell<Vec<String>>,
    /// The type inferred for each `["typed", value, "#slot:<n>"]` the typed-arguments pass tagged.
    slots: RefCell<Vec<(String, Option<OirType>)>>,
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
                if let orbital_core_base::ir::OirEffect::Os { op, args, key, .. } = effect.as_ref() {
                    if let Some(meta) = get_operator(op.name()) {
                        let refs: Vec<&OirExpr> = args.iter().collect();
                        self.call_signed(op.name(), &Signature::of(meta), &refs);
                        if let Some(key) = key {
                            self.known(key);
                        }
                        return Some(Known::Void);
                    }
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
                    OirExpr::Call { .. } | OirExpr::Conditional { .. } | OirExpr::BinaryOp { .. } | OirExpr::UnaryOp { .. }
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
        let key = binding.to_string();
        if self.present.borrow().contains(&key) {
            return None;
        }
        let absent = match &binding.root {
            OirBindingRoot::Local(name) => binding.path.is_empty() && self.absent_locals.borrow().contains(name),
            _ => self.ctx.may_be_absent(&binding),
        };
        absent.then(|| printed_arg(Some(&arg)))
    }

    fn binding(&self, binding: &OirBinding) -> Option<OirType> {
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
        if let Some(builtin) = language_binding(binding, self.ctx.delivery_payload(), self.ctx.prev_delivery_payload()) {
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
        let inferred = self.infer_bare(expr)?;
        Some(self.unfold_rows(inferred))
    }

    fn infer_bare(&self, expr: &OirExpr) -> Option<OirType> {
        match expr {
            OirExpr::Null => None,
            OirExpr::Bool { .. } => Some(OirType::Boolean),
            OirExpr::Number { .. } => Some(OirType::Number),
            OirExpr::String { .. } => Some(OirType::String),
            OirExpr::Array { items } if items.is_empty() => Some(OirType::Tuple { items: Vec::new() }),
            OirExpr::Array { items } => self.call("list", &items.iter().collect::<Vec<_>>()),
            // The fields an object literal writes are not all the fields of the type it stands for (a default that
            // omits optional keys): a record of unknown extent. The field values are still checked.
            OirExpr::Object { fields } => {
                let typed: Vec<(String, Option<OirType>)> =
                    fields.iter().map(|(name, value)| (name.clone(), self.infer(value))).collect();
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
                Some(OirType::Struct { fields: record })
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
            OirExpr::Call { function, args } if quoted_scalar(function, args).is_some() => quoted_scalar(function, args),
            OirExpr::Call { function, args } => self.call(function, &args.iter().collect::<Vec<_>>()),
            OirExpr::Map {
                source,
                variable,
                body,
            } => {
                let element = element_of(&self.infer(source)?);
                let result = self.with_scope(&[(variable.clone(), element)], || self.infer(body))?;
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
                    &[(variable.clone(), element), (accumulator.clone(), accumulated.clone())],
                    || self.infer(body),
                );
                accumulated
            }
            OirExpr::Lambda { .. } => None,
            // An `os/` effect has no value; its arguments are checked against the operation's parameters where it stands.
            OirExpr::Effect { effect } if matches!(effect.as_ref(), orbital_core_base::ir::OirEffect::Os { .. }) => {
                self.known(expr);
                None
            }
            OirExpr::Effect { .. } => None,
        }
    }

    fn with_scope<R>(&self, names: &[(String, Option<OirType>)], f: impl FnOnce() -> R) -> R {
        let depth = self.scopes.borrow().len();
        self.scopes.borrow_mut().extend(names.iter().cloned());
        let out = f();
        self.scopes.borrow_mut().truncate(depth);
        out
    }

    /// `(let [[name value] …] body)`: each name is in scope, typed by its value, for the bindings after it and the body.
    fn infer_let(&self, args: &[OirExpr]) -> Option<OirType> {
        let (OirExpr::Array { items: bindings }, Some(body)) = (args.first()?, args.get(1)) else {
            return None;
        };
        let depth = self.scopes.borrow().len();
        let absent_depth = self.absent_locals.borrow().len();
        for pair in bindings {
            if let OirExpr::Array { items } = pair {
                if let (Some(OirExpr::String { value: name }), Some(value)) = (items.first(), items.get(1)) {
                    let ty = self.infer(value);
                    if self.absent_operand(value).is_some() {
                        self.absent_locals.borrow_mut().push(name.clone());
                    }
                    self.scopes.borrow_mut().push((name.clone(), ty));
                }
            }
        }
        let names: Vec<(String, Option<OirType>)> = self.scopes.borrow()[depth..].to_vec();
        self.scopes.borrow_mut().truncate(depth);
        let body_known = self.with_scope(&names, || self.known(body));
        self.absent_locals.borrow_mut().truncate(absent_depth);
        match body_known {
            Some(Known::Type(ty)) => Some(ty),
            Some(Known::Void) => {
                self.void.set(true);
                None
            }
            _ => None,
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

    fn call(&self, name: &str, args: &[&OirExpr]) -> Option<OirType> {
        let signature = signature_of(name)?;
        self.call_signed(name, &signature, args)
    }

    fn call_signed(&self, name: &str, signature: &Signature<'_>, args: &[&OirExpr]) -> Option<OirType> {
        let name_fn = name;
        let mut bindings = CallBindings::new();
        let mut sound = true;
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
        for (index, arg) in args.iter().enumerate() {
            let narrowed = self.present.borrow().len();
            self.present.borrow_mut().extend(guards_for(name, args, index));
            let outcome = match arg {
                OirExpr::Lambda { params, body } => {
                    // A callback the operator also hands the element's index: that parameter is a number.
                    let index_at = orbital_core_eval::evaluator::callback_index_position(name);
                    let provisional = std::cell::Cell::new(signature.reads_lambda_twice(index));
                    let run = |types: &[Option<OirType>]| {
                        let dropped = provisional.replace(false).then(|| self.diagnostics.borrow().len());
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
                            let built = self.infer_lambda(params, body, &typed, index_at.is_some()).filter(|t| !is_empty_list(t));
                            self.diagnostics.borrow_mut().truncate(mark);
                            if let Some(built) = built {
                                for slot in typed.iter_mut().filter(|t| empty(t)) {
                                    *slot = Some(built.clone());
                                }
                            }
                        }
                        let result = self.infer_lambda(params, body, &typed, index_at.is_some());
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
                OirExpr::Object { fields } if signature.formal(index).is_some_and(is_closed_shape) => {
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
            if let Err(mismatch) = outcome {
                sound = false;
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
        let finished = if sound { signature.finish(&bindings, args.len()) } else { Ok(()) };
        if let Err((index, mismatch)) = finished {
            sound = false;
            self.diagnostics.borrow_mut().push(Diagnostic {
                function: name.to_string(),
                index,
                mismatch,
                expression: printed(name, args),
                argument: printed_arg(args.get(index)),
            });
        }
        self.void.set(sound && (signature.returns_void() || signature.result_is_void(&bindings)));
        if !sound {
            return None;
        }
        let result = signature.result(&bindings)?;
        if let (OperatorTypeRef::Tagged(OperatorTypeRefKind::Shape), OirType::TypeRef { name }) =
            (signature.returns(), &result)
        {
            if self.ctx.declared_type(name) == Some(false) {
                let index = signature
                    .shape_position()
                    .unwrap_or_default();
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
        self.with_scope(&names, || self.infer(body))
    }

    /// A by-name result (the shape `json/parse` was given) is the declared type it names, when the context knows it.
    fn unfold(&self, ty: OirType) -> OirType {
        match &ty {
            OirType::TypeRef { name } => self.ctx.named_type(name).unwrap_or(ty),
            _ => ty,
        }
    }
}

/// The type of a binding the language itself defines, whatever the program: the dispatch's timestamp, the
/// state names, the locale and theme keys, the delivery being processed, a sibling trait's render output.
fn language_binding(binding: &OirBinding, payload: Option<OirType>, prev_payload: Option<OirType>) -> Option<OirType> {
    let field = |name: &str, ty: OirType| OirField::new(name.to_string(), ty);
    // The emitter's envelope: the ten `BusEventSource` fields, each present only when the delivery carries it.
    let source = || OirType::Struct {
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
            OirType::Struct { fields }
        }
        OirBindingRoot::PrevEvents => {
            let mut fields = vec![field("event", OirType::String), field("source", source())];
            if let Some(payload) = prev_payload {
                fields.push(field("payload", payload));
            }
            OirType::array(OirType::Struct { fields })
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
fn quoted_literal(function: &str, args: &[OirExpr]) -> Option<orbital_core_base::schema::SExpression> {
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
        OirExpr::BinaryOp { op, left, right } => Some((binary_name(op), vec![left.as_ref(), right.as_ref()])),
        OirExpr::UnaryOp { op: OirUnaryOp::Not, operand } => Some(("not", vec![operand.as_ref()])),
        OirExpr::Conditional { condition, then_branch, else_branch } => {
            Some(("if", vec![condition.as_ref(), then_branch.as_ref(), else_branch.as_ref()]))
        }
        _ => None,
    }
}

/// The field an operand reads, spelled as its binding: `@entity.p` and `(object/get @entity p)` are one operand.
fn operand_binding(expr: &OirExpr) -> Option<OirBinding> {
    match expr {
        OirExpr::Binding { binding } => Some(binding.clone()),
        OirExpr::Call { function, args } => {
            let projection = signature_of(function)?.field_projection()?;
            if projection.default_arg.is_some_and(|i| args.get(i).is_some()) {
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

fn operand_key(expr: &OirExpr) -> Option<String> {
    operand_binding(expr).map(|b| b.to_string())
}

fn union_of(lists: impl Iterator<Item = Vec<String>>) -> Vec<String> {
    lists.flatten().collect()
}

fn intersection_of(mut lists: impl Iterator<Item = Vec<String>>) -> Vec<String> {
    let Some(first) = lists.next() else {
        return Vec::new();
    };
    lists.fold(first, |acc, next| acc.into_iter().filter(|k| next.contains(k)).collect())
}

/// The operands `expr` shows present when it evaluates to `truth`: `(validate/required x)`, `(!= x null)`, a bare
/// `x`, and `and` / `or` / `not` over them. Nothing else narrows.
fn presence(expr: &OirExpr, truth: bool) -> Vec<String> {
    if truth {
        if let Some(key) = operand_key(expr) {
            return vec![key];
        }
    }
    let Some((name, args)) = as_call(expr) else {
        return Vec::new();
    };
    let null_compared = |args: &[&OirExpr]| -> Option<String> {
        match args {
            [OirExpr::Null, other] | [other, OirExpr::Null] => operand_key(other),
            _ => None,
        }
    };
    match name {
        "validate/required" | "required" if truth => args.first().and_then(|a| operand_key(a)).into_iter().collect(),
        "not" => args.first().map(|a| presence(a, !truth)).unwrap_or_default(),
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
fn guards_for(name: &str, args: &[&OirExpr], index: usize) -> Vec<String> {
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

fn is_ordering(name: &str) -> bool {
    matches!(name, "<" | "<=" | ">" | ">=")
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
    arg.map(|a| orbital_core_base::ir::pretty_print_expr(a)).unwrap_or_default()
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
pub fn typed_slot_types<C: TypeCtx>(expr: &OirExpr, ctx: &C) -> std::collections::HashMap<String, OirType> {
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

    /// Resolves `@entity.<field>` against a fixed field list, whole `@entity` to the struct of them.
    struct Row(Vec<(&'static str, OirType)>);
    impl TypeCtx for Row {
        fn binding_type(&self, binding: &OirBinding) -> Option<OirType> {
            if !matches!(binding.root, OirBindingRoot::Entity) {
                return None;
            }
            let whole = OirType::Struct {
                fields: self.0.iter().map(|(n, t)| OirField::new((*n).to_string(), t.clone())).collect(),
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
        assert_eq!(infer_oir_expr_type(&num(1.0), &NoCtx), Some(OirType::Number));
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
        assert_eq!(infer_oir_expr_type(&call("str/concat", vec![text("a")]), &NoCtx), Some(OirType::String));
    }

    /// G-CROSS-041: `(quote x)` is S-expression data, not an opaque value.
    #[test]
    fn quote_infers_sexpr() {
        assert_eq!(infer_oir_expr_type(&call("quote", vec![text("[\">\",1,0]")]), &NoCtx), Some(OirType::SExpr));
    }

    fn quoted(value: orbital_core_base::schema::SExpression) -> OirExpr {
        call("quote", vec![text(&orbital_core_base::schema::quote::encode_quote_body(&value))])
    }

    fn quoted_text(v: &str) -> OirExpr {
        quoted(orbital_core_base::schema::SExpression::String(v.to_string()))
    }

    fn attribute_row() -> Row {
        Row(vec![("@href", OirType::String), ("#text", OirType::Number), ("rel", OirType::Boolean)])
    }

    #[test]
    fn quote_of_a_scalar_literal_is_that_scalar() {
        use orbital_core_base::schema::SExpression as S;
        assert_eq!(infer_oir_expr_type(&quoted_text("@href"), &NoCtx), Some(OirType::String));
        assert_eq!(infer_oir_expr_type(&quoted(S::Number(3.0)), &NoCtx), Some(OirType::Number));
        assert_eq!(infer_oir_expr_type(&quoted(S::Bool(true)), &NoCtx), Some(OirType::Boolean));
    }

    #[test]
    fn quote_of_anything_else_stays_sexpr() {
        use orbital_core_base::schema::SExpression as S;
        let list = quoted(S::Array(vec![S::String(">".into()), S::Number(1.0), S::Number(0.0)]));
        assert_eq!(infer_oir_expr_type(&list, &NoCtx), Some(OirType::SExpr));
        assert_eq!(infer_oir_expr_type(&quoted(S::Null), &NoCtx), Some(OirType::SExpr));
        assert_eq!(infer_oir_expr_type(&quoted(S::Object(Default::default())), &NoCtx), Some(OirType::SExpr));
    }

    #[test]
    fn object_get_with_a_quoted_key_reads_the_declared_field_type() {
        let row = attribute_row();
        let get = |key: OirExpr| call("object/get", vec![entity(&[]), key]);
        assert_eq!(infer_oir_expr_type(&get(quoted_text("@href")), &row), Some(OirType::String));
        assert_eq!(infer_oir_expr_type(&get(quoted_text("#text")), &row), Some(OirType::Number));
        // control: a bare-identifier field reads the same way through a plain key.
        assert_eq!(infer_oir_expr_type(&get(text("rel")), &row), Some(OirType::Boolean));
    }

    #[test]
    fn a_quoted_key_that_names_no_field_is_reported() {
        let row = attribute_row();
        let found = check_oir_expr(&call("object/get", vec![entity(&[]), quoted_text("@nope")]), &row);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(matches!(found[0].mismatch, Mismatch::NoField { .. }));
        assert!(check_oir_expr(&call("object/get", vec![entity(&[]), quoted_text("@href")]), &row).is_empty());
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
        assert_eq!(infer_oir_expr_type(&call("bytes/length", vec![blob.clone()]), &NoCtx), Some(OirType::Number));
        assert_eq!(infer_oir_expr_type(&call("bytes/to-string", vec![blob.clone()]), &NoCtx), Some(OirType::String));
        assert_eq!(
            infer_oir_expr_type(&call("bytes/concat", vec![list_of(vec![blob.clone(), blob.clone()])]), &NoCtx),
            Some(OirType::Bytes)
        );
        assert_eq!(infer_oir_expr_type(&call("bytes/equals", vec![blob.clone(), blob]), &NoCtx), Some(OirType::Boolean));
    }

    #[test]
    fn an_undetermined_argument_leaves_the_result_undetermined() {
        assert_eq!(infer_oir_expr_type(&call("array/first", vec![entity(&["missing"])]), &tags_row()), None);
        assert_eq!(infer_oir_expr_type(&call("totally/unknown", vec![]), &NoCtx), None);
    }

    #[test]
    fn object_get_reads_the_field_type_of_a_known_record() {
        let row = tags_row();
        assert_eq!(
            infer_oir_expr_type(&call("object/get", vec![entity(&[]), text("count")]), &row),
            Some(OirType::Number)
        );
        assert_eq!(
            infer_oir_expr_type(&call("object/get", vec![entity(&[]), text("name"), text("anon")]), &row),
            Some(OirType::String)
        );
    }

    #[test]
    fn a_lambda_is_inferred_with_its_parameter_typed_by_the_signature() {
        // (array/map @entity.tags (fn t (str/len @t))) is [number]
        let row = tags_row();
        let lambda = OirExpr::Lambda {
            params: vec!["t".into()],
            body: Box::new(call("str/len", vec![OirExpr::Binding {
                binding: OirBinding::new(OirBindingRoot::Local("t".into()), vec![]),
            }])),
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
        let found = check_oir_expr(&call("array/append", vec![entity(&["tags"]), num(1.0)]), &row);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].index, 1);
        // control: the same call with a string
        assert!(check_oir_expr(&call("array/append", vec![entity(&["tags"]), text("x")]), &row).is_empty());
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
            assert!(matches!(found[0].mismatch, Mismatch::Undetermined { .. }), "{found:?}");
        }
        // control: the same operators over a typed list are clean.
        assert!(check_oir_expr(&call("array/len", vec![entity(&["tags"])]), &row).is_empty());
        // control: two empty lists type each other no better.
        assert_eq!(check_oir_expr(&call("or", vec![empty(), empty()]), &row).len(), 1);
    }

    #[test]
    fn a_key_that_names_no_field_is_reported() {
        let row = tags_row();
        let found = check_oir_expr(&call("object/get", vec![entity(&[]), text("nope")]), &row);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(matches!(found[0].mismatch, Mismatch::NoField { .. }));
        assert!(check_oir_expr(&call("object/get", vec![entity(&[]), text("count")]), &row).is_empty());
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

    #[test]
    fn a_let_binding_is_typed_by_its_value() {
        // (let [[n 2]] (+ @n 1)) is a number; (let [[s "a"]] (+ @s 1)) is a string in arithmetic
        let local = |n: &str| OirExpr::Binding {
            binding: OirBinding::new(OirBindingRoot::Local(n.into()), vec![]),
        };
        let ok = call("let", vec![list_of(vec![list_of(vec![text("n"), num(2.0)])]), OirExpr::BinaryOp {
            op: OirBinaryOp::Add,
            left: Box::new(local("n")),
            right: Box::new(num(1.0)),
        }]);
        assert_eq!(infer_oir_expr_type(&ok, &NoCtx), Some(OirType::Number));
        assert!(check_oir_expr(&ok, &NoCtx).is_empty());
        let bad = call("let", vec![list_of(vec![list_of(vec![text("s"), text("a")])]), OirExpr::BinaryOp {
            op: OirBinaryOp::Add,
            left: Box::new(local("s")),
            right: Box::new(num(1.0)),
        }]);
        assert_eq!(check_oir_expr(&bad, &NoCtx).len(), 1);
    }

    fn lowered(value: serde_json::Value) -> OirExpr {
        let sexpr: orbital_core_base::schema::SExpression = serde_json::from_value(value).expect("s-expression");
        orbital_core_eval::lower::lower_sexpr(&sexpr).expect("lowers")
    }

    /// The maps of two reads inside a lambda merge into one map, so the lambda's result is typed.
    #[test]
    fn a_merge_of_maps_read_in_a_lambda_is_a_typed_result() {
        let words = OirType::map(OirType::String, OirType::String);
        let item = OirType::Struct { fields: vec![OirField::new("entities".to_string(), words)] };
        let typed = Row(vec![("items", OirType::array(item)), ("out", OirType::String)]);
        let body = |entities: serde_json::Value| {
            serde_json::json!(["let", [["n", 1]], ["set", "@entity.out", ["json/stringify", ["array/map", "@entity.items", ["fn", "v", entities]]]]])
        };
        let merged = body(serde_json::json!(["object/merge", ["object/get", "@v", "entities", {}], ["object/get", "@v", "entities", {}]]));
        let found = check_oir_expr(&lowered(merged), &typed);
        assert!(found.is_empty(), "{:?}", found.iter().map(|d| &d.expression).collect::<Vec<_>>());
        let unlike = body(serde_json::json!(["object/merge", ["object/get", "@v", "entities", {}], { "n": 1 }]));
        assert_eq!(check_oir_expr(&lowered(unlike), &typed).len(), 1, "control: a map and a number record have no one value type");
    }

    fn tree_row() -> Row {
        let own = || OirType::TypeRef { name: "Tree".to_string() };
        let tree = OirType::Struct {
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
        serde_json::json!(["tree/fold", "@entity.tree", ["fn", ["n", "k"], step], "Tree"])
    }

    fn reported(expr: &OirExpr, ctx: &Row) -> Vec<String> {
        check_oir_expr(expr, ctx).iter().map(|d| format!("{}: {}", d.expression, d.mismatch)).collect()
    }

    #[test]
    fn a_fold_step_reads_child_results_of_its_own_result_type() {
        let ctx = tree_row();
        let sum = lowered(fold_over_tree(serde_json::json!(["+", 1, ["array/len", "@k.kids"]])));
        assert_eq!(reported(&sum, &ctx), Vec::<String>::new(), "the provisional reading's findings are dropped");
        assert_eq!(infer_oir_expr_type(&sum, &ctx), Some(OirType::Number));
        let joined = lowered(fold_over_tree(serde_json::json!(["str/concat", "@n.label", ["str/join", "@k.kids", ","]])));
        assert_eq!(reported(&joined, &ctx), Vec::<String>::new(), "control: string children of a string step");
        assert_eq!(infer_oir_expr_type(&joined, &ctx), Some(OirType::String));
    }

    #[test]
    fn a_fold_step_that_reads_its_child_results_as_another_type_is_a_mismatch() {
        let ctx = tree_row();
        let step = serde_json::json!(["+", ["array/len", "@n.tags"], ["str/len", ["object/get", "@k", "parent", ""]]]);
        let found = reported(&lowered(fold_over_tree(step)), &ctx);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("parent") && found[0].contains("expected number, got string"),
            "the child result is the step's number, read with a string default: {found:?}"
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
                    fields: vec![OirField::new("x".to_string(), OirType::Number)],
                })
            }
        }
        let parse = call("json/parse", vec![text("{}"), text("Point")]);
        assert_eq!(
            infer_oir_expr_type(&parse, &Named),
            Some(OirType::Struct { fields: vec![OirField::new("x".to_string(), OirType::Number)] })
        );
        assert_eq!(
            infer_oir_expr_type(&parse, &NoCtx),
            Some(OirType::TypeRef { name: "Point".into() }),
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
        assert_eq!(infer_oir_expr_type(&both_strings, &tags_row()), Some(OirType::String));
        let mixed = OirExpr::BinaryOp {
            op: OirBinaryOp::Or,
            left: Box::new(entity(&["name"])),
            right: Box::new(OirExpr::Bool { value: true }),
        };
        assert_eq!(check_oir_expr(&mixed, &tags_row()).len(), 1);
    }

    // ---- Phase 4: collect_guard_type_warnings (C-GUARD-NO-BOOLEAN-CHECK) ----
    use orbital_core_base::ir::{OirInterval, OirListener, OirModule, OirTick, OirTrait, OirTransition};

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
}
