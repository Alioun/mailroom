# Core sender data

This directory contains the CC0 core sender lists. The application code remains MIT. The checked-in seed contains no reviewed sender rows and is incomplete. It is not a researched or verified sender dataset.

Each category CSV uses `domain,country,priority,settings_url,source,checked`. A domain must be a canonical ASCII registrable domain in the pinned Public Suffix List. Country is an ISO 3166-1 alpha-2 code or `*`. Priority is 1, 2 or 3. Source is a Wikidata item such as `wikidata:Q123`, or `contrib`. The checked date records a person's confirmation that the domain sends mail. A website domain or an MX/A record alone does not establish that fact. Priority-1 rows require one public HTTPS settings link. Links containing credentials, a query, a fragment or a literal IP address are refused. No links are fetched or launched.

The first reviewed list requires at least 20 rows in each of the six categories and coverage of Germany, Austria, Switzerland and international brands. `release.yaml` versions the whole core under the preset's major/minor update policy. The loader reports an incomplete seed rather than manufacturing reviews. Rows unchecked for two calendar years are flagged at the report's explicit as-of time.

Wikidata is the agreed CC0 candidate source. A future maintainer seed tool may propose candidates using P856, P452 and P17. Human confirmation remains required before rows enter a release. No Wikidata query or online classification runs when the application loads a report.

`transactional-services.csv` records an additional explicit review that a core domain represents a transactional service for wind-down grouping. Its columns are `domain,source,checked`. Every domain must already occur in a category list. Category membership, automation headers and settings links alone do not establish this classification. Missing, stale or future service reviews leave that grouping unresolved. This file records generic public service facts, never an owner's addresses, accounts or associations.

UT1 web-domain lists are outside this CC0 core. Their separate CC BY-SA package, per-account opt-in and attribution remain separate integration work. A row with `source: ut1` cannot enter this core. Data rows do not grant address ownership, delivery provenance, authentication, invoice approval or provider-write authority.

The loader, wind-down projection and whole-category compiler integration are implemented. Curated rows, preset release review, the accounting-tool table, contribution tooling and scheduled upkeep remain unfinished. No first-list or complete-catalog claim is made.
