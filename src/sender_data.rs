use crate::winddown::{SenderData, SenderEntry};
use chrono::{DateTime, Months, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const CATEGORIES: [&str; 6] = [
    "money",
    "contracts",
    "orders",
    "travel",
    "social",
    "newsletters",
];
const HEADER: [&str; 6] = [
    "domain",
    "country",
    "priority",
    "settings_url",
    "source",
    "checked",
];
const SERVICE_HEADER: [&str; 3] = ["domain", "source", "checked"];
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 10_000;
const COUNTRIES: &str = "AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BV BW BY BZ CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU ID IE IL IM IN IO IQ IR IS IT JE JM JO JP KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO RS RU RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW TZ UA UG UM US UY UZ VA VC VE VG VI VN VU WF WS YE YT ZA ZM ZW";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Release,
    Csv,
    Domain,
    Country,
    Priority,
    Link,
    Source,
    Date,
    Duplicate,
    ServiceWithoutSender,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bundled sender catalog refused: {self:?}")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    schema: u32,
    version: String,
    released: String,
    license: String,
}
pub struct Package<'a> {
    pub release: &'a str,
    pub license: &'a str,
    pub sources: &'a str,
    pub categories: [&'a str; 6],
    pub services: &'a str,
}
#[derive(Debug)]
pub struct Sender {
    pub domain: String,
    pub category: &'static str,
    pub country: String,
    pub priority: u8,
    pub settings_url: Option<String>,
    pub source: String,
    pub checked: NaiveDate,
}
pub struct Catalog {
    version: String,
    digest: String,
    senders: BTreeMap<String, Sender>,
    services: BTreeMap<String, NaiveDate>,
}
#[derive(Clone, Serialize)]
pub struct Status {
    pub release: String,
    pub license: &'static str,
    pub rows: usize,
    pub categories: BTreeMap<&'static str, usize>,
    pub missing_categories: Vec<&'static str>,
    pub missing_countries: Vec<&'static str>,
    pub seed_complete: bool,
    pub stale_rows: usize,
    pub future_rows: usize,
    pub service_reviews: usize,
    pub unusable_service_reviews: usize,
}

pub fn bundled() -> Result<Catalog, Error> {
    Catalog::parse(Package {
        release: include_str!("../data/release.yaml"),
        license: include_str!("../data/LICENSE"),
        sources: include_str!("../data/SOURCES.md"),
        categories: [
            include_str!("../data/money.csv"),
            include_str!("../data/contracts.csv"),
            include_str!("../data/orders.csv"),
            include_str!("../data/travel.csv"),
            include_str!("../data/social.csv"),
            include_str!("../data/newsletters.csv"),
        ],
        services: include_str!("../data/transactional-services.csv"),
    })
}

impl Catalog {
    pub fn parse(package: Package<'_>) -> Result<Self, Error> {
        let total = package.release.len()
            + package.license.len()
            + package.sources.len()
            + package.services.len()
            + package.categories.iter().map(|c| c.len()).sum::<usize>();
        if total > MAX_BYTES {
            return Err(Error::Release);
        }
        let release: Release =
            serde_yaml_ng::from_str(package.release).map_err(|_| Error::Release)?;
        let parts: Vec<_> = release.version.split('.').collect();
        if release.schema != 1
            || release.license != "CC0-1.0"
            || !package.license.contains("SPDX-License-Identifier: CC0-1.0")
            || package.sources.trim().is_empty()
            || parts.len() != 3
            || parts.iter().any(|p| {
                p.is_empty()
                    || p.parse::<u32>().is_err()
                    || (p.len() > 1 && p.starts_with('0'))
                    || !p.bytes().all(|b| b.is_ascii_digit())
            })
        {
            return Err(Error::Release);
        }
        let released = date(&release.released)?;
        let mut senders = BTreeMap::new();
        for (category, content) in CATEGORIES.into_iter().zip(package.categories) {
            let mut rows = csv(content)?.into_iter();
            if !rows
                .next()
                .is_some_and(|row| row.iter().map(String::as_str).eq(HEADER))
            {
                return Err(Error::Csv);
            }
            for row in rows {
                let [domain, country, priority, settings, origin, checked]: [String; 6] =
                    row.try_into().map_err(|_| Error::Csv)?;
                valid_domain(&domain)?;
                if country != "*" && !COUNTRIES.split(' ').any(|c| c == country) {
                    return Err(Error::Country);
                }
                let priority = match priority.as_str() {
                    "1" => 1,
                    "2" => 2,
                    "3" => 3,
                    _ => return Err(Error::Priority),
                };
                if (!settings.is_empty() && !safe_link(&settings))
                    || (priority == 1 && settings.is_empty())
                {
                    return Err(Error::Link);
                }
                source(&origin)?;
                let checked = date(&checked)?;
                if checked > released {
                    return Err(Error::Date);
                }
                let sender = Sender {
                    domain: domain.clone(),
                    category,
                    country,
                    priority,
                    settings_url: (!settings.is_empty()).then_some(settings),
                    source: origin,
                    checked,
                };
                if senders.insert(domain, sender).is_some() {
                    return Err(Error::Duplicate);
                }
                if senders.len() > MAX_ROWS {
                    return Err(Error::Csv);
                }
            }
        }
        let mut services = BTreeMap::new();
        let mut rows = csv(package.services)?.into_iter();
        if !rows
            .next()
            .is_some_and(|row| row.iter().map(String::as_str).eq(SERVICE_HEADER))
        {
            return Err(Error::Csv);
        }
        for row in rows {
            let [domain, origin, checked]: [String; 3] = row.try_into().map_err(|_| Error::Csv)?;
            if !senders.contains_key(&domain) {
                return Err(Error::ServiceWithoutSender);
            }
            source(&origin)?;
            let checked = date(&checked)?;
            if checked > released {
                return Err(Error::Date);
            }
            if services.insert(domain, checked).is_some() {
                return Err(Error::Duplicate);
            }
        }
        let bytes = serde_json::to_vec(&(
            "sender-core-v1",
            package.release,
            package.license,
            package.sources,
            package.categories,
            package.services,
        ))
        .map_err(|_| Error::Release)?;
        Ok(Self {
            version: release.version,
            digest: data_encoding::HEXLOWER.encode(&Sha256::digest(bytes)),
            senders,
            services,
        })
    }

    pub fn senders(&self) -> impl Iterator<Item = &Sender> {
        self.senders.values()
    }

    pub fn winddown(&self, as_of: i64) -> Result<SenderData, Error> {
        let day = DateTime::<Utc>::from_timestamp(as_of, 0)
            .ok_or(Error::Date)?
            .date_naive();
        let cutoff = day.checked_sub_months(Months::new(24)).ok_or(Error::Date)?;
        let current = |checked: NaiveDate| checked > cutoff && checked <= day;
        let categories: BTreeMap<_, _> = CATEGORIES
            .into_iter()
            .map(|category| {
                (
                    category,
                    self.senders
                        .values()
                        .filter(|s| s.category == category)
                        .count(),
                )
            })
            .collect();
        let missing_categories: Vec<_> = CATEGORIES
            .into_iter()
            .filter(|c| categories[c] < 20)
            .collect();
        let missing_countries: Vec<_> = ["DE", "AT", "CH", "*"]
            .into_iter()
            .filter(|c| !self.senders.values().any(|s| s.country == *c))
            .collect();
        let status = Status {
            release: self.version.clone(),
            license: "CC0-1.0",
            rows: self.senders.len(),
            seed_complete: missing_categories.is_empty() && missing_countries.is_empty(),
            categories,
            missing_categories,
            missing_countries,
            stale_rows: self
                .senders
                .values()
                .filter(|s| s.checked <= cutoff)
                .count(),
            future_rows: self.senders.values().filter(|s| s.checked > day).count(),
            service_reviews: self.services.len(),
            unusable_service_reviews: self
                .services
                .iter()
                .filter(|(d, checked)| !current(**checked) || !current(self.senders[*d].checked))
                .count(),
        };
        let reviewed = self
            .senders
            .values()
            .map(|sender| {
                (
                    sender.domain.clone(),
                    SenderEntry {
                        transactional: current(sender.checked)
                            && self
                                .services
                                .get(&sender.domain)
                                .is_some_and(|checked| current(*checked)),
                        settings_link: sender.settings_url.clone(),
                    },
                )
            })
            .collect();
        Ok(SenderData {
            version: format!("sender-core-v1/{}/{}", self.version, self.digest),
            reviewed,
            catalog: Some(status),
        })
    }
}

fn date(value: &str) -> Result<NaiveDate, Error> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| Error::Date)?;
    if date.format("%Y-%m-%d").to_string() != value {
        return Err(Error::Date);
    }
    Ok(date)
}
fn source(value: &str) -> Result<(), Error> {
    if value == "contrib"
        || value.strip_prefix("wikidata:Q").is_some_and(|n| {
            !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
        })
    {
        Ok(())
    } else {
        Err(Error::Source)
    }
}
fn valid_domain(domain: &str) -> Result<(), Error> {
    if crate::rules::config::domain(domain)
        && psl::domain_str(domain) == Some(domain)
        && psl::suffix(domain.as_bytes()).is_some_and(|s| s.typ().is_some())
    {
        Ok(())
    } else {
        Err(Error::Domain)
    }
}
fn safe_link(value: &str) -> bool {
    if value.len() > 2048
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
        || value.contains(['?', '#', '\\'])
    {
        return false;
    }
    value.parse::<axum::http::Uri>().is_ok_and(|uri| {
        uri.scheme_str() == Some("https")
            && uri.authority().is_some_and(|a| {
                let host = a.host();
                !a.as_str().contains('@')
                    && crate::rules::config::domain(host)
                    && host.parse::<std::net::IpAddr>().is_err()
                    && psl::suffix(host.as_bytes()).is_some_and(|s| s.typ().is_some())
            })
    })
}

fn csv(input: &str) -> Result<Vec<Vec<String>>, Error> {
    #[derive(Clone, Copy)]
    enum State {
        Start,
        Plain,
        Quoted,
        Closed,
    }
    let mut state = State::Start;
    let mut field = String::new();
    let mut row = Vec::new();
    let mut rows = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match (state, c) {
            (State::Quoted, '"') => state = State::Closed,
            (State::Closed, '"') => {
                field.push('"');
                state = State::Quoted;
            }
            (State::Quoted, c) => field.push(c),
            (_, ',') => {
                row.push(std::mem::take(&mut field));
                state = State::Start;
            }
            (_, '\r' | '\n') => {
                if c == '\r' && chars.next() != Some('\n') {
                    return Err(Error::Csv);
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                state = State::Start;
                if rows.len() > MAX_ROWS + 1 {
                    return Err(Error::Csv);
                }
            }
            (State::Start, '"') => state = State::Quoted,
            (State::Closed, _) | (State::Plain, '"') => return Err(Error::Csv),
            (_, c) => {
                field.push(c);
                state = State::Plain;
            }
        }
        if row.len() > HEADER.len() || field.len() > 4096 {
            return Err(Error::Csv);
        }
    }
    if matches!(state, State::Quoted) {
        return Err(Error::Csv);
    }
    if !row.is_empty() || !field.is_empty() || matches!(state, State::Closed) {
        row.push(field);
        rows.push(row);
    }
    if rows.is_empty()
        || rows
            .iter()
            .flatten()
            .any(|s| s.chars().any(char::is_control))
    {
        return Err(Error::Csv);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
