use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture(uncertain: bool) -> (tempfile::TempDir, Client) {
    let root = tempfile::tempdir().unwrap();
    let cli = root.path().join("t3-fixture");
    let exit = if uncertain {
        "exit 1"
    } else {
        "printf '%s' '{\"threadId\":\"thread:fixture\",\"runId\":\"run-fixture\"}'"
    };
    fs::write(&cli, format!(r##"#!/bin/sh
set -eu
test -z "${{ELECTRON_RUN_AS_NODE+x}}"
case "$1 $2" in
  'thread list')
    if test -f unavailable; then exit 1; fi
    if test -f title; then printf '[{{"title":"%s","threadId":"thread:fixture"}}]' "$(/bin/cat title)"; else printf '[]'; fi;;
  'project add') exit 0;;
  'thread start')
    printf '%s' "$4" > title
    printf 'start\n' >> starts
    printf '%s\n' "$@" > arguments
    /bin/cat > prompt
    {exit};;
  'thread show') printf '%s' '{{"thread":{{"link":"t3-thread://v2/fixture/thread:fixture"}},"messages":[]}}';;
  *) exit 1;;
esac
"##)).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client {
        executable: cli,
        home: root.path().as_os_str().to_os_string(),
    };
    (root, client)
}

#[test]
fn cli_launch_retries_find_existing_thread_without_disclosing_prompt_in_arguments() {
    let (root, client) = fixture(false);
    let result = client
        .launch(
            root.path(),
            "Mailroom review fixture",
            "private-prompt-on-stdin",
            true,
        )
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(result.id, "thread:fixture");
    assert_eq!(
        result.link.as_deref(),
        Some("t3-thread://v2/fixture/thread:fixture")
    );
    let again = client
        .launch(
            root.path(),
            "Mailroom review fixture",
            "another-prompt",
            true,
        )
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(again.id, result.id);
    assert_eq!(
        fs::read_to_string(root.path().join("starts")).unwrap(),
        "start\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("prompt")).unwrap(),
        "private-prompt-on-stdin"
    );
    assert!(
        !fs::read_to_string(root.path().join("arguments"))
            .unwrap()
            .contains("private-prompt-on-stdin")
    );
}

#[test]
fn uncertain_cli_launch_recovers_the_created_thread_without_starting_another() {
    let (root, client) = fixture(true);
    let error = match client.launch(root.path(), "Mailroom review fixture", "prompt", true) {
        Err(e) => e,
        Ok(_) => panic!("expected uncertain launch"),
    };
    assert!(error.uncertain);
    let recovered = client
        .launch(root.path(), "Mailroom review fixture", "prompt", false)
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(recovered.id, "thread:fixture");
    assert_eq!(
        fs::read_to_string(root.path().join("starts")).unwrap(),
        "start\n"
    );
    fs::remove_file(root.path().join("title")).unwrap();
    let error = match client.launch(root.path(), "Mailroom review fixture", "prompt", false) {
        Err(e) => e,
        Ok(_) => panic!("missing thread must stay uncertain"),
    };
    assert!(error.uncertain);
    assert_eq!(
        fs::read_to_string(root.path().join("starts")).unwrap(),
        "start\n"
    );
}

#[test]
fn cli_unavailable_before_start_remains_retryable_and_output_is_bounded() {
    let (root, client) = fixture(false);
    fs::write(root.path().join("unavailable"), b"").unwrap();
    let error = match client.launch(root.path(), "Mailroom review fixture", "prompt", true) {
        Err(e) => e,
        Ok(_) => panic!("expected unavailable"),
    };
    assert!(!error.uncertain);
    assert!(!root.path().join("starts").exists());
    assert!(capture(std::io::Cursor::new(vec![b'x'; OUTPUT_LIMIT + 1])).is_err());
}

#[test]
fn deterministic_provider_or_model_rejection_in_stderr_stays_retryable() {
    let root = tempfile::tempdir().unwrap();
    let cli = root.path().join("t3-bad-model");
    fs::write(
        &cli,
        r##"#!/bin/sh
set -eu
case "$1 $2" in
  'thread list') printf '[]';;
  'thread start') echo "Error: unknown model 'nope-99'" >&2; exit 2;;
  *) exit 1;;
esac
"##,
    )
    .unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client {
        executable: cli,
        home: root.path().as_os_str().to_os_string(),
    };
    let err = match client.launch(root.path(), "Mailroom review fixture", "prompt", true) {
        Err(e) => e,
        Ok(_) => panic!("expected model rejection"),
    };
    assert!(
        !err.uncertain,
        "deterministic rejection must stay retryable"
    );
    assert!(err.message.contains("unknown model"), "{}", err.message);
}
