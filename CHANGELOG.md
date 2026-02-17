# Changelog

## [2.2.1](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.2.0...v2.2.1) (2026-02-17)


### Bug Fixes

* **catalog:** skip CREATE TABLE/INDEX IF NOT EXISTS when target exists ([4ef83be](https://github.com/robert-sjoblom/pg-migration-lint/commit/4ef83be95f83466200b65bfea7564bba1546e91b))

## [2.2.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.1.0...v2.2.0) (2026-02-17)


### Features

* **rules:** implement PGM023 — missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX ([23c7686](https://github.com/robert-sjoblom/pg-migration-lint/commit/23c7686592d0f3b8fb5932ea5d19d0483102b847))

## [2.1.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.0.1...v2.1.0) (2026-02-17)


### Features

* **rules:** implement PGM008 — missing IF EXISTS on DROP TABLE / DROP INDEX ([9f33031](https://github.com/robert-sjoblom/pg-migration-lint/commit/9f33031143e7c3a2225434988bb4895aa0ebd8f0))

## [2.0.1](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.0.0...v2.0.1) (2026-02-17)


### Bug Fixes

* **catalog:** resolve index columns into constraints created via USING INDEX ([4b20ad9](https://github.com/robert-sjoblom/pg-migration-lint/commit/4b20ad911f962425d23fff1f55e0959e4878433a))
* **rules:** rewrite PGM012/PGM021 to check USING INDEX instead of catalog index existence ([4f912ff](https://github.com/robert-sjoblom/pg-migration-lint/commit/4f912ff7fdca045eb919081bebecdfe4b0aa1f2e))

## [2.0.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.9.6...v2.0.0) (2026-02-17)


### ⚠ BREAKING CHANGES

* replace string-based rule IDs with strongly-typed RuleId enum
* SonarQube JSON output structure changed. Issues no longer carry engineId, severity, or type fields — these now live on rule definitions. Requires SonarQube 10.3+.

### Features

* upgrade SonarQube output to 10.3+ Generic Issue Import format ([5d768d9](https://github.com/robert-sjoblom/pg-migration-lint/commit/5d768d916c79f540877d1778c8baff54cf956cbe))


### Bug Fixes

* remove INSTA_UPDATE=always build layer that masked snapshot failures ([6d1f259](https://github.com/robert-sjoblom/pg-migration-lint/commit/6d1f2591e2d6b627e8cae949d3da649c2b2cfb3d))


### Code Refactoring

* replace string-based rule IDs with strongly-typed RuleId enum ([66e7d76](https://github.com/robert-sjoblom/pg-migration-lint/commit/66e7d76f6c12bce1f42c2ad6ad85546988e94c49))

## [1.9.6](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.9.5...v1.9.6) (2026-02-16)


### Bug Fixes

* resolve config paths relative to config file, not CWD ([97ce30b](https://github.com/robert-sjoblom/pg-migration-lint/commit/97ce30bc1514f74108a7b8e0750de63900e12a22))

## [1.9.5](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.9.4...v1.9.5) (2026-02-16)


### Bug Fixes

* PGM007 no longer fires for new tables ([0a65863](https://github.com/robert-sjoblom/pg-migration-lint/commit/0a6586370846400cf796be454e7e2ac6f9a69567))

## [1.9.4](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.9.3...v1.9.4) (2026-02-15)


### Bug Fixes

* add -DskipTests to release bridge build ([ffb7b5a](https://github.com/robert-sjoblom/pg-migration-lint/commit/ffb7b5a244380a323a33c847ef639b66b7e9e5f6))

## [1.9.3](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.9.2...v1.9.3) (2026-02-15)


### Bug Fixes

* replace unwrap() with let-else in apply_alter_table ([752e004](https://github.com/robert-sjoblom/pg-migration-lint/commit/752e0041c634defb020c361a152154242ad038fd))

## [1.9.2](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.9.1...v1.9.2) (2026-02-15)


### Bug Fixes

* replace unwrap() with let-else in apply_alter_table ([752e004](https://github.com/robert-sjoblom/pg-migration-lint/commit/752e0041c634defb020c361a152154242ad038fd))

## [1.9.1](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.9.0...v1.9.1) (2026-02-14)


### Bug Fixes

* **rules:** restore PGM013/014/015 scope to fire on any pre-existing table ([a080637](https://github.com/robert-sjoblom/pg-migration-lint/commit/a080637a26124dde1857e7e9d25ca3c7537c5d9f))

## [1.9.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.8.1...v1.9.0) (2026-02-14)


### Features

* **rules:** add PGM021 (ADD UNIQUE without USING INDEX) and PGM022 (DROP TABLE) ([45a7c7e](https://github.com/robert-sjoblom/pg-migration-lint/commit/45a7c7e37bd449e8975da3c4d880a255511fcd10))

## [1.8.1](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.8.0...v1.8.1) (2026-02-13)


### Bug Fixes

* **bridge:** skip changesets that fail SQL generation instead of aborting ([334299a](https://github.com/robert-sjoblom/pg-migration-lint/commit/334299a2ca8913c06e22417a7ec9ff0667ac2d86))

## [1.8.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.7.0...v1.8.0) (2026-02-12)


### Features

* **rules:** implement PGM016–PGM020 and PGM108 lint rules ([120e9c3](https://github.com/robert-sjoblom/pg-migration-lint/commit/120e9c3050a7c8cbd8026bec4708f70b368dfbae))

## [1.7.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.6.0...v1.7.0) (2026-02-12)


### Features

* **liquibase-xml:** support `references` shorthand on inline FK constraints ([a23e110](https://github.com/robert-sjoblom/pg-migration-lint/commit/a23e110503c9c8f0558c23f7e9c6bf44949a4404))


### Bug Fixes

* **bridge:** fix suppress warnings parser error in bridge runs due to relative path ([744f910](https://github.com/robert-sjoblom/pg-migration-lint/commit/744f910ee158a28ad6d9238c3f07c375ddcd13ce))

## [1.6.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.5.0...v1.6.0) (2026-02-12)


### Features

* **liquibase-xml:** extract inline FK constraints and map Liquibase types ([c8c9447](https://github.com/robert-sjoblom/pg-migration-lint/commit/c8c9447f059b2e9d38c29fda8703f002e3457598))

## [1.5.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.4.0...v1.5.0) (2026-02-12)


### Features

* **cli:** add --explain-config flag for built-in config reference ([c781454](https://github.com/robert-sjoblom/pg-migration-lint/commit/c7814549745c79e848fa675dd2382cb93922e612))
* **liquibase-xml:** add renameColumn, dropForeignKeyConstraint, dropPrimaryKey, dropUniqueConstraint, renameTable change types ([82dc036](https://github.com/robert-sjoblom/pg-migration-lint/commit/82dc036ace7753c274d01b603e42f1b0cb552ac5))


### Bug Fixes

* **pgm013,pgm015:** show constraint shape instead of '&lt;unnamed&gt;' for unnamed constraints ([a50f51f](https://github.com/robert-sjoblom/pg-migration-lint/commit/a50f51f5a3ebdd7a9bfd859f98a90677d0625bd8))

## [1.4.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.3.0...v1.4.0) (2026-02-12)


### Features

* **liquibase-xml:** add 5 change types, identifier quoting, run_in_transaction config ([44a843b](https://github.com/robert-sjoblom/pg-migration-lint/commit/44a843b454ee528a3895c74a2a04d93ac56b3ced))

## [1.3.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.2.2...v1.3.0) (2026-02-11)


### Features

* post-evaluation improvements (display names, config suppression, changed-files ([0b37adb](https://github.com/robert-sjoblom/pg-migration-lint/commit/0b37adb80f2349815ad44b61c540e32e7b9e57fb))

## [1.2.2](https://github.com/robert-sjoblom/pg-migration-lint/compare/v1.2.1...v1.2.2) (2026-02-11)


### Bug Fixes

* **liquibase:** skip duplicate includes and support formatted SQL ([6f7c415](https://github.com/robert-sjoblom/pg-migration-lint/commit/6f7c415be907b6c749ccc50e648e8616d2496b96))

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
