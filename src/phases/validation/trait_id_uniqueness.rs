//! Every trait of a program carries its own id: listens match their source by
//! trait id, so two traits sharing one would each hear the other's emits.

use super::error::{ErrorCode, ValidationError, ValidationResult};
use super::ValidationContext;
use orbital_core_base::schema::{OrbitalSchema, TraitReference};
use std::collections::BTreeMap;

pub fn validate_unique_trait_ids(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    ctx.merge(check(schema));
}

fn check(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut holders: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for orbital in &schema.orbitals {
        for tr in &orbital.traits {
            if let TraitReference::Inline(td) = tr {
                if let Some(id) = td.id.as_deref() {
                    holders
                        .entry(id)
                        .or_default()
                        .push(format!("{}.{}", orbital.name, td.name));
                }
            }
        }
    }
    for (id, names) in holders.into_iter().filter(|(_, n)| n.len() > 1) {
        result.add_error(ValidationError::error(
            ErrorCode::CrossDuplicateTraitId,
            "orbitals",
            format!(
                "trait id {id} is carried by {}; listens match sources by id, so each of these hears the others' emits",
                names.join(", ")
            ),
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::check;
    use crate::phases::validation::error::ErrorCode;
    use orbital_core_base::schema::OrbitalSchema;

    fn program(ids: [Option<&str>; 2]) -> OrbitalSchema {
        let trait_json = |name: &str, id: Option<&str>| {
            let mut t = serde_json::json!({ "name": name, "linkedEntity": "E", "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "INIT", "name": "INIT" }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
            } });
            if let Some(id) = id {
                t["id"] = serde_json::json!(id);
            }
            t
        };
        serde_json::from_value(serde_json::json!({ "name": "P", "orbitals": [{
            "name": "O",
            "entity": { "name": "E", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
            "traits": [trait_json("A", ids[0]), trait_json("B", ids[1])],
            "pages": []
        }] }))
        .expect("schema")
    }

    fn duplicates(schema: &OrbitalSchema) -> usize {
        check(schema)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::CrossDuplicateTraitId)
            .count()
    }

    #[test]
    fn two_traits_sharing_an_id_are_rejected() {
        let id = Some("trt_D3MHYC8PV7BDKNT20E3SN8RFSA");
        assert_eq!(duplicates(&program([id, id])), 1);
    }

    #[test]
    fn control_distinct_ids_pass() {
        let schema = program([
            Some("trt_D3MHYC8PV7BDKNT20E3SN8RFSA"),
            Some("trt_ZSKW13TYGSF8CQNE490K961PEB"),
        ]);
        assert_eq!(duplicates(&schema), 0);
    }

    #[test]
    fn control_traits_without_ids_are_not_compared() {
        assert_eq!(duplicates(&program([None, None])), 0);
    }
}
