//! Local Google Takeout `.mbox` header-only importer.
//!
//! Streams an `.mbox` file line-by-line, extracts only mail headers (never storing message
//! bodies), parses `X-Gmail-Labels` and standard headers through `census::parse_headers`,
//! and merges the resulting records into an account's encrypted census store.
use super::{Accounts, AcquisitionError, Provider};
use crate::census::{
    AccountId, Direction, MessageRecord, ScanState,
    acquisition::{
        Attempt, CountObservation, CountSource, FolderObservation, Selection, SessionObservation,
        Snapshot,
    },
    delivery::DeliveryEvidence,
    parse_headers,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_LINE_BYTES: usize = 32 * 1024;
const MAX_MESSAGES: usize = 500_000;
pub const TAKEOUT_ADAPTER: &str = "mailroom/takeout-mbox/v1";

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImportSummary {
    pub imported: usize,
    pub skipped: usize,
    pub folders: usize,
    pub labels: usize,
    pub folder: String,
    pub truncated: bool,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn is_from_separator(line: &[u8]) -> bool {
    line.starts_with(b"From ") && line.len() > 5
}

fn extract_gmail_labels(raw_headers: &[u8]) -> BTreeSet<String> {
    let mut labels = BTreeSet::new();
    let Ok((headers, _)) = mailparse::parse_headers(raw_headers) else {
        return labels;
    };
    for header in headers {
        if !header.get_key().eq_ignore_ascii_case("X-Gmail-Labels") {
            continue;
        }
        let value = header.get_value();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut escaped = false;
        for ch in value.chars() {
            if escaped {
                current.push(ch);
                escaped = false;
                continue;
            }
            match ch {
                '\\' if in_quotes => escaped = true,
                '"' => in_quotes = !in_quotes,
                ',' if !in_quotes => {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        labels.insert(trimmed.to_owned());
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }
        }
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            labels.insert(trimmed.to_owned());
        }
    }
    labels
}

fn flush_header_block(
    buf: &mut Vec<u8>,
    overflow: bool,
    size: u64,
    is_gmail: bool,
    uid: &mut u32,
    records: &mut Vec<MessageRecord>,
    skipped: &mut usize,
) {
    if buf.is_empty() || overflow {
        *skipped += 1;
        buf.clear();
        return;
    }
    let mut normalized = Vec::with_capacity(buf.len() + 64);
    let mut prev = 0u8;
    for &b in buf.iter() {
        if b == b'\n' && prev != b'\r' {
            normalized.push(b'\r');
        }
        normalized.push(b);
        prev = b;
    }
    if !normalized.ends_with(b"\r\n\r\n") {
        if normalized.ends_with(b"\r\n") {
            normalized.extend_from_slice(b"\r\n");
        } else {
            normalized.extend_from_slice(b"\r\n\r\n");
        }
    }
    let labels = extract_gmail_labels(&normalized);
    let direction = if labels
        .iter()
        .any(|l| l.eq_ignore_ascii_case("Sent") || l.eq_ignore_ascii_case("\\Sent"))
    {
        Direction::Sent
    } else {
        Direction::Received
    };
    match parse_headers(&normalized, direction) {
        Ok(headers) => {
            *uid = uid.saturating_add(1);
            let mut flags = BTreeSet::new();
            let mut mapped_labels = BTreeSet::new();
            let mut has_unread = false;
            for label in &labels {
                if label.eq_ignore_ascii_case("Unread") {
                    has_unread = true;
                } else if label.eq_ignore_ascii_case("Starred") {
                    flags.insert("\\Flagged".to_owned());
                } else if label.eq_ignore_ascii_case("Inbox") {
                    mapped_labels.insert("\\Inbox".to_owned());
                } else if label.eq_ignore_ascii_case("Sent") {
                    mapped_labels.insert("\\Sent".to_owned());
                } else if label.eq_ignore_ascii_case("Important") {
                    mapped_labels.insert("\\Important".to_owned());
                } else if !label.eq_ignore_ascii_case("Opened")
                    && !label.eq_ignore_ascii_case("Archived")
                {
                    mapped_labels.insert(label.clone());
                }
            }
            if !labels.is_empty() && !has_unread {
                flags.insert("\\Seen".to_owned());
            }
            let internal_date = headers
                .date
                .clone()
                .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_owned());
            let gmail_message_id = if is_gmail {
                let digest = Sha256::digest(&normalized);
                let id_num = u64::from_be_bytes(digest[0..8].try_into().unwrap());
                Some(id_num.to_string())
            } else {
                None
            };
            records.push(MessageRecord {
                delivery: DeliveryEvidence::Unavailable,
                uid: *uid,
                size,
                internal_date,
                flags_observed: !labels.is_empty(),
                flags,
                gmail_message_id,
                gmail_thread_id: None,
                gmail_labels: mapped_labels,
                headers,
            });
        }
        Err(_) => {
            *skipped += 1;
        }
    }
    buf.clear();
}

fn parse_mbox_headers(
    path: &Path,
    is_gmail: bool,
) -> Result<(Vec<MessageRecord>, usize, bool), AcquisitionError> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| AcquisitionError::InvalidInput)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(AcquisitionError::InvalidInput);
    }
    let file = File::open(path).map_err(|_| AcquisitionError::Storage)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut records = Vec::new();
    let mut skipped = 0usize;
    let mut truncated = false;
    let mut saw_any_message = false;

    let mut in_headers = false;
    let mut header_buf: Vec<u8> = Vec::with_capacity(4096);
    let mut header_overflow = false;
    let mut msg_bytes: u64 = 0;
    let mut prev_blank = true;
    let mut uid: u32 = 0;

    let mut line = Vec::with_capacity(512);
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| AcquisitionError::Storage)?;
        if read == 0 {
            break;
        }
        if line.len() > MAX_LINE_BYTES {
            if in_headers {
                header_overflow = true;
            }
            msg_bytes = msg_bytes.saturating_add(read as u64);
            prev_blank = false;
            continue;
        }
        if prev_blank && is_from_separator(&line) {
            if saw_any_message {
                if in_headers {
                    flush_header_block(
                        &mut header_buf,
                        header_overflow,
                        msg_bytes,
                        is_gmail,
                        &mut uid,
                        &mut records,
                        &mut skipped,
                    );
                } else if let Some(last) = records.last_mut() {
                    last.size = msg_bytes;
                }
            }
            if records.len() >= MAX_MESSAGES {
                truncated = true;
                break;
            }
            saw_any_message = true;
            in_headers = true;
            header_overflow = false;
            header_buf.clear();
            msg_bytes = 0;
            prev_blank = false;
            continue;
        }
        if !saw_any_message {
            if line == b"\n" || line == b"\r\n" {
                prev_blank = true;
                continue;
            }
            return Err(AcquisitionError::InvalidInput);
        }
        let blank = line == b"\n" || line == b"\r\n";
        msg_bytes = msg_bytes.saturating_add(read as u64);
        if in_headers {
            if blank {
                in_headers = false;
                flush_header_block(
                    &mut header_buf,
                    header_overflow,
                    msg_bytes,
                    is_gmail,
                    &mut uid,
                    &mut records,
                    &mut skipped,
                );
            } else if !header_overflow {
                if header_buf.len() + line.len() > MAX_HEADER_BYTES {
                    header_overflow = true;
                } else {
                    header_buf.extend_from_slice(&line);
                }
            }
        }
        prev_blank = blank;
    }
    if saw_any_message {
        if in_headers {
            flush_header_block(
                &mut header_buf,
                header_overflow,
                msg_bytes,
                is_gmail,
                &mut uid,
                &mut records,
                &mut skipped,
            );
        } else if let Some(last) = records.last_mut() {
            last.size = msg_bytes;
        }
    }
    if !saw_any_message {
        return Err(AcquisitionError::InvalidInput);
    }
    Ok((records, skipped, truncated))
}

impl Accounts {
    /// Imports message headers from a local Google Takeout `.mbox` file into the account's
    /// encrypted census store. Message bodies are never stored.
    pub(crate) fn import_takeout(
        &self,
        id: &AccountId,
        mbox_path: &Path,
    ) -> Result<ImportSummary, AcquisitionError> {
        let account = self.load(id)?;
        let store = self.store(&account)?;
        if store.recovery_pending() {
            return Err(AcquisitionError::LifecyclePending);
        }
        let is_gmail = account.provider == Provider::Gmail;
        let (records, skipped, truncated) = parse_mbox_headers(mbox_path, is_gmail)?;
        let mut saved = store.load_state()?;
        let folder_name = if is_gmail {
            "[Gmail]/All Mail".to_owned()
        } else {
            "Takeout".to_owned()
        };
        let mut live_uids = BTreeSet::new();
        let mut all_labels = BTreeSet::new();
        for record in &records {
            live_uids.insert(record.uid);
            for label in &record.gmail_labels {
                all_labels.insert(label.clone());
            }
        }
        let uidvalidity = saved
            .census
            .folders
            .get(&folder_name)
            .map(|f| f.uidvalidity.saturating_add(1).max(1))
            .unwrap_or(1);
        if !live_uids.is_empty() {
            saved
                .census
                .reconcile_folder(&folder_name, uidvalidity, live_uids)?;
            for record in &records {
                if let Some(folder) = saved.census.folders.get_mut(&folder_name) {
                    folder.insert(record.clone())?;
                }
            }
        }
        let mut roles = BTreeSet::new();
        if is_gmail {
            roles.insert("all".to_owned());
            saved
                .census
                .folder_roles
                .insert(folder_name.clone(), roles.clone());
        }
        let imported = records.len();
        let now = now_secs();
        let attempt_id = format!("{:032x}", rand::random::<u128>());
        let caps = BTreeSet::from(["TAKEOUT-MBOX".to_owned()]);
        let snapshot = Snapshot {
            version: 1,
            adapter: TAKEOUT_ADAPTER.to_owned(),
            account: account.id.clone(),
            provider: account.provider,
            attempt_id: attempt_id.clone(),
            topology_observed_at: now,
            completed_at: Some(now),
            imap_capabilities: caps.clone(),
            folders: vec![FolderObservation {
                wire: folder_name.clone(),
                name: folder_name.clone(),
                delimiter: Some("/".to_owned()),
                attributes: if is_gmail {
                    BTreeSet::from(["\\All".to_owned()])
                } else {
                    BTreeSet::new()
                },
                roles,
                selectable: true,
                selection: Selection::Selected,
                count: CountObservation::Observed {
                    messages: imported.min(u32::MAX as usize) as u32,
                    observed_at: now,
                    source: CountSource::ExamineSearch,
                },
                uidvalidity: Some(uidvalidity),
                selected_complete: true,
            }],
        };
        let attempt = Attempt {
            id: attempt_id,
            adapter: TAKEOUT_ADAPTER.to_owned(),
            provider: account.provider,
            started_at: now,
            finished_at: Some(now),
            state: ScanState::Complete,
            session: Some(SessionObservation {
                observed_at: now,
                imap_capabilities: caps,
            }),
            checkpoint: Some(snapshot.clone()),
        };
        saved.census.acquisition_state = ScanState::Complete;
        saved.census.acquisition.latest = Some(attempt);
        saved.census.acquisition.last_successful = Some(snapshot);
        store.save_state(&mut saved)?;
        Ok(ImportSummary {
            imported,
            skipped,
            folders: saved.census.folders.len(),
            labels: all_labels.len(),
            folder: folder_name,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acquisition::{SecretValue, persistent_test_accounts};

    #[test]
    fn imports_takeout_mbox_headers_without_storing_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let mbox = dir.path().join("takeout.mbox");
        std::fs::write(
            &mbox,
            concat!(
                "From 123456@xxx Sat Nov 21 00:00:00 +0000 2026\n",
                "X-Gmail-Labels: Inbox,Unread,Category Updates\n",
                "From: Billing <billing@hetzner.com>\n",
                "To: Owner <owner@example.test>\n",
                "Subject: Invoice 2026-11\n",
                "Date: Sat, 21 Nov 2026 00:00:00 +0000\n",
                "Message-ID: <inv-1@hetzner.com>\n",
                "\n",
                "SECRET BODY CANARY THAT MUST NEVER BE STORED\n",
                "\n",
                "From 789012@xxx Sat Nov 21 01:00:00 +0000 2026\n",
                "X-Gmail-Labels: Sent,Starred\n",
                "From: Owner <owner@example.test>\n",
                "To: Friend <friend@example.net>\n",
                "Subject: Re: Hello\n",
                "Date: Sat, 21 Nov 2026 01:00:00 +0000\n",
                "Message-ID: <sent-1@example.test>\n",
                "\n",
                "ANOTHER BODY CANARY\n",
            ),
        )
        .unwrap();
        let accounts = persistent_test_accounts(dir.path());
        let account = accounts
            .create(
                Provider::Gmail,
                "owner@example.test".into(),
                &SecretValue::new(b"test-pass".to_vec()),
            )
            .unwrap();
        let summary = accounts.import_takeout(&account.id, &mbox).unwrap();
        assert_eq!(summary.imported, 2);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.folder, "[Gmail]/All Mail");
        let store = accounts.store(&account).unwrap();
        let saved = store.load().unwrap();
        let folder = &saved.folders["[Gmail]/All Mail"];
        assert_eq!(folder.messages.len(), 2);
        let first = &folder.messages[&1];
        assert_eq!(first.headers.subject, "Invoice 2026-11");
        assert!(first.gmail_labels.contains("\\Inbox"));
        assert!(!first.flags.contains("\\Seen"));
        let second = &folder.messages[&2];
        assert_eq!(second.headers.direction, Direction::Sent);
        assert!(second.flags.contains("\\Flagged"));
        assert!(second.flags.contains("\\Seen"));
        let serialized = serde_json::to_string(&saved).unwrap();
        assert!(!serialized.contains("SECRET BODY CANARY"));
        assert!(!serialized.contains("ANOTHER BODY CANARY"));
    }
}
