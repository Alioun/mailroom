//! The wired production routes, end to end against loopback fakes that the
//! test-only write authorization admits. Production endpoints deny the same
//! routes at `policy::authorize`; see
//! `production_consequential_routes_deny_before_repository_or_confirmation_access`.
use super::*;
use crate::{
    acquisition::{Cancellation, OriginalLogs},
    send::{
        handoff::{Recorder, Request as Handoff},
        smtp::{
            Timeouts,
            tests::{DUMMY, Fake as Smtp, Mode},
        },
        wiring::{Endpoints, Fake},
    },
    unsubscribe::one_click::tests::{self as https, PUBLIC},
};
use std::time::Duration;
use tower::ServiceExt;

const LOGIN: &str = "books@work.example.test";
const AS_OF: i64 = 1792886400;

/// The original of synthetic census message `uid`, with a PDF part.
fn original(uid: u32) -> Vec<u8> {
    format!("From: Synthetic shop <news@shop.example>\r\nTo: owner@example.test\r\nDelivered-To: team@home.example.test\r\nSubject: Rechnung {uid}\r\nMessage-ID: <synthetic-account-{uid}@example.test>\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\n.See attachment.\r\n--b\r\nContent-Type: application/pdf; name=r.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--b--\r\n").into_bytes()
}

struct Fixture {
    root: tempfile::TempDir,
    repo: Arc<Accounts>,
    account: Account,
    imap: OriginalLogs,
}

/// A synthetic account whose census is complete, whose rules make its home
/// addresses business, and whose IMAP fake serves originals of UIDs 1 to 4.
/// The account password is the dummy secret. `provider_results` adds the
/// provider's Authentication-Results to the shop's newsletters.
fn fixture(provider_results: bool) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let mut repo = acquisition::synthetic::accounts(root.path());
    let imap = acquisition::install_original_fake(
        &mut repo,
        7,
        (1..=4).map(|uid| (uid, original(uid))).collect(),
    );
    let account = repo
        .create(
            Provider::MailboxOrg,
            LOGIN.into(),
            &SecretValue::new(DUMMY.as_bytes().to_vec()),
        )
        .unwrap();
    repo.connection_observed(&account.id, true).unwrap();
    acquisition::synthetic::scan(&repo, &account.id, Cancellation::default(), |_| {}).unwrap();
    let account = repo.load(&account.id).unwrap();
    let store = repo.store(&account).unwrap();
    if provider_results {
        let mut census = store.load().unwrap();
        for m in census
            .folders
            .get_mut("INBOX")
            .unwrap()
            .messages
            .values_mut()
        {
            if (1..=40).contains(&m.uid) {
                m.headers.evidence.insert(
                    "authentication-results".into(),
                    vec!["mx.fake.test; dkim=pass header.d=shop.example header.s=fixture header.b=synthetic".into()],
                );
            }
        }
        store.save(&census).unwrap();
    }
    let sample: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/sample.json")).unwrap();
    let mut documents: crate::engine::Documents =
        serde_json::from_value(sample["documents"].clone()).unwrap();
    documents.override_yaml = documents
        .override_yaml
        .replace("home.example.test: personal", "home.example.test: business")
        .replace(
            "- address: invoices@work.example.test",
            "- address: team@home.example.test",
        );
    let mut state = store.load_state().unwrap();
    state.workspace = Some(Workspace {
        revision: 1,
        input: WorkspaceInput {
            documents,
            pins: vec![],
            as_of: sample["as_of"].as_i64().unwrap(),
            baseline: crate::rules::proof::Baseline::Current,
        },
    });
    store.save_state(&mut state).unwrap();
    drop(store);
    Fixture {
        root,
        repo: Arc::new(repo),
        account,
        imap,
    }
}

fn endpoints(smtp: Option<&Smtp>, https: &https::Fake) -> Fake {
    Fake {
        smtp: smtp.map(|s| s.server(true)),
        network: https::network(&[("shop.example", &[PUBLIC])], &[(PUBLIC, https.address)]),
        https: https.address,
        tls: https.identity.connector(),
        launcher: Recorder::default(),
        authserv: Some("mx.fake.test".into()),
        timeouts: Timeouts {
            command: Duration::from_secs(5),
            final_reply: Duration::from_secs(2),
        },
    }
}

fn app(f: &Fixture, fake: Fake) -> Arc<AppState> {
    let mut app = AppState::new(true);
    app.accounts.source = AccountSource::Synthetic {
        accounts: f.repo.clone(),
        _directory: tempfile::tempdir().unwrap(),
        mailboxes: f.root.path().join("mailboxes"),
    };
    app.accounts.ledger =
        Some(crate::ledger::InstalledLedger::install(f.root.path().join("ledger")).unwrap());
    app.accounts.writes = Endpoints::Fake(fake);
    Arc::new(app)
}

fn recorded(app: &AppState) -> Vec<Handoff> {
    match &app.accounts.writes {
        Endpoints::Fake(fake) => fake.launcher.0.lock().unwrap().clone(),
        Endpoints::Real => unreachable!(),
    }
}

/// Posts to `/api/accounts/{path}` and returns the status and body. The body
/// may never carry the dummy password.
async fn post(
    app: &Arc<AppState>,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = super::super::router(app.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/api/accounts/{path}"))
                .header("Host", super::super::ADDRESS)
                .header("Origin", super::super::ORIGIN)
                .header("Content-Type", "application/json")
                .header("X-Mailroom-Token", app.token.as_str())
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .unwrap();
    secret_free(&bytes, "response");
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn secret_free(bytes: &[u8], place: &str) {
    let encoded = data_encoding::BASE64.encode(format!("\0{LOGIN}\0{DUMMY}").as_bytes());
    for needle in [DUMMY.as_bytes(), &encoded.as_bytes()[..16]] {
        assert!(
            !bytes.windows(needle.len()).any(|w| w == needle),
            "dummy password in {place}"
        );
    }
}

/// No file the fixture wrote holds the dummy password in any form.
fn files_secret_free(root: &std::path::Path) {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_secret_free(&path);
        } else {
            secret_free(&std::fs::read(&path).unwrap(), &path.display().to_string());
        }
    }
}

async fn invoice(
    app: &Arc<AppState>,
    f: &Fixture,
    command: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    post(
        app,
        "invoices",
        json!({"account": f.account.id, "command": command}),
    )
    .await
}

/// Acquires, previews and confirms one wired invoice send. Returns the send's response.
async fn send_invoice(
    app: &Arc<AppState>,
    f: &Fixture,
    selection: &str,
    mode: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, _) = invoice(app, f, json!({"action":"acquire","selection":selection})).await;
    assert_eq!(status, StatusCode::OK, "acquire {selection}");
    let (status, preview) = invoice(
        app,
        f,
        json!({"action":"preview","selection":selection,"target":"accounting","mode":mode}),
    )
    .await;
    if status != StatusCode::OK {
        return (status, preview);
    }
    assert!(
        preview["confirmation"]["display"]
            .as_str()
            .unwrap()
            .contains(&format!("{selection} to accounting for account")),
        "{preview}"
    );
    invoice(app, f, json!({"action":"send","selection":selection,"target":"accounting","receipt":preview["receipt"],"confirmation_action":preview["plan"]["confirmation_action"],"mode":mode})).await
}

fn send<'a>(view: &'a serde_json::Value, selection: &str) -> &'a serde_json::Value {
    view["sends"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|s| s["plan"]["selection"] == selection)
        .unwrap()
}

#[tokio::test]
async fn invoice_routes_acquire_send_by_smtp_hand_off_and_resolve_through_fake_endpoints() {
    let f = fixture(false);
    let web = https::fake("HTTP/1.1 202 Accepted\r\n\r\n", Duration::ZERO);

    // SMTP: the acquired original's exact bytes, from the account, to the named target.
    let smtp = Smtp::start(Mode::Accept, true);
    let wired = app(&f, endpoints(Some(&smtp), &web));
    let (_, view) = invoice(&wired, &f, json!({"action":"open"})).await;
    assert!(
        view["report"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "INBOX/7/1" && r["send_available"] == true)
    );
    let (status, _) = invoice(
        &wired,
        &f,
        json!({"action":"preview","selection":"INBOX/7/1","target":"accounting","mode":"smtp"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a fixture source is never sent by SMTP"
    );
    let (status, view) = send_invoice(&wired, &f, "INBOX/7/1", "smtp").await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let sent = send(&view, "INBOX/7/1");
    assert_eq!(sent["status"], "completed");
    assert_eq!(
        sent["smtp"],
        json!({"kind":"accepted","code":250,"enhanced":"2.0.0"})
    );
    let seen = smtp.finish();
    assert!(seen.credentials_matched && !seen.plain_text_auth);
    assert_eq!(seen.message.as_deref(), Some(original(1).as_slice()));
    assert!(seen.commands.contains(&format!("MAIL FROM:<{LOGIN}>")));
    assert!(
        seen.commands
            .contains(&"RCPT TO:<archive@example.net>".to_owned())
    );
    let logs = f.imap.lock().unwrap().clone();
    assert_eq!(logs.len(), 1, "one read-only connection per acquisition");
    let (commands, flags) = logs[0].lock().unwrap().clone();
    assert!(flags.is_empty(), "acquisition never sets a flag");
    assert!(commands.iter().any(|c| c.starts_with("EXAMINE")));
    assert!(
        commands
            .iter()
            .any(|c| c == "UID FETCH 1 (UID BODY.PEEK[])")
    );
    assert!(recorded(&wired).is_empty());

    // A Mail handoff is refused: Mail cannot be bound to the confirmed original and sender.
    let wired = app(&f, endpoints(None, &web));
    let (status, _) = invoice(
        &wired,
        &f,
        json!({"action":"acquire","selection":"INBOX/7/2"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for command in [
        json!({"action":"preview","selection":"INBOX/7/2","target":"accounting","mode":"mail_app"}),
        json!({"action":"send","selection":"INBOX/7/2","target":"accounting","receipt":"a".repeat(64),"confirmation_action":"x","mode":"mail_app"}),
    ] {
        let (status, _) = invoice(&wired, &f, command).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert!(recorded(&wired).is_empty(), "no app was launched");

    // An uncertain submission blocks the next send until the owner resolves it.
    let smtp = Smtp::start(Mode::DropAfterData, true);
    let wired = app(&f, endpoints(Some(&smtp), &web));
    let (status, view) = send_invoice(&wired, &f, "INBOX/7/3", "smtp").await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let uncertain = send(&view, "INBOX/7/3").clone();
    assert_eq!(uncertain["status"], "uncertain");
    assert_eq!(
        view["sent_evidence"][uncertain["id"].as_str().unwrap()],
        json!([])
    );
    assert_eq!(
        smtp.finish().message.as_deref(),
        Some(original(3).as_slice())
    );
    let (status, _) = send_invoice(&wired, &f, "INBOX/7/4", "smtp").await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an uncertain send blocks later sends"
    );
    let (status, view) = invoice(
        &wired,
        &f,
        json!({"action":"resolve","send":uncertain["id"],"resolution":"not_sent"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resolved = send(&view, "INBOX/7/3");
    assert_eq!(
        (&resolved["status"], &resolved["resolved"]),
        (&json!("not_sent"), &json!("not_sent"))
    );
    let (status, _) = invoice(
        &wired,
        &f,
        json!({"action":"resolve","send":uncertain["id"],"resolution":"sent"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "only an uncertain send resolves"
    );
    let row = view["report"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "INBOX/7/3")
        .unwrap();
    assert_eq!(
        row["send_available"], true,
        "a send recorded as not sent can be previewed afresh"
    );
    let (status, preview) = invoice(
        &wired,
        &f,
        json!({"action":"preview","selection":"INBOX/7/3","target":"accounting","mode":"smtp"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(
        preview["plan"]["state_revision"],
        uncertain["plan"]["state_revision"]
    );

    files_secret_free(f.root.path());
    let saved = f.repo.store(&f.account).unwrap().load_state().unwrap();
    secret_free(&serde_json::to_vec(&saved).unwrap(), "saved state");
}

async fn unsubscribe(
    app: &Arc<AppState>,
    f: &Fixture,
    command: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    post(
        app,
        "unsubscribe",
        json!({"account": f.account.id, "command": command}),
    )
    .await
}

/// The subscription to the shop's list `list` received at `recipient`.
fn subscription<'a>(
    report: &'a serde_json::Value,
    recipient: &str,
    list: &str,
) -> &'a serde_json::Value {
    report["report"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["recipient"] == recipient && r["list_id"] == list)
        .unwrap()
}

fn methods(row: &serde_json::Value) -> Vec<&str> {
    row["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect()
}

async fn confirm_unsubscribe(
    app: &Arc<AppState>,
    f: &Fixture,
    selections: serde_json::Value,
) -> (StatusCode, serde_json::Value, serde_json::Value) {
    let input = json!({"as_of":AS_OF,"selections":selections,"folders":{"personal":"Muted","business":"Quiet"}});
    let (status, preview) = unsubscribe(app, f, json!({"action":"preview","input":input})).await;
    if status != StatusCode::OK {
        return (status, preview.clone(), preview);
    }
    let (status, view) = unsubscribe(app, f, json!({"action":"confirm","input":input,"receipt":preview["receipt"],"confirmation_action":preview["confirmation_action"],"mail_confirmation_action":preview["local_mail_confirmation"]})).await;
    (status, preview, view)
}

#[tokio::test]
async fn unsubscribe_routes_post_one_click_send_mail_and_hand_off_through_fake_endpoints() {
    let f = fixture(true);
    let web = https::fake(
        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n",
        Duration::ZERO,
    );
    let smtp = Smtp::start(Mode::Accept, true);
    let wired = app(&f, endpoints(Some(&smtp), &web));
    let (_, report) = unsubscribe(&wired, &f, json!({"action":"open","as_of":AS_OF})).await;
    assert_eq!(report["wired"], true);
    let post = subscription(&report, "team@home.example.test", "<offers.shop.example>");
    let mail = subscription(&report, "alias@home.example.test", "<offers.shop.example>");
    let app_mail = subscription(&report, "team@home.example.test", "<updates.shop.example>");
    assert_eq!(post["authentication"], "provider_dkim_pass_both");
    assert!(methods(post).contains(&"one_click") && methods(mail).contains(&"local_mail"));
    let (status, preview, view) = confirm_unsubscribe(
        &wired,
        &f,
        json!([
            {"subscription":post["id"],"method":"one_click"},
            {"subscription":mail["id"],"method":"local_mail"},
            {"subscription":app_mail["id"],"method":"mail_app_preparation"},
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let display = preview["confirmation"]["display"].as_str().unwrap();
    for named in [
        "One HTTPS POST of List-Unsubscribe=One-Click to shop.example for mail received at team@home.example.test",
        "Send by SMTP to leave@shop.example from alias@home.example.test",
        "Open a new message in Mail to leave@shop.example from team@home.example.test with the confirmed subject and body",
    ] {
        assert!(display.contains(named), "{display}");
    }
    let history = view["history"].as_array().unwrap();
    let entry = |id: &serde_json::Value| history.iter().find(|e| &e["subscription"] == id).unwrap();
    assert_eq!(
        (
            &entry(&post["id"])["status"],
            &entry(&post["id"])["http_status"]
        ),
        (&json!("http_observed"), &json!(202))
    );
    assert_eq!(entry(&mail["id"])["status"], "submitted");
    assert_eq!(entry(&app_mail["id"])["status"], "handed_off");
    let request = String::from_utf8(web.requests.lock().unwrap()[0].clone()).unwrap();
    assert!(
        request.starts_with(
            "POST /remove?token=synthetic-private-token HTTP/1.1\r\nHost: shop.example\r\n"
        ) && request.ends_with("\r\n\r\nList-Unsubscribe=One-Click"),
        "{request}"
    );
    assert!(
        !view["history"]
            .to_string()
            .contains("synthetic-private-token"),
        "only the host is kept"
    );
    let seen = smtp.finish();
    assert!(seen.credentials_matched);
    assert!(
        seen.commands
            .contains(&"MAIL FROM:<alias@home.example.test>".to_owned())
    );
    assert!(
        seen.commands
            .contains(&"RCPT TO:<leave@shop.example>".to_owned())
    );
    assert!(
        matches!(&recorded(&wired)[..], [Handoff::ComposeInMail { to, from, .. }] if to == "leave@shop.example" && from == "team@home.example.test"),
        "Mail composes from the confirmed receiving address"
    );
    let submitted = preview["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["method"] == "local_mail")
        .unwrap();
    assert_eq!(
        submitted["payload_digest"].as_str().unwrap(),
        crate::unsubscribe::digest(seen.message.as_deref().unwrap()),
        "the confirmed digest is the SHA-256 of exactly the submitted bytes"
    );
    files_secret_free(f.root.path());
}

#[tokio::test]
async fn an_uncertain_unsubscribe_message_is_resolved_by_the_owner_and_never_resent() {
    let f = fixture(false);
    let web = https::fake("HTTP/1.1 202 Accepted\r\n\r\n", Duration::ZERO);
    let smtp = Smtp::start(Mode::DropAfterData, true);
    let wired = app(&f, endpoints(Some(&smtp), &web));
    let (_, report) = unsubscribe(&wired, &f, json!({"action":"open","as_of":AS_OF})).await;
    let post = subscription(&report, "team@home.example.test", "<offers.shop.example>");
    assert!(
        !methods(post).contains(&"one_click"),
        "no provider results, no one-click offer"
    );
    assert_eq!(post["authentication"], "unknown_no_provider_results");
    let mail = subscription(&report, "alias@home.example.test", "<offers.shop.example>");
    let (status, _, view) = confirm_unsubscribe(
        &wired,
        &f,
        json!([{"subscription":mail["id"],"method":"local_mail"}]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let entry = view["history"][0].clone();
    assert_eq!(entry["status"], "uncertain");
    assert!(smtp.finish().message.is_some());
    let (status, _, _) = confirm_unsubscribe(
        &wired,
        &f,
        json!([{"subscription":post["id"],"method":"keep"}]),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an uncertain message blocks later actions"
    );
    let (status, view) = unsubscribe(
        &wired,
        &f,
        json!({"action":"resolve","entry":entry["id"],"resolution":"not_sent","as_of":AS_OF}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (
            &view["history"][0]["status"],
            &view["history"][0]["resolved"]
        ),
        (&json!("not_sent"), &json!("not_sent"))
    );
    assert_eq!(
        view["history"].as_array().unwrap().len(),
        1,
        "resolving sends nothing"
    );
    let (status, _) = unsubscribe(
        &wired,
        &f,
        json!({"action":"resolve","entry":entry["id"],"resolution":"sent","as_of":AS_OF}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Runs the wired SMTP and acquisition routes, on success and on failure paths
/// including a server that echoes the credentials, and prints every response,
/// outcome and error it sees. Run only by the parent test below.
#[tokio::test]
async fn wired_secret_flow_worker() {
    if std::env::var("MAILROOM_WIRED_SECRET_WORKER").as_deref() != Ok("1") {
        return;
    }
    let f = fixture(false);
    let web = https::fake("HTTP/1.1 202 Accepted\r\n\r\n", Duration::ZERO);
    for (uid, mode) in [
        (1, Mode::AuthFailEcho),
        (2, Mode::RcptReject),
        (3, Mode::Accept),
    ] {
        let smtp = Smtp::start(mode, true);
        let wired = app(&f, endpoints(Some(&smtp), &web));
        let (status, view) = send_invoice(&wired, &f, &format!("INBOX/7/{uid}"), "smtp").await;
        println!("{mode:?} {status} {view}");
        println!("{:?}", smtp.finish());
    }
    for (validity, uid) in [(8, 1), (7, 99)] {
        let failed = f.repo.acquire_original(&f.account, "INBOX", validity, uid);
        println!("acquire {validity}/{uid} {:?}", failed.err());
    }
    println!("worker-complete");
}

#[test]
fn dummy_password_never_reaches_child_output_on_any_wired_path() {
    let exe = std::env::current_exe().unwrap();
    let args = [
        "--exact",
        "server::accounts::wiring_tests::wired_secret_flow_worker",
        "--nocapture",
        "--test-threads=1",
    ];
    assert!(args.iter().all(|a| !a.contains(DUMMY)));
    let output = std::process::Command::new(exe)
        .args(args)
        .env("MAILROOM_WIRED_SECRET_WORKER", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("worker-complete"));
    assert!(
        stdout.contains("AuthFailEcho 200 OK") && stdout.contains("\"stage\":\"auth\""),
        "the echoing failure ran: {stdout}"
    );
    assert!(stdout.contains("acquire 8/1 Some("));
    secret_free(&output.stdout, "child stdout");
    secret_free(&output.stderr, "child stderr");
}

fn handoff_endpoints() -> Fake {
    Fake {
        smtp: None,
        network: https::network(&[], &[]),
        https: "127.0.0.1:1".parse().unwrap(),
        tls: native_tls::TlsConnector::new().unwrap(),
        launcher: Recorder::default(),
        authserv: Some("mx.fake.test".into()),
        timeouts: Timeouts::default(),
    }
}

fn second_account(f: &Fixture) -> Account {
    let account = f
        .repo
        .create(
            Provider::MailboxOrg,
            "second@example.test".into(),
            &SecretValue::new(DUMMY.as_bytes().to_vec()),
        )
        .unwrap();
    f.repo.connection_observed(&account.id, true).unwrap();
    acquisition::synthetic::scan(&f.repo, &account.id, Cancellation::default(), |_| {}).unwrap();
    let account = f.repo.load(&account.id).unwrap();
    let store = f.repo.store(&account).unwrap();
    let mut saved = store.load_state().unwrap();
    saved.workspace = f
        .repo
        .store(&f.account)
        .unwrap()
        .load_state()
        .unwrap()
        .workspace;
    store.save_state(&mut saved).unwrap();
    account
}

async fn combined(
    app: &Arc<AppState>,
    command: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    post(app, "subscription-review", json!({"command":command})).await
}

fn combined_input(view: &serde_json::Value, method: &str) -> serde_json::Value {
    let selections: Vec<_> = view["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let row = subscription(a, "team@home.example.test", "<offers.shop.example>");
            json!({"account":a["account"],"subscription":row["id"],"method":method})
        })
        .collect();
    json!({"as_of":AS_OF,"selections":selections,"folders":{"personal":"Muted","business":"Quiet"}})
}

fn combined_confirm(input: &serde_json::Value, preview: &serde_json::Value) -> serde_json::Value {
    json!({"action":"confirm","input":input,"receipt":preview["receipt"],"confirmation_action":preview["confirmation_action"],"mail_confirmation_action":preview["local_mail_confirmation"]})
}

#[tokio::test]
async fn combined_wired_handoffs_are_separate_durable_and_single_use() {
    let f = fixture(true);
    let other = second_account(&f);
    let app = app(&f, handoff_endpoints());
    let (status, view) = combined(&app, json!({"action":"open","as_of":AS_OF})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["review_available"], true);
    assert_eq!(view["synthetic"], false);
    assert_eq!(view["accounts"].as_array().unwrap().len(), 2);
    for a in view["accounts"].as_array().unwrap() {
        assert_eq!(a["wired"], true);
        assert_eq!(a["synthetic"], false);
        assert!(
            methods(subscription(
                a,
                "team@home.example.test",
                "<offers.shop.example>"
            ))
            .contains(&"mail_app_preparation")
        );
    }
    let input = combined_input(&view, "mail_app_preparation");
    let (status, preview) = combined(&app, json!({"action":"preview","input":input})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(recorded(&app).is_empty());
    let display = preview["confirmation"]["display"].as_str().unwrap();
    for id in [&f.account.id, &other.id] {
        assert!(display.contains(id.as_str()));
    }
    assert!(
        display.contains(
            "Open a new message in Mail to leave@shop.example from team@home.example.test"
        )
    );
    assert_eq!(preview["previews"].as_array().unwrap().len(), 2);
    let (status, done) = combined(&app, combined_confirm(&input, &preview)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recorded(&app).len(), 2);
    for a in done["accounts"].as_array().unwrap() {
        let history = a["history"].as_array().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["status"], "handed_off");
        assert_eq!(
            history[0]["subscription"],
            subscription(a, "team@home.example.test", "<offers.shop.example>")["id"]
        );
    }
    for a in [&f.account, &other] {
        let store = f.repo.store(a).unwrap();
        let saved = store.load_state().unwrap();
        assert_eq!(saved.unsubscribe.history.len(), 1);
        assert_eq!(
            saved.unsubscribe.history[0].status,
            crate::unsubscribe::Status::HandedOff
        );
    }
    assert_eq!(
        combined(&app, combined_confirm(&input, &preview)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(recorded(&app).len(), 2);
    files_secret_free(f.root.path());
    let mut display_fixture = json!({"view":view,"preview":preview,"done":done});
    for preview in display_fixture["preview"]["previews"]
        .as_array_mut()
        .unwrap()
    {
        for action in preview["actions"].as_array_mut().unwrap() {
            if let Some(card) = action["copy_card"].as_object_mut() {
                card.insert("body".into(), serde_json::Value::Null);
                card.insert("uri".into(), serde_json::Value::Null);
            }
        }
    }
    let encoded = serde_json::to_vec(&display_fixture).unwrap();
    secret_free(&encoded, "UI fixture export");
    for sentinel in [
        "synthetic-mail-token",
        "synthetic-private-token",
        "synthetic-relay-token",
    ] {
        assert!(
            !encoded
                .windows(sentinel.len())
                .any(|w| w == sentinel.as_bytes()),
            "Private unsubscribe value in UI fixture export"
        );
    }
    println!("MAILROOM_COMBINED_REVIEW_FIXTURE {}", display_fixture);
}

#[tokio::test]
async fn combined_wired_preflight_failure_saves_no_account_intents() {
    let f = fixture(true);
    second_account(&f);
    let app = app(&f, handoff_endpoints());
    let (_, view) = combined(&app, json!({"action":"open","as_of":AS_OF})).await;
    let mut input = combined_input(&view, "mail_app_preparation");
    input["selections"][1]["method"] = json!("local_mail");
    let (status, preview) = combined(&app, json!({"action":"preview","input":input})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        combined(&app, combined_confirm(&input, &preview)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(recorded(&app).is_empty());
    for a in f.repo.list().unwrap() {
        assert!(
            f.repo
                .store(&a)
                .unwrap()
                .load_state()
                .unwrap()
                .unsubscribe
                .history
                .is_empty()
        );
    }
    files_secret_free(f.root.path());
}

#[tokio::test]
async fn combined_wired_changed_account_or_selection_denies_all_effects() {
    let f = fixture(true);
    let other = second_account(&f);
    let app = app(&f, handoff_endpoints());
    let (_, view) = combined(&app, json!({"action":"open","as_of":AS_OF})).await;
    let input = combined_input(&view, "mail_app_preparation");
    let (_, preview) = combined(&app, json!({"action":"preview","input":input})).await;
    let mut altered = input.clone();
    altered["selections"].as_array_mut().unwrap().pop();
    assert_eq!(
        combined(&app, combined_confirm(&altered, &preview)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (_, preview) = combined(&app, json!({"action":"preview","input":input})).await;
    {
        let store = f.repo.store(&other).unwrap();
        let mut saved = store.load_state().unwrap();
        saved.workspace.as_mut().unwrap().revision += 1;
        store.save_state(&mut saved).unwrap();
    }
    assert_eq!(
        combined(&app, combined_confirm(&input, &preview)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(recorded(&app).is_empty());
    for a in f.repo.list().unwrap() {
        assert!(
            f.repo
                .store(&a)
                .unwrap()
                .load_state()
                .unwrap()
                .unsubscribe
                .history
                .is_empty()
        );
    }
    files_secret_free(f.root.path());
}

#[tokio::test]
async fn combined_wired_confirmation_failure_saves_no_intents() {
    struct Cancel;
    impl crate::confirmation::Backend for Cancel {
        fn available(&self) -> Result<(), String> {
            Ok(())
        }
        fn synthetic(&self) -> bool {
            false
        }
        fn exchange(&mut self, _: &[u8], _: &[u8; 32]) -> Result<Vec<u8>, String> {
            Err(crate::confirmation::CANCELLED.into())
        }
    }
    let f = fixture(true);
    second_account(&f);
    let app = app(&f, handoff_endpoints());
    *app.accounts.confirmations.lock().unwrap() =
        crate::confirmation::Boundary::with_backend(Box::new(Cancel));
    let (_, view) = combined(&app, json!({"action":"open","as_of":AS_OF})).await;
    let input = combined_input(&view, "mail_app_preparation");
    let (status, preview) = combined(&app, json!({"action":"preview","input":input})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["confirmation"]["synthetic"], false);
    assert_eq!(
        combined(&app, combined_confirm(&input, &preview)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(recorded(&app).is_empty());
    for a in f.repo.list().unwrap() {
        assert!(
            f.repo
                .store(&a)
                .unwrap()
                .load_state()
                .unwrap()
                .unsubscribe
                .history
                .is_empty()
        );
    }
    files_secret_free(f.root.path());
}

#[tokio::test]
async fn account_wired_handoff_uses_shared_executor_without_network() {
    let f = fixture(true);
    let app = app(&f, handoff_endpoints());
    let (_, view) = unsubscribe(&app, &f, json!({"action":"open","as_of":AS_OF})).await;
    let row = subscription(&view, "team@home.example.test", "<offers.shop.example>");
    let (status, _, done) = confirm_unsubscribe(
        &app,
        &f,
        json!([{"subscription":row["id"],"method":"mail_app_preparation"}]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["history"][0]["status"], "handed_off");
    assert_eq!(recorded(&app).len(), 1);
    files_secret_free(f.root.path());
}

#[tokio::test]
async fn combined_wired_one_click_resolver_failure_is_recorded_without_retry() {
    let f = fixture(true);
    let other = second_account(&f);
    let app = app(&f, handoff_endpoints());
    let (_, view) = combined(&app, json!({"action":"open","as_of":AS_OF})).await;
    let mut input = combined_input(&view, "keep");
    for target in input["selections"].as_array_mut().unwrap() {
        if target["account"] == json!(f.account.id) {
            target["method"] = json!("one_click");
        }
    }
    let (status, preview) = combined(&app, json!({"action":"preview","input":input})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, done) = combined(&app, combined_confirm(&input, &preview)).await;
    assert_eq!(status, StatusCode::OK);
    for a in done["accounts"].as_array().unwrap() {
        let entry = &a["history"][0];
        assert_eq!(
            entry["status"],
            if a["account"] == json!(f.account.id) {
                "not_sent"
            } else {
                "kept"
            }
        );
        assert!(!entry.to_string().contains("synthetic-private-token"));
    }
    assert!(recorded(&app).is_empty());
    if let Endpoints::Fake(fake) = &app.accounts.writes {
        assert!(fake.network.dialed.lock().unwrap().is_empty());
    }
    assert_eq!(
        f.repo
            .store(&other)
            .unwrap()
            .load_state()
            .unwrap()
            .unsubscribe
            .history
            .len(),
        1
    );
    assert_eq!(
        combined(&app, combined_confirm(&input, &preview)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    files_secret_free(f.root.path());
}
