# pg-migration-lint

Static analyzer for PostgreSQL migration files.

![CI](https://github.com/robert-sjoblom/pg-migration-lint/actions/workflows/ci.yml/badge.svg)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![codecov](https://codecov.io/gh/robert-sjoblom/pg-migration-lint/graph/badge.svg?token=NBOA6Y7GB2)](https://codecov.io/gh/robert-sjoblom/pg-migration-lint)

## What it does

pg-migration-lint replays your full migration history to build an internal table catalog, then lints only new or changed migration files against 52 safety and correctness rules. It catches dangerous operations -- missing `CONCURRENTLY`, table rewrites, missing indexes on foreign keys, unsafe constraint additions, silent constraint removal, risky renames, type anti-patterns -- before they reach production.

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

pg-migration-lint ships with 52 rules across seven categories:

- **Unsafe DDL (PGM001-PGM022)** -- Critical/Major. Missing `CONCURRENTLY`, table rewrites, unsafe constraint additions, silent side effects from `DROP COLUMN`,
`VACUUM FULL`, `CLUSTER`.
- **Type Anti-patterns (PGM101-PGM109)** -- Minor/Info. `timestamp` without time zone, `char(n)`, `money`, `serial`, `json`, `varchar(n)`, floating-point columns.
Derived from the PostgreSQL wiki "Don't Do This" page.
- **Destructive Operations (PGM201-PGM205)** -- Minor/Major/Critical. `DROP TABLE`, `TRUNCATE`, `DROP SCHEMA CASCADE`.
- **DML in Migrations (PGM301-PGM303)** -- Info/Minor. `INSERT`, `UPDATE`, `DELETE` on existing tables.
- **Idempotency Guards (PGM401-PGM403)** -- Minor. Missing `IF EXISTS` / `IF NOT EXISTS`, misleading no-ops.
- **Schema Design (PGM501-PGM509)** -- Major/Info. Missing FK index, no primary key, risky renames, unlogged tables, redundant indexes, mixed-case identifiers.
- **Meta-behavior (PGM901)** -- Down migrations cap all findings to Info.

Use `--explain <RULE_ID>` for a detailed explanation of any rule, including why it is dangerous and how to fix it:

```bash
./pg-migration-lint --explain PGM001
```

See the [full rule reference](https://robert-sjoblom.github.io/pg-migration-lint/rules) for every rule with examples and fixes.

```markdown
  ## Integrations

  - [GitHub Actions](https://robert-sjoblom.github.io/pg-migration-lint/github-actions) -- Workflow YAML for linting changed migrations on PRs with SARIF upload
  - [SonarQube](https://robert-sjoblom.github.io/pg-migration-lint/sonarqube) -- Generic Issue Import JSON setup
  - [Liquibase XML](https://robert-sjoblom.github.io/pg-migration-lint/liquibase) -- Bridge JAR and `update-sql` configuration

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

# Whether plain SQL files run inside a transaction by default.
# Set to false for golang-migrate repos where files run outside transactions.
# Default: true
run_in_transaction = true

[liquibase]
# Path to liquibase-bridge.jar.
# Default: "tools/liquibase-bridge.jar"
bridge_jar_path = "tools/liquibase-bridge.jar"

# Path to the liquibase binary (used by the "update-sql" secondary strategy).
# Default: "liquibase"
binary_path = "/usr/local/bin/liquibase"

# Path to a liquibase properties file (passed as --defaults-file to the CLI).
# Default: none
# properties_file = "liquibase.properties"

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

# Optional prefix to strip from finding file paths before emitting reports.
# Useful when running from a project root but SonarQube expects module-relative paths.
# Example: strip_prefix = "impl/" turns "impl/src/main/..." into "src/main/..."
# Default: none
# strip_prefix = "impl/"

[rules]
# Rule IDs to disable globally. Findings from disabled rules are not emitted.
# Invalid rule IDs cause a config-load error (exit 2).
# Default: []
disabled = []

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
-- pgm-lint:suppress PGM001,PGM501
CREATE INDEX idx_foo ON bar (col);
```

**Suppress rules for the entire file** (must appear before any SQL statements):

```sql
-- pgm-lint:suppress-file PGM001,PGM501
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
<!-- pgm-lint:suppress-file PGM001,PGM501 -->
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
