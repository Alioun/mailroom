use super::{composition, program, proof};
use crate::census::{AccountState, ScanState, sieve_transaction};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
mod window;
pub use window::{ArrivalPeriod, ArrivalWindow};

pub const SEMANTICS: &str = "mailroom/recorded-placement-comparison/v1";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineSource {
    RetainedDeployment,
    LegacyExport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    NoRecordedDeployment,
    TransactionUnsettled,
    NoActiveComposedDeployment,
    HistoricalSemantics,
    ArtifactNotRetained,
    RecordedBytesDiffer,
    NoCompletedRefresh,
    EvaluationUnavailable,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Report {
    Unavailable {
        reason: Unavailable,
    },
    Compared {
        semantics: &'static str,
        account: String,
        census_sha256: String,
        refreshed_at: i64,
        scan_state: ScanState,
        complete: bool,
        historical: bool,
        transaction: String,
        artifact: String,
        baseline_source: BaselineSource,
        target: String,
        counts: Counts,
        arrival_window: ArrivalWindow,
        rows: Vec<Row>,
    },
}

#[derive(Default, Serialize)]
pub struct Counts {
    pub matches: usize,
    pub elsewhere: usize,
    pub additional_folders: usize,
    pub uncheckable: usize,
}

#[derive(Serialize)]
pub struct Row {
    pub id: String,
    pub subject: String,
    pub from: String,
    pub observed: BTreeSet<String>,
    pub prediction: Prediction,
    pub arrival_period: ArrivalPeriod,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Mailroom,
    ExistingRule,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Matches,
    Elsewhere,
    AdditionalFolders,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Uncheckable {
    RecipientEvidence,
    ExistingRuleUnknown,
    NotAFolderDestination,
    NoObservedFolder,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Prediction {
    Folder {
        destination: proof::Destination,
        origin: Origin,
        comparison: Comparison,
    },
    Uncheckable {
        reason: Uncheckable,
    },
}

fn unavailable(reason: Unavailable) -> Report {
    Report::Unavailable { reason }
}

/// Compares recorded snapshots. Neither a matching folder nor a saved script proves delivery.
pub fn compare(saved: &AccountState, historical: bool) -> Report {
    use sieve_transaction::{Outcome, Purpose, Status};
    let Some(transaction) = saved.sieve_transactions.history.last() else {
        return unavailable(Unavailable::NoRecordedDeployment);
    };
    if transaction.status != Status::Complete
        || transaction
            .steps
            .last()
            .is_none_or(|s| s.outcome != Outcome::Observed || s.after != transaction.after)
    {
        return unavailable(Unavailable::TransactionUnsettled);
    }
    let Some(recorded) = transaction.composition.as_ref().filter(|c| {
        c.purpose == Purpose::Deploy
            && transaction.compensates.is_none()
            && transaction.after.active() == Some(transaction.target.as_str())
    }) else {
        return unavailable(Unavailable::NoActiveComposedDeployment);
    };
    if recorded.semantics != composition::SEMANTICS {
        return unavailable(Unavailable::HistoricalSemantics);
    }
    let (artifact, baseline_source) = if let Some(retained) =
        saved.deployment_artifacts.get(&transaction.id)
    {
        if retained.semantics != program::SEMANTICS || retained.proof_semantics != proof::SEMANTICS
        {
            return unavailable(Unavailable::HistoricalSemantics);
        }
        (&retained.artifact, BaselineSource::RetainedDeployment)
    } else if let Some(export) = saved.exports.iter().rev().find(|e| {
        e.artifact.identity() == transaction.artifact
            && e.artifact.context().account == saved.census.account.as_str()
            && e.artifact.context().engine == super::Engine::MailboxSieve
            && e.context_binding.is_some()
            && e.semantics == program::SEMANTICS
            && e.proof_semantics == proof::SEMANTICS
    }) {
        (&export.artifact, BaselineSource::LegacyExport)
    } else {
        return unavailable(Unavailable::ArtifactNotRetained);
    };
    let Some(host) = transaction.after.body(&transaction.target) else {
        return unavailable(Unavailable::RecordedBytesDiffer);
    };
    let Some(placement) = &recorded.placement else {
        return unavailable(Unavailable::RecordedBytesDiffer);
    };
    let Ok((foreign, _)) = composition::strip(&host.bytes, &transaction.target) else {
        return unavailable(Unavailable::RecordedBytesDiffer);
    };
    let Ok(script) = composition::parse_foreign(&foreign, &transaction.target) else {
        return unavailable(Unavailable::EvaluationUnavailable);
    };
    let Ok(composed) = composition::compose(
        &foreign,
        &script,
        placement,
        artifact.executable(),
        &transaction.target,
    ) else {
        return unavailable(Unavailable::RecordedBytesDiffer);
    };
    if composed.host != host.bytes
        || composed.owned.as_ref().is_some_and(|(name, bytes)| {
            transaction
                .after
                .body(name)
                .is_none_or(|b| b.bytes != *bytes)
        })
    {
        return unavailable(Unavailable::RecordedBytesDiffer);
    }
    let Some(refreshed_at) = saved.census.last_completed_at else {
        return unavailable(Unavailable::NoCompletedRefresh);
    };
    let Ok(proof) = proof::prove_shared(
        artifact.clone(),
        &saved.census,
        refreshed_at,
        &[],
        proof::Baseline::Current,
        None,
    ) else {
        return unavailable(Unavailable::EvaluationUnavailable);
    };
    let Ok(precedence) = composition::precedence(
        &proof,
        &saved.census,
        &script,
        &transaction.target,
        placement,
    ) else {
        return unavailable(Unavailable::EvaluationUnavailable);
    };
    let existing: BTreeMap<_, _> = precedence
        .decisions
        .iter()
        .map(|d| (d.id.as_str(), d))
        .collect();
    let mut counts = Counts::default();
    let observation = saved
        .deployment_artifacts
        .get(&transaction.id)
        .and_then(|r| r.active_observation);
    let periods = window::periods(&saved.census, observation, refreshed_at);
    let rows = proof
        .decisions
        .into_iter()
        .map(|decision| {
            let predicted = match existing.get(decision.id.as_str()) {
                Some(taken) => match &taken.existing {
                    Some(composition::ForeignDest::Inbox) => {
                        Ok((proof::Destination::Inbox, Origin::ExistingRule))
                    }
                    Some(composition::ForeignDest::Folder { name }) => Ok((
                        proof::Destination::Folder { name: name.clone() },
                        Origin::ExistingRule,
                    )),
                    Some(_) => Err(Uncheckable::NotAFolderDestination),
                    None => Err(Uncheckable::ExistingRuleUnknown),
                },
                None if decision.review => Err(Uncheckable::RecipientEvidence),
                None => Ok((decision.destination, Origin::Mailroom)),
            };
            let prediction = match predicted {
                _ if decision.current_folders.is_empty() => {
                    counts.uncheckable += 1;
                    Prediction::Uncheckable {
                        reason: Uncheckable::NoObservedFolder,
                    }
                }
                Err(reason) => {
                    counts.uncheckable += 1;
                    Prediction::Uncheckable { reason }
                }
                Ok((destination, origin)) => {
                    let found = decision
                        .current_folders
                        .iter()
                        .any(|name| match &destination {
                            proof::Destination::Inbox => name.eq_ignore_ascii_case("INBOX"),
                            proof::Destination::Folder { name: expected } => name == expected,
                        });
                    let comparison = if !found {
                        counts.elsewhere += 1;
                        Comparison::Elsewhere
                    } else if decision.current_folders.len() > 1 {
                        counts.additional_folders += 1;
                        Comparison::AdditionalFolders
                    } else {
                        counts.matches += 1;
                        Comparison::Matches
                    };
                    Prediction::Folder {
                        destination,
                        origin,
                        comparison,
                    }
                }
            };
            Row {
                arrival_period: periods
                    .get(&decision.id)
                    .copied()
                    .unwrap_or(ArrivalPeriod::Unavailable),
                id: decision.id,
                subject: decision.subject,
                from: decision.from,
                observed: decision.current_folders,
                prediction,
            }
        })
        .collect::<Vec<_>>();
    let arrival_window = window::summarize(observation, refreshed_at, &rows);
    Report::Compared {
        semantics: SEMANTICS,
        account: saved.census.account.as_str().to_owned(),
        census_sha256: data_encoding::HEXLOWER.encode(&saved.census.digest()),
        refreshed_at,
        scan_state: saved.census.acquisition_state,
        complete: saved.census.acquisition_state == ScanState::Complete
            && saved
                .census
                .folders
                .values()
                .all(|f| f.state == ScanState::Complete),
        historical,
        transaction: transaction.id.clone(),
        artifact: transaction.artifact.clone(),
        baseline_source,
        target: transaction.target.clone(),
        counts,
        arrival_window,
        rows,
    }
}

#[cfg(test)]
pub(crate) mod tests;
