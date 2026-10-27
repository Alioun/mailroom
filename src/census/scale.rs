//! Synthetic personal-scale census for measuring journaled operations.
//! Every value is invented. Messages land in folders of their own, outside
//! Inbox and without filing roles, so they never enter a backlog plan.

use super::{Census, Direction, FolderCensus, MessageRecord, ScanState, parse_headers};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(crate) struct Spec {
    pub messages: u32,
    pub folders: u32,
    pub sent_per_mille: u32,
    pub gmail: bool,
}

pub(crate) const PERSONAL: Spec = Spec {
    messages: 100_000,
    folders: 30,
    sent_per_mille: 120,
    gmail: false,
};

/// Messages in one measured backlog run, `$MAILROOM_SCALE_RUN` or 501.
pub(crate) fn run_from_env() -> u32 {
    std::env::var("MAILROOM_SCALE_RUN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(501)
}

pub(crate) fn spec_from_env(gmail: bool) -> Spec {
    let messages = std::env::var("MAILROOM_SCALE_MESSAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(PERSONAL.messages);
    Spec {
        messages,
        gmail,
        ..PERSONAL
    }
}

fn received(hop: u32, seed: u32) -> String {
    format!(
        "Received: from mx{hop}.relay{r}.example.net (mx{hop}.relay{r}.example.net [192.0.2.{a}])\r\n\tby mx.mailbox.example.test (Postfix) with ESMTPS id {seed:08X}{hop}\r\n\tfor <owner@home.example.test>; Wed, 21 Oct 2026 10:{m:02}:00 +0000\r\n",
        r = seed % 97,
        a = seed % 250 + 1,
        m = (seed + hop) % 60
    )
}

fn raw(seed: u32, sent: bool) -> String {
    let sender = seed % 3_000;
    let bulk = !sent && seed % 5 < 3;
    let mut h = String::new();
    if sent {
        h.push_str(&format!(
            "From: Owner <owner@home.example.test>\r\nTo: Contact {c} <person{c}@friends{d}.example.org>\r\n",
            c = sender,
            d = sender % 211
        ));
        if seed % 4 == 0 {
            h.push_str("Cc: Second <second@friends.example.org>\r\n");
        }
    } else {
        h.push_str(&received(1, seed));
        h.push_str(&received(2, seed));
        h.push_str(&received(3, seed));
        h.push_str(&format!(
            "Return-Path: <bounce-{seed}@mail.sender{sender}.example.com>\r\nAuthentication-Results: mx.mailbox.example.test; dkim=pass header.d=sender{sender}.example.com header.s=s1 header.b=AbCdEf12; spf=pass smtp.mailfrom=mail.sender{sender}.example.com; dmarc=pass header.from=sender{sender}.example.com\r\n",
        ));
        h.push_str(&format!(
            "From: Sender {sender} <news@sender{sender}.example.com>\r\nTo: owner@home.example.test\r\nDelivered-To: owner@home.example.test\r\n"
        ));
        if bulk {
            h.push_str(&format!(
                "List-Id: Sender {sender} updates <updates.sender{sender}.example.com>\r\nList-Unsubscribe: <https://sender{sender}.example.com/u?t={seed:016x}>, <mailto:unsub-{seed}@sender{sender}.example.com>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nFeedback-ID: {seed}:campaign:sender{sender}\r\n"
            ));
        }
        h.push_str(&format!(
            "DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=sender{sender}.example.com; s=s1; t=1792886400; h=from:to:subject:date:message-id:list-unsubscribe; bh={seed:032x}=; b={seed:064x}{sender:064x}{seed:064x}\r\n"
        ));
    }
    h.push_str(&format!(
        "Subject: Synthetic subject {seed} about order {o} and the weekly update\r\nDate: Wed, 21 Oct 2026 10:00:00 +0000\r\nMessage-ID: <{seed:08x}.{sender}@sender{sender}.example.com>\r\n\r\n",
        o = seed % 9_973
    ));
    h
}

/// One record per seed. The same seed always yields the same record.
pub(crate) fn record(seed: u32, uid: u32, sent: bool, gmail: bool) -> MessageRecord {
    let direction = if sent {
        Direction::Sent
    } else {
        Direction::Received
    };
    MessageRecord {
        delivery: Default::default(),
        uid,
        size: 4_000 + u64::from(seed % 60_000),
        internal_date: "2026-10-21T10:00:00Z".into(),
        flags_observed: true,
        flags: if seed % 3 == 0 {
            BTreeSet::new()
        } else {
            BTreeSet::from(["\\Seen".to_owned()])
        },
        gmail_message_id: gmail
            .then(|| format!("{}", 9_000_000_000_000_000_000u64 + u64::from(seed))),
        gmail_thread_id: gmail
            .then(|| format!("{}", 8_000_000_000_000_000_000u64 + u64::from(seed / 3))),
        gmail_labels: if gmail {
            BTreeSet::from([format!("Scale-{}", seed % 30)])
        } else {
            BTreeSet::new()
        },
        headers: parse_headers(raw(seed, sent).as_bytes(), direction).expect("synthetic headers"),
    }
}

/// Adds `spec.messages` invented messages in `spec.folders` new folders, one
/// of them a Sent folder holding the sent share.
pub(crate) fn inflate(census: &mut Census, spec: Spec) {
    let sent_total = spec.messages / 1000 * spec.sent_per_mille;
    let received_total = spec.messages - sent_total;
    let received_folders = spec.folders - 1;
    let mut folders: BTreeMap<String, Vec<MessageRecord>> = BTreeMap::new();
    for i in 0..received_total {
        let folder = format!("Scale/Folder-{:02}", i % received_folders);
        let list = folders.entry(folder).or_default();
        let uid = list.len() as u32 + 1;
        list.push(record(i, uid, false, spec.gmail));
    }
    for i in 0..sent_total {
        let list = folders.entry("Scale/Sent".into()).or_default();
        let uid = list.len() as u32 + 1;
        list.push(record(received_total + i, uid, true, spec.gmail));
    }
    census
        .folder_roles
        .entry("Scale/Sent".into())
        .or_default()
        .insert("sent".into());
    for (name, records) in folders {
        let live_uids = records.iter().map(|m| m.uid).collect();
        census.folders.insert(
            name,
            FolderCensus {
                uidvalidity: 7,
                state: ScanState::Complete,
                live_uids,
                messages: records.into_iter().map(|m| (m.uid, m)).collect(),
            },
        );
    }
}

pub(crate) fn messages(census: &Census) -> usize {
    census.folders.values().map(|f| f.messages.len()).sum()
}

/// Appends one JSON line to `$MAILROOM_SCALE_OUT` (or stdout) for the receipt.
pub(crate) fn report(operation: &str, extra: serde_json::Value, stats: super::io::Stats) {
    let line = serde_json::json!({"operation": operation, "stats": stats, "extra": extra});
    println!("scale.result {line}");
    if let Some(path) = std::env::var_os("MAILROOM_SCALE_OUT") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }
}

/// Inflates the saved census of an existing fixture account and clears the
/// counters, so the next measured step starts from a personal-scale store.
pub(crate) fn inflate_store(store: &super::EncryptedStore, gmail: bool) -> serde_json::Value {
    let mut state = store.load_state().unwrap();
    let fixture = messages(&state.census);
    let began = std::time::Instant::now();
    inflate(&mut state.census, spec_from_env(gmail));
    store.save_state(&mut state).unwrap();
    let save = super::io::take();
    serde_json::json!({
        "fixture_messages": fixture,
        "messages": messages(&state.census),
        "file_bytes": std::fs::metadata(store.path()).unwrap().len(),
        "inflate_and_save_ms": began.elapsed().as_millis() as u64,
        "inflate_save_us": save.save_us,
    })
}

/// CPU time of the calling thread. On a loaded host wall time mostly
/// measures waiting for a core; this is what the step itself costs.
pub(crate) fn thread_cpu_ms() -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    t.tv_sec as u64 * 1000 + t.tv_nsec as u64 / 1_000_000
}

/// Runs one measured step and reports its counters, wall time, the calling
/// thread's CPU time and the load average at the end.
pub(crate) fn measure<T>(operation: &str, extra: serde_json::Value, step: impl FnOnce() -> T) -> T {
    super::io::take();
    let began = std::time::Instant::now();
    let cpu = thread_cpu_ms();
    let result = step();
    let wall_ms = began.elapsed().as_millis() as u64;
    let mut extra = extra;
    extra["wall_ms"] = wall_ms.into();
    extra["thread_cpu_ms"] = (thread_cpu_ms() - cpu).into();
    let mut load = [0f64; 3];
    unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    extra["load1"] = load[0].into();
    report(operation, extra, super::io::take());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::census::{AccountId, CensusKeys, EncryptedStore, LocalSecret, io};

    #[test]
    #[ignore = "personal-scale measurement; run explicitly"]
    fn scale_store_journal_save() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountId::new("scale-store").unwrap();
        let keys = CensusKeys::derive(&LocalSecret::from_bytes(vec![7; 32]).unwrap(), &account);
        let store = EncryptedStore::new(dir.path().into(), keys).unwrap();
        let mut census = Census::new(account);
        let began = std::time::Instant::now();
        inflate(&mut census, spec_from_env(false));
        let generate_ms = began.elapsed().as_millis();
        let json = serde_json::to_vec(&census).unwrap().len();
        println!(
            "scale.census messages={} json_bytes={json}",
            messages(&census)
        );
        store.save(&census).unwrap();
        let initial = io::take();
        report(
            "store.initial_save",
            serde_json::json!({"messages": messages(&census), "census_json_bytes": json, "generate_ms": generate_ms, "file_bytes": std::fs::metadata(store.path()).unwrap().len()}),
            initial,
        );
        let mut state = store.load_state().unwrap();
        report("store.load_state", serde_json::json!({}), io::take());
        for round in 0..5 {
            state
                .synthetic_uid_next
                .insert(format!("journal-change-{round}"), round);
            store.save_state(&mut state).unwrap();
            report(
                "store.journal_only_save",
                serde_json::json!({"round": round}),
                io::take(),
            );
        }
        for round in 0..5u32 {
            let folder = state.census.folders.get_mut("Scale/Folder-00").unwrap();
            for n in 0..25 {
                let uid = folder.messages.keys().last().unwrap() + 1;
                folder.live_uids.insert(uid);
                folder
                    .messages
                    .insert(uid, record(2_000_000 + round * 25 + n, uid, false, false));
            }
            store.save_state(&mut state).unwrap();
            report(
                "store.census_batch_save",
                serde_json::json!({"round": round, "added_messages": 25}),
                io::take(),
            );
        }
    }
}
