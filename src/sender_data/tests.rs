use super::*;

const RELEASE: &str = "schema: 1\nversion: 1.0.0\nreleased: 2026-10-29\nlicense: CC0-1.0\n";
const LICENSE: &str = "SPDX-License-Identifier: CC0-1.0";
const SOURCES: &str = "Synthetic catalog fixtures. No observed sender facts.";
const EMPTY: &str = "domain,country,priority,settings_url,source,checked\n";
const SERVICES: &str = "domain,source,checked\n";
const ROW: &str = "example.com,DE,1,https://example.com/settings,contrib,2026-10-01\n";
fn parse(money: &str, services: &str) -> Result<Catalog, Error> {
    Catalog::parse(Package {
        release: RELEASE,
        license: LICENSE,
        sources: SOURCES,
        categories: [money, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY],
        services,
    })
}
fn as_of(date: &str) -> i64 {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp()
}

#[test]
fn bundled_seed_is_explicitly_empty_and_contains_no_invented_reviews() {
    let catalog = bundled().unwrap();
    assert_eq!(catalog.senders().count(), 0);
    let projected = catalog.winddown(as_of("2026-10-29")).unwrap();
    assert!(projected.reviewed.is_empty());
    let status = projected.catalog.unwrap();
    assert!(!status.seed_complete);
    assert_eq!(status.missing_categories.len(), 6);
    assert_eq!(status.missing_countries, ["DE", "AT", "CH", "*"]);
    assert_eq!(status.service_reviews, 0);
    assert_eq!(COUNTRIES.split(' ').count(), 249);
}

#[test]
fn csv_quoting_crlf_and_exact_public_settings_paths_round_trip() {
    let rows = format!(
        "{}\"example.com\",DE,1,\"https://example.com/a,b%2Fc\",wikidata:Q123,2026-10-01\r\n",
        EMPTY.replace('\n', "\r\n")
    );
    let service = format!("{SERVICES}example.com,contrib,2026-10-01\n");
    let catalog = parse(&rows, &service).unwrap();
    let entry = catalog.senders().next().unwrap();
    assert_eq!(entry.source, "wikidata:Q123");
    assert_eq!(entry.priority, 1);
    let projected = catalog.winddown(as_of("2026-10-24")).unwrap();
    assert_eq!(
        projected.reviewed["example.com"].settings_link.as_deref(),
        Some("https://example.com/a,b%2Fc")
    );
    assert!(projected.reviewed["example.com"].transactional);
    assert_eq!(csv("a,\"b\"\"c\"\n").unwrap(), vec![vec!["a", "b\"c"]]);
    for input in [
        "a,\"unclosed",
        "a,b\"c",
        "a,\"b\"c",
        "a\rb",
        "a,\"b\nc\"",
        "",
    ] {
        assert!(
            csv(input).is_err(),
            "Malformed or control-bearing CSV accepted"
        );
    }
}

#[test]
fn core_boundary_refuses_ambiguous_domains_dates_origins_and_links() {
    let valid = format!("{EMPTY}{ROW}");
    for (from, to, error) in [
        ("example.com,", "mail.example.com,", Error::Domain),
        ("example.com,", "EXAMPLE.COM,", Error::Domain),
        ("example.com,", "co.uk,", Error::Domain),
        ("example.com,", "127.0.0.1,", Error::Domain),
        ("example.com,", "example.invalid,", Error::Domain),
        (",DE,", ",ZZ,", Error::Country),
        (",DE,", ",UK,", Error::Country),
        (",1,", ",4,", Error::Priority),
        (",1,", ",01,", Error::Priority),
        ("contrib", "ut1", Error::Source),
        ("contrib", "wikidata:Q0", Error::Source),
        ("2026-10-01", "2026-02-30", Error::Date),
        ("2026-10-01", "2026-1-01", Error::Date),
        ("2026-10-01", "2026-10-30", Error::Date),
        ("https://example.com/settings", "", Error::Link),
        (
            "https://example.com/settings",
            "http://example.com/settings",
            Error::Link,
        ),
        (
            "https://example.com/settings",
            "https://user:pass@example.com/settings",
            Error::Link,
        ),
        (
            "https://example.com/settings",
            "https://example.com/settings?token=dummy",
            Error::Link,
        ),
        (
            "https://example.com/settings",
            "https://example.com/settings#fragment",
            Error::Link,
        ),
        (
            "https://example.com/settings",
            "https://127.0.0.1/settings",
            Error::Link,
        ),
        (
            "https://example.com/settings",
            "https://localhost/settings",
            Error::Link,
        ),
        (
            "https://example.com/settings",
            "https://example.com/white space",
            Error::Link,
        ),
    ] {
        assert!(
            matches!(parse(&valid.replace(from, to), SERVICES), Err(actual) if actual == error)
        );
    }
    assert!(
        parse(
            &valid
                .replace(",DE,1,", ",*,2,")
                .replace("https://example.com/settings", ""),
            SERVICES
        )
        .is_ok()
    );
    assert!(matches!(
        parse(&format!("{EMPTY}{ROW}{ROW}"), SERVICES),
        Err(Error::Duplicate)
    ));
    assert!(matches!(
        parse(
            &valid,
            &format!("{SERVICES}example.net,contrib,2026-10-01\n")
        ),
        Err(Error::ServiceWithoutSender)
    ));
    assert!(matches!(
        parse(&valid, &format!("{SERVICES}example.com,ut1,2026-10-01\n")),
        Err(Error::Source)
    ));
}

#[test]
fn release_and_provenance_bytes_bind_the_entire_installed_core() {
    let rows = format!("{EMPTY}{ROW}");
    let make = |release: &str, license: &str, sources: &str| {
        Catalog::parse(Package {
            release,
            license,
            sources,
            categories: [&rows, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY],
            services: SERVICES,
        })
    };
    let first = make(RELEASE, LICENSE, SOURCES)
        .unwrap()
        .winddown(as_of("2026-10-24"))
        .unwrap();
    let changed = make(RELEASE, LICENSE, "Changed synthetic provenance")
        .unwrap()
        .winddown(as_of("2026-10-24"))
        .unwrap();
    assert_ne!(first.version, changed.version);
    assert!(
        !first.reviewed["example.com"].transactional,
        "Category membership must not invent transactional identity"
    );
    for release in [
        RELEASE.replace("1.0.0", "01.0.0"),
        RELEASE.replace("1.0.0", "1.0"),
        RELEASE.replace("CC0-1.0", "CC-BY-SA-4.0"),
        format!("{RELEASE}unknown: true\n"),
    ] {
        assert!(make(&release, LICENSE, SOURCES).is_err());
    }
    assert!(make(RELEASE, "CC-BY-SA-4.0", SOURCES).is_err());
    assert!(make(RELEASE, LICENSE, "").is_err());
    assert!(parse(&"x".repeat(MAX_BYTES + 1), SERVICES).is_err());
}

#[test]
fn service_reviews_require_current_mail_sender_and_explicit_service_facts() {
    for (checked, day, current, stale, future) in [
        ("2024-10-24", "2026-10-24", false, 1, 0),
        ("2024-10-25", "2026-10-24", true, 0, 0),
        ("2026-10-25", "2026-10-24", false, 0, 1),
        ("2026-10-25", "2026-10-25", true, 0, 0),
        ("2026-02-28", "2028-02-29", false, 1, 0),
    ] {
        let rows = format!("{EMPTY}{}", ROW.replace("2026-10-01", checked));
        let services = format!("{SERVICES}example.com,contrib,{checked}\n");
        let projected = parse(&rows, &services)
            .unwrap()
            .winddown(as_of(day))
            .unwrap();
        assert_eq!(projected.reviewed["example.com"].transactional, current);
        let status = projected.catalog.unwrap();
        assert_eq!(status.stale_rows, stale);
        assert_eq!(status.future_rows, future);
        assert_eq!(status.unusable_service_reviews, usize::from(!current));
    }
    let rows = format!("{EMPTY}{ROW}");
    let old_service = format!("{SERVICES}example.com,contrib,2024-10-24\n");
    assert!(
        !parse(&rows, &old_service)
            .unwrap()
            .winddown(as_of("2026-10-24"))
            .unwrap()
            .reviewed["example.com"]
            .transactional
    );
}

#[test]
fn first_list_counts_and_country_coverage_derive_from_actual_rows() {
    let contents: Vec<_> = CATEGORIES
        .iter()
        .map(|category| {
            let mut csv = EMPTY.to_string();
            for i in 0..20 {
                csv.push_str(&format!(
                    "synthetic-{category}-{i}.com,{},2,,contrib,2026-10-01\n",
                    ["DE", "AT", "CH", "*"][i % 4]
                ));
            }
            csv
        })
        .collect();
    let categories = std::array::from_fn(|i| contents[i].as_str());
    let projected = Catalog::parse(Package {
        release: RELEASE,
        license: LICENSE,
        sources: SOURCES,
        categories,
        services: SERVICES,
    })
    .unwrap()
    .winddown(as_of("2026-10-24"))
    .unwrap();
    let status = projected.catalog.unwrap();
    assert!(status.seed_complete);
    assert_eq!(status.rows, 120);
    assert!(status.missing_categories.is_empty());
    assert!(status.missing_countries.is_empty());
    assert!(projected.reviewed.values().all(|s| !s.transactional));
}
