use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ConfirmRequest {
    account: AccountId,
    address: String,
    confirmed: bool,
}

pub(in crate::server) async fn open(
    State(state): State<Arc<AppState>>,
    body: Result<Json<AccountRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    census_action(state, request.account, None).await
}

pub(in crate::server) async fn confirm(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ConfirmRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    census_action(
        state,
        request.account,
        Some((request.address, request.confirmed)),
    )
    .await
}

#[derive(Serialize)]
struct CensusView {
    census: Census,
    evidence: crate::census::acquisition::EvidenceView,
    addresses: Vec<crate::census::AddressCandidate>,
    landing: crate::rules::landing::Report,
}

async fn census_action(
    state: Arc<AppState>,
    id: AccountId,
    confirm: Option<(String, bool)>,
) -> Response {
    if state.accounts.job_running() {
        return failure(AcquisitionError::InvalidInput);
    }
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        let repo = state.accounts.repository()?;
        let account = repo.load(&id)?;
        let store = repo.store(&account)?;
        if state.accounts.synthetic() && !repo.installed_test_transport() {
            let mut saved = store.load_state()?;
            if crate::invoice::initialize_synthetic(&mut saved) {
                store.save_state(&mut saved)?;
            }
        }
        let mut saved = store.load_state()?;
        if let Some((address, confirmed)) = confirm {
            saved.census.confirm_address(&address, confirmed)?;
            store.save_state(&mut saved)?;
        }
        let historical = store.recovery_pending();
        let landing = crate::rules::landing::compare(&saved, historical);
        let census = saved.census.into_inner();
        let mut evidence = census.acquisition.view(crate::census::acquisition::now());
        if historical {
            evidence.historical();
        }
        Ok::<_, AcquisitionError>(CensusView {
            evidence,
            landing,
            addresses: census.own_address_candidates(),
            census,
        })
    })
    .await
    {
        Ok(Ok(view)) => Json(view).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ReportRequest {
    account: AccountId,
    as_of: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ReviewRequest {
    account: AccountId,
    as_of: i64,
    version: String,
    approved: std::collections::BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ExportRequest {
    account: AccountId,
    as_of: i64,
    version: String,
    approved: std::collections::BTreeSet<String>,
    receipt: String,
    confirmed: bool,
}

pub(in crate::server) async fn report(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ReportRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    report_action(state, request, None, None).await
}

pub(in crate::server) async fn review_summary(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ReviewRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    report_action(
        state,
        ReportRequest {
            account: request.account.clone(),
            as_of: request.as_of,
        },
        Some(request),
        None,
    )
    .await
}

pub(in crate::server) async fn export_summary(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ExportRequest>, JsonRejection>,
) -> Response {
    let request = match input(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !request.confirmed {
        return failure(AcquisitionError::InvalidInput);
    }
    report_action(
        state,
        ReportRequest {
            account: request.account.clone(),
            as_of: request.as_of,
        },
        Some(ReviewRequest {
            account: request.account,
            as_of: request.as_of,
            version: request.version,
            approved: request.approved,
        }),
        Some(request.receipt),
    )
    .await
}

async fn report_action(
    state: Arc<AppState>,
    request: ReportRequest,
    review: Option<ReviewRequest>,
    receipt: Option<String>,
) -> Response {
    match tokio::task::spawn_blocking(move || {
        let _operation = state.accounts.operation.lock().expect("account operation");
        if state.accounts.job_running() {
            return Err(AcquisitionError::InvalidInput);
        }
        let repo = state.accounts.repository()?;
        let account = repo.load(&request.account)?;
        let store = repo.store(&account)?;
        let census = store.load()?;
        let mut report = census.report(
            store.keys(),
            request.as_of,
            account.provider == Provider::Gmail,
        )?;
        if store.recovery_pending() {
            report.acquisition.historical();
        }
        if let Some(review) = review {
            if report.version != review.version {
                return Err(AcquisitionError::InvalidInput);
            }
            let preview = report.summary_preview(store.keys(), &review.approved)?;
            if let Some(receipt) = receipt {
                use subtle::ConstantTimeEq;
                if !bool::from(preview.receipt.as_bytes().ct_eq(receipt.as_bytes())) {
                    return Err(AcquisitionError::InvalidInput);
                }
                disclosure::confirm(
                    &state,
                    &account,
                    &store,
                    &report,
                    &preview,
                    &review.approved,
                )?;
                return Ok(json!({"summary": preview.summary, "filename": "census-summary.md"}));
            }
            return Ok(json!(preview));
        }
        Ok::<_, AcquisitionError>(json!(report))
    })
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(AcquisitionError::Storage),
    }
}
