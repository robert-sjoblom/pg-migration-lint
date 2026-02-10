# Changelog

## [1.0.1](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.0.0...v1.0.1) (2026-02-10)


### Bug Fixes

* **catalog:** track PK implicit index for FK covering-index checks ([f255336](https://github.com/robert-sjoblom/pg-migration-lint/commit/f2553362b6f327f91d82b4a0a1e6da0c48c591b1))
* **cli:** validate --fail-on severity and improve changed-file matching ([ecd0ae1](https://github.com/robert-sjoblom/pg-migration-lint/commit/ecd0ae1ea4576942cdd047c83a88f6e17f8f688f))
* **liquibase:** report errors for missing required XML attributes ([ecd0ae1](https://github.com/robert-sjoblom/pg-migration-lint/commit/ecd0ae1ea4576942cdd047c83a88f6e17f8f688f))
* **parser:** propagate inline constraints on ALTER TABLE ADD COLUMN ([73ce0ef](https://github.com/robert-sjoblom/pg-migration-lint/commit/73ce0ef6a954043b190d7422e6f1cfe781045662))
* **sarif:** use crate version, correct URL, and rule descriptions ([927d625](https://github.com/robert-sjoblom/pg-migration-lint/commit/927d6251f30ceabb98c55f12be27b49b0401208a))

## 1.0.0 — 2026-02-10

Initial release of pg-migration-lint.

### Features

- **16 lint rules** for PostgreSQL migration safety:
  - **PGM001**: `CREATE INDEX` on existing table missing `CONCURRENTLY`
  - **PGM002**: `DROP INDEX` on existing table missing `CONCURRENTLY`
  - **PGM003**: Foreign key without covering index
  - **PGM004**: Table without primary key
  - **PGM005**: Adding unique constraint without pre-existing unique index
  - **PGM006**: `CONCURRENTLY` used inside a transaction
  - **PGM007**: Volatile default expression (forces table rewrite)
  - **PGM008**: Down migration severity cap (all findings reduced to INFO)
  - **PGM009**: `ALTER COLUMN TYPE` causing table rewrite
  - **PGM010**: `ADD COLUMN NOT NULL` without default on existing table
  - **PGM011**: `DROP COLUMN` on existing table
  - **PGM012**: `ADD PRIMARY KEY` without prior `UNIQUE` constraint
  - **PGM101**: `timestamp` without time zone (use `timestamptz`)
  - **PGM102**: `timestamp(0)` / `timestamptz(0)` precision truncation
  - **PGM103**: `char(n)` type (use `text` or `varchar`)
  - **PGM104**: `money` type (use `numeric`)
  - **PGM105**: `serial` / `bigserial` (use identity columns)

- **Three output formats**: SARIF (GitHub Code Scanning), SonarQube Generic Issue Import, human-readable text
- **Inline suppression**: `-- pgm-lint:suppress PGM001` and `-- pgm-lint:suppress-file PGM001` comments in SQL; `<!-- pgm-lint:suppress ... -->` in Liquibase XML
- **Changed-file filtering**: `--changed-files` and `--changed-files-from` for CI incremental linting
- **Liquibase support**: Three-tier loading (bridge JAR, update-sql, XML fallback parser)
- **go-migrate support**: Detects `.up.sql` / `.down.sql` naming convention
- **Single-pass catalog replay**: Builds table state incrementally, lints only changed files
- **CLI**: `--explain PGM001` for detailed rule documentation, `--fail-on` severity threshold, `--format` override, `--version`
- **Configuration**: TOML config file with migration paths, output formats, severity thresholds, Liquibase settings
