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
        };
        validate_record(&record, transaction, &transaction.context.account)?;
        self.records.push(record);
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
        || serde_json::to_vec(record)
            .map_err(|_| CensusError::Storage)?
            .len()
            > 512 * 1024
    {
        return Err(CensusError::Storage);
    }
    Ok(())
}
