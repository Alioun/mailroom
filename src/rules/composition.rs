use super::{
    proof::{self, Destination, PinResult, Proof},
    sieve::{MessageView, Test, Truth},
};
use crate::census::Census;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const SEMANTICS: &str = "mailroom-foreign-composition-v1";
const MARK: &str = "# mailroom:";
const REQUIRE: Segment = Segment(
    "\n# mailroom:require-begin v1\n",
    "# mailroom:require-end v1\n",
);
const BLOCK: Segment = Segment("\n# mailroom:block-begin v1\n", "# mailroom:block-end v1\n");
const INCLUDE: Segment = Segment(
    "\n# mailroom:include-begin v1\n",
    "# mailroom:include-end v1\n",
);
pub const OWNED: &str = "# mailroom:owned-script v1\n";
const DEPTH: usize = 32;

struct Segment(&'static str, &'static str);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Placement {
    Include { script: String },
    Block {},
}
impl Placement {
    pub fn valid_for(&self, host: &str) -> bool {
        match self {
            Self::Include { script } => {
                script != host
                    && script.len() <= 128
                    && !script.contains(['"', '\\'])
                    && crate::census::sieve::valid_script_name(script)
            }
            Self::Block {} => true,
        }
    }
    pub fn owned(&self) -> Option<&str> {
        match self {
            Self::Include { script } => Some(script),
            Self::Block {} => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Refusal {
    PlacementUnchosen,
    InvalidPlacement,
    HostMissing {
        script: String,
    },
    HostInactive {
        script: String,
        active: Option<String>,
    },
    Unparseable {
        script: String,
        line: usize,
        detail: &'static str,
    },
    MarkerDamaged {
        script: String,
        detail: &'static str,
    },
    OwnedNameForeign {
        script: String,
    },
    UnsafeRule {
        script: String,
        rule: String,
        line: usize,
        detail: &'static str,
    },
    MailroomUnreachable {
        script: String,
        rule: String,
        line: usize,
    },
    MailroomContentInvalid,
    ProofBlocked,
    NothingToRemove,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForeignDest {
    Inbox,
    Folder { name: String },
    Discard,
    Redirect,
    Reject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForeignRule {
    pub index: usize,
    pub name: String,
    pub line: usize,
    pub may_stop: bool,
    pub dispositions: Vec<ForeignDest>,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Tag(String),
    Number(u64),
    Str(String),
    Punct(u8),
}
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}
#[derive(Clone, Debug)]
enum Arg {
    Strings(Vec<String>),
    Number,
    Tag(String),
}
#[derive(Clone, Debug)]
struct Node {
    name: String,
    args: Vec<Arg>,
    tests: Vec<Node>,
}
#[derive(Clone, Debug)]
struct Command {
    name: String,
    args: Vec<Arg>,
    tests: Vec<Node>,
    block: Option<Vec<Command>>,
    start: usize,
    end: usize,
}
struct Rule {
    commands: Vec<Command>,
    name: String,
    line: usize,
}
pub(crate) struct Script {
    requires: BTreeSet<String>,
    require_end: usize,
    rules: Vec<Rule>,
    unique_ids: Vec<u64>,
}
type Failure = (usize, &'static str);

fn line(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}
fn word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

struct Lexed {
    tokens: Vec<Token>,
    comments: Vec<(usize, usize)>,
    opaque: Vec<(usize, usize)>,
}
fn lex(text: &str) -> Result<Lexed, Failure> {
    let b = text.as_bytes();
    let mut out = Lexed {
        tokens: vec![],
        comments: vec![],
        opaque: vec![],
    };
    let mut i = 0;
    let text_of = |v: Vec<u8>, at: usize| String::from_utf8(v).map_err(|_| (at, "invalid UTF-8"));
    while i < b.len() {
        let start = i;
        let tok = match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
                continue;
            }
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                out.comments.push((start, i));
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = text[i + 2..]
                    .find("*/")
                    .ok_or((start, "unclosed bracket comment"))?;
                i += end + 4;
                out.opaque.push((start, i));
                continue;
            }
            b'"' => {
                i += 1;
                let mut v = vec![];
                loop {
                    match b.get(i) {
                        None => return Err((start, "unclosed string")),
                        Some(b'"') => {
                            i += 1;
                            break;
                        }
                        Some(b'\\') => {
                            v.push(*b.get(i + 1).ok_or((start, "unclosed string"))?);
                            i += 2;
                        }
                        Some(c) => {
                            v.push(*c);
                            i += 1;
                        }
                    }
                }
                out.opaque.push((start, i));
                Tok::Str(text_of(v, start)?)
            }
            c @ (b'[' | b']' | b'(' | b')' | b'{' | b'}' | b',' | b';') => {
                i += 1;
                Tok::Punct(c)
            }
            b':' => {
                i += 1;
                while i < b.len() && word(b[i]) {
                    i += 1;
                }
                if i == start + 1 || !b[start + 1].is_ascii_alphabetic() && b[start + 1] != b'_' {
                    return Err((start, "invalid tag"));
                }
                Tok::Tag(text[start + 1..i].to_ascii_lowercase())
            }
            c if c.is_ascii_digit() => {
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let mut n: u64 = text[start..i]
                    .parse()
                    .map_err(|_| (start, "invalid number"))?;
                let scale = match b.get(i).map(u8::to_ascii_uppercase) {
                    Some(b'K') => 1 << 10,
                    Some(b'M') => 1 << 20,
                    Some(b'G') => 1 << 30,
                    _ => 1,
                };
                if scale > 1 {
                    i += 1;
                    n = n.checked_mul(scale).ok_or((start, "invalid number"))?;
                }
                Tok::Number(n)
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                while i < b.len() && word(b[i]) {
                    i += 1;
                }
                let ident = text[start..i].to_ascii_lowercase();
                if ident == "text" && b.get(i) == Some(&b':') {
                    i += 1;
                    while matches!(b.get(i), Some(b' ' | b'\t')) {
                        i += 1;
                    }
                    if b.get(i) == Some(&b'#') {
                        while i < b.len() && b[i] != b'\n' {
                            i += 1;
                        }
                    }
                    if b.get(i) == Some(&b'\r') {
                        i += 1;
                    }
                    if b.get(i) != Some(&b'\n') {
                        return Err((start, "malformed multi-line string"));
                    }
                    i += 1;
                    let mut v = vec![];
                    loop {
                        if i >= b.len() {
                            return Err((start, "unclosed multi-line string"));
                        }
                        let end = text[i..].find('\n').map(|n| i + n + 1).unwrap_or(b.len());
                        let row = &b[i..end];
                        let bare = row.strip_suffix(b"\n").unwrap_or(row);
                        let bare = bare.strip_suffix(b"\r").unwrap_or(bare);
                        i = end;
                        if bare == b"." {
                            break;
                        }
                        v.extend_from_slice(row.strip_prefix(b"..").map_or(row, |_| &row[1..]));
                    }
                    out.opaque.push((start, i));
                    Tok::Str(text_of(v, start)?)
                } else {
                    Tok::Ident(ident)
                }
            }
            _ => return Err((start, "unexpected character")),
        };
        out.tokens.push(Token { tok, start, end: i });
    }
    Ok(out)
}

struct Parser<'a> {
    tokens: &'a [Token],
    i: usize,
    depth: usize,
    end: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.i).map(|t| &t.tok)
    }
    fn at(&self) -> usize {
        self.tokens.get(self.i).map(|t| t.start).unwrap_or(self.end)
    }
    fn punct(&mut self, c: u8) -> Result<usize, Failure> {
        match self.tokens.get(self.i) {
            Some(Token {
                tok: Tok::Punct(p),
                end,
                ..
            }) if *p == c => {
                self.i += 1;
                Ok(*end)
            }
            _ => Err((self.at(), "malformed command")),
        }
    }
    fn deeper(&mut self) -> Result<(), Failure> {
        self.depth += 1;
        if self.depth > DEPTH {
            return Err((self.at(), "nesting too deep"));
        }
        Ok(())
    }
    fn commands(&mut self, top: bool) -> Result<Vec<Command>, Failure> {
        let mut out = vec![];
        loop {
            match self.peek() {
                None if top => return Ok(out),
                None => return Err((self.end, "unclosed block")),
                Some(Tok::Punct(b'}')) if !top => return Ok(out),
                _ => out.push(self.command()?),
            }
        }
    }
    fn command(&mut self) -> Result<Command, Failure> {
        let start = self.at();
        let Some(Tok::Ident(name)) = self.peek().cloned() else {
            return Err((start, "expected command"));
        };
        self.i += 1;
        let (args, tests) = self.arguments()?;
        let (block, end) = if self.peek() == Some(&Tok::Punct(b'{')) {
            self.i += 1;
            self.deeper()?;
            let block = self.commands(false)?;
            self.depth -= 1;
            (Some(block), self.punct(b'}')?)
        } else {
            (None, self.punct(b';')?)
        };
        Ok(Command {
            name,
            args,
            tests,
            block,
            start,
            end,
        })
    }
    fn arguments(&mut self) -> Result<(Vec<Arg>, Vec<Node>), Failure> {
        let mut args = vec![];
        loop {
            match self.peek().cloned() {
                Some(Tok::Str(s)) => {
                    self.i += 1;
                    args.push(Arg::Strings(vec![s]));
                }
                Some(Tok::Number(_)) => {
                    self.i += 1;
                    args.push(Arg::Number);
                }
                Some(Tok::Tag(t)) => {
                    self.i += 1;
                    args.push(Arg::Tag(t));
                }
                Some(Tok::Punct(b'[')) => {
                    self.i += 1;
                    let mut list = vec![];
                    loop {
                        let Some(Tok::Str(s)) = self.peek().cloned() else {
                            return Err((self.at(), "malformed string list"));
                        };
                        self.i += 1;
                        list.push(s);
                        if self.peek() == Some(&Tok::Punct(b',')) {
                            self.i += 1;
                        } else {
                            break;
                        }
                    }
                    self.punct(b']')?;
                    args.push(Arg::Strings(list));
                }
                _ => break,
            }
        }
        let tests = match self.peek() {
            Some(Tok::Ident(_)) => vec![self.test()?],
            Some(Tok::Punct(b'(')) => {
                self.i += 1;
                let mut tests = vec![self.test()?];
                while self.peek() == Some(&Tok::Punct(b',')) {
                    self.i += 1;
                    tests.push(self.test()?);
                }
                self.punct(b')')?;
                tests
            }
            _ => vec![],
        };
        Ok((args, tests))
    }
    fn test(&mut self) -> Result<Node, Failure> {
        let Some(Tok::Ident(name)) = self.peek().cloned() else {
            return Err((self.at(), "expected test"));
        };
        self.i += 1;
        self.deeper()?;
        let (args, tests) = self.arguments()?;
        self.depth -= 1;
        Ok(Node { name, args, tests })
    }
}

fn structure(commands: &[Command], top: bool) -> Result<(), Failure> {
    let mut leading = top;
    for (i, c) in commands.iter().enumerate() {
        let previous = i.checked_sub(1).map(|p| commands[p].name.as_str());
        match c.name.as_str() {
            "require" if leading => {
                if !matches!(c.args.as_slice(), [Arg::Strings(_)])
                    || !c.tests.is_empty()
                    || c.block.is_some()
                {
                    return Err((c.start, "malformed require"));
                }
                continue;
            }
            "require" => return Err((c.start, "require after other commands")),
            "if" | "elsif" => {
                if c.name == "elsif" && !matches!(previous, Some("if" | "elsif")) {
                    return Err((c.start, "elsif without if"));
                }
                if c.tests.len() != 1 || !c.args.is_empty() || c.block.is_none() {
                    return Err((c.start, "malformed condition"));
                }
            }
            "else" => {
                if !matches!(previous, Some("if" | "elsif"))
                    || !c.tests.is_empty()
                    || !c.args.is_empty()
                    || c.block.is_none()
                {
                    return Err((c.start, "malformed else"));
                }
            }
            _ => {}
        }
        leading = false;
        if let Some(block) = &c.block {
            structure(block, false)?;
        }
    }
    Ok(())
}

fn rule_name(text: &str, comments: &[(usize, usize)], from: usize, to: usize) -> Option<String> {
    comments
        .iter()
        .rev()
        .filter(|(s, _)| *s >= from && *s < to)
        .find_map(|(s, e)| {
            let c = &text[*s..*e];
            c.starts_with("## Flag:")
                .then(|| c.split_once("Rulename:").map(|(_, n)| n.trim().to_owned()))
                .flatten()
        })
}

pub(crate) fn parse(text: &str) -> Result<Script, Failure> {
    let lexed = lex(text)?;
    let mut parser = Parser {
        tokens: &lexed.tokens,
        i: 0,
        depth: 0,
        end: text.len(),
    };
    let commands = parser.commands(true)?;
    structure(&commands, true)?;
    let mut requires = BTreeSet::new();
    let mut require_end = 0;
    let mut rest = commands.into_iter().peekable();
    while let Some(c) = rest.next_if(|c| c.name == "require") {
        if let [Arg::Strings(list)] = c.args.as_slice() {
            requires.extend(list.iter().cloned());
        }
        let line_end = text[c.end..]
            .find('\n')
            .map(|n| c.end + n + 1)
            .unwrap_or(text.len());
        require_end = if lexed
            .tokens
            .iter()
            .any(|t| t.start >= c.end && t.start < line_end)
        {
            c.end
        } else {
            line_end
        };
    }
    let mut rules: Vec<Rule> = vec![];
    let mut previous_end = require_end;
    for c in rest {
        if matches!(c.name.as_str(), "elsif" | "else") {
            let rule = rules.last_mut().expect("validated chain");
            previous_end = c.end;
            rule.commands.push(c);
            continue;
        }
        let index = rules.len() + 1;
        let name = rule_name(text, &lexed.comments, previous_end, c.start)
            .unwrap_or_else(|| format!("Rule {index}"));
        previous_end = c.end;
        rules.push(Rule {
            line: line(text, c.start),
            name,
            commands: vec![c],
        });
    }
    let unique_ids = lexed
        .comments
        .iter()
        .filter_map(|(s, e)| {
            let c = &text[*s..*e];
            let id = c.strip_prefix("## Flag:")?.split_once("UniqueId:")?.1;
            id.split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|v| v.parse().ok())
        })
        .collect();
    Ok(Script {
        requires,
        require_end,
        rules,
        unique_ids,
    })
}

fn tag(args: &[Arg], name: &str) -> bool {
    args.iter().any(|a| matches!(a, Arg::Tag(t) if t == name))
}
fn disposition(c: &Command) -> Option<ForeignDest> {
    let first = c.args.iter().find_map(|a| match a {
        Arg::Strings(s) => s.first().cloned(),
        _ => None,
    });
    match c.name.as_str() {
        "keep" => Some(ForeignDest::Inbox),
        "discard" => Some(ForeignDest::Discard),
        "reject" | "ereject" => Some(ForeignDest::Reject),
        "fileinto" if !tag(&c.args, "copy") => Some(match first {
            Some(name) if name.eq_ignore_ascii_case("INBOX") => ForeignDest::Inbox,
            Some(name) => ForeignDest::Folder { name },
            None => ForeignDest::Inbox,
        }),
        "redirect" if !tag(&c.args, "copy") => Some(ForeignDest::Redirect),
        _ => None,
    }
}
const PASSIVE: &[&str] = &[
    "addflag",
    "setflag",
    "removeflag",
    "set",
    "vacation",
    "notify",
    "addheader",
    "deleteheader",
    "fileinto",
    "redirect",
];

#[derive(Clone, Copy, Default)]
struct Paths {
    clean: bool,
    disposed: bool,
    stopped: bool,
}
impl Paths {
    fn join(self, other: Self) -> Self {
        Self {
            clean: self.clean || other.clean,
            disposed: self.disposed || other.disposed,
            stopped: self.stopped || other.stopped,
        }
    }
}
fn chain_end(commands: &[Command], i: usize) -> usize {
    let mut j = i + 1;
    while j < commands.len() && matches!(commands[j].name.as_str(), "elsif" | "else") {
        j += 1;
    }
    j
}
fn flow(commands: &[Command], mut p: Paths) -> Result<Paths, Failure> {
    let mut i = 0;
    while i < commands.len() {
        let c = &commands[i];
        match c.name.as_str() {
            "if" => {
                let end = chain_end(commands, i);
                let entry = Paths {
                    stopped: false,
                    ..p
                };
                let mut out = Paths {
                    stopped: p.stopped,
                    ..Default::default()
                };
                let mut exhaustive = false;
                for branch in &commands[i..end] {
                    let literal = branch
                        .tests
                        .first()
                        .filter(|t| t.args.is_empty() && t.tests.is_empty());
                    if literal.is_some_and(|t| t.name == "false") {
                        continue;
                    }
                    out = out.join(flow(branch.block.as_deref().unwrap_or_default(), entry)?);
                    if branch.name == "else" || literal.is_some_and(|t| t.name == "true") {
                        exhaustive = true;
                        break;
                    }
                }
                if !exhaustive {
                    out = out.join(entry);
                }
                p = out;
                i = end;
                continue;
            }
            "stop" | "return" => {
                p.stopped |= p.clean || p.disposed;
                p.clean = false;
                p.disposed = false;
            }
            "include" => return Err((c.start, "existing include; its rules cannot be verified")),
            _ if disposition(c).is_some() => {
                if p.clean {
                    p.clean = false;
                    p.disposed = true;
                }
            }
            name if PASSIVE.contains(&name) => {}
            _ => return Err((c.start, "unrecognized command")),
        }
        if c.block.is_some() {
            return Err((c.start, "unrecognized command"));
        }
        i += 1;
    }
    Ok(p)
}
fn dispositions(commands: &[Command], out: &mut Vec<ForeignDest>) {
    for c in commands {
        if let Some(d) = disposition(c) {
            if !out.contains(&d) {
                out.push(d);
            }
        }
        if let Some(b) = &c.block {
            dispositions(b, out);
        }
    }
}

impl Script {
    /// Checks that Mailroom content appended after these rules runs only when no existing
    /// filing decision was made, so existing rules keep precedence.
    pub(crate) fn analyze(&self, script: &str) -> Result<Vec<ForeignRule>, Refusal> {
        let mut p = Paths {
            clean: true,
            ..Default::default()
        };
        let mut rules = vec![];
        for (index, rule) in self.rules.iter().enumerate() {
            let unsafe_rule = |detail| Refusal::UnsafeRule {
                script: script.into(),
                rule: rule.name.clone(),
                line: rule.line,
                detail,
            };
            let before = p;
            p = flow(&rule.commands, p).map_err(|(_, detail)| unsafe_rule(detail))?;
            if p.disposed && !before.disposed {
                return Err(unsafe_rule(
                    "files or keeps without stop, so later Mailroom rules would also act",
                ));
            }
            if !p.clean {
                return Err(Refusal::MailroomUnreachable {
                    script: script.into(),
                    rule: rule.name.clone(),
                    line: rule.line,
                });
            }
            let mut d = vec![];
            dispositions(&rule.commands, &mut d);
            rules.push(ForeignRule {
                index,
                name: rule.name.clone(),
                line: rule.line,
                may_stop: flow(
                    &rule.commands,
                    Paths {
                        clean: true,
                        ..Default::default()
                    },
                )
                .is_ok_and(|p| p.stopped),
                dispositions: d,
            });
        }
        Ok(rules)
    }
}

fn strings(arg: Option<&Arg>) -> Option<&[String]> {
    match arg {
        Some(Arg::Strings(s)) => Some(s),
        _ => None,
    }
}
/// Evaluates a foreign test three-valued over the proof's modeled subset.
fn truth(node: &Node, view: &MessageView<'_>) -> Truth {
    let plain = node.args.is_empty();
    let values = || node.tests.iter().map(|t| truth(t, view));
    match node.name.as_str() {
        "not" if plain && node.tests.len() == 1 => match truth(&node.tests[0], view) {
            Truth::Match => Truth::NoMatch,
            Truth::NoMatch => Truth::Match,
            Truth::Unknown => Truth::Unknown,
        },
        "allof" | "anyof" if plain && !node.tests.is_empty() => {
            let (decisive, other) = if node.name == "allof" {
                (Truth::NoMatch, Truth::Match)
            } else {
                (Truth::Match, Truth::NoMatch)
            };
            if values().any(|v| v == decisive) {
                decisive
            } else if values().any(|v| v == Truth::Unknown) {
                Truth::Unknown
            } else {
                other
            }
        }
        _ => leaf(node).map_or(Truth::Unknown, |t| t.evaluate(view, &BTreeSet::new())),
    }
}
fn leaf(node: &Node) -> Option<Test> {
    if !node.tests.is_empty() {
        return None;
    }
    let mut mode = "is";
    let mut part = "all";
    let mut tagged = false;
    let mut lists: Vec<&[String]> = vec![];
    let mut args = node.args.iter();
    while let Some(a) = args.next() {
        match a {
            Arg::Tag(t) => {
                tagged = true;
                match t.as_str() {
                    "is" | "contains" | "matches" => mode = t,
                    "all" | "domain" => part = t,
                    "comparator" => {
                        if strings(args.next())? != ["i;ascii-casemap"] {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
            Arg::Strings(s) => lists.push(s),
            Arg::Number => return None,
        }
    }
    let combine = |tests: Vec<Test>| Some(Test::Any(tests));
    let captured = |field: &str| {
        field != "message-id"
            && crate::census::HEADER_FIELDS
                .iter()
                .any(|h| h.eq_ignore_ascii_case(field))
    };
    match (node.name.as_str(), lists.as_slice()) {
        ("true", []) if !tagged => Some(Test::True),
        ("false", []) if !tagged => Some(Test::False),
        ("header", [fields, keys]) if part == "all" => {
            let mut tests = vec![];
            for f in *fields {
                let f = f.to_ascii_lowercase();
                if !captured(&f) {
                    return None;
                }
                for k in *keys {
                    tests.push(Test::Header {
                        field: f.clone(),
                        mode: mode.into(),
                        value: k.clone(),
                    });
                }
            }
            combine(tests)
        }
        ("address", [fields, keys]) => {
            let mut tests = vec![];
            for f in *fields {
                let f = f.to_ascii_lowercase();
                if !["from", "to", "cc", "delivered-to", "x-original-to"].contains(&f.as_str()) {
                    return None;
                }
                for k in *keys {
                    tests.push(Test::Address {
                        field: f.clone(),
                        part: part.into(),
                        mode: mode.into(),
                        value: k.clone(),
                    });
                }
            }
            combine(tests)
        }
        ("envelope", [fields, keys]) if fields.iter().all(|f| f.eq_ignore_ascii_case("to")) => {
            combine(
                keys.iter()
                    .map(|k| Test::Envelope {
                        part: part.into(),
                        mode: mode.into(),
                        value: k.clone(),
                    })
                    .collect(),
            )
        }
        ("exists", [fields]) if !tagged => {
            let mut tests = vec![];
            for f in *fields {
                let f = f.to_ascii_lowercase();
                if !captured(&f) && f != "message-id" {
                    return None;
                }
                tests.push(Test::Exists(f));
            }
            Some(Test::All(tests))
        }
        _ => None,
    }
}

enum Run {
    Continue,
    Stop(ForeignDest),
    Unknown,
}
fn run(commands: &[Command], view: &MessageView<'_>, dest: &mut Option<ForeignDest>) -> Run {
    let mut i = 0;
    while i < commands.len() {
        let c = &commands[i];
        match c.name.as_str() {
            "if" => {
                let end = chain_end(commands, i);
                for (k, branch) in commands[i..end].iter().enumerate() {
                    let truth = if branch.name == "else" {
                        Truth::Match
                    } else {
                        truth(&branch.tests[0], view)
                    };
                    match truth {
                        Truth::Match => {
                            match run(branch.block.as_deref().unwrap_or_default(), view, dest) {
                                Run::Continue => break,
                                other => return other,
                            }
                        }
                        Truth::NoMatch => {}
                        Truth::Unknown => {
                            let stoppable = commands[i + k..end].iter().any(|b| {
                                flow(
                                    b.block.as_deref().unwrap_or_default(),
                                    Paths {
                                        clean: true,
                                        ..Default::default()
                                    },
                                )
                                .is_ok_and(|p| p.stopped)
                            });
                            if stoppable {
                                return Run::Unknown;
                            }
                            break;
                        }
                    }
                }
                i = end;
                continue;
            }
            "stop" | "return" => return Run::Stop(dest.clone().unwrap_or(ForeignDest::Inbox)),
            _ => {
                if let Some(d) = disposition(c) {
                    *dest = Some(d);
                }
            }
        }
        i += 1;
    }
    Run::Continue
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Taken {
    pub id: String,
    pub message_id_hash: Option<String>,
    pub rule: String,
    pub status: &'static str,
    pub existing: Option<ForeignDest>,
    pub mailroom: Destination,
    pub conflicting: bool,
    pub suppressed_copies: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub mailroom: usize,
    pub existing_wins: usize,
    pub conflicting: usize,
    pub undetermined: usize,
    pub suppressed_copies: usize,
}
#[derive(Serialize)]
pub struct Precedence {
    pub semantics: &'static str,
    pub placement: Placement,
    pub host: String,
    pub foreign_rules: Vec<ForeignRule>,
    pub decisions: Vec<Taken>,
    pub counts: Counts,
    pub pins: Vec<PinResult>,
    pub gate: &'static str,
    pub warnings: Vec<String>,
}

/// Evaluates the composed order: existing rules first, Mailroom's proven decisions only where
/// no existing rule stopped processing.
pub(crate) fn precedence(
    proof: &Proof,
    census: &Census,
    foreign: &Script,
    host: &str,
    placement: &Placement,
) -> Result<Precedence, Refusal> {
    let foreign_rules = foreign.analyze(host)?;
    let artifact = &proof.artifact;
    let input: std::collections::BTreeMap<_, _> = proof::messages(census, false)
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();
    let mut decisions = vec![];
    let mut counts = Counts::default();
    for d in &proof.decisions {
        let m = input.get(&d.id).ok_or(Refusal::ProofBlocked)?;
        let view = proof::view(artifact, m);
        let mut outcome = None;
        for rule in &foreign.rules {
            let mut dest = None;
            match run(&rule.commands, &view, &mut dest) {
                Run::Continue => continue,
                Run::Stop(d) => outcome = Some((rule.name.clone(), Some(d))),
                Run::Unknown => outcome = Some((rule.name.clone(), None)),
            }
            break;
        }
        let Some((rule, existing)) = outcome else {
            counts.mailroom += 1;
            continue;
        };
        let conflicting = match &existing {
            Some(ForeignDest::Inbox) => d.destination != Destination::Inbox,
            Some(ForeignDest::Folder { name }) => {
                d.destination != Destination::Folder { name: name.clone() }
            }
            Some(_) => true,
            None => false,
        };
        if existing.is_some() {
            counts.existing_wins += 1;
        } else {
            counts.undetermined += 1;
        }
        counts.conflicting += usize::from(conflicting);
        counts.suppressed_copies += d.copies.len();
        decisions.push(Taken {
            id: d.id.clone(),
            message_id_hash: d.message_id_hash.clone(),
            rule,
            status: if existing.is_some() {
                "existing_rule_wins"
            } else {
                "undetermined"
            },
            existing,
            mailroom: d.destination.clone(),
            conflicting,
            suppressed_copies: d.copies.len(),
        });
    }
    let pins: Vec<PinResult> = proof
        .pins
        .iter()
        .map(|p| {
            let affected: Vec<&Taken> = decisions
                .iter()
                .filter(|t| t.message_id_hash.as_ref() == Some(&p.message_id_hash))
                .collect();
            if affected.is_empty() || p.status == "stale" {
                return PinResult {
                    account: p.account.clone(),
                    message_id_hash: p.message_id_hash.clone(),
                    status: p.status,
                    expected: p.expected.clone(),
                    actual: p.actual.clone(),
                };
            }
            let mut actual = vec![];
            let mut status = p.status;
            for t in &affected {
                match &t.existing {
                    Some(ForeignDest::Inbox) => actual.push(Destination::Inbox),
                    Some(ForeignDest::Folder { name }) => {
                        actual.push(Destination::Folder { name: name.clone() })
                    }
                    Some(_) => status = "fail",
                    None => status = "unknown",
                }
            }
            if status == "pass" && actual.iter().any(|a| *a != p.expected) {
                status = "fail";
            }
            PinResult {
                account: p.account.clone(),
                message_id_hash: p.message_id_hash.clone(),
                status,
                expected: p.expected.clone(),
                actual,
            }
        })
        .collect();
    let mut warnings = vec![
        "Existing rules run first. Mailroom content runs only when no existing rule stopped processing. Existing tests outside the modeled header, address, envelope and exists subset are undetermined.".to_owned(),
        "Foreign syntax is parsed locally. Provider acceptance, OX editor behavior and delivery are unverified.".to_owned(),
    ];
    if counts.existing_wins > 0 {
        warnings.push(format!(
            "Existing rules take precedence for {} census messages, {} with a different destination",
            counts.existing_wins, counts.conflicting
        ));
    }
    if counts.undetermined > 0 {
        warnings.push(format!(
            "Precedence is undetermined for {} census messages",
            counts.undetermined
        ));
    }
    if counts.suppressed_copies > 0 {
        warnings.push(format!(
            "Existing rules would stop {} Mailroom accounting copies; accounting copies must stay unaffected",
            counts.suppressed_copies
        ));
    }
    let gate = if proof.gate == "blocked"
        || counts.suppressed_copies > 0
        || pins.iter().any(|p| matches!(p.status, "fail" | "unknown"))
    {
        "blocked"
    } else {
        "warnings"
    };
    Ok(Precedence {
        semantics: SEMANTICS,
        placement: placement.clone(),
        host: host.into(),
        foreign_rules,
        decisions,
        counts,
        pins,
        gate,
        warnings,
    })
}

/// Mailroom segments located by their exact markers.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Found {
    pub require: bool,
    pub block: bool,
    pub include: Option<String>,
}
fn located(text: &str) -> Result<(Vec<(usize, usize)>, Found), &'static str> {
    let mut ranges = vec![];
    let mut found = Found::default();
    let mut from = 0;
    while let Some(n) = text[from..].find(MARK) {
        let at = from + n;
        let segment = [&REQUIRE, &BLOCK, &INCLUDE]
            .into_iter()
            .find(|s| at >= 1 && text[at - 1..].starts_with(s.0))
            .ok_or("unexpected Mailroom marker")?;
        let body = at - 1 + segment.0.len();
        let next = text[body..]
            .find(MARK)
            .map(|n| body + n)
            .ok_or("Mailroom segment has no end marker")?;
        if !text[next..].starts_with(segment.1) || text.as_bytes()[next - 1] != b'\n' {
            return Err("Mailroom segment end marker is damaged");
        }
        let end = next + segment.1.len();
        let content = &text[body..next];
        match segment.0 {
            s if s == REQUIRE.0 && !found.require => found.require = true,
            s if s == BLOCK.0 && !found.block && found.include.is_none() => found.block = true,
            s if s == INCLUDE.0 && !found.block && found.include.is_none() => {
                let name = content
                    .strip_prefix("include :personal \"")
                    .and_then(|v| v.strip_suffix("\";\n"))
                    .filter(|v| !v.contains(['"', '\\']))
                    .ok_or("Mailroom include line is damaged")?;
                found.include = Some(name.into());
            }
            _ => return Err("duplicate Mailroom segment"),
        }
        ranges.push((at - 1, end));
        from = end;
    }
    Ok((ranges, found))
}

/// Separates the exact foreign bytes from Mailroom's marked segments.
pub(crate) fn strip(host: &[u8], script: &str) -> Result<(String, Found), Refusal> {
    let text = std::str::from_utf8(host).map_err(|_| Refusal::Unparseable {
        script: script.into(),
        line: 1,
        detail: "invalid UTF-8",
    })?;
    let damaged = |detail| Refusal::MarkerDamaged {
        script: script.into(),
        detail,
    };
    let (ranges, found) = located(text).map_err(damaged)?;
    let lexed = lex(text).map_err(|(at, detail)| Refusal::Unparseable {
        script: script.into(),
        line: line(text, at),
        detail,
    })?;
    if lexed.opaque.iter().any(|(s, e)| {
        ranges
            .iter()
            .any(|(a, b)| s < b && a < e && (s < a || e > b))
    }) {
        return Err(damaged("Mailroom marker inside foreign string or comment"));
    }
    let mut foreign = String::with_capacity(text.len());
    let mut at = 0;
    for (s, e) in &ranges {
        foreign.push_str(&text[at..*s]);
        at = *e;
    }
    foreign.push_str(&text[at..]);
    Ok((foreign, found))
}

pub(crate) fn parse_foreign(foreign: &str, script: &str) -> Result<Script, Refusal> {
    parse(foreign).map_err(|(at, detail)| Refusal::Unparseable {
        script: script.into(),
        line: line(foreign, at),
        detail,
    })
}

pub(crate) struct Composed {
    pub host: Vec<u8>,
    pub owned: Option<(String, Vec<u8>)>,
}

/// Inserts Mailroom's missing requirements after the leading foreign `require` commands and
/// appends Mailroom content after every foreign rule. Foreign bytes are never rewritten.
pub(crate) fn compose(
    foreign: &str,
    script: &Script,
    placement: &Placement,
    executable: &str,
    host_name: &str,
) -> Result<Composed, Refusal> {
    if executable.contains(MARK) {
        return Err(Refusal::MailroomContentInvalid);
    }
    let own = parse(executable).map_err(|_| Refusal::MailroomContentInvalid)?;
    let body_start = executable[own.require_end..]
        .find("\n#@ ")
        .map_or(executable.len(), |n| own.require_end + n);
    let (needed, tail): (BTreeSet<String>, String) = match placement {
        Placement::Block {} => {
            let mut next = script.unique_ids.iter().max().map_or(0, |m| m + 1);
            let mut body = String::new();
            for row in executable[body_start..].split_inclusive('\n') {
                if let Some(id) = row.strip_prefix("#@ ") {
                    body.push_str(&format!(
                        "## Flag: |UniqueId:{next}|Rulename: Mailroom {}\n",
                        id.trim_end()
                    ));
                    next += 1;
                }
                body.push_str(row);
            }
            if !body.ends_with('\n') {
                body.push('\n');
            }
            (
                own.requires.clone(),
                format!("{}{}{}", BLOCK.0, body, BLOCK.1),
            )
        }
        Placement::Include { script: name } => {
            if name.contains(['"', '\\']) {
                return Err(Refusal::InvalidPlacement);
            }
            (
                BTreeSet::from(["include".into()]),
                format!("{}include :personal \"{name}\";\n{}", INCLUDE.0, INCLUDE.1),
            )
        }
    };
    let missing: Vec<String> = needed
        .difference(&script.requires)
        .map(|r| super::sieve::quote(r))
        .collect();
    let mut host = String::with_capacity(foreign.len() + tail.len() + 128);
    host.push_str(&foreign[..script.require_end]);
    if !missing.is_empty() {
        host.push_str(&format!(
            "{}require [{}];\n{}",
            REQUIRE.0,
            missing.join(", "),
            REQUIRE.1
        ));
    }
    host.push_str(&foreign[script.require_end..]);
    host.push_str(&tail);
    let (round_trip, _) = strip(host.as_bytes(), host_name)?;
    if round_trip != foreign || parse(&host).is_err() {
        return Err(Refusal::MailroomContentInvalid);
    }
    Ok(Composed {
        host: host.into_bytes(),
        owned: placement
            .owned()
            .map(|name| (name.to_owned(), format!("{OWNED}{executable}").into_bytes())),
    })
}

#[cfg(test)]
mod tests;
