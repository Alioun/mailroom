# Core sender data

This directory contains the CC0 core sender lists. The application code remains MIT.

Each category CSV uses `domain,country,priority,settings_url,source,checked`. A domain must be a canonical ASCII registrable domain in the pinned Public Suffix List. Country is an ISO 3166-1 alpha-2 code or `*`. Priority is 1, 2 or 3. Source is a Wikidata item such as `wikidata:Q123`, or `contrib`. Priority-1 rows require one public HTTPS settings link. Links containing credentials, a query, a fragment or a literal IP address are refused. No links are fetched or launched.

Release `1.1.0` provides the starter CC0 catalog covering Germany (`DE`), Austria (`AT`), Switzerland (`CH`), and international brands (`*`) with at least 24 domains in each of the six categories (`money`, `contracts`, `orders`, `travel`, `social`, `newsletters`) plus transactional service classifications in `transactional-services.csv`. Rows unchecked for two calendar years are flagged at the report's explicit as-of time.

`transactional-services.csv` records explicit classifications that a core domain represents a transactional service for wind-down grouping. Its columns are `domain,source,checked`. Every domain must already occur in a category list. This file records generic public service facts, never an owner's addresses, accounts or associations.

UT1 web-domain lists are outside this CC0 core. Data rows do not grant address ownership, delivery provenance, authentication, invoice approval or provider-write authority.
