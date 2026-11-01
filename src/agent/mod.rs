mod files;
mod protocol;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io, path::Path};

const SUMMARY_LIMIT: usize = 262_144;
const REQUEST_LIMIT: usize = 131_072;
const EDIT_LIMIT: usize = 20;

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recommendation {
    #[schemars(length(min = 1, max = 120))]
    title: String,
    #[schemars(length(min = 1, max = 2000))]
    reason: String,
    #[schemars(length(min = 1, max = 2000))]
    suggested_change: String,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proposal {
    #[schemars(range(min = 1, max = 1))]
    version: u8,
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub(crate) summary_sha256: String,
    #[schemars(length(min = 1, max = 20))]
    recommendations: Vec<Recommendation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rule_edits: Option<RuleEdits>,
}

/// Structured local rule edits for owner review: patches to existing preset
/// rules and new `user.*` filing rules. Nothing else in the override document
/// can be proposed.
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleEdits {
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub(crate) patches: Vec<crate::rules::config::Patch>,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub(crate) additions: Vec<crate::rules::config::Rule>,
}

fn digest(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
}

fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= limit
        && !value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}

impl Proposal {
    pub(crate) fn validate(&self, summary: &str) -> Result<(), &'static str> {
        if self.version != 1
            || self.summary_sha256 != digest(summary.as_bytes())
            || self.recommendations.is_empty()
            || self.recommendations.len() > 20
            || self.rule_edits.as_ref().is_some_and(|edits| {
                let count = edits.patches.len() + edits.additions.len();
                count == 0 || count > EDIT_LIMIT
            })
            || serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > REQUEST_LIMIT)
            || self.recommendations.iter().any(|r| {
                !text(&r.title, 120) || !text(&r.reason, 2000) || !text(&r.suggested_change, 2000)
            })
        {
            return Err("invalid_or_changed_proposal");
        }
        Ok(())
    }
}

pub fn run(arguments: &[String]) -> u8 {
    let [command, folder_flag, folder] = arguments else {
        eprintln!("status=invalid_agent_command");
        return 64;
    };
    if folder_flag != "--folder" || !matches!(command.as_str(), "init" | "serve") {
        eprintln!("status=invalid_agent_command");
        return 64;
    }
    let result = if command == "init" {
        files::Exchange::initialize(Path::new(folder)).map(|_| ())
    } else {
        files::Exchange::open(Path::new(folder))
            .and_then(|exchange| protocol::serve(exchange, io::stdin().lock(), io::stdout().lock()))
    };
    match result {
        Ok(()) => {
            if command == "init" {
                println!("status=agent_exchange_initialized");
            }
            0
        }
        Err(code) => {
            eprintln!("status={code}");
            65
        }
    }
}

#[cfg(test)]
mod tests;
