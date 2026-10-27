//! Mail-app handoff: the person finishes and sends the message in their own mail app.
//!
//! A handoff is recorded as handed off, never as sent. The launch passes no
//! secret: the AppleScript goes through stdin, argv is fixed, and the child gets
//! an empty environment, so neither API keys nor `ELECTRON_RUN_AS_NODE` reach
//! the app LaunchServices starts.
use serde::Serialize;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// One request to the mail app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Request {
    /// Open Apple Mail's forward window for the message with this Message-ID
    /// (without angle brackets), with the target filled in. Mail forwards the
    /// original with its attachments.
    ForwardOriginal { message_id: String, target: String },
    /// Open a `mailto:` link in the default mail app.
    OpenMailto {
        #[serde(skip)]
        uri: Zeroizing<String>,
        host: String,
    },
}

pub(crate) trait Launcher: Send + Sync {
    fn launch(&self, request: &Request) -> Result<(), String>;
}

fn applescript_string(value: &str) -> Option<String> {
    if value.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// The AppleScript for `request`. Finder opens Mail first so Mail starts with
/// a Dock launch's environment.
pub(crate) fn script(request: &Request) -> Result<Zeroizing<String>, String> {
    let open_mail = "tell application \"Finder\" to open (POSIX file \"/System/Applications/Mail.app\" as alias)\n";
    Ok(Zeroizing::new(match request {
        Request::ForwardOriginal { message_id, target } => {
            if !crate::rules::config::mailbox(target)
                || message_id.is_empty()
                || message_id.len() > 998
                || message_id.contains(['<', '>'])
            {
                return Err("Invalid handoff".into());
            }
            let id = applescript_string(message_id).ok_or("Invalid Message-ID")?;
            let target = applescript_string(target).ok_or("Invalid target")?;
            format!(
                "{open_mail}tell application \"Mail\"\n  set found to missing value\n  repeat with b in (every mailbox of every account)\n    repeat with c in b\n      set hits to (messages of c whose message id is {id})\n      if hits is not {{}} then\n        set found to item 1 of hits\n        exit repeat\n      end if\n    end repeat\n    if found is not missing value then exit repeat\n  end repeat\n  if found is missing value then error \"Original not found in Mail\" number 1\n  set reply to forward found with opening window\n  tell reply to make new to recipient at end of to recipients with properties {{address:{target}}}\n  activate\nend tell\n"
            )
        }
        Request::OpenMailto { uri, .. } => {
            if !uri.starts_with("mailto:") {
                return Err("Invalid mailto".into());
            }
            let uri = Zeroizing::new(applescript_string(uri).ok_or("Invalid mailto")?);
            format!("open location {}\n", uri.as_str())
        }
    }))
}

/// Runs `/usr/bin/osascript -` with the script on stdin and an empty environment.
/// Tests never launch an app; they use `Recorder`.
#[cfg_attr(test, allow(dead_code))]
pub(crate) struct Osascript;
impl Launcher for Osascript {
    fn launch(&self, request: &Request) -> Result<(), String> {
        let script = script(request)?;
        let mut child = Command::new("/usr/bin/osascript")
            .arg("-")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Mail app unavailable")?;
        let written = child
            .stdin
            .take()
            .ok_or("Mail app unavailable")?
            .write_all(script.as_bytes());
        drop(script);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() && written.is_ok() => return Ok(()),
                Ok(Some(_)) => return Err("Mail app did not open the message".into()),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Mail app did not answer".into());
                }
            }
        }
    }
}

/// Records requests instead of launching anything.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Recorder(pub std::sync::Mutex<Vec<Request>>);
#[cfg(test)]
impl Launcher for Recorder {
    fn launch(&self, request: &Request) -> Result<(), String> {
        script(request)?;
        self.0.lock().unwrap().push(request.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_quote_inputs_and_refuse_injection() {
        let forward = Request::ForwardOriginal {
            message_id: "m1\"&quit&\"@shop.example".into(),
            target: "intake@accounting.example.net".into(),
        };
        let text = script(&forward).unwrap();
        assert!(text.contains("message id is \"m1\\\"&quit&\\\"@shop.example\""));
        assert!(text.contains("{address:\"intake@accounting.example.net\"}"));
        assert!(text.starts_with("tell application \"Finder\" to open"));
        for bad in [
            Request::ForwardOriginal {
                message_id: "<m1@x>".into(),
                target: "intake@accounting.example.net".into(),
            },
            Request::ForwardOriginal {
                message_id: "m1@x".into(),
                target: "a\"@x.example".into(),
            },
            Request::ForwardOriginal {
                message_id: "m1\n@x".into(),
                target: "intake@accounting.example.net".into(),
            },
            Request::OpenMailto {
                uri: Zeroizing::new("https://x.example".into()),
                host: "x.example".into(),
            },
        ] {
            assert!(script(&bad).is_err());
        }
        let mailto = Request::OpenMailto {
            uri: Zeroizing::new("mailto:leave@shop.example?subject=unsubscribe&body=token".into()),
            host: "shop.example".into(),
        };
        assert_eq!(
            script(&mailto).unwrap().as_str(),
            "open location \"mailto:leave@shop.example?subject=unsubscribe&body=token\"\n"
        );
        let recorded = serde_json::to_string(&mailto).unwrap();
        assert!(
            !recorded.contains("token"),
            "the token-bearing URI is never serialized"
        );
    }

    #[test]
    fn recorder_launches_nothing_and_keeps_the_exact_request() {
        let recorder = Recorder::default();
        let request = Request::ForwardOriginal {
            message_id: "m1@shop.example".into(),
            target: "intake@accounting.example.net".into(),
        };
        recorder.launch(&request).unwrap();
        assert_eq!(*recorder.0.lock().unwrap(), vec![request]);
    }
}
