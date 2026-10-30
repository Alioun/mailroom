use super::*;
use crate::{
    acquisition::{
        backlog::{
            compensation_observer::CompensationReadTransport,
            compensation_worker::CompensationTransport,
        },
        folder_provisioning::transport::message_move::{
            CompensationCheckpoints, conditional::compensation::Mode,
        },
    },
    backlog::imap_copy::CompensationEvidence,
    census::folder_provisioning::Inventory,
};

struct UndoPeer {
    io: Script,
    topology: Inventory,
    inventories: usize,
}

impl CompensationReadTransport for UndoPeer {
    fn inventory(&mut self) -> Result<Inventory, Error> {
        self.inventories += 1;
        Ok(self.topology.clone())
    }

    fn observe(
        &mut self,
        permit: &BacklogPermit<'_>,
        forward: &CopyEvidence,
    ) -> Result<(Readback, Readback), Error> {
        self.io
            .conditional()
            .observe_for_compensation(permit, forward)
    }
}

impl CompensationTransport for UndoPeer {
    fn inventory(&mut self) -> Result<Inventory, Error> {
        self.inventories += 1;
        Ok(self.topology.clone())
    }

    fn execute(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.io
            .conditional()
            .compensate(permit, evidence, cancel, checkpoints, Mode::Execute)
    }

    fn reconcile(
        &mut self,
        permit: &BacklogPermit<'_>,
        evidence: &mut CompensationEvidence,
        cancel: &Cancellation,
        checkpoints: &mut impl CompensationCheckpoints,
    ) -> Result<(), Error> {
        self.io
            .conditional()
            .compensate(permit, evidence, cancel, checkpoints, Mode::Reconcile)
    }
}

fn review_flight(forward: &CopyEvidence, batch: usize) -> Flight {
    let intent = forward.intent();
    let original: Vec<_> = intent.messages().keys().map(|uid| uid.get()).collect();
    let copied: Vec<_> = forward
        .copy()
        .mapping()
        .unwrap()
        .pairs()
        .values()
        .map(|uid| uid.get())
        .collect();
    let highest = BASE + 60;
    let mut bytes = observed(intent.source(), &[], highest, BASE, false, 1);
    bytes.extend(observed(
        intent.destination(),
        &copied,
        highest,
        BASE + 10 + 30 * u64::try_from(batch).unwrap(),
        false,
        3,
    ));
    Flight {
        event: "undo-review",
        commands: vec![
            format!("SELECT \"{}\" (CONDSTORE)", intent.source().name.wire),
            format!("UID FETCH {} {FETCH_FIELDS}", set(&original)),
            format!("SELECT \"{}\" (CONDSTORE)", intent.destination().name.wire),
            format!("UID FETCH {} {FETCH_FIELDS}", set(&copied)),
        ],
        response: Ok(bytes),
    }
}

#[test]
fn separately_approved_multi_batch_undo_restores_the_exact_saved_filing_run() {
    let (_root, mut repo, account, store, mut saved) = saved_rule_batches(false);
    let passwords = repo.track_password_accesses(&account);
    let run = &saved.protocol_backlog.runs[0];
    let id = run.id.clone();
    let forward = run.batches.clone();
    let forward_receipt = run.receipt.clone();
    let forward_plan = run.plan.clone();
    let mut peer = UndoPeer {
        io: Script {
            source: run.plan.binding.source.clone(),
            flights: forward
                .iter()
                .enumerate()
                .map(|(index, batch)| review_flight(batch.as_copy().unwrap(), index))
                .collect(),
            events: Rc::new(RefCell::new(vec![])),
        },
        topology: run.plan.topology.clone(),
        inventories: 0,
    };
    let source = repo.backlog_source.as_deref().unwrap();
    let permit = permitted(source);
    let ciphertext = std::fs::read(store.path()).unwrap();
    let observed = repo
        .observe_copy_compensation(
            &account,
            &store,
            &saved,
            &id,
            &mut peer,
            &permit,
            &Cancellation::default(),
            &|_| Ok(()),
        )
        .unwrap();
    assert_eq!(peer.inventories, 2);
    assert!(peer.io.flights.is_empty());
    assert_eq!(&*peer.io.events.borrow(), &["undo-review", "undo-review"]);
    assert_eq!(std::fs::read(store.path()).unwrap(), ciphertext);
    let actions = observed.review().actions();
    assert_eq!(actions.restore, MESSAGES);
    assert_eq!(
        (
            actions.remove_copy,
            actions.clear_deleted,
            actions.conflicts
        ),
        (0, 0, 0)
    );
    let undo_receipt = observed.review().receipt().to_owned();
    assert_ne!(undo_receipt, forward_receipt);
    observed
        .confirm(&repo, &account, &store, &mut saved, &undo_receipt, |_| {
            Ok(())
        })
        .unwrap();
    let run = &saved.protocol_backlog.runs[0];
    assert_eq!(run.status, Status::Stopped);
    assert_eq!(run.undo_receipt.as_deref(), Some(undo_receipt.as_str()));
    assert!(run.batches == forward);
    assert_eq!(run.compensation.len(), 2);
    peer.io.flights = run
        .compensation
        .iter()
        .enumerate()
        .flat_map(|(index, compensation)| {
            complete_batch(compensation.evidence.restore().unwrap(), index)
        })
        .collect();
    let continuation = repo
        .copy_continuation(
            &account,
            &store,
            &mut saved,
            &id,
            CopyContinuationWork::ResumeUndo,
            &|_| Ok(()),
        )
        .unwrap();
    let Worker::Compensation(worker) = continuation.worker else {
        panic!("Confirmed Undo must select the compensation worker")
    };
    worker
        .drive(&mut peer, &permit, &Cancellation::default())
        .unwrap();
    assert!(peer.io.flights.is_empty());
    assert_eq!(peer.inventories, 4);
    assert_eq!(
        (
            count(&peer.io, "copy"),
            count(&peer.io, "store"),
            count(&peer.io, "expunge")
        ),
        (2, 2, 2)
    );
    let run = &saved.protocol_backlog.runs[0];
    assert_eq!(run.status, Status::Undone);
    assert_eq!(run.compensation_counts().restored, MESSAGES);
    assert_eq!(run.compensation_counts().copies_removed, 0);
    assert_eq!(run.receipt, forward_receipt);
    assert!(run.plan == forward_plan);
    assert!(run.batches == forward);
    drop(store);
    let store = repo.store(&account).unwrap();
    let reopened = store.load_state().unwrap();
    assert!(reopened.protocol_backlog == saved.protocol_backlog);
    assert_eq!(passwords.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(!repo.backlog_installed(Some(&account.id)));
}
