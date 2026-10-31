//! R13 (G-ORB-468): a composed behavior's local types are `<Behavior>::<Type>` in the composing program.
//!
//! The behavior that declares a type is the one scope its name is written in, so crossing into a composing orbital
//! renames every type-reference position of what crosses (the pulled alias table, payload schemas, config, entity
//! fields, and the shape argument of an operator that names a type) in one pass.

use std::collections::{BTreeMap, BTreeSet};

use orbital_core_base::schema::{
    ConfigField, EntityDefinition, EntityRef, FieldDefinition, FieldProjection, Orbital, PayloadField,
    SExpression, TraitDefinition, TraitReference,
};
use orbital_core_base::type_compat::{
    qualified_type_name, rename_type_names, split_qualified_type_name,
};

use crate::type_infer::signature_of;

/// The types one behavior declares itself (a name another behavior it composes brought in is already qualified).
pub(crate) struct LocalTypes {
    behavior: String,
    names: BTreeSet<String>,
}

impl LocalTypes {
    pub(crate) fn declared_by(behavior: &str, types: &BTreeMap<String, Vec<PayloadField>>) -> Self {
        Self {
            behavior: behavior.to_string(),
            names: types
                .keys()
                .filter(|name| split_qualified_type_name(name).0.is_none())
                .cloned()
                .collect(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub(crate) fn qualify(&self, name: &str) -> Option<String> {
        self.names
            .contains(name)
            .then(|| qualified_type_name(&self.behavior, name))
    }

    pub(crate) fn rename_text(&self, text: &str) -> String {
        rename_type_names(text, &|name| self.qualify(name))
    }

    fn rename_declared(&self, text: &str) -> String {
        use orbital_core_base::ir::types::DeclaredShape;
        use orbital_core_base::ir::OirType;
        let Some(mut shape) = DeclaredShape::parse(text) else {
            return self.rename_text(text);
        };
        let replace = |ty: &OirType| match ty {
            OirType::TypeRef { name } => self.qualify(name).map(|name| OirType::TypeRef { name }),
            OirType::Reference { entity } => self.qualify(entity).map(|entity| OirType::Reference { entity }),
            _ => None,
        };
        shape.ty.rewrite(&replace);
        shape.name = shape.name.map(|name| self.qualify(&name).unwrap_or(name));
        shape.named = std::mem::take(&mut shape.named)
            .into_iter()
            .map(|(name, mut ty)| {
                ty.rewrite(&replace);
                (self.qualify(&name).unwrap_or(name), ty)
            })
            .collect();
        serde_json::to_string(&shape).expect("declared shape serializes")
    }

    fn rename_in_place(&self, text: &mut String) {
        let renamed = self.rename_text(text);
        if renamed != *text {
            *text = renamed;
        }
    }

    pub(crate) fn types(
        &self,
        types: &BTreeMap<String, Vec<PayloadField>>,
    ) -> BTreeMap<String, Vec<PayloadField>> {
        types
            .iter()
            .map(|(name, members)| {
                let mut members = members.clone();
                members
                    .iter_mut()
                    .for_each(|member| self.payload_field(member));
                (self.qualify(name).unwrap_or_else(|| name.clone()), members)
            })
            .collect()
    }

    pub(crate) fn payload_field(&self, field: &mut PayloadField) {
        self.rename_in_place(&mut field.field_type);
        self.projection(field.projected_from.as_mut());
        field
            .properties
            .iter_mut()
            .flatten()
            .for_each(|member| self.payload_field(member));
        if let Some(items) = field.items.as_mut() {
            self.payload_field(items);
        }
        if let Some(additional) = field.additional_properties.as_mut() {
            self.payload_field(additional);
        }
    }

    fn projection(&self, projection: Option<&mut FieldProjection>) {
        if let Some(projection) = projection {
            self.rename_in_place(&mut projection.type_name);
        }
    }

    fn record(&self, record: &mut Option<String>) {
        if let Some(text) = record.as_mut() {
            self.rename_in_place(text);
        }
    }

    pub(crate) fn field_definition(&self, field: &mut FieldDefinition) {
        if let Some(name) = field.by_name_reference().and_then(|name| self.qualify(name)) {
            field.values = Some(vec![name]);
        }
        self.record(&mut field.record);
        self.projection(field.projected_from.as_mut());
        if let Some(items) = field.items.as_mut() {
            self.field_definition(items);
        }
        if let Some(additional) = field.additional_properties.as_mut() {
            self.field_definition(additional);
        }
        field
            .properties
            .iter_mut()
            .flatten()
            .for_each(|(_, member)| self.field_definition(member));
    }

    fn config_field(&self, field: &mut ConfigField) {
        self.rename_in_place(&mut field.field_type);
        self.record(&mut field.record);
        if let Some(items) = field.items.as_mut() {
            self.field_definition(items);
        }
        if let Some(additional) = field.additional_properties.as_mut() {
            self.field_definition(additional);
        }
        field
            .properties
            .iter_mut()
            .flatten()
            .for_each(|(_, member)| self.field_definition(member));
    }

    pub(crate) fn entity(&self, entity: &mut EntityDefinition) {
        entity
            .fields
            .iter_mut()
            .for_each(|field| self.field_definition(field));
    }

    /// Every type-reference position of a trait cloned out of the behavior that declares the types.
    pub(crate) fn trait_definition(&self, definition: &mut TraitDefinition) {
        if let Some(source) = definition.source_entity_definition.as_mut() {
            self.entity(std::sync::Arc::make_mut(source));
        }
        for emit in definition.emits.iter_mut().flatten() {
            emit.payload_schema
                .iter_mut()
                .flatten()
                .for_each(|field| self.payload_field(field));
        }
        definition.state_machine.edit(|machine| {
            for event in machine.events.iter_mut().flatten() {
                event
                    .payload_schema
                    .iter_mut()
                    .flatten()
                    .for_each(|field| self.payload_field(field));
            }
        });
        definition
            .config
            .iter_mut()
            .flatten()
            .for_each(|(_, field)| self.config_field(field));
        definition
            .type_params
            .iter_mut()
            .flatten()
            .for_each(|param| {
                if let Some(default) = param.default.as_mut() {
                    self.rename_in_place(default);
                }
            });
        super::rewrite::rewrite_trait_expressions(definition, &mut |expr| {
            self.shape_arguments(expr)
        });
    }

    fn shape_arguments(&self, expr: &mut SExpression) {
        use orbital_core_base::schema::RewriteVisit;
        let output: Result<_, std::convert::Infallible> = expr.try_rewrite(
            |node| {
                Ok(if matches!(node.operator(), Some("quote" | "quasiquote")) {
                    RewriteVisit::Keep
                } else {
                    RewriteVisit::Descend
                })
            },
            |mut node| {
                if let SExpression::Array(items) = &mut node {
                    if items.first().and_then(SExpression::as_str) == Some("let") {
                        if let Some(SExpression::Array(bindings)) = items.get_mut(1) {
                            for binding in bindings {
                                if let SExpression::Array(parts) = binding {
                                    if let [_, _, SExpression::String(declared)] = parts.as_mut_slice() {
                                        let renamed = self.rename_declared(declared);
                                        if renamed != **declared {
                                            *declared = renamed.into();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let at = items
                        .first()
                        .and_then(SExpression::as_str)
                        .and_then(signature_of)
                        .and_then(|signature| signature.shape_position());
                    if let Some(SExpression::String(shape)) =
                        at.and_then(|at| items.get_mut(at + 1))
                    {
                        let renamed = self.rename_declared(shape);
                        if renamed != **shape {
                            *shape = renamed.into();
                        }
                    }
                }
                Ok(node)
            },
        );
        match output {
            Ok(output) => *expr = output,
            Err(never) => match never {},
        }
    }
}

impl LocalTypes {
    /// An upstream orbital pulled whole by `uses X.orbitals.Y`: its types table, entities and traits.
    pub(crate) fn orbital(&self, orbital: &mut Orbital) {
        if let Some(types) = orbital.types.as_mut() {
            *types = self.types(types);
        }
        if let EntityRef::Inline(entity) = &mut orbital.entity {
            self.entity(entity);
        }
        for entity in orbital.auxiliary_entities.iter_mut().flatten() {
            if let EntityRef::Inline(entity) = entity {
                self.entity(entity);
            }
        }
        for reference in orbital.traits.iter_mut() {
            if let TraitReference::Inline(definition) = reference {
                self.trait_definition(std::sync::Arc::make_mut(definition));
            }
        }
    }
}
