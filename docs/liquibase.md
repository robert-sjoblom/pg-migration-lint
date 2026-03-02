---
layout: default
title: Liquibase XML Support
---

# Liquibase XML Support

If your migrations are managed by Liquibase XML changelogs, set the strategy in your config file:

```toml
[migrations]
paths = ["db/changelog/migrations.xml"]
strategy = "liquibase"

[liquibase]
bridge_jar_path = "tools/liquibase-bridge.jar"
strategy = "auto"
```

For Liquibase, `paths` must point to the root changelog file (e.g. `migrations.xml`), not the directory containing it. The tool follows `<include>` elements from this entrypoint to discover changesets in order.

## Two-tier processing

The tool uses a two-tier approach for Liquibase XML processing (JRE required):

1. **Bridge JAR (preferred)** -- A small Java CLI that embeds Liquibase to extract exact changeset-to-SQL-to-line mappings. Download `liquibase-bridge.jar` from the [releases page](https://github.com/robert-sjoblom/pg-migration-lint/releases) and place it at the configured `bridge_jar_path`. Requires a JRE.

2. **`liquibase update-sql` (secondary)** -- If the bridge JAR is unavailable but the Liquibase binary is on the PATH, the tool invokes `liquibase update-sql` for less structured but functional output.

## Notes

> **Note:** Liquibase `<rollback>` blocks are not detected as down migrations. Down migration detection (PGM901 severity cap) only applies to SQL files with `.down.sql` or `_down.sql` filename suffixes.

> **Note:** `liquibase update-sql` rejects changelogs that `<include>` the same file more than once ("duplicate identifiers" validation error). The bridge JAR does not have this limitation. In production, Liquibase silently skips already-applied changesets, so duplicate includes are harmless. Prefer the bridge JAR for maximum compatibility.
