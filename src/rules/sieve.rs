use crate::census::HeaderRecord;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub enum Test {
    True,
    False,
    All(Vec<Test>),
    Any(Vec<Test>),
    Not(Box<Test>),
    Exists(String),
    Count(String),
    Address {
        field: String,
        part: String,
        mode: String,
        value: String,
    },
    Header {
        field: String,
        mode: String,
        value: String,
    },
    Duplicate(String),
}
#[derive(Clone, Debug)]
pub enum Action {
    Keep,
    File(String),
    Copy(String),
    Seen,
    Stop,
}
#[derive(Clone, Debug)]
pub struct Statement {
    pub id: String,
    pub test: Test,
    pub actions: Vec<Action>,
}

pub fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
impl Test {
    pub fn render(&self) -> String {
        match self {
            Self::True => "true".into(),
            Self::False => "false".into(),
            Self::All(t) | Self::Any(t) => format!(
                "{} ({})",
                if matches!(self, Self::All(_)) {
                    "allof"
                } else {
                    "anyof"
                },
                t.iter().map(Self::render).collect::<Vec<_>>().join(", ")
            ),
            Self::Not(t) => format!("not {}", t.render()),
            Self::Exists(f) => format!("exists {}", quote(f)),
            Self::Count(f) => format!(
                "address :count \"eq\" :comparator \"i;ascii-numeric\" {} \"1\"",
                quote(f)
            ),
            Self::Address {
                field,
                part,
                mode,
                value,
            } => format!(
                "address :{part} :{mode} :comparator \"i;ascii-casemap\" {} {}",
                quote(field),
                quote(value)
            ),
            Self::Header { field, mode, value } => format!(
                "header :{mode} :comparator \"i;ascii-casemap\" {} {}",
                quote(field),
                quote(value)
            ),
            Self::Duplicate(handle) => {
                format!("duplicate :header \"message-id\" :handle {}", quote(handle))
            }
        }
    }
    pub fn evaluate(&self, h: &HeaderRecord, duplicates: &BTreeSet<String>) -> bool {
        let addresses = |field: &str| match field {
            "from" => &h.from,
            "to" => &h.to,
            "cc" => &h.cc,
            "delivered-to" => &h.delivered_to,
            "x-original-to" => &h.original_to,
            _ => unreachable!("validated Sieve header"),
        };
        match self {
            Self::True => true,
            Self::False => false,
            Self::All(t) => t.iter().all(|t| t.evaluate(h, duplicates)),
            Self::Any(t) => t.iter().any(|t| t.evaluate(h, duplicates)),
            Self::Not(t) => !t.evaluate(h, duplicates),
            Self::Exists(f) if f == "message-id" => h.message_id_hash.is_some(),
            Self::Exists(f) => h.evidence.contains_key(f),
            Self::Count(f) => addresses(f).len() == 1,
            Self::Address {
                field,
                part,
                mode,
                value,
            } => addresses(field).iter().any(|a| {
                let v = if part == "domain" {
                    a.address.rsplit_once('@').map(|(_, d)| d).unwrap_or("")
                } else {
                    &a.address
                };
                compare(v, value, mode)
            }),
            Self::Header { field, mode, value } if field == "subject" => {
                compare(&h.subject, value, mode)
            }
            Self::Header { field, mode, value } => h.evidence.get(field).is_some_and(|values| {
                values.iter().any(|raw| {
                    let line = format!("{field}: {raw}");
                    let decoded = mailparse::parse_header(line.as_bytes())
                        .map(|(h, _)| h.get_value())
                        .unwrap_or_default();
                    compare(&decoded, value, mode)
                })
            }),
            Self::Duplicate(handle) => h
                .message_id_hash
                .as_ref()
                .is_some_and(|hash| duplicates.contains(&format!("{handle}/{hash}"))),
        }
    }
}
fn compare(value: &str, pattern: &str, mode: &str) -> bool {
    let value = value.to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    match mode {
        "is" => value == pattern,
        "contains" => value.contains(&pattern),
        "matches" => glob(value.as_bytes(), pattern.as_bytes()),
        _ => unreachable!("validated Sieve match type"),
    }
}
fn glob(value: &[u8], pattern: &[u8]) -> bool {
    let mut row = vec![false; value.len() + 1];
    row[0] = true;
    for &p in pattern {
        let mut next = vec![false; row.len()];
        if p == b'*' {
            next[0] = row[0];
        }
        for i in 1..row.len() {
            next[i] = if p == b'*' {
                row[i] || next[i - 1]
            } else {
                row[i - 1] && (p == b'?' || p == value[i - 1])
            };
        }
        row = next;
    }
    row[value.len()]
}

pub fn render(statements: &[Statement], extensions: &BTreeSet<String>) -> String {
    let mut text = "# Local proof artifact. Provider deployment remains locked.\n".to_owned();
    if !extensions.is_empty() {
        text.push_str(&format!(
            "require [{}];\n",
            extensions
                .iter()
                .map(|s| quote(s))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for s in statements {
        text.push_str(&format!("\n#@ {}\nif {} {{\n", s.id, s.test.render()));
        for a in &s.actions {
            let command = match a {
                Action::Keep => "keep".into(),
                Action::File(f) => format!("fileinto {}", quote(f)),
                Action::Copy(t) => format!("redirect :copy {}", quote(t)),
                Action::Seen => "addflag \"\\\\Seen\"".into(),
                Action::Stop => "stop".into(),
            };
            text.push_str(&format!("  {command};\n"));
        }
        text.push_str("}\n");
    }
    text
}

// This parser consumes executable Sieve, never the source rule matcher.
pub fn parse(text: &str) -> Result<Vec<Statement>, String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '#' {
            let line: String = chars.by_ref().take_while(|c| *c != '\n').collect();
            if let Some(id) = line.strip_prefix("@ ") {
                tokens.push(format!("@{id}"));
            }
        } else if c == '"' {
            let mut value = String::from("\"");
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '"' {
                    closed = true;
                    break;
                }
                value.push(if c == '\\' {
                    chars.next().ok_or("Invalid Sieve escape")?
                } else {
                    c
                });
            }
            if !closed {
                return Err("Unclosed Sieve string".into());
            }
            tokens.push(value);
        } else if "[](){},;".contains(c) {
            tokens.push(c.to_string());
        } else {
            let mut value = c.to_string();
            while chars
                .peek()
                .is_some_and(|c| !c.is_whitespace() && !"[](){},;".contains(*c))
            {
                value.push(chars.next().unwrap());
            }
            tokens.push(value);
        }
    }
    let mut p = Parser { tokens, offset: 0 };
    if p.peek() == Some("require") {
        p.expect("require")?;
        p.expect("[")?;
        while p.peek() != Some("]") {
            p.string()?;
            if p.peek() == Some(",") {
                p.next()?;
            } else {
                break;
            }
        }
        p.expect("]")?;
        p.expect(";")?;
    }
    let mut statements = Vec::new();
    while p.peek().is_some() {
        let id = p
            .next()?
            .strip_prefix('@')
            .ok_or("Missing Sieve rule marker")?
            .to_owned();
        p.expect("if")?;
        let test = p.test()?;
        p.expect("{")?;
        let mut actions = Vec::new();
        while p.peek() != Some("}") {
            let a = match p.next()?.as_str() {
                "keep" => Action::Keep,
                "stop" => Action::Stop,
                "fileinto" => Action::File(p.string()?),
                "redirect" => {
                    p.expect(":copy")?;
                    Action::Copy(p.string()?)
                }
                "addflag" => {
                    if p.string()? != "\\Seen" {
                        return Err("Unsupported Sieve flag".into());
                    }
                    Action::Seen
                }
                _ => return Err("Unsupported Sieve action".into()),
            };
            p.expect(";")?;
            actions.push(a);
        }
        p.expect("}")?;
        statements.push(Statement { id, test, actions });
    }
    Ok(statements)
}
struct Parser {
    tokens: Vec<String>,
    offset: usize,
}
impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.offset).map(String::as_str)
    }
    fn next(&mut self) -> Result<String, String> {
        let v = self
            .tokens
            .get(self.offset)
            .cloned()
            .ok_or("Incomplete Sieve output")?;
        self.offset += 1;
        Ok(v)
    }
    fn expect(&mut self, expected: &str) -> Result<(), String> {
        if self.next()? == expected {
            Ok(())
        } else {
            Err("Unsupported or malformed Sieve output".into())
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.next()?
            .strip_prefix('"')
            .map(str::to_owned)
            .ok_or("Expected Sieve string".into())
    }
    fn field(&mut self) -> Result<String, String> {
        let field = self.string()?;
        if !["from", "to", "cc", "delivered-to", "x-original-to"].contains(&field.as_str()) {
            return Err("Unsupported Sieve address field".into());
        }
        Ok(field)
    }
    fn mode(&mut self) -> Result<String, String> {
        match self.next()?.as_str() {
            ":is" => Ok("is".into()),
            ":contains" => Ok("contains".into()),
            ":matches" => Ok("matches".into()),
            _ => Err("Unsupported Sieve match type".into()),
        }
    }
    fn comparator(&mut self, value: &str) -> Result<(), String> {
        self.expect(":comparator")?;
        if self.string()? == value {
            Ok(())
        } else {
            Err("Unsupported Sieve comparator".into())
        }
    }
    fn test(&mut self) -> Result<Test, String> {
        Ok(match self.next()?.as_str() {
            "true" => Test::True,
            "false" => Test::False,
            "not" => Test::Not(Box::new(self.test()?)),
            word @ ("allof" | "anyof") => {
                let all = word == "allof";
                self.expect("(")?;
                let mut tests = vec![self.test()?];
                while self.peek() == Some(",") {
                    self.next()?;
                    tests.push(self.test()?);
                }
                self.expect(")")?;
                if all {
                    Test::All(tests)
                } else {
                    Test::Any(tests)
                }
            }
            "exists" => Test::Exists(self.string()?),
            "duplicate" => {
                self.expect(":header")?;
                if self.string()? != "message-id" {
                    return Err("Unsupported duplicate key".into());
                }
                self.expect(":handle")?;
                Test::Duplicate(self.string()?)
            }
            "header" => {
                let mode = self.mode()?;
                self.comparator("i;ascii-casemap")?;
                Test::Header {
                    field: self.string()?,
                    mode,
                    value: self.string()?,
                }
            }
            "address" => {
                let part = self.next()?;
                if part == ":count" {
                    if self.string()? != "eq" {
                        return Err("Unsupported address count".into());
                    }
                    self.comparator("i;ascii-numeric")?;
                    let f = self.field()?;
                    if self.string()? != "1" {
                        return Err("Unsupported address count".into());
                    }
                    Test::Count(f)
                } else {
                    if part != ":all" && part != ":domain" {
                        return Err("Unsupported address part".into());
                    }
                    let mode = self.mode()?;
                    self.comparator("i;ascii-casemap")?;
                    Test::Address {
                        field: self.field()?,
                        part: part[1..].into(),
                        mode,
                        value: self.string()?,
                    }
                }
            }
            _ => return Err("Unsupported Sieve test".into()),
        })
    }
}
