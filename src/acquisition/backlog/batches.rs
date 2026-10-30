use super::*;
use crate::backlog::{
    Plan,
    imap_copy::{CopyEvidence, Readback},
};

pub(super) fn prepare(
    filing: &Plan,
    topology: &Inventory,
    observed: &Census,
    profile: Profile,
    mut persistent: impl FnMut(&MoveIntent) -> Result<Readback, AcquisitionError>,
) -> Result<Vec<model::Batch>, AcquisitionError> {
    topology.validate().map_err(failure)?;
    profile
        .validate_capabilities(&topology.capabilities)
        .map_err(failure)?;
    if filing.account != observed.account.as_str() {
        return Err(invalid());
    }
    let mut grouped: BTreeMap<(String, String), BTreeMap<Uid, Metadata>> = BTreeMap::new();
    let mut members = BTreeSet::new();
    for c in &filing.changes {
        let folder = observed.folders.get(&c.source.folder).ok_or_else(invalid)?;
        let m = folder.messages.get(&c.source.uid).ok_or_else(invalid)?;
        if c.operation
            != (crate::backlog::Operation::Imap {
                method: profile.method(),
            })
            || folder.uidvalidity != c.source.uidvalidity
            || m.uid != c.source.uid
            || m.flags != c.flags
            || !members.insert((c.source.folder.clone(), c.source.uid))
        {
            return Err(invalid());
        }
        grouped
            .entry((c.source.folder.clone(), c.destination.clone()))
            .or_default()
            .insert(
                Uid::new(m.uid).map_err(failure)?,
                Metadata {
                    message_id_hash: c.source.message_id_hash.clone(),
                    size: u32::try_from(m.size).map_err(failure)?,
                    internal_date: m.internal_date.clone(),
                    flags: m.flags.clone(),
                },
            );
    }
    let identity = |name: &str| -> Result<MailboxIdentity, AcquisitionError> {
        Ok(MailboxIdentity {
            name: topology
                .get(name)
                .filter(|m| m.selectable())
                .ok_or_else(invalid)?
                .name
                .clone(),
            uidvalidity: UidValidity::new(
                observed.folders.get(name).ok_or_else(invalid)?.uidvalidity,
            )
            .map_err(failure)?,
        })
    };
    let mut batches = vec![];
    for ((src, dst), messages) in grouped {
        let entries: Vec<_> = messages.into_iter().collect();
        for chunk in entries.chunks(crate::backlog::BATCH_LIMIT) {
            let intent = MoveIntent::new(
                Purpose::Filing {
                    protect_flagged: filing.options.protect_flagged,
                },
                identity(&src)?,
                identity(&dst)?,
                chunk.iter().cloned().collect(),
            )
            .map_err(failure)?;
            batches.push(match profile {
                Profile::Move => MoveEvidence::intent(intent).into(),
                Profile::Copy => {
                    let before = persistent(&intent)?;
                    model::Batch::copy(CopyEvidence::new(intent, before).map_err(failure)?)
                }
            });
            if batches.len() > model::BATCHES {
                return Err(invalid());
            }
        }
    }
    Ok(batches)
}

#[cfg(test)]
mod tests;
