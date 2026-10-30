use super::{
    AccountId, CensusError,
    sieve_transaction::{HISTORY, Journal, Purpose, Transaction},
};
use crate::rules::{CompiledArtifact, Engine, composition, program, proof};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub transaction: String,
    pub semantics: String,
    pub proof_semantics: String,
    #[serde(with = "program::persisted")]
    pub artifact: CompiledArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_observation: Option<ActiveObservation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationVia {
    Readback,
    Reconciliation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveObservation {
    pub at: i64,
    pub via: ObservationVia,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifacts {
    records: Vec<Record>,
}

impl Artifacts {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn get(&self, transaction: &str) -> Option<&Record> {
        self.records.iter().find(|r| r.transaction == transaction)
    }

    pub(crate) fn insert(
        &mut self,
        transaction: &Transaction,
        artifact: CompiledArtifact,
    ) -> Result<(), CensusError> {
        if self.records.len() >= HISTORY || self.get(&transaction.id).is_some() {
            return Err(CensusError::Storage);
        }
        let record = Record {
            transaction: transaction.id.clone(),
            semantics: program::SEMANTICS.into(),
            proof_semantics: proof::SEMANTICS.into(),
            artifact,
            active_observation: None,
        };
        validate_record(&record, transaction, &transaction.context.account)?;
        self.records.push(record);
        Ok(())
    }

    pub(crate) fn observe_active(
        &mut self,
        transaction: &Transaction,
        observation: ActiveObservation,
    ) -> Result<(), CensusError> {
        let Some(record) = self
            .records
            .iter_mut()
            .find(|r| r.transaction == transaction.id)
        else {
            return Ok(());
        };
        if !active_complete(transaction)
            || chrono::DateTime::from_timestamp(observation.at, 0).is_none()
        {
            return Err(CensusError::Storage);
        }
        record.active_observation.get_or_insert(observation);
        Ok(())
    }

    pub(crate) fn validate(
        &self,
        account: &AccountId,
        journal: &Journal,
    ) -> Result<(), CensusError> {
        if self.records.len() > HISTORY
            || self
                .records
                .iter()
                .map(|r| &r.transaction)
                .collect::<BTreeSet<_>>()
                .len()
                != self.records.len()
        {
            return Err(CensusError::Storage);
        }
        for record in &self.records {
            let transaction = journal
                .history
                .iter()
                .find(|t| t.id == record.transaction)
                .ok_or(CensusError::Storage)?;
            validate_record(record, transaction, account)?;
        }
        Ok(())
    }
}

fn validate_record(
    record: &Record,
    transaction: &Transaction,
    account: &AccountId,
) -> Result<(), CensusError> {
    if transaction.compensates.is_some()
        || transaction
            .composition
            .as_ref()
            .is_none_or(|c| c.purpose != Purpose::Deploy || c.semantics != composition::SEMANTICS)
        || record.artifact.identity() != transaction.artifact
        || record.artifact.context().account != account.as_str()
        || record.artifact.context().engine != Engine::MailboxSieve
        || record.semantics.is_empty()
        || record.semantics.len() > 128
        || record.proof_semantics.is_empty()
        || record.proof_semantics.len() > 128
        || record.active_observation.is_some_and(|o| {
            !observed_active_image(transaction)
                || chrono::DateTime::from_timestamp(o.at, 0).is_none()
        })
        || serde_json::to_vec(record)
            .map_err(|_| CensusError::Storage)?
            .len()
            > 512 * 1024
    {
        return Err(CensusError::Storage);
    }
    Ok(())
}

fn active_complete(transaction: &Transaction) -> bool {
    transaction.status == super::sieve_transaction::Status::Complete
        && observed_active_image(transaction)
}

fn observed_active_image(transaction: &Transaction) -> bool {
    use super::sieve_transaction::Outcome;
    transaction.after.active() == Some(transaction.target.as_str())
        && transaction
            .steps
            .last()
            .is_some_and(|s| s.outcome == Outcome::Observed && s.after == transaction.after)
}

pub(crate) fn has_observation(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => {
            fields.contains_key("active_observation") || fields.values().any(has_observation)
        }
        serde_json::Value::Array(values) => values.iter().any(has_observation),
        _ => false,
    }
}
