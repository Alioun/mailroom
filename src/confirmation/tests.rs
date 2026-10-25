use super::*;

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

#[derive(Clone, Copy)]
enum Fault {
    None,
    WrongIdentity,
    WrongOwner,
    WrongPath,
    Missing,
    Replaced,
    Cancelled,
    Malformed,
    Forged,
    WrongRequest,
    Failure,
}
struct Fake {
    fault: Fault,
    checks: usize,
    prompts: usize,
    displayed: Option<Request>,
}
impl Fake {
    fn new(fault: Fault) -> Self {
        Self {
            fault,
            checks: 0,
            prompts: 0,
            displayed: None,
        }
    }
}
impl Backend for Fake {
    fn verify_expected_identity(&mut self) -> Result<VerifiedExecutable, String> {
        self.checks += 1;
        if matches!(self.fault, Fault::Missing) {
            return Err("Wrong or missing helper".into());
        }
        let mut verified = SyntheticBackend.verify_expected_identity()?;
        match self.fault {
            Fault::WrongIdentity => verified.signing_requirement = "any-valid-signature".into(),
            Fault::WrongOwner => verified.owner = 1000,
            Fault::WrongPath => verified.path = "/tmp/substituted-helper".into(),
            _ => {}
        }
        Ok(verified)
    }
    fn confirm(
        &mut self,
        executable: &VerifiedExecutable,
        request: &Request,
        key: &[u8; 32],
    ) -> Result<Response, String> {
        self.prompts += 1;
        self.displayed = Some(request.clone());
        if matches!(self.fault, Fault::Failure) {
            return Err("Helper failed".into());
        }
        let mut response = SyntheticBackend.confirm(executable, request, key)?;
        match self.fault {
            Fault::Replaced => response.executable.file_identity.push_str("-replaced"),
            Fault::Malformed => response.bytes = b"{\"approved\":true}".to_vec(),
            Fault::Forged => {
                let mut wire: WireOutcome = serde_json::from_slice(&response.bytes).unwrap();
                wire.authentication = "0".repeat(64);
                response.bytes = serde_json::to_vec(&wire).unwrap();
            }
            Fault::WrongRequest => {
                let mut altered = request.clone();
                altered.request_id = "0".repeat(64);
                response = SyntheticBackend.confirm(executable, &altered, key)?;
            }
            Fault::Cancelled => {
                let request_digest = digest(&serde_json::to_vec(request).unwrap());
                let outcome = Outcome::Cancelled;
                response.bytes = serde_json::to_vec(&WireOutcome {
                    request_id: request.request_id.clone(),
                    authentication: tag(key, &request.request_id, &request_digest, outcome),
                    request_digest,
                    outcome,
                })
                .unwrap();
            }
            _ => {}
        }
        Ok(response)
    }
}

#[test]
fn exact_display_and_single_use_consumption() {
    let mut boundary = Boundary::synthetic();
    let plan = plan();
    let (handle, request) = boundary.offer(plan.clone()).unwrap();
    let mut fake = Fake::new(Fault::None);
    let consent = boundary
        .confirm(&handle, &plan, &mut fake, || Ok(plan.clone()))
        .unwrap();
    assert_eq!(consent.receipt(), plan.exact_receipt);
    let shown = fake.displayed.unwrap();
    assert_eq!(
        serde_json::to_value(&shown).unwrap(),
        serde_json::to_value(&request).unwrap()
    );
    for exact in [
        "invented-account",
        "target@example.test",
        "sender@example.test",
        &"b".repeat(64),
    ] {
        assert!(shown.display.contains(exact));
    }
    assert!(
        boundary
            .confirm(&handle, &plan, &mut SyntheticBackend, || Ok(plan.clone()))
            .is_err()
    );
    assert_eq!(fake.checks, 1);
    assert_eq!(fake.prompts, 1);
}

#[test]
fn missing_wrong_replaced_cancelled_malformed_forged_and_failed_helpers_consume_request() {
    for fault in [
        Fault::WrongIdentity,
        Fault::WrongOwner,
        Fault::WrongPath,
        Fault::Missing,
        Fault::Replaced,
        Fault::Cancelled,
        Fault::Malformed,
        Fault::Forged,
        Fault::WrongRequest,
        Fault::Failure,
    ] {
        let mut boundary = Boundary::synthetic();
        let plan = plan();
        let (handle, _) = boundary.offer(plan.clone()).unwrap();
        let mut fake = Fake::new(fault);
        let mut revalidated = false;
        assert!(
            boundary
                .confirm(&handle, &plan, &mut fake, || {
                    revalidated = true;
                    Ok(plan.clone())
                })
                .is_err()
        );
        assert!(!revalidated);
        if matches!(
            fault,
            Fault::WrongIdentity | Fault::WrongOwner | Fault::WrongPath | Fault::Missing
        ) {
            assert_eq!(fake.prompts, 0);
        }
        assert!(
            boundary
                .confirm(&handle, &plan, &mut SyntheticBackend, || Ok(plan.clone()))
                .is_err()
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
            let mut boundary = Boundary::synthetic();
            let (handle, _) = boundary.offer(plan.clone()).unwrap();
            let mut fake = Fake::new(Fault::None);
            let current = if after { &plan } else { &changed };
            assert!(
                boundary
                    .confirm(&handle, current, &mut fake, || Ok(changed.clone()))
                    .is_err()
            );
            assert_eq!(fake.prompts, usize::from(after));
        }
    }
}

#[test]
fn expiry_restart_and_native_gap_deny_without_effect() {
    let plan = plan();
    let mut boundary = Boundary::synthetic();
    let (handle, _) = boundary.offer(plan.clone()).unwrap();
    boundary.pending.get_mut(&handle).unwrap().created = Instant::now() - LIFETIME;
    let mut fake = Fake::new(Fault::None);
    assert!(
        boundary
            .confirm(&handle, &plan, &mut fake, || Ok(plan.clone()))
            .is_err()
    );
    assert_eq!(fake.checks, 0);
    let (handle, _) = boundary.offer(plan.clone()).unwrap();
    assert!(
        Boundary::synthetic()
            .confirm(&handle, &plan, &mut fake, || Ok(plan.clone()))
            .is_err()
    );
    let result = boundary.confirm(&handle, &plan, &mut MacOsBackend, || {
        panic!("Native denial must precede access")
    });
    assert!(matches!(result, Err(e) if e == NATIVE_GAP));
}

#[test]
fn outcome_rejects_unknown_fields_oversize_and_previous_challenge() {
    let mut boundary = Boundary::synthetic();
    let (_, request) = boundary.offer(plan()).unwrap();
    let key = random();
    let executable = SyntheticBackend.verify_expected_identity().unwrap();
    let response = SyntheticBackend
        .confirm(&executable, &request, &key)
        .unwrap();
    assert!(authenticate(&request, &random(), &response.bytes).is_err());
    assert!(authenticate(&request, &key, &vec![b' '; 4097]).is_err());
    let mut wire: serde_json::Value = serde_json::from_slice(&response.bytes).unwrap();
    wire["extra"] = true.into();
    assert!(authenticate(&request, &key, &serde_json::to_vec(&wire).unwrap()).is_err());
}

#[test]
fn production_boundary_has_no_trusted_helper_or_request_issuance() {
    assert!(Boundary::default().offer(plan()).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_credentials_deny_without_touching_keyring() {
    use crate::acquisition::{CredentialBackend, OsCredentials, SecretValue};
    assert!(OsCredentials.get("synthetic-never-looked-up").is_err());
    assert!(
        OsCredentials
            .put("synthetic-never-installed", &SecretValue::new(vec![]))
            .is_err()
    );
    assert!(OsCredentials.delete("synthetic-never-deleted").is_err());
}
