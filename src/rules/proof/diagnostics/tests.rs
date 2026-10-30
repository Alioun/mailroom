use super::*;
use crate::{
    census::ScanState,
    engine,
    rules::proof::{self, Baseline, Destination, Pin},
};

#[test]
fn proof_notice_codes_follow_partial_census_dates_and_stale_pins_without_opening_gates() {
    let mut input: engine::PreviewInput =
        serde_json::from_str(include_str!("../../../../examples/sample.json")).unwrap();
    input.pins.clear();
    let artifact = engine::artifact(&input.documents, &input.context).unwrap();
    let mut census = engine::synthetic_census(&input).unwrap();
    let complete = proof::prove(
        artifact.clone(),
        &census,
        input.as_of,
        &[],
        Baseline::Current,
        None,
    )
    .unwrap();
    assert!(
        !complete
            .diagnostics
            .notices
            .contains(&Notice::PartialCensus)
    );
    assert!(!complete.diagnostics.notices.contains(&Notice::StalePins));
    census.acquisition_state = ScanState::Running;
    census
        .folders
        .values_mut()
        .next()
        .unwrap()
        .messages
        .values_mut()
        .next()
        .unwrap()
        .headers
        .date = None;
    let pins = [Pin {
        account: census.account.as_str().into(),
        message_id_hash: "f".repeat(64),
        expected: Destination::Inbox,
    }];
    let partial = proof::prove(
        artifact,
        &census,
        input.as_of,
        &pins,
        Baseline::Current,
        None,
    )
    .unwrap();
    for notice in [
        Notice::PartialCensus,
        Notice::InvalidDates,
        Notice::StalePins,
    ] {
        assert!(partial.diagnostics.notices.contains(&notice));
    }
    assert_eq!(partial.gate, "warnings");
    assert_eq!(partial.pins[0].status, "stale");
    assert_eq!(partial.counts.messages, complete.counts.messages);
    assert_eq!(partial.counts.filed, complete.counts.filed);
    assert_eq!(partial.counts.copies, complete.counts.copies);
    assert_eq!(
        partial.warnings,
        partial
            .diagnostics
            .notices
            .iter()
            .map(|notice| notice.message().to_owned())
            .collect::<Vec<_>>()
    );
    println!(
        "MAILROOM_PROOF_DIAGNOSTICS_FIXTURE {}",
        serde_json::json!({"complete":complete,"partial":partial})
    );
}

#[test]
fn proof_diagnostics_emit_every_supported_notice_and_keep_independent_execution_unverified() {
    let diagnostics = Diagnostics {
        notices: vec![
            Notice::DeliveryTagLimits,
            Notice::SyntheticRecipientLimits,
            Notice::AttachmentAndDuplicateLimits,
            Notice::FolderTopologyUnverified,
            Notice::PartialCensus,
            Notice::InvalidDates,
            Notice::RulesWithoutSelections,
            Notice::ShadowedRules,
            Notice::StalePins,
        ],
        runtime_cross_check: RuntimeCheck::NotRun,
    };
    let wire = serde_json::to_value(&diagnostics).unwrap();
    assert_eq!(wire["runtime_cross_check"], "not_run");
    assert_eq!(
        diagnostics.runtime_cross_check.message(),
        "Not run by this proof. Run scripts/verify-sieve.sh on synthetic artifacts. This does not establish provider behavior."
    );
    println!("MAILROOM_PROOF_DIAGNOSTIC_VARIANTS {wire}");
}
