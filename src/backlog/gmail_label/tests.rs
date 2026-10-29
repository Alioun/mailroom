use super::*;
use crate::backlog::imap_move::UidValidity;

fn context(validity: u32) -> MailboxIdentity {
    MailboxIdentity {
        name: ExactName::new("[Gmail]/All Mail").unwrap(),
        uidvalidity: UidValidity::new(validity).unwrap(),
    }
}
fn label() -> ExactName {
    ExactName::new("Geschäftlich/Studio").unwrap()
}
fn snapshot(id: &str, labels: &[&str], flags: &[&str]) -> Snapshot {
    Snapshot {
        gmail_id: GmailId::new(id).unwrap(),
        message_id_hash: format!("hash-{id}"),
        size: 1200,
        internal_date: "25-Oct-2026 12:00:00 +0000".into(),
        flags: flags.iter().map(|f| (*f).to_owned()).collect(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
    }
}
fn uid(n: u32) -> Uid {
    Uid::new(n).unwrap()
}
fn intent(messages: Vec<(u32, Snapshot)>) -> Option<LabelIntent> {
    LabelIntent::new(
        Purpose::Filing {
            protect_flagged: true,
        },
        context(11),
        label(),
        messages.into_iter().map(|(u, s)| (uid(u), s)).collect(),
    )
}
fn batch() -> LabelBatch {
    LabelBatch::forward(
        intent(vec![
            (4, snapshot("1001", &[INBOX], &["\\Seen"])),
            (8, snapshot("1002", &[INBOX, "Geschäftlich/Studio"], &[])),
            (30, snapshot("1005", &[], &[])),
        ])
        .unwrap(),
    )
    .unwrap()
}
fn ok() -> Completion {
    Completion::Tagged {
        status: TaggedStatus::Ok,
        code: None,
    }
}
fn readback(batch: &LabelBatch, index: usize, edit: impl Fn(Uid, &mut Snapshot)) -> LabelReadback {
    LabelReadback {
        identity: context(11),
        messages: batch
            .expected(index)
            .into_iter()
            .map(|(u, (_, mut after))| {
                edit(u, &mut after);
                (u, after)
            })
            .collect(),
    }
}

#[test]
fn approval_steps_add_only_missing_labels_then_remove_only_present_inbox() {
    let b = batch();
    assert_eq!(b.steps().len(), 2);
    assert_eq!(b.steps()[0].kind, StepKind::AddLabel);
    assert_eq!(b.steps()[0].uids, [uid(4), uid(30)].into());
    assert_eq!(b.steps()[1].kind, StepKind::RemoveInbox);
    assert_eq!(b.steps()[1].uids, [uid(4), uid(8)].into());
    let (before, after) = &b.expected(1)[&uid(4)];
    assert!(before.labels.contains("Geschäftlich/Studio") && before.labels.contains(INBOX));
    assert_eq!(after.labels, ["Geschäftlich/Studio".to_owned()].into());
    assert_eq!(after.flags, before.flags);
}

#[test]
fn intents_reject_protected_excluded_duplicate_and_system_destinations() {
    assert!(intent(vec![(4, snapshot("1001", &[INBOX], &["\\Flagged"]))]).is_none());
    assert!(intent(vec![(4, snapshot("1001", &[INBOX], &["\\Deleted"]))]).is_none());
    assert!(intent(vec![(4, snapshot("1001", &["\\Sent"], &[]))]).is_none());
    assert!(intent(vec![(4, snapshot("1001", &["\\Trash"], &[]))]).is_none());
    assert!(
        intent(vec![
            (4, snapshot("1001", &[INBOX], &[])),
            (8, snapshot("1001", &[INBOX], &[])),
        ])
        .is_none(),
        "X-GM-MSGID is the identity, never two UIDs"
    );
    assert!(intent(vec![]).is_none());
    for name in ["INBOX", "inbox", "[Gmail]/All Mail"] {
        assert!(
            LabelIntent::new(
                Purpose::ConditionalUndo,
                context(11),
                ExactName::new(name).unwrap(),
                [(uid(4), snapshot("1001", &[], &[]))].into()
            )
            .is_none(),
            "{name}"
        );
    }
    for id in ["0", "0123", "12a", "", "184467440737095516150"] {
        assert!(GmailId::new(id).is_none(), "{id}");
    }
    let labelled_only = intent(vec![(4, snapshot("1001", &["Geschäftlich/Studio"], &[]))]).unwrap();
    assert!(
        LabelBatch::forward(labelled_only).is_err(),
        "Nothing to change"
    );
}

#[test]
fn step_outcomes_follow_completion_and_exact_readback() {
    type Edit = fn(Uid, &mut Snapshot);
    let none: Edit = |_, _| {};
    let extra: Edit = |u, s| {
        if u == Uid::new(4).unwrap() {
            s.labels.insert("Andere".into());
        }
    };
    let undo: Edit = |u, s| {
        if u == Uid::new(4).unwrap() {
            s.labels.remove("Geschäftlich/Studio");
        }
    };
    let identity: Edit = |u, s| {
        if u == Uid::new(4).unwrap() {
            s.gmail_id = GmailId::new("9999").unwrap();
        }
    };
    let no = Completion::Tagged {
        status: TaggedStatus::No,
        code: None,
    };
    for (completion, edit, at4, at30) in [
        (ok(), none, Outcome::Verified, Outcome::Verified),
        (ok(), extra, Outcome::Changed, Outcome::Verified),
        (ok(), undo, Outcome::Conflict, Outcome::Verified),
        (ok(), identity, Outcome::Conflict, Outcome::Verified),
        (no.clone(), undo, Outcome::Unapplied, Outcome::Uncertain),
        (
            Completion::Lost,
            none,
            Outcome::Uncertain,
            Outcome::Uncertain,
        ),
    ] {
        let mut b = batch();
        b.begin_attempt(0).unwrap();
        assert_eq!(
            b.outcomes(0)[&uid(4)],
            Outcome::Uncertain,
            "attempted without completion"
        );
        b.record_completion(0, completion.clone()).unwrap();
        let r = readback(&b, 0, edit);
        b.observe(0, r).unwrap();
        let o = b.outcomes(0);
        assert_eq!((o[&uid(4)], o[&uid(30)]), (at4, at30), "{completion:?}");
        assert_eq!(b.outcomes(1)[&uid(8)], Outcome::Pending);
    }
    let mut b = batch();
    b.begin_attempt(0).unwrap();
    b.record_completion(0, ok()).unwrap();
    let mut r = readback(&b, 0, none);
    r.identity = context(12);
    b.observe(0, r).unwrap();
    assert!(
        b.outcomes(0).values().all(|o| *o == Outcome::Conflict),
        "UIDVALIDITY changed"
    );
    let mut b = batch();
    b.begin_attempt(0).unwrap();
    b.record_completion(0, ok()).unwrap();
    let mut r = readback(&b, 0, none);
    r.messages.remove(&uid(30));
    b.observe(0, r).unwrap();
    assert_eq!(
        b.outcomes(0)[&uid(30)],
        Outcome::Conflict,
        "absent from All Mail"
    );
}

#[test]
fn phases_are_ordered_and_smuggled_evidence_never_deserializes() {
    let mut b = batch();
    assert!(b.begin_attempt(1).is_err(), "Remove waits for add readback");
    assert!(b.record_completion(0, ok()).is_err());
    b.begin_attempt(0).unwrap();
    assert!(b.begin_attempt(0).is_err());
    assert!(b.begin_attempt(1).is_err());
    b.record_completion(0, ok()).unwrap();
    assert!(b.record_completion(0, ok()).is_err());
    let r = readback(&b, 0, |_, _| {});
    let mut outside = r.clone();
    outside
        .messages
        .insert(uid(8), snapshot("1002", &[INBOX], &[]));
    assert!(
        b.clone().observe(0, outside).is_err(),
        "UID outside the step"
    );
    b.observe(0, r).unwrap();
    b.begin_attempt(1).unwrap();
    let saved = serde_json::to_value(&b).unwrap();
    let back: LabelBatch = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(back, b);
    let mutations: [fn(&mut serde_json::Value); 8] = [
        |v| v["steps"][1]["readback"] = v["steps"][0]["readback"].clone(),
        |v| v["steps"][0]["attempted"] = false.into(),
        |v| v["steps"][0]["completion"] = serde_json::Value::Null,
        |v| v["steps"].as_array_mut().unwrap().reverse(),
        |v| v["steps"][0]["uids"] = serde_json::json!([4, 8, 30]),
        |v| v["steps"][1]["uids"] = serde_json::json!([4, 30]),
        |v| v["intent"]["destination"] = serde_json::json!({"decoded":"INBOX","wire":"INBOX"}),
        |v| v["steps"][0]["kind"] = "remove_label".into(),
    ];
    for (n, mutate) in mutations.iter().enumerate() {
        let mut changed = saved.clone();
        mutate(&mut changed);
        assert!(
            serde_json::from_value::<LabelBatch>(changed).is_err(),
            "mutation {n}"
        );
    }
}

#[test]
fn reverse_steps_restore_inbox_before_removing_the_added_label() {
    let undo = LabelIntent::new(
        Purpose::ConditionalUndo,
        context(11),
        label(),
        [
            (
                uid(4),
                snapshot("1001", &["Geschäftlich/Studio"], &["\\Flagged"]),
            ),
            (uid(30), snapshot("1005", &["Geschäftlich/Studio"], &[])),
        ]
        .into(),
    )
    .unwrap();
    let b = LabelBatch::reverse(undo.clone(), [uid(4)].into(), [uid(4), uid(30)].into()).unwrap();
    assert_eq!(
        b.steps().iter().map(|s| s.kind).collect::<Vec<_>>(),
        [StepKind::RestoreInbox, StepKind::RemoveLabel]
    );
    assert_eq!(b.expected(1)[&uid(4)].1.labels, [INBOX.to_owned()].into());
    assert!(b.expected(1)[&uid(4)].1.flags.contains("\\Flagged"));
    assert!(
        LabelBatch::reverse(undo, [uid(30)].into(), [uid(30)].into()).is_err(),
        "Each member needs a reverse step"
    );
}

#[test]
fn archive_only_removes_inbox_and_undo_only_restores_it() {
    let before = snapshot("2001", &[INBOX, "Unrelated"], &["\\Answered"]);
    let archive = LabelIntent::archive(
        Purpose::Filing {
            protect_flagged: true,
        },
        context(11),
        [(uid(4), before.clone())].into(),
    )
    .unwrap();
    let mut b = LabelBatch::forward(archive).unwrap();
    assert_eq!(
        b.steps().iter().map(|s| s.kind).collect::<Vec<_>>(),
        [StepKind::RemoveInbox]
    );
    assert_eq!(
        b.expected(0)[&uid(4)].1.labels,
        ["Unrelated".to_owned()].into()
    );
    assert_eq!(b.expected(0)[&uid(4)].1.flags, before.flags);
    b.begin_attempt(0).unwrap();
    assert_eq!(b.outcomes(0)[&uid(4)], Outcome::Uncertain);
    assert!(
        b.begin_attempt(0).is_err(),
        "An uncertain command cannot be retried"
    );
    b.record_completion(0, ok()).unwrap();
    b.observe(0, readback(&b, 0, |_, _| {})).unwrap();
    assert_eq!(b.outcomes(0)[&uid(4)], Outcome::Verified);
    let mut current = b.expected(0)[&uid(4)].1.clone();
    current.flags.insert("\\Seen".into());
    current.labels.insert("Later".into());
    let undo = LabelIntent::archive(
        Purpose::ConditionalUndo,
        context(11),
        [(uid(4), current.clone())].into(),
    )
    .unwrap();
    assert!(LabelBatch::reverse(undo.clone(), [uid(4)].into(), [uid(4)].into()).is_err());
    let reverse = LabelBatch::reverse(undo, [uid(4)].into(), BTreeSet::new()).unwrap();
    assert_eq!(
        reverse.steps().iter().map(|s| s.kind).collect::<Vec<_>>(),
        [StepKind::RestoreInbox]
    );
    current.labels.insert(INBOX.into());
    assert_eq!(reverse.expected(0)[&uid(4)].1, current);
    assert_eq!(
        serde_json::from_value::<LabelBatch>(serde_json::to_value(&b).unwrap()).unwrap(),
        b
    );
    let mut wrong = serde_json::to_value(&b).unwrap();
    wrong["intent"]["destination"] = serde_json::to_value(label()).unwrap();
    assert!(serde_json::from_value::<LabelBatch>(wrong).is_err());
    let mut wrong = serde_json::to_value(&b).unwrap();
    wrong["steps"][0]["kind"] = "add_label".into();
    assert!(serde_json::from_value::<LabelBatch>(wrong).is_err());
    let legacy = serde_json::to_value(batch()).unwrap();
    assert!(legacy["intent"].get("archive_only").is_none());
    assert!(serde_json::from_value::<LabelBatch>(legacy).is_ok());
}
