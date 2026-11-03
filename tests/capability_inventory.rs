use fah::{domain::models::capabilities::*, presentation::cli::commands::ShellCommand};
use std::collections::BTreeSet;
#[test]
fn every_deserializable_cli_operation_has_canonical_capability_coverage() {
    // Serde derives the accepted tag list directly from the enum. This catches
    // added commands omitted from the inventory without maintaining a second list.
    let error = serde_json::from_str::<ShellCommand>(r#"{"op":"__coverage_probe__"}"#)
        .unwrap_err()
        .to_string();
    let names: BTreeSet<_> = error
        .split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, part)| part)
        .filter(|name| *name != "__coverage_probe__")
        .collect();
    assert!(
        names.len() > 25,
        "expected Serde's complete variant inventory: {error}"
    );
    let advertised: BTreeSet<_> = command_names().into_iter().collect();
    assert_eq!(names, advertised);
}
#[test]
fn evidence_never_promotes_unresolved_or_unsupported_work_to_guest_or_physical() {
    let report = report();
    assert_eq!(report.schema_version, 1);
    assert!(!report.physical_validated);
    let runs: BTreeSet<_> = report.runs.iter().map(|run| run.suite).collect();
    let mut operations = BTreeSet::new();
    for row in report.capabilities {
        assert!(operations.insert(row.operation));
        assert!(!row.effect.is_empty() && !row.precondition.is_empty());
        assert!(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(row.implementation)
                .is_file()
        );
        assert!(!row.evidence.contains(&"physical_tested"));
        assert!(row.runs.iter().all(|run| runs.contains(run)));
        if row.evidence.contains(&"emulated_tested") {
            assert_eq!(row.availability, Availability::Implemented);
            assert!(!row.runs.is_empty());
        }
        if row.availability != Availability::Implemented {
            assert!(!row.evidence.contains(&"emulated_tested"));
        }
    }
    assert!(
        registry()
            .iter()
            .any(|row| row.operation == "ipad_a5_case_study"
                && row.availability == Availability::Planned)
    );
}
