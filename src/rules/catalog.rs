use super::config::{Category, Condition, Preset, Rule, Stage};
use crate::sender_data::Catalog;
use std::collections::BTreeSet;

pub(super) struct Prepared {
    pub preset: Preset,
    pub version: Option<String>,
    pub authored_rules: usize,
}

pub(super) fn load(preset: &Preset) -> Result<Prepared, String> {
    match preset.core_senders {
        None => Ok(Prepared {
            preset: preset.clone(),
            version: None,
            authored_rules: preset.rules.len(),
        }),
        Some(_) => prepare(
            preset,
            &crate::sender_data::bundled().map_err(|e| e.to_string())?,
        ),
    }
}

pub(super) fn prepare(preset: &Preset, catalog: &Catalog) -> Result<Prepared, String> {
    let selected = preset
        .core_senders
        .as_ref()
        .ok_or("Preset has no core sender selection")?;
    if selected.major == 0
        || catalog.version().split('.').next() != Some(selected.major.to_string().as_str())
    {
        return Err(
            "Core sender major changed. Review the new release before changing its pin".into(),
        );
    }
    let authored_rules = preset.rules.len();
    let mut preset = preset.clone();
    for category in Category::ALL {
        preset.rules.push(Rule {
            id: format!("sender.core.{}", category.key()),
            category,
            stage: Stage::From,
            when: vec![
                Condition::SenderSet {
                    addresses: BTreeSet::new(),
                    domains: catalog
                        .senders()
                        .filter(|s| s.category == category.key())
                        .map(|s| s.domain.clone())
                        .collect(),
                },
                Condition::NonBulk,
            ],
            folder: None,
            enabled: true,
            accounts: BTreeSet::new(),
            retain: false,
            mark_read: false,
        });
    }
    Ok(Prepared {
        preset,
        version: Some(catalog.binding()),
        authored_rules,
    })
}

#[cfg(test)]
mod tests;
