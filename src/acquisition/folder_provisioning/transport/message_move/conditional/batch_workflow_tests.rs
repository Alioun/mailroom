use super::*;
use crate::{
    acquisition::{
        AcquisitionError, CopyContinuationWork,
        backlog::{
            copy_continuation::Worker,
            copy_start::tests::confirm_prepared,
            copy_worker::CopyWork,
            preview::tests::{fixture, review},
        },
        folder_provisioning::transport::message_move::Profile,
    },
    backlog::{BATCH_LIMIT, imap_copy::VersionedMetadata, protocol::Status},
    census::Census,
};

const MESSAGES: usize = BATCH_LIMIT + 3;
const MAPPED_OFFSET: u32 = 10_000;
const FETCH_FIELDS: &str =
    "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] MODSEQ)";

fn set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn observed(
    identity: &MailboxIdentity,
    uids: &[u32],
    highest: u64,
    version: u64,
    deleted: bool,
    select_tag: usize,
) -> Vec<u8> {
    let mut bytes = selection(identity, highest, select_tag);
    for uid in uids {
        let record = fetch(*uid, Some(version));
        if deleted {
            bytes.extend(
                String::from_utf8(record)
                    .unwrap()
                    .replace(
                        "FLAGS (\\Seen \\Recent $owner)",
                        "FLAGS (\\Seen \\Deleted $owner)",
                    )
                    .as_bytes(),
            );
        } else {
            bytes.extend(record);
        }
    }
    bytes.extend(format!("F{} OK fetched\r\n", select_tag + 1).as_bytes());
    bytes
}

fn complete_batch(e: &CopyEvidence, batch: usize) -> VecDeque<Flight> {
    let highest = BASE + 30 * u64::try_from(batch).unwrap();
    let source = e.intent().source();
    let destination = e.intent().destination();
    let members = e
        .intent()
        .messages()
        .keys()
        .map(|u| u.get())
        .collect::<Vec<_>>();
    let copies = members
        .iter()
        .map(|u| u + MAPPED_OFFSET)
        .collect::<Vec<_>>();
    let members_text = set(&members);
    let copies_text = set(&copies);
    let source_select = format!("SELECT \"{}\" (CONDSTORE)", source.name.wire);
    let destination_select = format!("SELECT \"{}\" (CONDSTORE)", destination.name.wire);
    let source_fetch = format!("UID FETCH {members_text} {FETCH_FIELDS}");
    let destination_fetch = format!("UID FETCH {copies_text} {FETCH_FIELDS}");
    let pair = |present: &[u32], deleted: bool| {
        let mut bytes = observed(
            source,
            present,
            if present.is_empty() {
                highest + 30
            } else if deleted {
                highest + 20
            } else {
                highest
            },
            if deleted { highest + 20 } else { BASE },
            deleted,
            1,
        );
        bytes.extend(observed(
            destination,
            &copies,
            highest + 10,
            highest + 10,
            false,
            3,
        ));
        Flight {
            event: "observe-pair",
            commands: vec![
                source_select.clone(),
                source_fetch.clone(),
                destination_select.clone(),
                destination_fetch.clone(),
            ],
            response: Ok(bytes),
        }
    };
    let mut initial = selection(destination, highest, 1);
    initial.extend(selection(source, highest, 2));
    for uid in &members {
        initial.extend(fetch(*uid, Some(BASE)));
    }
    initial.extend_from_slice(b"F3 OK fetched\r\n");
    let mut store_result = vec![];
    for uid in &members {
        store_result
            .extend(format!("* 2 FETCH (UID {uid} MODSEQ ({}))\r\n", highest + 20).as_bytes());
    }
    store_result.extend_from_slice(b"F1 OK marked\r\n");
    [
        Flight {
            event: "copy-readbacks",
            commands: vec![
                destination_select.clone(),
                source_select.clone(),
                source_fetch.clone(),
            ],
            response: Ok(initial),
        },
        Flight {
            event: "copy-select",
            commands: vec![source_select.clone()],
            response: Ok(selection(source, highest, 1)),
        },
        Flight {
            event: "copy",
            commands: vec![format!(
                "UID COPY {members_text} \"{}\"",
                destination.name.wire
            )],
            response: Ok(format!(
                "F1 OK [COPYUID {} {members_text} {copies_text}] copied\r\n",
                destination.uidvalidity.get()
            )
            .into_bytes()),
        },
        pair(&members, false),
        pair(&members, false),
        pair(&members, false),
        Flight {
            event: "store-select",
            commands: vec![source_select.clone()],
            response: Ok(selection(source, highest, 1)),
        },
        Flight {
            event: "store",
            commands: vec![format!(
                "UID STORE {members_text} (UNCHANGEDSINCE {BASE}) +FLAGS.SILENT (\\Deleted)"
            )],
            response: Ok(store_result),
        },
        Flight {
            event: "observe-store",
            commands: vec![source_select.clone(), source_fetch.clone()],
            response: Ok(observed(
                source,
                &members,
                highest + 20,
                highest + 20,
                true,
                1,
            )),
        },
        pair(&members, true),
        Flight {
            event: "expunge-select",
            commands: vec![source_select.clone()],
            response: Ok(selection(source, highest + 20, 1)),
        },
        Flight {
            event: "expunge",
            commands: vec![format!("UID EXPUNGE {members_text}")],
            response: Ok(b"F1 OK complete\r\n".to_vec()),
        },
        pair(&[], false),
    ]
    .into()
}

fn saved_rule_batches(stop: bool) {
    let (_root, mut repo, account, store, mut saved, topology, _) = fixture(Profile::Copy);
    let passwords = repo.track_password_accesses(&account);
    let sample = saved.census.folders["INBOX"]
        .messages
        .values()
        .next()
        .unwrap()
        .clone();
    let members: BTreeSet<_> = (0..MESSAGES)
        .map(|n| 4 + 4 * u32::try_from(n).unwrap())
        .collect();
    let mut census = Census::new(account.id.clone());
    census
        .reconcile_folder("INBOX", 101, members.clone())
        .unwrap();
    census
        .reconcile_folder("Privat/News", 202, BTreeSet::new())
        .unwrap();
    for uid in &members {
        let mut message = sample.clone();
        message.uid = *uid;
        census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .insert(message)
            .unwrap();
    }
    saved.census = census.into();
    store.save_state(&mut saved).unwrap();
    let mut requested = vec![];
    let preview = review(&repo, &account, &store, &saved, topology, |intent| {
        requested.push(
            intent
                .messages()
                .keys()
                .map(|u| u.get())
                .collect::<BTreeSet<_>>(),
        );
        Ok(Readback::new(
            intent.source().clone(),
            ModSeq::new(BASE).unwrap(),
            intent
                .messages()
                .iter()
                .map(|(uid, metadata)| {
                    (
                        *uid,
                        VersionedMetadata {
                            metadata: metadata.clone(),
                            modseq: ModSeq::new(BASE).unwrap(),
                        },
                    )
                })
                .collect(),
        )
        .unwrap())
    })
    .unwrap();
    assert_eq!(
        requested.iter().map(BTreeSet::len).collect::<Vec<_>>(),
        [BATCH_LIMIT, 3]
    );
    assert!(requested[0].is_disjoint(&requested[1]));
    assert_eq!(
        requested.iter().flatten().copied().collect::<BTreeSet<_>>(),
        members
    );
    assert_eq!(preview.operation.plan.filing.changes.len(), MESSAGES);
    assert_eq!(preview.operation.batches.len(), 2);
    let receipt = preview.receipt.clone();
    let plan = preview.operation.plan.clone();
    let source = repo.backlog_source.as_deref().unwrap();
    let io = Script {
        source: source.identity(),
        flights: preview
            .operation
            .batches
            .iter()
            .enumerate()
            .flat_map(|(index, batch)| complete_batch(batch.as_copy().unwrap(), index))
            .collect(),
        events: Rc::new(RefCell::new(vec![])),
    };
    let id = confirm_prepared(
        &repo,
        &account,
        &store,
        &mut saved,
        preview.operation,
        &receipt,
    )
    .unwrap();
    let permit = permitted(source);
    let mut transport = WorkerTransport {
        io,
        topology: plan.topology.clone(),
        stop_at: stop.then_some(10),
        inventories: 0,
    };
    let result = repo
        .copy_worker(&account, &store, &mut saved, &id, CopyWork::File, &|_| {
            Ok(())
        })
        .unwrap()
        .drive(&mut transport, &permit, &Cancellation::default());
    if stop {
        assert!(matches!(result, Err(AcquisitionError::Cancelled)));
        let run = &saved.protocol_backlog.runs[0];
        assert_eq!(run.status, Status::Stopped);
        assert!(run.stop_requested);
        assert!(crate::backlog::protocol::verified(&run.batches[0]));
        assert!(!run.batches[1].attempted());
        assert_eq!(
            (
                count(&transport.io, "copy"),
                count(&transport.io, "store"),
                count(&transport.io, "expunge")
            ),
            (1, 1, 1)
        );
        assert_eq!(transport.inventories, 1);
        drop(store);
        let store = repo.store(&account).unwrap();
        let mut reopened = store.load_state().unwrap();
        assert!(reopened.protocol_backlog == saved.protocol_backlog);
        let continuation = repo
            .copy_continuation(
                &account,
                &store,
                &mut reopened,
                &id,
                CopyContinuationWork::Resume,
                &|_| Ok(()),
            )
            .unwrap();
        let Worker::Forward(worker) = continuation.worker else {
            panic!("Filing Resume must keep the forward worker")
        };
        worker
            .drive(&mut transport, &permit, &Cancellation::default())
            .unwrap();
        saved = reopened;
        assert!(store.load_state().unwrap().protocol_backlog == saved.protocol_backlog);
    } else {
        result.unwrap();
        assert!(store.load_state().unwrap().protocol_backlog == saved.protocol_backlog);
    }
    let run = &saved.protocol_backlog.runs[0];
    assert_eq!(run.status, Status::Complete);
    assert!(run.completed_at.is_some());
    assert!(!run.stop_requested);
    assert!(run.plan == plan);
    assert_eq!(run.receipt, receipt);
    assert!(run.batches.iter().all(crate::backlog::protocol::verified));
    assert!(transport.io.flights.is_empty());
    assert_eq!(transport.inventories, 2);
    assert_eq!(
        (
            count(&transport.io, "copy"),
            count(&transport.io, "store"),
            count(&transport.io, "expunge")
        ),
        (2, 2, 2)
    );
    assert_eq!(passwords.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(!repo.backlog_installed(Some(&account.id)));
}

#[test]
fn saved_rule_copy_files_exact_sparse_batches_above_the_effect_limit() {
    saved_rule_batches(false);
}

#[test]
fn stop_preserves_the_completed_batch_and_explicit_resume_never_repeats_it() {
    saved_rule_batches(true);
}
