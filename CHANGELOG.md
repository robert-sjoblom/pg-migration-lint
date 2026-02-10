# Changelog

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
