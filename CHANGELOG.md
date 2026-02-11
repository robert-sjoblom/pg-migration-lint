# Changelog

## [1.2.1](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.2.0...v1.2.1) (2026-02-11)


### Bug Fixes

* **liquibase:** resolve XML includes relative to classpath root ([1fee1ae](https://github.com/robert-sjoblom/pg-migration-lint/commit/1fee1ae9ab84503e74354d97c33b50c9172172e1))

## [1.2.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.1.0...v1.2.0) (2026-02-11)


### Features

* **rules:** add PGM013 — DROP COLUMN silently removes unique constraint ([c1c9bd8](https://github.com/robert-sjoblom/pg-migration-lint/commit/c1c9bd8981a9d2a25146c5bcba79dd70bffd6c95))
* **rules:** add PGM014 — DROP COLUMN silently removes primary key ([0ce1364](https://github.com/robert-sjoblom/pg-migration-lint/commit/0ce136463d89aaeb2cda1aefa081e2e3ccc001e9))
* **rules:** add PGM015 — DROP COLUMN silently removes foreign key ([8ff1d90](https://github.com/robert-sjoblom/pg-migration-lint/commit/8ff1d900819be73bc2235a121cf832e6cdfd436a))
* schema-aware catalog with configurable default_schema ([4c79fec](https://github.com/robert-sjoblom/pg-migration-lint/commit/4c79fecab8c03d1b251da57b4a74b4ceb25da4f9))


### Bug Fixes

* **catalog:** remove_column now cleans up constraints ([33c7728](https://github.com/robert-sjoblom/pg-migration-lint/commit/33c77287bd549ad584c7df54a7e3aa78ccb582ef))

## [1.1.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.0.1...v1.1.0) (2026-02-11)


### Features

* **suppress:** warn on unknown rule IDs in suppression comments ([3227f89](https://github.com/robert-sjoblom/pg-migration-lint/commit/3227f894691bcae5b85d047b6ad904e8b76755f7))


### Bug Fixes

* **cli:** include blocker in --fail-on error message ([d823337](https://github.com/robert-sjoblom/pg-migration-lint/commit/d8233374674189c4536eb836f43c9ad638b2410e))
* **config:** validate fail_on at config load time ([ae90504](https://github.com/robert-sjoblom/pg-migration-lint/commit/ae90504008ec061291cfd2cbe61e752aec915bfb))
* **liquibase:** validate required XML attributes before generating SQL ([75d3605](https://github.com/robert-sjoblom/pg-migration-lint/commit/75d3605ca1f73e7f998c9a4c9639cbcb5471fe7d))
* **parser:** handle smallserial type as serial variant ([a120aea](https://github.com/robert-sjoblom/pg-migration-lint/commit/a120aea7c86bfcfe58d260471c65a6ea03156d13))
* **PGM002:** include index and table name in finding message ([ece37d7](https://github.com/robert-sjoblom/pg-migration-lint/commit/ece37d7f56b16ee28b7c1015f7872cfdfce4af26))
* **PGM009:** treat numeric(P) as equivalent to numeric(P, 0) ([2f612b2](https://github.com/robert-sjoblom/pg-migration-lint/commit/2f612b2908a3bd24bf9fa3bada20d3065ca1d96b))


### Performance Improvements

* **catalog:** O(1) index-to-table lookup via reverse map ([63f37fa](https://github.com/robert-sjoblom/pg-migration-lint/commit/63f37fa1ad2c5c1c6ff9c761d21c1a6d42b9a570))

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
