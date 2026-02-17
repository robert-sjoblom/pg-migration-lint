# pg-migration-lint

Static analyzer for PostgreSQL migration files.

![CI](https://github.com/robert-sjoblom/pg-migration-lint/actions/workflows/ci.yml/badge.svg)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![codecov](https://codecov.io/gh/robert-sjoblom/pg-migration-lint/graph/badge.svg?token=NBOA6Y7GB2)](https://codecov.io/gh/robert-sjoblom/pg-migration-lint)

## What it does

pg-migration-lint replays your full migration history to build an internal table catalog, then lints only new or changed migration files against 28 safety and correctness rules. It catches dangerous operations -- missing `CONCURRENTLY`, table rewrites, missing indexes on foreign keys, unsafe constraint additions, silent constraint removal, risky renames, type anti-patterns -- before they reach production.

Output formats include SARIF (for GitHub Code Scanning inline PR annotations), SonarQube Generic Issue Import JSON, and human-readable text.

## Quick Start

### Install from release

```bash
curl -LO https://github.com/robert-sjoblom/pg-migration-lint/releases/latest/download/pg-migration-lint-x86_64-linux.tar.gz
tar xzf pg-migration-lint-x86_64-linux.tar.gz
chmod +x pg-migration-lint
```

### Run locally

```bash
# Lint specific changed files (text output for local development)
./pg-migration-lint --format text --changed-files db/migrations/V042__add_index.sql

# Lint all migrations (useful for first adoption or full-repo scans)
./pg-migration-lint --format text

# Explain what a specific rule checks for
./pg-migration-lint --explain PGM001
```

## Rules

pg-migration-lint ships with 28 rules across two categories: migration safety rules (PGM001-PGM022) and PostgreSQL type anti-pattern rules (PGM101-PGM105, PGM108).

### Migration Safety Rules

| Rule | Severity | Description | Example (bad) |
|------|----------|-------------|---------------|
| PGM001 | Critical | Missing `CONCURRENTLY` on `CREATE INDEX` | `CREATE INDEX idx_foo ON orders (status);` |
| PGM002 | Critical | Missing `CONCURRENTLY` on `DROP INDEX` | `DROP INDEX idx_foo;` |
| PGM003 | Major | Foreign key without covering index | `ALTER TABLE orders ADD CONSTRAINT fk FOREIGN KEY (customer_id) REFERENCES customers (id);` |
| PGM004 | Major | Table without primary key | `CREATE TABLE events (name text, ts timestamptz);` |
| PGM005 | Info | `UNIQUE NOT NULL` used instead of primary key | `CREATE TABLE t (id int NOT NULL UNIQUE);` |
| PGM006 | Critical | `CONCURRENTLY` inside transaction | `CREATE INDEX CONCURRENTLY idx ON t (col);` inside a transactional changeset |
| PGM007 | Minor | Volatile default on column | `ALTER TABLE t ADD COLUMN created_at timestamptz DEFAULT now();` |
| PGM009 | Critical | `ALTER COLUMN TYPE` causing table rewrite | `ALTER TABLE orders ALTER COLUMN status TYPE int;` |
| PGM010 | Critical | `ADD COLUMN NOT NULL` without default | `ALTER TABLE orders ADD COLUMN region text NOT NULL;` |
| PGM011 | Info | `DROP COLUMN` on existing table | `ALTER TABLE orders DROP COLUMN legacy_col;` |
| PGM012 | Major | `ADD PRIMARY KEY` without prior `UNIQUE` index | `ALTER TABLE orders ADD PRIMARY KEY (id);` |
| PGM013 | Minor | `DROP COLUMN` silently removes unique constraint | `ALTER TABLE users DROP COLUMN email;` (where `email` has a UNIQUE constraint) |
| PGM014 | Major | `DROP COLUMN` silently removes primary key | `ALTER TABLE orders DROP COLUMN id;` (where `id` is the PK) |
| PGM015 | Minor | `DROP COLUMN` silently removes foreign key | `ALTER TABLE orders DROP COLUMN customer_id;` (where `customer_id` is an FK) |
| PGM016 | Critical | `SET NOT NULL` requires ACCESS EXCLUSIVE lock | `ALTER TABLE orders ALTER COLUMN status SET NOT NULL;` |
| PGM017 | Critical | `ADD FOREIGN KEY` without `NOT VALID` | `ALTER TABLE orders ADD CONSTRAINT fk FOREIGN KEY (cust_id) REFERENCES customers (id);` |
| PGM018 | Critical | `ADD CHECK` without `NOT VALID` | `ALTER TABLE orders ADD CONSTRAINT chk CHECK (amount > 0);` |
| PGM019 | Info | `RENAME TABLE` on existing table | `ALTER TABLE orders RENAME TO orders_old;` |
| PGM020 | Info | `RENAME COLUMN` on existing table | `ALTER TABLE orders RENAME COLUMN status TO order_status;` |
| PGM021 | Critical | `ADD UNIQUE` without `USING INDEX` | `ALTER TABLE users ADD CONSTRAINT uq_email UNIQUE (email);` |
| PGM022 | Minor | `DROP TABLE` on existing table | `DROP TABLE legacy_orders;` |

PGM001 and PGM002 do not fire when the table is created in the same set of changed files, because locking a new/empty table is harmless.

PGM003 and PGM004 check the catalog state *after* the entire file is processed, so creating an index or adding a primary key later in the same file avoids false positives.

PGM016, PGM017, and PGM018 only fire on tables that existed before the current set of changed files. Use `ADD CONSTRAINT ... NOT VALID` followed by `VALIDATE CONSTRAINT` for safe online constraint addition.

PGM019 includes replacement detection: if the old table name is re-created in the same file (a common rename-and-replace pattern), the finding is suppressed.

PGM021 follows the same pattern as PGM012: create the unique index `CONCURRENTLY` first, then use `ADD CONSTRAINT ... UNIQUE USING INDEX` to promote it. This avoids a full table scan under an `ACCESS EXCLUSIVE` lock.

PGM022 only fires on tables that existed before the current set of changed files. Dropping a table created in the same changeset is harmless.

### PostgreSQL "Don't Do This" Rules

These rules are derived from the [PostgreSQL wiki "Don't Do This"](https://wiki.postgresql.org/wiki/Don%27t_Do_This) page. They detect type anti-patterns in `CREATE TABLE`, `ALTER TABLE ... ADD COLUMN`, and `ALTER TABLE ... ALTER COLUMN TYPE` statements.

| Rule | Severity | Description | Example (bad) |
|------|----------|-------------|---------------|
| PGM101 | Minor | Don't use `timestamp` without time zone | `CREATE TABLE t (created_at timestamp);` |
| PGM102 | Minor | Don't use `timestamp(0)` or `timestamptz(0)` | `CREATE TABLE t (ts timestamptz(0));` |
| PGM103 | Minor | Don't use `char(n)` | `CREATE TABLE t (code char(3));` |
| PGM104 | Minor | Don't use the `money` type | `CREATE TABLE t (price money);` |
| PGM105 | Info | Don't use `serial` / `bigserial` | `CREATE TABLE t (id serial PRIMARY KEY);` |
| PGM108 | Minor | Don't use `json` (use `jsonb`) | `CREATE TABLE t (data json);` |

### Meta-behavior Rules

The 9xx range is reserved for meta-behaviors that modify how other rules operate. These are not standalone lint rules.

| Rule | Severity | Description |
|------|----------|-------------|
| PGM901 | -- | Down migration severity cap — all findings in `.down.sql` files are capped to Info |

Use `--explain <RULE_ID>` for a detailed explanation of any rule, including why it is dangerous and how to fix it:

```bash
./pg-migration-lint --explain PGM001
```

## GitHub Actions Integration

### Basic: Lint changed migrations on pull requests

This workflow detects changed SQL migration files, runs pg-migration-lint, and uploads SARIF results so that findings appear as inline annotations on the pull request.

Copy this file to `.github/workflows/migration-lint.yml` in your repository:

```yaml
name: Migration Lint

on:
  pull_request:
    paths:
      - 'db/migrations/**'

jobs:
  lint:
    runs-on: ubuntu-latest
    permissions:
      security-events: write  # Required for uploading SARIF to GitHub Code Scanning
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0  # Full history so git diff can compare against the base branch

      - name: Get changed migration files
        id: changes
        run: |
          files=$(git diff --name-only origin/${{ github.base_ref }}...HEAD -- 'db/migrations/*.sql' | tr '\n' ',')
          echo "files=$files" >> "$GITHUB_OUTPUT"

      - name: Download pg-migration-lint
        run: |
          curl -LO https://github.com/robert-sjoblom/pg-migration-lint/releases/latest/download/pg-migration-lint-x86_64-linux.tar.gz
          tar xzf pg-migration-lint-x86_64-linux.tar.gz
          chmod +x pg-migration-lint

      - name: Run linter
        if: steps.changes.outputs.files != ''
        run: |
          ./pg-migration-lint \
            --changed-files "${{ steps.changes.outputs.files }}" \
            --format sarif \
            --fail-on critical

      - name: Upload SARIF
        if: always() && steps.changes.outputs.files != ''
        uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: build/reports/migration-lint/findings.sarif
```

**What each step does:**

- **`fetch-depth: 0`** -- By default, `actions/checkout` performs a shallow clone (only the latest commit). The `git diff` step needs the full commit history to compare your PR branch against the base branch. Without this, the diff will fail or produce incorrect results.

- **`permissions: security-events: write`** -- GitHub requires this permission to upload SARIF files to the Code Scanning API. Without it, the "Upload SARIF" step will fail with a 403 error.

- **Get changed migration files** -- This step runs `git diff --name-only` to list only the SQL files that changed between the base branch and the PR head. The filenames are joined with commas because `--changed-files` expects a comma-separated list. If no migration files changed, the output is empty and subsequent steps are skipped.

- **`--fail-on critical`** -- The linter exits with code 1 if any finding has Critical severity or higher. This blocks the PR (when branch protection requires the check to pass). Set to `major`, `minor`, or `info` to be stricter, or `none` to never fail.

- **`if: always()`** on the SARIF upload step -- The "Run linter" step exits with code 1 when it finds Critical issues, which normally causes subsequent steps to be skipped. The `if: always()` condition ensures the SARIF file is still uploaded so that findings appear as inline annotations on the PR, even when the lint step "fails."

### SonarQube integration

pg-migration-lint can produce SonarQube Generic Issue Import JSON alongside SARIF. Since the `--format` CLI flag accepts only a single format, use a configuration file to produce multiple formats simultaneously.

Create or update your `pg-migration-lint.toml`:

```toml
[output]
formats = ["sarif", "sonarqube"]
```

The SonarQube JSON file will be written to `build/reports/migration-lint/findings.json`.

Then configure your SonarQube scanner to import the findings. Add this to your `sonar-project.properties`:

```properties
sonar.externalIssuesReportPaths=build/reports/migration-lint/findings.json
```

In your GitHub Actions workflow, run the linter before the SonarQube scanner step:

```yaml
      - name: Run migration linter
        if: steps.changes.outputs.files != ''
        run: |
          ./pg-migration-lint \
            --changed-files "${{ steps.changes.outputs.files }}" \
            --fail-on critical

      - name: SonarQube Scan
        uses: SonarSource/sonarqube-scan-action@v3
        env:
          SONAR_TOKEN: ${{ secrets.SONAR_TOKEN }}
```

When using the config file, the `--format` flag is not needed -- the tool reads formats from `[output].formats` in the config.

### Liquibase XML support

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

The tool uses a two-tier approach for Liquibase XML processing (JRE required):

1. **Bridge JAR (preferred)** -- A small Java CLI that embeds Liquibase to extract exact changeset-to-SQL-to-line mappings. Download `liquibase-bridge.jar` from the [releases page](https://github.com/robert-sjoblom/pg-migration-lint/releases) and place it at the configured `bridge_jar_path`. Requires a JRE.

2. **`liquibase update-sql` (secondary)** -- If the bridge JAR is unavailable but the Liquibase binary is on the PATH, the tool invokes `liquibase update-sql` for less structured but functional output.

## Configuration Reference

Default config file: `pg-migration-lint.toml` in the working directory. Override with `--config <path>`.

If no config file is found at the default path, the tool uses built-in defaults and prints a warning.

You can also view this reference from the CLI with `--explain-config`:

```bash
./pg-migration-lint --explain-config              # all sections
./pg-migration-lint --explain-config migrations    # just [migrations]
```

```toml
[migrations]
# Paths to migration sources. Scanned in order.
# For filename_lexicographic: directories containing .sql files.
# For liquibase: the root changelog file (e.g. "db/changelog/migrations.xml").
# Default: ["db/migrations"]
paths = ["db/migrations"]

# How to determine migration order.
#   "filename_lexicographic" - sorted by filename (go-migrate, Flyway convention)
#   "liquibase" - order derived from Liquibase changelog includes
# Default: "filename_lexicographic"
strategy = "filename_lexicographic"

# File patterns to include when scanning migration directories.
# Default: ["*.sql", "*.xml"]
include = ["*.sql", "*.xml"]

# File patterns to exclude.
# Default: []
exclude = ["**/test/**"]

# Default schema for unqualified table names.
# Unqualified names like "orders" are normalized to "public.orders" for
# catalog lookups, so that "orders" and "public.orders" resolve to the
# same table. Set this to your service's search_path schema if it
# differs from "public".
# Default: "public"
default_schema = "public"

[liquibase]
# Path to liquibase-bridge.jar (see "Liquibase XML support" above).
# Default: "tools/liquibase-bridge.jar"
bridge_jar_path = "tools/liquibase-bridge.jar"

# Path to the liquibase binary (used by the "update-sql" secondary strategy).
# Default: "liquibase"
binary_path = "/usr/local/bin/liquibase"

# Liquibase processing strategy.
#   "auto" - tries bridge -> update-sql in order
#   "bridge" - bridge JAR only
#   "update-sql" - liquibase update-sql only
# Default: "auto"
strategy = "auto"

[output]
# Output formats to produce. One or more of: "sarif", "sonarqube", "text"
# Default: ["sarif"]
formats = ["sarif", "sonarqube"]

# Directory for output files.
# SARIF is written to <dir>/findings.sarif
# SonarQube JSON is written to <dir>/findings.json
# Default: "build/reports/migration-lint"
dir = "build/reports/migration-lint"

[cli]
# Exit non-zero if any finding meets or exceeds this severity.
# One of: "blocker", "critical", "major", "minor", "info", "none"
# Default: "critical"
fail_on = "critical"
```

## Suppression

Sometimes a finding is intentional and should be suppressed. pg-migration-lint supports inline suppression comments in both SQL and XML files.

### SQL files

**Suppress a single rule on the next statement:**

```sql
-- pgm-lint:suppress PGM001
CREATE INDEX idx_foo ON bar (col);
```

**Suppress multiple rules on the next statement:**

```sql
-- pgm-lint:suppress PGM001,PGM003
CREATE INDEX idx_foo ON bar (col);
```

**Suppress rules for the entire file** (must appear before any SQL statements):

```sql
-- pgm-lint:suppress-file PGM001,PGM003
```

### Liquibase XML files

The same directives work inside XML comments:

```xml
<!-- pgm-lint:suppress PGM001 -->
<changeSet id="42" author="dev">
    <createIndex indexName="idx_foo" tableName="bar">
        <column name="col"/>
    </createIndex>
</changeSet>
```

```xml
<!-- pgm-lint:suppress-file PGM001,PGM003 -->
```

Only single-line XML comments are recognized. Multi-line `<!-- ... -->` comments spanning multiple lines are not parsed for directives.

## CLI Reference

```
pg-migration-lint [OPTIONS]

OPTIONS:
  -c, --config <path>              Path to configuration file
                                   (default: ./pg-migration-lint.toml)
  --changed-files <list>           Comma-separated list of changed files to lint
  --changed-files-from <path>      Path to file containing changed file paths
                                   (one per line)
  --format <format>                Override output format: sarif, sonarqube, text
  --fail-on <severity>             Override exit code threshold:
                                   blocker, critical, major, minor, info, none
  --explain <rule>                 Print detailed explanation of a rule and exit
  --explain-config [section]       Print configuration reference and exit.
                                   Omit section to print all; valid sections:
                                   migrations, liquibase, output, cli, rules
  -V, --version                    Print version and exit
  -h, --help                       Print help
```

When `--changed-files` is omitted, all migration files are linted.

When `--format` is provided, it overrides the `[output].formats` setting from the config file with a single format. To produce multiple formats in one run, use the config file.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | No findings at or above the configured severity threshold |
| 1 | One or more findings at or above the threshold (blocks CI) |
| 2 | Tool error (invalid config, missing files, parse failure) |

## Building from Source

Requires Rust 1.70+ and a C compiler (for the `pg_query` native bindings).

```bash
# Install system dependencies (Debian/Ubuntu)
sudo apt-get install build-essential libclang-dev clang

# Build optimized release binary
cargo build --release

# Binary is at target/release/pg-migration-lint
```

To run the test suite:

```bash
cargo test
```

To build the Liquibase bridge JAR (requires Docker or a local Maven + JDK 21 installation):

```bash
cd bridge
docker run --rm -v "$PWD:/build" -w /build maven:3.9-eclipse-temurin-21 mvn package -q -DskipTests
# JAR is at bridge/target/liquibase-bridge.jar
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
