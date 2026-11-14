use super::{
    Proposal, digest,
    files::{self, Exchange},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    path::{Path, PathBuf},
};

const ERROR: &str = "Assistant request unavailable";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Summary {
    pub as_of: i64,
    pub version: String,
    #[serde(default)]
    pub approved: BTreeSet<String>,
    pub receipt: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum Launch {
    Prepared,
    Launching,
    Launched {
        thread_id: String,
        thread_link: Option<String>,
    },
    Failed {
        error: String,
        uncertain: bool,
    },
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Record {
    pub id: String,
    pub summary: Summary,
    pub summary_sha256: String,
    pub launch: Launch,
}

#[derive(Serialize)]
pub(crate) struct ProposalView {
    id: String,
    proposal: Proposal,
    dismissed: bool,
}

#[derive(Serialize)]
pub(crate) struct View {
    pub id: String,
    summary: Summary,
    summary_sha256: String,
    phase: &'static str,
    thread_id: Option<String>,
    thread_link: Option<String>,
    error: Option<String>,
    proposals: Vec<ProposalView>,
}

pub(crate) fn identifier(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) struct Inbox {
    root: File,
    path: PathBuf,
}

impl Inbox {
    pub(crate) fn open(data: &Path, account: &str) -> Result<Self, &'static str> {
        let root = files::root(data)?;
        let assistant = files::directory(&root, "assistant", true)?;
        let root = files::directory(&assistant, account, true)?;
        Ok(Self {
            root,
            path: data.join("assistant").join(account),
        })
    }

    fn request(&self, id: &str) -> Result<File, &'static str> {
        if !identifier(id) {
            return Err(ERROR);
        }
        files::directory(&self.root, id, false)
    }

    pub(crate) fn exchange_path(&self, id: &str) -> PathBuf {
        self.path.join(id).join("exchange")
    }

    pub(crate) fn prepare(
        &self,
        summary: Summary,
        text: &str,
        executable: &Path,
    ) -> Result<String, &'static str> {
        let summary_sha256 = digest(text.as_bytes());
        let id = digest(&serde_json::to_vec(&(&summary, &summary_sha256)).map_err(|_| ERROR)?);
        if self.request(&id).is_err() && self.records()?.len() >= 100 {
            return Err("Assistant inbox is full");
        }
        let root = files::directory(&self.root, &id, true)?;
        files::directory(&root, "dismissals", true)?;
        let exchange = Exchange::initialize(&self.exchange_path(&id))?;
        exchange.publish(text, executable)?;
        let record = Record {
            id: id.clone(),
            summary,
            summary_sha256,
            launch: Launch::Prepared,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| ERROR)?;
        if files::read(&root, "request.json", 32768).is_err() {
            files::write_once(&root, "request.json", &bytes)?;
        }
        self.record(&id)?;
        Ok(id)
    }

    pub(crate) fn record(&self, id: &str) -> Result<Record, &'static str> {
        let bytes = files::read(&self.request(id)?, "request.json", 32768)?;
        let record: Record = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
        if record.id != id {
            return Err(ERROR);
        }
        Ok(record)
    }

    pub(crate) fn save(&self, record: &Record) -> Result<(), &'static str> {
        files::replace(
            &self.request(&record.id)?,
            "request.json",
            &serde_json::to_vec(record).map_err(|_| ERROR)?,
        )
    }

    pub(crate) fn launch_lock(
        &self,
        id: &str,
    ) -> Result<crate::file_lock::ExclusiveFileLock, &'static str> {
        let root = self.request(id)?;
        files::write_once(&root, "launch.lock", b"")?;
        let path = self.path.join(id).join("launch.lock");
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| ERROR)?;
        crate::file_lock::ExclusiveFileLock::try_lock(file)
            .map_err(|_| "Assistant launch already running")
    }

    pub(crate) fn dismiss(&self, request: &str, proposal: &str) -> Result<(), &'static str> {
        if !identifier(proposal) {
            return Err(ERROR);
        }
        let root = self.request(request)?;
        let dismissals = files::directory(&root, "dismissals", true)?;
        files::write_once(&dismissals, proposal, b"dismissed")
    }

    pub(crate) fn records(&self) -> Result<Vec<Record>, &'static str> {
        let mut records = Vec::new();
        for entry in std::fs::read_dir(&self.path).map_err(|_| ERROR)?.take(101) {
            let name = entry.map_err(|_| ERROR)?.file_name();
            let Some(id) = name.to_str().filter(|id| identifier(id)) else {
                continue;
            };
            records.push(self.record(id)?);
            if records.len() > 100 {
                return Err("Assistant inbox is full");
            }
        }
        records.sort_by(|a, b| b.summary.as_of.cmp(&a.summary.as_of).then(a.id.cmp(&b.id)));
        Ok(records)
    }

    pub(crate) fn views(&self) -> Result<Vec<View>, &'static str> {
        self.records()?
            .into_iter()
            .map(|record| {
                let exchange = Exchange::open(&self.exchange_path(&record.id))?;
                let dismissals = files::directory(&self.request(&record.id)?, "dismissals", true)?;
                let proposals: Vec<_> = exchange
                    .proposals()?
                    .into_iter()
                    .map(|(id, proposal)| {
                        let dismissed =
                            files::read(&dismissals, &id, 9).is_ok_and(|b| b == b"dismissed");
                        ProposalView {
                            id,
                            proposal,
                            dismissed,
                        }
                    })
                    .collect();
                let (mut phase, thread_id, thread_link, error) = match record.launch {
                    Launch::Prepared => ("prepared", None, None, None),
                    Launch::Launching => ("launching", None, None, None),
                    Launch::Launched {
                        thread_id,
                        thread_link,
                    } => ("launched", Some(thread_id), thread_link, None),
                    Launch::Failed { error, .. } => ("failed", None, None, Some(error)),
                };
                if exchange.was_read() {
                    phase = "connected";
                }
                if proposals.iter().any(|p| !p.dismissed) {
                    phase = "proposal_ready";
                }
                Ok(View {
                    id: record.id,
                    summary: record.summary,
                    summary_sha256: record.summary_sha256,
                    phase,
                    thread_id,
                    thread_link,
                    error,
                    proposals,
                })
            })
            .collect()
    }
}
