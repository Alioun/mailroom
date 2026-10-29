use crate::{
    acquisition::Provider,
    census::{Census, MessageRecord},
    rules::proof::{Decision, Destination},
};
use chrono::{DateTime, Datelike, Months, Utc};

pub(super) fn destination(
    census: &Census,
    provider: Provider,
    message: &MessageRecord,
    decision: &Decision,
    as_of: i64,
) -> Result<String, &'static str> {
    if decision.review {
        return Err("archive_review_hold");
    }
    if decision.destination != Destination::Inbox
        || decision.matched_rule.as_deref() != Some("system.unmatched")
    {
        return Err("archive_matched");
    }
    let now = DateTime::<Utc>::from_timestamp(as_of, 0).ok_or("archive_invalid_as_of")?;
    let cutoff = now
        .checked_sub_months(Months::new(12))
        .ok_or("archive_invalid_as_of")?;
    let date = DateTime::parse_from_str(&message.internal_date, "%d-%b-%Y %H:%M:%S %z")
        .map_err(|_| "archive_unknown_date")?;
    if date >= cutoff {
        return Err("archive_recent_or_future");
    }
    let role = if provider == Provider::Gmail {
        "all"
    } else {
        "archive"
    };
    let roots: Vec<_> = census
        .folder_roles
        .iter()
        .filter(|(_, roles)| roles.contains(role))
        .map(|(name, _)| name)
        .collect();
    let [root] = roots.as_slice() else {
        return Err("archive_destination_unavailable");
    };
    if provider == Provider::Gmail {
        return Ok((*root).clone());
    }
    if !matches!(root.as_str(), "Archive" | "Archiv") {
        return Err("archive_destination_unavailable");
    }
    Ok(format!("{root}/{:04}", date.year()))
}
