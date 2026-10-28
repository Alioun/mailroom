use super::*;
use std::sync::{Arc, Mutex};

fn plan() -> Plan {
    Plan {
        exact_receipt: "a".repeat(64),
        actions: vec![ExactAction::InvoiceSend {
            account: "invented-account".into(),
            revision: 9,
            report_version: "report-v1".into(),
            target_name: "accounting".into(),
            target: "target@example.test".into(),
            sending_identity: "sender@example.test".into(),
            source: "INBOX/1/3".into(),
            content_digest: "b".repeat(64),
        }],
    }
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    Unverified,
    Cancelled,
    Malformed,
    Forged,
    OtherResponder,
    WrongRequest,
    Oversized,
    Failure,
}
#[derive(Default)]
struct Seen {
    prompts: usize,
    shown: Vec<Vec<u8>>,
}
struct Fake {
    fault: Fault,
    seen: Arc<Mutex<Seen>>,
}
impl Backend for Fake {
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    fn synthetic(&self) -> bool {
        false
    }
    fn exchange(&mut self, request: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
        if matches!(self.fault, Fault::Unverified) {
            return Err(crate::native::LaunchError::IdentityMismatch.to_string());
        }
        let mut seen = self.seen.lock().unwrap();
        seen.prompts += 1;
        seen.shown.push(request.to_vec());
        Ok(match self.fault {
            Fault::None | Fault::Unverified => answer(request, key, Outcome::Approved)?,
            Fault::Cancelled => answer(request, key, Outcome::Cancelled)?,
            Fault::Malformed => b"{\"approved\":true}".to_vec(),
            Fault::Oversized => vec![b' '; 4097],
            Fault::Failure => return Err("Helper refused or failed".into()),
            Fault::Forged => {
                let mut wire: WireOutcome =
                    serde_json::from_slice(&answer(request, key, Outcome::Approved)?).unwrap();
                wire.authentication = "0".repeat(64);
                serde_json::to_vec(&wire).unwrap()
            }
            Fault::OtherResponder => answer(request, &random(), Outcome::Approved)?,
            Fault::WrongRequest => {
                let mut altered: serde_json::Value = serde_json::from_slice(request).unwrap();
                altered["request_id"] = "0".repeat(64).into();
                answer(
                    &serde_json::to_vec(&altered).unwrap(),
                    key,
                    Outcome::Approved,
                )?
            }
        })
    }
}
fn boundary(fault: Fault) -> (Boundary, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    (
        Boundary::with_backend(Box::new(Fake {
            fault,
            seen: seen.clone(),
        })),
        seen,
    )
}

#[test]
fn exact_display_and_single_use_consumption() {
    let (mut boundary, seen) = boundary(Fault::None);
    let plan = plan();
    let (handle, request) = boundary.offer(plan.clone()).unwrap();
    assert!(!request.synthetic);
    let consent = boundary
        .confirm(&handle, &plan, || Ok(plan.clone()))
        .unwrap();
    assert_eq!(consent.receipt(), plan.exact_receipt);
    let shown = seen.lock().unwrap().shown[0].clone();
    assert_eq!(shown, serde_json::to_vec(&request).unwrap());
    for exact in [
        "invented-account",
        "target@example.test",
        "sender@example.test",
        &"b".repeat(64),
    ] {
        assert!(request.display.contains(exact));
    }
    assert!(
        boundary
            .confirm(&handle, &plan, || Ok(plan.clone()))
            .is_err()
    );
    assert_eq!(seen.lock().unwrap().prompts, 1);
}

#[test]
fn unverified_cancelled_malformed_forged_unauthenticated_and_failed_helpers_deny_and_consume() {
    for fault in [
        Fault::Unverified,
        Fault::Cancelled,
        Fault::Malformed,
        Fault::Forged,
        Fault::OtherResponder,
        Fault::WrongRequest,
        Fault::Oversized,
        Fault::Failure,
    ] {
        let (mut boundary, seen) = boundary(fault);
        let plan = plan();
        let (handle, _) = boundary.offer(plan.clone()).unwrap();
        let mut revalidated = false;
        let denied = boundary.confirm(&handle, &plan, || {
            revalidated = true;
            Ok(plan.clone())
        });
        assert!(denied.is_err(), "{fault:?}");
        assert!(!revalidated, "{fault:?}");
        if matches!(fault, Fault::Unverified) {
            assert_eq!(seen.lock().unwrap().prompts, 0);
        }
        let replay = boundary.confirm(&handle, &plan, || Ok(plan.clone()));
        assert_eq!(
            replay.err().as_deref(),
            Some("Unknown or consumed confirmation request")
        );
    }
}

#[test]
fn every_action_binding_is_revalidated_before_and_after_confirmation() {
    let plan = plan();
    let mut alterations = Vec::new();
    for field in 0..9 {
        let mut changed = plan.clone();
        let ExactAction::InvoiceSend {
            account,
            revision,
            report_version,
            target_name,
            target,
            sending_identity,
            source,
            content_digest,
        } = &mut changed.actions[0]
        else {
            unreachable!()
        };
        match field {
            0 => account.push('x'),
            1 => *revision += 1,
            2 => report_version.push('x'),
            3 => target_name.push('x'),
            4 => target.push('x'),
            5 => sending_identity.push('x'),
            6 => source.push('x'),
            7 => content_digest.push('x'),
            _ => changed.exact_receipt = "c".repeat(64),
        }
        alterations.push(changed);
    }
    alterations.push(Plan {
        exact_receipt: plan.exact_receipt.clone(),
        actions: vec![],
    });
    for changed in alterations {
        for after in [false, true] {
            let (mut boundary, seen) = boundary(Fault::None);
            let (handle, _) = boundary.offer(plan.clone()).unwrap();
            let current = if after { &plan } else { &changed };
            assert!(
                boundary
                    .confirm(&handle, current, || Ok(changed.clone()))
                    .is_err()
            );
            assert_eq!(seen.lock().unwrap().prompts, usize::from(after));
        }
    }
}

#[test]
fn expiry_and_other_boundaries_deny_without_prompt() {
    let plan = plan();
    let (mut boundary, seen) = boundary(Fault::None);
    let (handle, _) = boundary.offer(plan.clone()).unwrap();
    boundary.pending.get_mut(&handle).unwrap().created = Instant::now() - LIFETIME;
    assert!(
        boundary
            .confirm(&handle, &plan, || Ok(plan.clone()))
            .is_err()
    );
    let (handle, _) = boundary.offer(plan.clone()).unwrap();
    assert!(
        Boundary::synthetic()
            .confirm(&handle, &plan, || Ok(plan.clone()))
            .is_err()
    );
    assert_eq!(seen.lock().unwrap().prompts, 0);
}

#[test]
fn outcome_rejects_unknown_fields_oversize_and_previous_challenge() {
    let (mut boundary, _) = boundary(Fault::None);
    let (_, request) = boundary.offer(plan()).unwrap();
    let bytes = serde_json::to_vec(&request).unwrap();
    let key = random();
    let answer = answer(&bytes, &key, Outcome::Approved).unwrap();
    assert!(authenticate(&request.request_id, &bytes, &key, &answer).is_ok());
    assert!(authenticate(&request.request_id, &bytes, &random(), &answer).is_err());
    assert!(authenticate(&request.request_id, &bytes, &key, &vec![b' '; 4097]).is_err());
    let mut wire: serde_json::Value = serde_json::from_slice(&answer).unwrap();
    wire["extra"] = true.into();
    assert!(
        authenticate(
            &request.request_id,
            &bytes,
            &key,
            &serde_json::to_vec(&wire).unwrap()
        )
        .is_err()
    );
}

/// The Swift helper rederives this text and these tags from the same fixture.
#[test]
fn display_and_authentication_match_the_swift_helper_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../native/macos/Tests/MailroomNativeTests/exact-actions.json"
    ))
    .unwrap();
    let actions = [
        ExactAction::Recovery {
            account: "invented-account".into(),
            operation: "Restore captured state".into(),
            manifest_digest: "c".repeat(64),
            consequences: "Local state only. No provider effect.".into(),
        },
        plan().actions.remove(0),
        ExactAction::UnsubscribeLocalMail {
            account: "invented-account".into(),
            revision: 3,
            workspace_revision: None,
            subscription: "sub-1".into(),
            target: "unsubscribe@list.example.test".into(),
            sending_identity: "sender@example.test".into(),
            receiving_identity: "alias@example.test".into(),
            subject: "unsubscribe".into(),
            content_digest: "b".repeat(64),
            source_digest: "e".repeat(64),
        },
        ExactAction::SubscriptionAction {
            account: "invented-account".into(),
            revision: 3,
            workspace_revision: Some(7),
            subscription: "sub-2".into(),
            method: crate::unsubscribe::Method::OneClick,
            receiving_identity: "alias@example.test".into(),
            host: None,
            content_digest: "b".repeat(64),
            source_digest: "e".repeat(64),
            effect: "Success unknown.".into(),
            destination: None,
        },
        ExactAction::MailboxChange {
            account: "invented-account".into(),
            operation: "Create the previewed folders".into(),
            plan_receipt: "d".repeat(64),
        },
        ExactAction::SmtpSend {
            account: "invented-account".into(),
            operation: "Send this invoice".into(),
            target: "target@example.test".into(),
            sending_identity: "sender@example.test".into(),
            content_digest: "b".repeat(64),
        },
        ExactAction::OneClickPost {
            account: "invented-account".into(),
            operation: "Unsubscribe with one click".into(),
            target: "list.example.test".into(),
            sending_identity: "alias@example.test".into(),
            content_digest: "b".repeat(64),
        },
        ExactAction::MailAppHandoff {
            account: "invented-account".into(),
            operation: "Open the unsubscribe message for subscription sub-3".into(),
            target: "leave@list.example.test".into(),
            sending_identity: "alias@example.test".into(),
            content_digest: "b".repeat(64),
        },
    ];
    let cases = fixture["actions"].as_array().unwrap();
    assert_eq!(cases.len(), actions.len());
    for (action, case) in actions.iter().zip(cases) {
        assert_eq!(serde_json::to_value(action).unwrap(), case["action"]);
        assert_eq!(
            display(std::slice::from_ref(action)),
            case["display"].as_str().unwrap()
        );
    }
    let outcome = &fixture["outcome"];
    let key: [u8; 32] = data_encoding::HEXLOWER
        .decode(outcome["challenge_hex"].as_str().unwrap().as_bytes())
        .unwrap()
        .try_into()
        .unwrap();
    let request = outcome["request"].as_str().unwrap().as_bytes();
    assert_eq!(digest(request), outcome["request_digest"].as_str().unwrap());
    assert_eq!(
        tag(&key, &"1".repeat(64), &digest(request), Outcome::Approved),
        outcome["approved_tag"].as_str().unwrap()
    );
}

#[test]
fn production_boundary_issues_nothing_without_a_pinned_helper() {
    let mut native = Boundary::native();
    assert_eq!(
        native.offer(plan()).err().as_deref(),
        Some("No helper identity was pinned into this build")
    );
    assert_eq!(
        native_status(),
        "No helper identity was pinned into this build"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn native_backend_denies_substituted_and_non_answering_helpers() {
    let fixture = crate::native::tests::Fixture::new();
    let echo = fixture.pinned("echo");
    let mut wrong = echo.clone();
    wrong.pin = crate::native::Pin::AdHocCdHash([7; 20]);
    for (helper, expected) in [
        (
            wrong,
            "Started helper does not have the pinned code identity",
        ),
        (echo, "Malformed confirmation outcome"),
    ] {
        let mut boundary = Boundary::with_backend(Box::new(NativeBackend { helper: Ok(helper) }));
        let plan = plan();
        let (handle, request) = boundary.offer(plan.clone()).unwrap();
        assert!(!request.synthetic);
        let denied = boundary.confirm(&handle, &plan, || panic!("no revalidation after denial"));
        assert_eq!(denied.err().as_deref(), Some(expected));
    }
}
