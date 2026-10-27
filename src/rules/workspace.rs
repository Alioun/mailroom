use super::{
    CompiledArtifact, Engine, EngineContext,
    program::SEMANTICS,
    proof::{self, Baseline, Pin, Proof},
};
use crate::{
    acquisition::Provider,
    census::{AccountState, CensusKeys},
    engine::{self, Documents},
};
use serde::{Deserialize, Serialize};

pub const HISTORY_LIMIT: usize = 8;
/// v2 binds the census by its digest (`census::binding`).
const RECEIPT_VERSION: &str = "mailroom/local-sieve-download/v2";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInput {
    pub documents: Documents,
    pub pins: Vec<Pin>,
    pub as_of: i64,
    pub baseline: Baseline,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub revision: u64,
    pub input: WorkspaceInput,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_binding: Option<String>,
    pub semantics: String,
    pub proof_semantics: String,
    #[serde(with = "super::program::persisted")]
    pub artifact: CompiledArtifact,
    pub workspace: Workspace,
    pub receipt: String,
    pub exported_at: u64,
    pub provenance: String,
}
#[derive(Serialize)]
pub struct ExportInfo {
    pub identity: String,
    pub semantics: String,
    pub proof_semantics: String,
    pub exported_at: u64,
    pub provenance: String,
    pub usable: bool,
}
#[derive(Serialize)]
pub struct WorkspaceView {
    pub revision: u64,
    pub workspace: Option<Workspace>,
    pub exports: Vec<ExportInfo>,
    pub engine: Engine,
}
#[derive(Serialize)]
pub struct ProofPreview {
    pub proof: Proof,
    pub receipt: String,
    pub revision: u64,
    pub workspace_revision: u64,
    pub as_of: i64,
    pub download_available: bool,
    pub previous: Option<ExportInfo>,
    pub filename: &'static str,
}
/// Resolved by the account repository, never accepted as request input.
#[derive(Clone, Serialize)]
pub struct WorkspaceContext {
    provider: Provider,
    account: Option<crate::census::AccountId>,
    engine: EngineContext,
    binding: String,
}
impl WorkspaceContext {
    pub fn local(provider: Provider) -> Self {
        Self {
            provider,
            account: None,
            engine: EngineContext::local(
                String::new(),
                match provider {
                    Provider::MailboxOrg => Engine::MailboxSieve,
                    Provider::Gmail => Engine::GmailBacklog,
                },
            ),
            binding: "uninstalled-local-v1".into(),
        }
    }
    pub fn provider(&self) -> Provider {
        self.provider
    }
    pub(crate) fn bound(
        account: crate::census::AccountId,
        provider: Provider,
        engine: EngineContext,
        binding: String,
    ) -> Self {
        Self {
            provider,
            account: Some(account),
            engine,
            binding,
        }
    }
    pub(crate) fn binding(&self) -> &str {
        &self.binding
    }
    pub(crate) fn engine(
        &self,
        account: &crate::census::AccountId,
    ) -> Result<EngineContext, String> {
        if self.account.as_ref().is_some_and(|id| id != account) {
            return Err("Workspace context belongs to another account".into());
        }
        let mut engine = self.engine.clone();
        engine.account = account.as_str().into();
        Ok(engine)
    }
}
impl ExportRecord {
    fn info(&self, context: &WorkspaceContext, account: &crate::census::AccountId) -> ExportInfo {
        ExportInfo {
            identity: self.artifact.identity().into(),
            semantics: self.semantics.clone(),
            proof_semantics: self.proof_semantics.clone(),
            exported_at: self.exported_at,
            provenance: self.provenance.clone(),
            usable: self.semantics == SEMANTICS
                && self.proof_semantics == proof::SEMANTICS
                && self.context_binding.as_deref() == Some(context.binding())
                && context
                    .engine(account)
                    .is_ok_and(|c| *self.artifact.context() == c),
        }
    }
}
pub fn view(state: &AccountState, context: &WorkspaceContext) -> WorkspaceView {
    WorkspaceView {
        revision: state.revision,
        workspace: state.workspace.clone(),
        exports: state
            .exports
            .iter()
            .map(|e| e.info(context, &state.census.account))
            .collect(),
        engine: context.engine.engine.clone(),
    }
}
fn compile_input(
    input: &WorkspaceInput,
    state: &AccountState,
    context: &WorkspaceContext,
) -> Result<CompiledArtifact, String> {
    let account = state.census.account.as_str();
    if input.pins.iter().any(|p| p.account != account) {
        return Err("Pins belong to the selected account".into());
    }
    let engine = context.engine(&state.census.account)?;
    engine::artifact(&input.documents, &engine)
}

pub fn validate(
    input: &WorkspaceInput,
    state: &AccountState,
    context: &WorkspaceContext,
) -> Result<(), String> {
    let artifact = compile_input(input, state, context)?;
    // Saving is allowed before an export exists. Previous-baseline availability is checked for proof.
    proof::prove(
        artifact,
        &state.census,
        input.as_of,
        &input.pins,
        Baseline::Current,
        None,
    )?;
    Ok(())
}
pub fn preview(
    state: &AccountState,
    context: &WorkspaceContext,
    keys: &CensusKeys,
) -> Result<ProofPreview, String> {
    let workspace = state
        .workspace
        .as_ref()
        .ok_or("Save the account's rule workspace first")?;
    let input = &workspace.input;
    let last = state.exports.last();
    let previous = last.map(|e| e.info(context, &state.census.account));
    let baseline = if input.baseline == Baseline::Previous {
        let record = last.ok_or("No recorded local export baseline")?;
        if !record.info(context, &state.census.account).usable {
            return Err("Recorded export uses obsolete semantics or another engine".into());
        }
        Some(&record.artifact)
    } else {
        None
    };
    let artifact = compile_input(input, state, context)?;
    let proof = proof::prove_shared(
        artifact,
        &state.census,
        input.as_of,
        &input.pins,
        input.baseline,
        baseline,
    )?;
    let payload = zeroize::Zeroizing::new(
        crate::census::binding::bound(|| {
            serde_json::to_vec(&(
                RECEIPT_VERSION,
                SEMANTICS,
                context,
                state,
                &proof,
                proof.artifact.executable().as_bytes(),
                "rules.sieve",
            ))
        })
        .map_err(|_| "Cannot bind proof")?,
    );
    let receipt = keys.binding_for(RECEIPT_VERSION.as_bytes(), &payload);
    let download_available = context.provider == Provider::MailboxOrg && proof.gate != "blocked";
    Ok(ProofPreview {
        proof,
        receipt,
        revision: state.revision,
        workspace_revision: workspace.revision,
        as_of: input.as_of,
        download_available,
        previous,
        filename: "rules.sieve",
    })
}
pub fn confirm_export(
    state: &mut AccountState,
    context: &WorkspaceContext,
    keys: &CensusKeys,
    receipt: &str,
) -> Result<ProofPreview, String> {
    use subtle::ConstantTimeEq;
    let preview = preview(state, context, keys)?;
    if !preview.download_available
        || !bool::from(preview.receipt.as_bytes().ct_eq(receipt.as_bytes()))
    {
        return Err("Proof changed or download is blocked. Run and review a fresh proof".into());
    }
    state.exports.push(ExportRecord {
        context_binding: Some(context.binding.clone()),
        semantics: SEMANTICS.into(),
        proof_semantics: proof::SEMANTICS.into(),
        artifact: preview.proof.artifact.clone(),
        workspace: state.workspace.as_ref().ok_or("Missing workspace")?.clone(),
        receipt: preview.receipt.clone(),
        exported_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "Invalid clock")?
            .as_secs(),
        provenance: "Owner-confirmed local download prepared. Provider application is unverified"
            .into(),
    });
    if state.exports.len() > HISTORY_LIMIT {
        state.exports.remove(0);
    }
    Ok(preview)
}
