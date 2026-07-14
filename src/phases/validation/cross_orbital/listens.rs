use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{ListenSource, OrbitalSchema, TraitReference};
use std::collections::{HashMap, HashSet};

/// Listens-integrity backstop (error).
///
/// Defense-in-depth duplicate of the L1 `LowerError::ListenSourceUnresolved`
/// check, run on the resolved IR so `use`-imported behaviors that inline
/// listens after lowering are also covered. Reads the typed
/// `ListenSource` enum produced by `split_listen_source`; never reparses
/// the original event string.
pub(crate) fn validate_listens_integrity(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    let mut trait_emits: HashMap<(String, String), HashSet<String>> = HashMap::new();
    let mut orbitals_in_schema: HashSet<String> = HashSet::new();
    for orbital in &schema.orbitals {
        orbitals_in_schema.insert(orbital.name.clone());
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let entry = trait_emits
                .entry((orbital.name.clone(), td.name.clone()))
                .or_default();
            if let Some(emits) = &td.emits {
                for emit in emits {
                    entry.insert(emit.event.clone());
                }
            }
        }
    }

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };
            for listen in listens {
                let (source_orbital, source_trait) = match &listen.source {
                    Some(ListenSource::Trait { trait_name, .. }) => {
                        (orbital.name.clone(), trait_name.clone())
                    }
                    Some(ListenSource::Orbital {
                        orbital: o,
                        trait_name,
                        ..
                    }) => (o.clone(), trait_name.clone()),
                    Some(ListenSource::Any) | None => continue,
                };
                // Single-orbital validation context: when the listen's source
                // orbital isn't part of this schema, we can't resolve it. Skip
                // — the composed-schema validation pass catches truly unresolved
                // cross-orbital references once all sibling orbitals are loaded.
                if !orbitals_in_schema.contains(&source_orbital) {
                    continue;
                }
                let resolved = trait_emits
                    .get(&(source_orbital.clone(), source_trait.clone()))
                    .map(|events| events.contains(&listen.event))
                    .unwrap_or(false);
                if !resolved {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::CrossOrbitalListenSourceUnresolved,
                            format!("orbitals.{}.traits.{}.listens", orbital.name, td.name),
                            format!(
                                "Listen entry in '{}.{}' references source '{}.{}' event '{}' \
                                 which is not declared on the source trait.",
                                orbital.name, td.name, source_orbital, source_trait, listen.event
                            ),
                        )
                        .with_suggestion(
                            "Either declare the event in the source trait's `emits` block, or \
                             remove the listens entry. Cross-orbital channels require the source \
                             trait to declare the emit explicitly.",
                        ),
                    );
                }
            }
        }
    }

    result
}
