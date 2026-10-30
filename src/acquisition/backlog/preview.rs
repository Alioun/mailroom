use super::*;
use crate::backlog::imap_copy::Readback;

pub(super) struct Evidence {
    pub saved_proof: workspace::ProofPreview,
    pub binding: Binding,
    pub topology: Inventory,
    pub observed: Census,
    pub profile: Profile,
}

pub(super) fn prepare(
    saved: &AccountState,
    store: &EncryptedStore,
    options: Options,
    evidence: Evidence,
    persistent: impl FnMut(&MoveIntent) -> Result<Readback, AcquisitionError>,
) -> Result<Preview, AcquisitionError> {
    let Evidence {
        saved_proof,
        binding,
        topology,
        observed,
        profile,
    } = evidence;
    let input = &saved.workspace.as_ref().ok_or_else(invalid)?.input;
    let baseline = if input.baseline == proof::Baseline::Previous {
        Some(&saved.exports.last().ok_or_else(invalid)?.artifact)
    } else {
        None
    };
    let execution = proof::prove(
        saved_proof.proof.artifact.clone(),
        &observed,
        input.as_of,
        &input.pins,
        input.baseline,
        baseline,
    )
    .map_err(failure)?;
    if execution.gate == "blocked" {
        return Err(invalid());
    }
    let filing =
        model::project(saved, &observed, &execution, options, profile.method()).map_err(failure)?;
    #[cfg(test)]
    if let Some(root) = std::env::var_os("MAILROOM_MOVE_FIXTURE") {
        std::fs::write(
            std::path::Path::new(&root).join("move-preview-debug.json"),
            serde_json::to_vec(
                &serde_json::json!({"topology":topology,"execution":execution,"filing":filing}),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let batches = batches::prepare(&filing, &topology, &observed, profile, persistent)?;
    let plan = SavedPlan {
        filing,
        binding,
        topology,
        saved_proof: saved_proof.receipt.clone(),
        execution_proof: store.keys().binding_for(
            b"mailroom/imap-execution-proof/v1",
            &serde_json::to_vec(&(&observed, &execution)).map_err(failure)?,
        ),
    };
    let receipt = start_receipt(store, saved, &plan, &batches)?;
    let id = store
        .keys()
        .binding_for(b"mailroom/imap-backlog-id/v1", receipt.as_bytes());
    let operation = Run {
        id,
        receipt: receipt.clone(),
        plan,
        status: Status::Prepared,
        stop_requested: false,
        batches,
        reverse: vec![],
        compensation: vec![],
        undo_receipt: None,
        undo_conflicts: 0,
        completed_at: None,
        closed: None,
    };
    Ok(Preview {
        operation,
        receipt,
        saved: saved_proof,
        execution,
        limitations: if profile == Profile::Copy {
            model::COPY_PREVIEW_LIMITATIONS
        } else {
            model::LIMITATIONS
        },
    })
}

#[cfg(test)]
mod tests;
