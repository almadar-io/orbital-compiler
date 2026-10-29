//! Regression: when TWO orbitals in one app each compose the SAME source atom,
//! each auto-pulls the atom's `@trait.<Sibling>` under the sibling's original
//! name. Identical names in two orbitals collide app-wide
//! (`ORB_T_DUPLICATE_NAME`). The inline phase uniquifies the colliding pulled
//! siblings — prefixing with the owning orbital name — and rewrites both the
//! pulling trait's `@trait.<old>` render literal AND its `listens` source so the
//! sibling stays referenced. Single-use pulls keep their clean source name.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{ListenSource, TraitReference};

const ATOM: &str = r#"{
  "name": "test-checkin-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "CheckinOrbital",
      "entity": {
        "name": "Checkin",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "CheckinScan",
          "linkedEntity": "Checkin",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "VERIFY" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "VERIFY",
                "effects": [
                  ["render-ui", "main", {
                    "type": "stack",
                    "children": [ "@trait.CheckinStepper" ]
                  }]
                ]
              }
            ]
          },
          "listens": [
            { "event": "ADVANCE", "triggers": "VERIFY", "source": { "kind": "trait", "trait": "CheckinStepper" } }
          ]
        },
        {
          "name": "CheckinStepper",
          "linkedEntity": "Checkin",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "ADVANCE" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "ADVANCE",
                "effects": [
                  ["render-ui", "main", { "type": "stepper" }]
                ]
              }
            ]
          },
          "emits": [ { "event": "ADVANCE" } ]
        }
      ],
      "pages": [
        { "name": "CheckinPage", "path": "/checkin", "traits": [ { "ref": "CheckinScan" }, { "ref": "CheckinStepper" } ] }
      ]
    }
  ]
}"#;

const CONSUMER: &str = r#"{
  "name": "test-checkin-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TechnicianOrbital",
      "uses": [ { "from": "./atom.orb", "as": "Checkin" } ],
      "entity": {
        "name": "Technician",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Checkin.traits.CheckinScan", "linkedEntity": "Technician", "name": "TechnicianFieldCheckin" }
      ],
      "pages": [
        { "name": "TechPage", "path": "/tech", "traits": [ { "ref": "TechnicianFieldCheckin" } ] }
      ]
    },
    {
      "name": "JobCheckinOrbital",
      "uses": [ { "from": "./atom.orb", "as": "Checkin" } ],
      "entity": {
        "name": "Job",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Checkin.traits.CheckinScan", "linkedEntity": "Job", "name": "JobArrivalCheckin" }
      ],
      "pages": [
        { "name": "JobPage", "path": "/job", "traits": [ { "ref": "JobArrivalCheckin" } ] }
      ]
    }
  ]
}"#;

fn orbital_trait_names(schema: &orbital_core_base::schema::OrbitalSchema, idx: usize) -> Vec<String> {
    schema.orbitals[idx]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.clone()),
            other => other.trait_name().map(|s| s.to_string()),
        })
        .collect()
}

#[test]
fn cross_orbital_pulled_siblings_are_uniquified() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");

    let tech_names = orbital_trait_names(&schema, 0);
    let job_names = orbital_trait_names(&schema, 1);

    // Each orbital pulled its own stepper sibling, uniquified by orbital name.
    assert!(
        tech_names
            .iter()
            .any(|n| n == "TechnicianOrbitalCheckinStepper"),
        "TechnicianOrbital should carry a uniquified stepper; got {tech_names:?}"
    );
    assert!(
        job_names
            .iter()
            .any(|n| n == "JobCheckinOrbitalCheckinStepper"),
        "JobCheckinOrbital should carry a uniquified stepper; got {job_names:?}"
    );

    // The bare colliding name must NOT survive in either orbital.
    assert!(
        !tech_names.iter().any(|n| n == "CheckinStepper"),
        "bare CheckinStepper must be renamed in TechnicianOrbital; got {tech_names:?}"
    );
    assert!(
        !job_names.iter().any(|n| n == "CheckinStepper"),
        "bare CheckinStepper must be renamed in JobCheckinOrbital; got {job_names:?}"
    );

    // App-wide trait names are now unique (no ORB_T_DUPLICATE_NAME).
    let mut all: Vec<String> = tech_names.iter().chain(job_names.iter()).cloned().collect();
    all.sort();
    let dedup_len = {
        let mut s = all.clone();
        s.dedup();
        s.len()
    };
    assert_eq!(
        all.len(),
        dedup_len,
        "trait names must be app-unique; got {all:?}"
    );

    // The pulling trait's `listens` source tracked the rename in each orbital.
    for (idx, expected_src) in [
        (0usize, "TechnicianOrbitalCheckinStepper"),
        (1usize, "JobCheckinOrbitalCheckinStepper"),
    ] {
        let parent = schema.orbitals[idx]
            .traits
            .iter()
            .find_map(|t| match t {
                TraitReference::Inline(td)
                    if td.name == "TechnicianFieldCheckin" || td.name == "JobArrivalCheckin" =>
                {
                    Some(td)
                }
                _ => None,
            })
            .expect("pulling trait present");
        let listens = parent.listens.as_ref().expect("parent has listens");
        let src = listens
            .iter()
            .find_map(|l| match l.source.as_ref() {
                Some(ListenSource::Trait { trait_name, .. }) => Some(trait_name.clone()),
                _ => None,
            })
            .expect("listen has a trait source");
        assert_eq!(
            src, expected_src,
            "orbital {idx} listen source should track the sibling rename"
        );
    }
}

/// An app that re-imports the consumer's two orbitals under their own names.
const REIMPORT: &str = r#"{
  "name": "test-checkin-reimport",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TechnicianOrbital",
      "uses": [ { "from": "./consumer.orb", "as": "App" } ],
      "entity": "App.orbitals.TechnicianOrbital.entity",
      "traits": [],
      "reference": { "ref": "App.orbitals.TechnicianOrbital" }
    },
    {
      "name": "JobCheckinOrbital",
      "uses": [ { "from": "./consumer.orb", "as": "App" } ],
      "entity": "App.orbitals.JobCheckinOrbital.entity",
      "traits": [],
      "reference": { "ref": "App.orbitals.JobCheckinOrbital" }
    }
  ]
}"#;

#[test]
fn a_same_name_reimport_keeps_the_pulled_siblings_unique() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    std::fs::write(dir.path().join("consumer.orb"), CONSUMER).expect("write consumer");
    let app_path = dir.path().join("app.orb");
    std::fs::write(&app_path, REIMPORT).expect("write app");

    let mut schema = parse_file(&app_path).expect("app parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&app_path)).expect("inline succeeds");

    let tech = orbital_trait_names(&schema, 0);
    let job = orbital_trait_names(&schema, 1);
    let shared: Vec<&String> = tech.iter().filter(|n| job.contains(n)).collect();
    assert!(shared.is_empty(), "every trait name is app-unique, as in the imported program: {tech:?} / {job:?}");
}
