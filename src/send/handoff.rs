//! Mail-app handoff: the person finishes and sends the message in Apple Mail.
//!
//! A handoff is recorded as handed off, never as sent. It always opens Mail,
//! never the default `mailto:` handler, and composes the confirmed fields from
//! the confirmed sender; when Mail does not take that sender, the script closes
//! the draft unsaved and fails. The launch passes no account password or API key.
//! The AppleScript goes through stdin, argv is fixed, and the child gets an empty environment,
//! so neither API keys nor `ELECTRON_RUN_AS_NODE` reach the app LaunchServices
//! starts. The script is built in one zeroizing buffer sized up front, so the
//! token-bearing fields leave no unwiped intermediate copies.
use serde::Serialize;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// One request to Mail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Request {
    /// A new message in Mail to `to` from `from` with exactly this subject and body.
    ComposeInMail {
        to: String,
        from: String,
        #[serde(skip)]
        subject: Zeroizing<String>,
        #[serde(skip)]
        body: Zeroizing<String>,
    },
}

pub(crate) trait Launcher: Send + Sync {
    fn launch(&self, request: &Request) -> Result<(), String>;
}

/// The longest field one handoff carries.
const FIELD_LIMIT: usize = 64 * 1024;
/// The widest AppleScript one input character becomes: `" & linefeed & "`.
const WIDEST: usize = 16;

/// Appends `value` as an AppleScript string expression. Line breaks and tabs
/// become `linefeed` and `tab`; any other control character is refused.
fn quote(out: &mut Zeroizing<String>, value: &str) -> Result<(), String> {
    out.push('"');
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' | '\n' => out.push_str("\" & linefeed & \""),
            '\t' => out.push_str("\" & tab & \""),
            c if c.is_control() => return Err("Control character in handoff".into()),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(())
}

/// The AppleScript for `request`. Finder opens Mail first so Mail starts with
/// a Dock launch's environment.
pub(crate) fn script(request: &Request) -> Result<Zeroizing<String>, String> {
    let Request::ComposeInMail {
        to,
        from,
        subject,
        body,
    } = request;
    if !crate::rules::config::mailbox(to)
        || !crate::rules::config::mailbox(from)
        || subject.len() > FIELD_LIMIT
        || body.len() > FIELD_LIMIT
    {
        return Err("Invalid handoff".into());
    }
    // Each field follows its part; `TAIL` closes the script.
    const PARTS: [&str; 4] = [
        "tell application \"Finder\" to open (POSIX file \"/System/Applications/Mail.app\" as alias)\ntell application \"Mail\"\n  set m to make new outgoing message with properties {visible:false, subject:",
        ", content:",
        "}\n  tell m to make new to recipient at end of to recipients with properties {address:",
        "}\n  set wanted to ",
    ];
    const TAIL: &str = "\n  set sender of m to wanted\n  set chosen to sender of m\n  if chosen is not wanted and chosen does not end with (\"<\" & wanted & \">\") then\n    close m saving no\n    error \"Mail did not take the confirmed sender\" number 1\n  end if\n  set visible of m to true\n  activate\nend tell\n";
    let fields = [subject.as_str(), body.as_str(), to.as_str(), from.as_str()];
    let capacity = PARTS.iter().map(|p| p.len()).sum::<usize>()
        + TAIL.len()
        + fields.iter().map(|f| f.len() * WIDEST + 2).sum::<usize>();
    let mut out = Zeroizing::new(String::with_capacity(capacity));
    let allocated = out.capacity();
    for (part, field) in PARTS.iter().zip(fields) {
        out.push_str(part);
        quote(&mut out, field)?;
    }
    out.push_str(TAIL);
    debug_assert_eq!(out.capacity(), allocated, "the script never reallocates");
    Ok(out)
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
            .map_err(|_| "Mail unavailable")?;
        let written = child
            .stdin
            .take()
            .ok_or("Mail unavailable")?
            .write_all(script.as_bytes());
        drop(script);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() && written.is_ok() => return Ok(()),
                Ok(Some(_)) => return Err("Mail did not open the message".into()),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Mail did not answer".into());
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

    fn compose(to: &str, from: &str, subject: &str, body: &str) -> Request {
        Request::ComposeInMail {
            to: to.into(),
            from: from.into(),
            subject: Zeroizing::new(subject.into()),
            body: Zeroizing::new(body.into()),
        }
    }

    #[test]
    fn opens_mail_itself_from_the_confirmed_sender_or_fails() {
        let request = compose(
            "leave@shop.example",
            "alias@home.example.test",
            "unsubscribe \"now\"",
            "token\r\nline\ttwo",
        );
        let text = script(&request).unwrap();
        assert!(text.starts_with(
            "tell application \"Finder\" to open (POSIX file \"/System/Applications/Mail.app\""
        ));
        assert!(text.contains("tell application \"Mail\""));
        assert!(
            !text.contains("open location"),
            "never the default mailto handler"
        );
        assert!(text.contains("subject:\"unsubscribe \\\"now\\\"\""));
        assert!(text.contains("content:\"token\" & linefeed & \"line\" & tab & \"two\""));
        assert!(text.contains("{address:\"leave@shop.example\"}"));
        assert!(text.contains("set wanted to \"alias@home.example.test\""));
        assert!(text.contains("set sender of m to wanted"));
        assert!(
            text.contains("close m saving no"),
            "a different sender closes the draft"
        );
        assert!(
            text.find("close m saving no") < text.find("set visible of m to true"),
            "nothing is shown before the sender is checked"
        );
        for bad in [
            compose("a\"@x.example", "alias@home.example.test", "s", "b"),
            compose("leave@shop.example", "not-an-address", "s", "b"),
            compose(
                "leave@shop.example",
                "alias@home.example.test",
                "s\u{7}",
                "b",
            ),
            compose(
                "leave@shop.example",
                "alias@home.example.test",
                "s",
                &"x".repeat(FIELD_LIMIT + 1),
            ),
        ] {
            assert!(script(&bad).is_err());
        }
        let recorded = serde_json::to_string(&request).unwrap();
        assert!(
            !recorded.contains("token"),
            "the token-bearing fields are never serialized"
        );
    }

    #[test]
    fn the_script_buffer_is_sized_once_for_the_widest_input() {
        let request = compose(
            "leave@shop.example",
            "alias@home.example.test",
            &"\n".repeat(300),
            &"\"\t\\".repeat(300),
        );
        let text = script(&request).unwrap();
        assert!(text.matches("linefeed").count() == 300 && text.matches("tab").count() >= 300);
    }

    #[test]
    fn recorder_launches_nothing_and_keeps_the_exact_request() {
        let recorder = Recorder::default();
        let request = compose("leave@shop.example", "alias@home.example.test", "s", "b");
        recorder.launch(&request).unwrap();
        assert_eq!(*recorder.0.lock().unwrap(), vec![request]);
    }
}
