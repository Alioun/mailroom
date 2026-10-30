use super::*;
use crate::{
    acquisition::SecretValue,
    census::folder_provisioning::{FakeProvider, Namespace, Namespaces},
};

#[path = "copy_connected_tests.rs"]
mod connected;

pub(super) fn admitted(root: &Path, command: &str) -> bool {
    root.join("copy-workflow").exists()
        && ["UID COPY ", "UID STORE ", "UID EXPUNGE "]
            .iter()
            .any(|prefix| command.starts_with(prefix))
}

fn uid_set(text: &str) -> Option<Vec<u32>> {
    let values = text
        .split(',')
        .map(|part| {
            let value: u32 = part.parse().ok()?;
            (value != 0 && value.to_string() == part).then_some(value)
        })
        .collect::<Option<Vec<_>>>()?;
    (values.len() <= crate::backlog::BATCH_LIMIT
        && values.iter().copied().collect::<BTreeSet<_>>().len() == values.len())
    .then_some(values)
}

pub(super) fn handle(
    root: &Path,
    store: &EncryptedStore,
    state: &mut AccountState,
    selected: Option<&str>,
    tag: &str,
    command: &str,
) -> std::io::Result<String> {
    let bad = || format!("{tag} BAD unsupported fixture command\r\n");
    let Some(source) = selected.filter(|name| state.census.folders.contains_key(*name)) else {
        return Ok(bad());
    };
    if !admitted(root, command) {
        return Ok(bad());
    }
    let (event, reply) = if let Some(argument) = command.strip_prefix("UID COPY ") {
        let Some((set, target)) = argument.split_once(' ') else {
            return Ok(bad());
        };
        let Some(uids) = uid_set(set) else {
            return Ok(bad());
        };
        let target = unquote(target)?;
        let Some(destination) = state.census.folders.get(&target) else {
            return Ok(bad());
        };
        if source == target {
            return Ok(bad());
        }
        let validity = destination.uidvalidity;
        let mut next = *state.synthetic_uid_next.get(&target).unwrap_or(&60);
        if let Some(last) = destination.live_uids.last() {
            let Some(after) = last.checked_add(1) else {
                return Ok(bad());
            };
            next = next.max(after);
        }
        let origin = &state.census.folders[source];
        let mut copies = vec![];
        for uid in uids {
            if let Some(message) = origin
                .messages
                .get(&uid)
                .filter(|_| origin.live_uids.contains(&uid))
            {
                let Some(after) = next.checked_add(4) else {
                    return Ok(bad());
                };
                let mut copy = message.clone();
                copy.uid = next;
                copies.push((uid, copy));
                next = after;
            }
        }
        let mapping = copies
            .iter()
            .map(|(original, copy)| (*original, copy.uid))
            .collect::<Vec<_>>();
        let destination = state.census.folders.get_mut(&target).unwrap();
        for (_, copy) in copies {
            destination.live_uids.insert(copy.uid);
            destination.messages.insert(copy.uid, copy);
        }
        state.synthetic_uid_next.insert(target, next);
        modseqs::sync(root, store, state, true);
        let reply = if mapping.is_empty() {
            format!("{tag} OK no matching messages\r\n")
        } else {
            let original = mapping
                .iter()
                .map(|(uid, _)| uid.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let copied = mapping
                .iter()
                .map(|(_, uid)| uid.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!("{tag} OK [COPYUID {validity} {original} {copied}] copied\r\n")
        };
        ("UID COPY", reply)
    } else if let Some(argument) = command.strip_prefix("UID STORE ") {
        let Some((set, conditional)) = argument.split_once(" (UNCHANGEDSINCE ") else {
            return Ok(bad());
        };
        let Some(uids) = uid_set(set) else {
            return Ok(bad());
        };
        let Some((version, action)) = conditional.split_once(") ") else {
            return Ok(bad());
        };
        let Ok(version) = version.parse::<u64>() else {
            return Ok(bad());
        };
        if version == 0 {
            return Ok(bad());
        }
        let add = match action {
            "+FLAGS.SILENT (\\Deleted)" => true,
            "-FLAGS.SILENT (\\Deleted)" => false,
            _ => return Ok(bad()),
        };
        let mut changed = vec![];
        let mut modified = vec![];
        for uid in uids {
            if state.census.folders[source].messages.contains_key(&uid) {
                if modseqs::of(root, source, uid) > version {
                    modified.push(uid);
                    continue;
                }
                let flags = &mut state
                    .census
                    .folders
                    .get_mut(source)
                    .unwrap()
                    .messages
                    .get_mut(&uid)
                    .unwrap()
                    .flags;
                if add {
                    flags.insert("\\Deleted".into());
                } else {
                    flags.remove("\\Deleted");
                }
                changed.push(uid);
            }
        }
        modseqs::sync(root, store, state, true);
        let folder = &state.census.folders[source];
        let mut reply = String::new();
        for uid in changed {
            let sequence = folder
                .live_uids
                .iter()
                .position(|value| *value == uid)
                .unwrap()
                + 1;
            reply.push_str(&format!(
                "* {sequence} FETCH (UID {uid} MODSEQ ({}))\r\n",
                modseqs::of(root, source, uid)
            ));
        }
        if modified.is_empty() {
            reply.push_str(&format!("{tag} OK conditional flags\r\n"));
        } else {
            let modified = modified
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            reply.push_str(&format!(
                "{tag} OK [MODIFIED {modified}] conditional flags\r\n"
            ));
        }
        ("UID STORE", reply)
    } else if let Some(set) = command.strip_prefix("UID EXPUNGE ") {
        let Some(uids) = uid_set(set) else {
            return Ok(bad());
        };
        let folder = state.census.folders.get_mut(source).unwrap();
        let mut reply = String::new();
        for uid in uids {
            if folder
                .messages
                .get(&uid)
                .is_some_and(|message| message.flags.contains("\\Deleted"))
            {
                let sequence = folder
                    .live_uids
                    .iter()
                    .position(|value| *value == uid)
                    .unwrap()
                    + 1;
                folder.messages.remove(&uid);
                folder.live_uids.remove(&uid);
                reply.push_str(&format!("* {sequence} EXPUNGE\r\n"));
            }
        }
        modseqs::sync(root, store, state, true);
        reply.push_str(&format!("{tag} OK exact expunge\r\n"));
        ("UID EXPUNGE", reply)
    } else {
        return Ok(bad());
    };
    let mut events = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("copy-command-names.log"))?;
    writeln!(events, "{event}")?;
    events.sync_all()?;
    Ok(reply)
}

fn events(root: &Path) -> Vec<String> {
    match std::fs::read_to_string(root.join("copy-command-names.log")) {
        Ok(text) => text.lines().map(String::from).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => panic!("fixture event log unavailable: {error:?}"),
    }
}

fn world(root: &Path) -> (Accounts, Account) {
    let repo = persistent_test_accounts(root);
    let account = repo
        .create(
            Provider::MailboxOrg,
            "fixture-owner@example.test".into(),
            &SecretValue::new(b"invented-fixture-password".to_vec()),
        )
        .unwrap();
    let primary = repo.store(&account).unwrap();
    let mut state = primary.load_state().unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    state.census.folders = [
        ("INBOX".into(), folder(101, &[4, 8, 19, 99])),
        ("Personal/News".into(), folder(202, &[])),
    ]
    .into();
    state
        .census
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .get_mut(&99)
        .unwrap()
        .flags
        .insert("\\Deleted".into());
    state.synthetic_uid_next.insert("Personal/News".into(), 60);
    state.synthetic_uid_next.insert("INBOX".into(), 104);
    state.synthetic_folders = Some(FakeProvider {
        address: "127.0.0.1:0".parse().unwrap(),
        root: cert.der().to_vec(),
        key: key.serialize_der(),
        epoch: "move-copy-fixture/v1".into(),
        inventory: Inventory {
            capabilities: [
                "IMAP4REV1",
                "NAMESPACE",
                "SPECIAL-USE",
                "UIDPLUS",
                "CONDSTORE",
            ]
            .map(String::from)
            .into(),
            namespaces: Namespaces {
                personal: vec![Namespace {
                    prefix: String::new(),
                    delimiter: Some("/".into()),
                }],
                other: vec![],
                shared: vec![],
            },
            mailboxes: ["INBOX", "Personal/News"]
                .map(|name| Mailbox {
                    name: ExactName::new(name).unwrap(),
                    delimiter: Some("/".into()),
                    attributes: BTreeSet::new(),
                })
                .into(),
        },
        commands: vec![],
    });
    let effect = repo.fixture_sieve_store(&account.id, root).unwrap();
    effect.save(&state.census).unwrap();
    state.revision = effect.load_state().unwrap().revision;
    modseqs::sync(root, &effect, &mut state, true);
    effect.save_state(&mut state).unwrap();
    modseqs::saved(root, &effect);
    std::fs::write(root.join("copy-workflow"), b"explicit fixture").unwrap();
    (repo, account)
}

fn apply(root: &Path, repo: &Accounts, account: &Account, command: &str) -> String {
    let (store, mut state) = external(repo, &account.id, root);
    let reply = handle(root, &store, &mut state, Some("INBOX"), "F1", command).unwrap();
    store.save_state(&mut state).unwrap();
    modseqs::saved(root, &store);
    modseqs::persist(root);
    reply
}

#[test]
fn copy_preserves_sources_flags_and_sparse_mapping_across_encrypted_reopen() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account) = world(root.path());
    let (_, before) = external(&repo, &account.id, root.path());
    assert!(
        apply(
            root.path(),
            &repo,
            &account,
            "UID COPY 4,19 \"Personal/News\""
        )
        .contains("[COPYUID 202 4,19 60,64]")
    );
    let (_, after) = external(&repo, &account.id, root.path());
    assert_eq!(
        serde_json::to_value(&before.census.folders["INBOX"]).unwrap(),
        serde_json::to_value(&after.census.folders["INBOX"]).unwrap()
    );
    for (original, copied) in [(4, 60), (19, 64)] {
        let mut expected = before.census.folders["INBOX"].messages[&original].clone();
        expected.uid = copied;
        assert_eq!(
            serde_json::to_value(&after.census.folders["Personal/News"].messages[&copied]).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    assert!(
        apply(root.path(), &repo, &account, "UID COPY 8 \"Personal/News\"")
            .contains("[COPYUID 202 8 68]")
    );
}

#[test]
fn conditional_store_returns_modified_members_and_preserves_other_flags() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account) = world(root.path());
    let initial = modseqs::of(root.path(), "INBOX", 4);
    let command = format!("UID STORE 4 (UNCHANGEDSINCE {initial}) +FLAGS.SILENT (\\Deleted)");
    assert!(apply(root.path(), &repo, &account, &command).contains("MODSEQ"));
    let command = format!("UID STORE 4,8 (UNCHANGEDSINCE {initial}) +FLAGS.SILENT (\\Deleted)");
    assert!(apply(root.path(), &repo, &account, &command).contains("[MODIFIED 4]"));
    let version = modseqs::of(root.path(), "INBOX", 4);
    let command = format!("UID STORE 4 (UNCHANGEDSINCE {version}) -FLAGS.SILENT (\\Deleted)");
    assert!(!apply(root.path(), &repo, &account, &command).contains("MODIFIED"));
    let (_, state) = external(&repo, &account.id, root.path());
    assert_eq!(
        state.census.folders["INBOX"].messages[&4].flags,
        record(4).flags
    );
    assert!(
        state.census.folders["INBOX"].messages[&8]
            .flags
            .contains("\\Deleted")
    );
}

#[test]
fn exact_uid_expunge_removes_only_requested_deleted_messages() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account) = world(root.path());
    let version = modseqs::of(root.path(), "INBOX", 4);
    apply(
        root.path(),
        &repo,
        &account,
        &format!("UID STORE 4 (UNCHANGEDSINCE {version}) +FLAGS.SILENT (\\Deleted)"),
    );
    assert!(apply(root.path(), &repo, &account, "UID EXPUNGE 4,8").contains("EXPUNGE"));
    let (_, state) = external(&repo, &account.id, root.path());
    assert_eq!(state.census.folders["INBOX"].live_uids, [8, 19, 99].into());
    assert!(
        state.census.folders["INBOX"].messages[&99]
            .flags
            .contains("\\Deleted")
    );
}

#[test]
fn invalid_or_uninstalled_fixture_commands_cannot_change_mailbox_state() {
    let root = tempfile::tempdir().unwrap();
    let (repo, account) = world(root.path());
    for command in [
        "UID COPY 4,4 \"Personal/News\"",
        "UID COPY 0 \"Personal/News\"",
        "UID COPY 4:* \"Personal/News\"",
        "UID COPY 04 \"Personal/News\"",
        "UID COPY 4 \"Absent\"",
        "UID STORE 4 (UNCHANGEDSINCE 1) +FLAGS.SILENT (\\Seen)",
        "UID EXPUNGE 4:*",
        "EXPUNGE",
        "CLOSE",
    ] {
        let (store, mut state) = external(&repo, &account.id, root.path());
        let before = serde_json::to_value(&state).unwrap();
        assert!(
            handle(
                root.path(),
                &store,
                &mut state,
                Some("INBOX"),
                "F1",
                command
            )
            .unwrap()
            .contains("BAD")
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
    }
    std::fs::remove_file(root.path().join("copy-workflow")).unwrap();
    assert!(!admitted(root.path(), "UID COPY 4 \"Personal/News\""));
}
