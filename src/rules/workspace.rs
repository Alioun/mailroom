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
const RECEIPT_VERSION: &str = "mailroom/local-sieve-download/v1";

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
fn context(account: &str, provider: Provider) -> EngineContext {
    EngineContext::local(
        account.into(),
        match provider {
            Provider::MailboxOrg => Engine::MailboxSieve,
            Provider::Gmail => Engine::GmailBacklog,
        },
    )
}
impl ExportRecord {
    fn info(&self, context: &EngineContext) -> ExportInfo {
        ExportInfo {
            identity: self.artifact.identity().into(),
            semantics: self.semantics.clone(),
            proof_semantics: self.proof_semantics.clone(),
            exported_at: self.exported_at,
            provenance: self.provenance.clone(),
            usable: self.semantics == SEMANTICS
                && self.proof_semantics == proof::SEMANTICS
                && self.artifact.context().account == context.account
                && self.artifact.context().engine == context.engine,
        }
    }
}
pub fn view(state: &AccountState, provider: Provider) -> WorkspaceView {
    let context = context(state.census.account.as_str(), provider);
    WorkspaceView {
        revision: state.revision,
        workspace: state.workspace.clone(),
        exports: state.exports.iter().map(|e| e.info(&context)).collect(),
        engine: context.engine,
    }
}
fn compile_input(
    input: &WorkspaceInput,
    state: &AccountState,
    provider: Provider,
) -> Result<CompiledArtifact, String> {
    let account = state.census.account.as_str();
    if input.pins.iter().any(|p| p.account != account) {
        return Err("Pins belong to the selected account".into());
    }
    let context = context(account, provider);
    engine::artifact(&input.documents, &context)
}

pub fn validate(
    input: &WorkspaceInput,
    state: &AccountState,
    provider: Provider,
) -> Result<(), String> {
    let artifact = compile_input(input, state, provider)?;
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
    provider: Provider,
    keys: &CensusKeys,
) -> Result<ProofPreview, String> {
    let workspace = state
        .workspace
        .as_ref()
        .ok_or("Save the account's rule workspace first")?;
    let input = &workspace.input;
    let context = context(state.census.account.as_str(), provider);
    let last = state.exports.last();
    let previous = last.map(|e| e.info(&context));
    let baseline = if input.baseline == Baseline::Previous {
        let record = last.ok_or("No recorded local export baseline")?;
        if !record.info(&context).usable {
            return Err("Recorded export uses obsolete semantics or another engine".into());
        }
        Some(&record.artifact)
    } else {
        None
    };
    let artifact = compile_input(input, state, provider)?;
    let proof = proof::prove(
        artifact,
        &state.census,
        input.as_of,
        &input.pins,
        input.baseline,
        baseline,
    )?;
    let payload = zeroize::Zeroizing::new(
        serde_json::to_vec(&(
            RECEIPT_VERSION,
            SEMANTICS,
            provider,
            state,
            &proof,
            proof.artifact.executable().as_bytes(),
            "rules.sieve",
        ))
        .map_err(|_| "Cannot bind proof")?,
    );
    let receipt = keys.binding_for(RECEIPT_VERSION.as_bytes(), &payload);
    let download_available = provider == Provider::MailboxOrg && proof.gate != "blocked";
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
    provider: Provider,
    keys: &CensusKeys,
    receipt: &str,
) -> Result<ProofPreview, String> {
    use subtle::ConstantTimeEq;
    let preview = preview(state, provider, keys)?;
    if !preview.download_available
        || !bool::from(preview.receipt.as_bytes().ct_eq(receipt.as_bytes()))
    {
        return Err("Proof changed or download is blocked. Run and review a fresh proof".into());
    }
    state.exports.push(ExportRecord {
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
