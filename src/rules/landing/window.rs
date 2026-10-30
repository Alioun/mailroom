use super::{Comparison, Counts, Prediction, Row, proof};
use crate::census::{
    Census,
    deployment::{ActiveObservation, ObservationVia},
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrivalPeriod {
    Unavailable,
    BeforeOrAtObservation,
    AfterObservation,
    AfterRefresh,
    InvalidDate,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArrivalWindow {
    Unavailable {
        reason: WindowUnavailable,
    },
    Timed {
        after: i64,
        through: i64,
        via: ObservationVia,
        counts: Counts,
    },
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowUnavailable {
    NoActiveObservation,
    RefreshPredatesObservation,
}

pub(super) fn periods(
    census: &Census,
    observation: Option<ActiveObservation>,
    through: i64,
) -> BTreeMap<String, ArrivalPeriod> {
    proof::messages(census, false)
        .into_iter()
        .map(|message| {
            let period = match observation {
                None => ArrivalPeriod::Unavailable,
                Some(o) if through < o.at => ArrivalPeriod::Unavailable,
                Some(o) => match chrono::DateTime::parse_from_str(
                    &message.record.internal_date,
                    "%d-%b-%Y %H:%M:%S %z",
                ) {
                    Err(_) => ArrivalPeriod::InvalidDate,
                    Ok(date) if date.timestamp() > through => ArrivalPeriod::AfterRefresh,
                    Ok(date) if date.timestamp() <= o.at => ArrivalPeriod::BeforeOrAtObservation,
                    Ok(_) => ArrivalPeriod::AfterObservation,
                },
            };
            (message.id, period)
        })
        .collect()
}

pub(super) fn summarize(
    observation: Option<ActiveObservation>,
    through: i64,
    rows: &[Row],
) -> ArrivalWindow {
    let Some(observation) = observation else {
        return ArrivalWindow::Unavailable {
            reason: WindowUnavailable::NoActiveObservation,
        };
    };
    if through < observation.at {
        return ArrivalWindow::Unavailable {
            reason: WindowUnavailable::RefreshPredatesObservation,
        };
    }
    let mut counts = Counts::default();
    for row in rows
        .iter()
        .filter(|r| r.arrival_period == ArrivalPeriod::AfterObservation)
    {
        match row.prediction {
            Prediction::Uncheckable { .. } => counts.uncheckable += 1,
            Prediction::Folder {
                comparison: Comparison::Matches,
                ..
            } => counts.matches += 1,
            Prediction::Folder {
                comparison: Comparison::Elsewhere,
                ..
            } => counts.elsewhere += 1,
            Prediction::Folder {
                comparison: Comparison::AdditionalFolders,
                ..
            } => counts.additional_folders += 1,
        }
    }
    ArrivalWindow::Timed {
        after: observation.at,
        through,
        via: observation.via,
        counts,
    }
}
