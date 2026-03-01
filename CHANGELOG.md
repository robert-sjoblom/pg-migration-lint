# Changelog

## Unreleased


### BREAKING CHANGES

* **rules:** reorganize rule IDs into category-based ranges. All rule IDs have been renumbered to reflect their category. The renumbering map:
  - **0xx — Unsafe DDL**: PGM001 (unchanged), PGM002 (unchanged), PGM006→PGM003, PGM007→PGM006, PGM009→PGM007, PGM010→PGM008, PGM011→PGM009, PGM013→PGM010, PGM014→PGM011, PGM015→PGM012, PGM016→PGM013, PGM017→PGM014, PGM018→PGM015, PGM012→PGM016, PGM021→PGM017
  - **1xx — Type anti-patterns**: PGM101–PGM105 (unchanged), PGM108→PGM106
  - **2xx — Destructive operations**: PGM022→PGM201
  - **4xx — Idempotency guards**: PGM008→PGM401, PGM023→PGM402
  - **5xx — Schema design**: PGM003→PGM501, PGM004→PGM502, PGM005→PGM503, PGM019→PGM504, PGM020→PGM505
  - **9xx — Meta-behavior**: PGM901 (unchanged)
* **rules:** rename enum types: `MigrationRule`→`UnsafeDdlRule`, `TypeChoiceRule`→`TypeAntiPatternRule`. New enums: `DestructiveRule`, `IdempotencyRule`, `SchemaDesignRule`.

  Users must update any `--explain`, suppression comments, and CI configurations that reference old rule IDs.

## [2.14.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.13.0...v2.14.0) (2026-03-01)


### Features

* **catalog:** add DropNotNull, DropConstraint, ValidateConstraint support ([3516094](https://github.com/robert-sjoblom/pg-migration-lint/commit/351609453dd28cb11bb19986730046b31001f8eb))
* **rules:** add PGM023 -- VACUUM FULL on existing table ([5e75bf8](https://github.com/robert-sjoblom/pg-migration-lint/commit/5e75bf808a2717e976f9371f35750ebf2601df99))
* **rules:** add PGM107 — integer primary key detection ([5cf0d12](https://github.com/robert-sjoblom/pg-migration-lint/commit/5cf0d1240aa1959ca09cd46a9baacf566bc10171))
* **rules:** add PGM507 — DROP NOT NULL on existing table ([4277d1c](https://github.com/robert-sjoblom/pg-migration-lint/commit/4277d1cda633d2b6ef4d12da09bc4537916191a7))


### Bug Fixes

* **rules:** correct lock level and volatile functions ([264d9a6](https://github.com/robert-sjoblom/pg-migration-lint/commit/264d9a6df13fac02ab5d3c04892b13324dea56db))

## [2.13.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.12.0...v2.13.0) (2026-02-27)


### Features

* **rules:** add PGM205 — DROP SCHEMA CASCADE ([6d553a1](https://github.com/robert-sjoblom/pg-migration-lint/commit/6d553a1baa6bf536c7a7dcbeee53ab5078796c43))

## [2.12.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.11.0...v2.12.0) (2026-02-27)


### Features

* **rules:** add PGM019 — ADD EXCLUDE constraint on existing table ([97932e2](https://github.com/robert-sjoblom/pg-migration-lint/commit/97932e23a0dd54e2260bc4845dbdaa5ebf25bab1))
* **rules:** add PGM020 — DISABLE TRIGGER on tables ([1af3cd7](https://github.com/robert-sjoblom/pg-migration-lint/commit/1af3cd78601894e5b00d1b747cc16c8d4ffe20b9))


### Bug Fixes

* **ci:** add explicit permissions block to workflows ([#129](https://github.com/robert-sjoblom/pg-migration-lint/issues/129)) ([8fd0f19](https://github.com/robert-sjoblom/pg-migration-lint/commit/8fd0f19dcf884248675d996191fa6ca950940ca2))
* **rules:** remove bit(N)-&gt;bit(M) from PGM007 safe-widening list ([ed0cdba](https://github.com/robert-sjoblom/pg-migration-lint/commit/ed0cdba62a31d1277f51bcbb9cb8471724eed81a))

## [2.11.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.10.0...v2.11.0) (2026-02-26)


### Features

* **rules:** add ALTER INDEX ATTACH PARTITION support ([a22d29b](https://github.com/robert-sjoblom/pg-migration-lint/commit/a22d29b0a87c97a0360a42c76a053d3c197f893e))
* **rules:** make PGM005 CHECK matching partition-column-aware ([39c208e](https://github.com/robert-sjoblom/pg-migration-lint/commit/39c208e20eaa807d84734951f06ceefa70df44f6))

## [2.10.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.9.0...v2.10.0) (2026-02-25)


### Features

* **catalog:** add partition awareness to IR, parser, catalog, and replay ([07ced02](https://github.com/robert-sjoblom/pg-migration-lint/commit/07ced02b5bb4a2684ff24bd6bc3ad20e819c6527))
* **rules:** add PGM004 and PGM005 partition rules ([d73bf24](https://github.com/robert-sjoblom/pg-migration-lint/commit/d73bf24c4e2755a2e4fca8ceabfc75027fb0d75a))
* **rules:** make PGM001, PGM501, PGM502, PGM503 partition-aware (Pass 2) ([145a40d](https://github.com/robert-sjoblom/pg-migration-lint/commit/145a40d4678f3720b8d9cdb7cb704669364cb79b))

## [2.9.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.8.0...v2.9.0) (2026-02-23)


### Features

* add SonarQube native plugin design document ([144e4fc](https://github.com/robert-sjoblom/pg-migration-lint/commit/144e4fca538a985d4bce815f5ab610b7b8c2f6d1))
* **catalog:** track expression index column references for DROP/RENAME ([5ad8663](https://github.com/robert-sjoblom/pg-migration-lint/commit/5ad86630def98fec6a0bcb9e5a5e00805bc2a5d8))

## [2.8.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.7.0...v2.8.0) (2026-02-23)


### Features

* **catalog:** track partial and expression indexes in index pipeline ([dd89ce0](https://github.com/robert-sjoblom/pg-migration-lint/commit/dd89ce05532e766f0b016f1e4a0b73e32027b32b))

## [2.7.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.6.0...v2.7.0) (2026-02-20)


### Features

* **docs:** add GitHub Pages site and link SonarQube descriptions to docs ([1eb2458](https://github.com/robert-sjoblom/pg-migration-lint/commit/1eb2458753b9dcacbe618725b30bfabcb78508d9))

## [2.6.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.5.1...v2.6.0) (2026-02-19)


### Features

* **rules:** implement PGM018 — CLUSTER on existing table ([d5c66db](https://github.com/robert-sjoblom/pg-migration-lint/commit/d5c66dbb780ce61d8f80a56252daff70f40c364d))


### Bug Fixes

* **input:** warn when update-sql fails (duplication) ([0f009f5](https://github.com/robert-sjoblom/pg-migration-lint/commit/0f009f53d1745bc36be0d7c954e8368396a38585))
* **main:** CREATE TABLE IF NOT EXISTS no-op should not mask existing-table rules ([8c110c2](https://github.com/robert-sjoblom/pg-migration-lint/commit/8c110c24452589c91d490e973fba8155dfc7758f))

## [2.5.1](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.5.0...v2.5.1) (2026-02-19)


### Bug Fixes

* **input:** tighten down-migration filename detection to suffix-based matching ([68f8286](https://github.com/robert-sjoblom/pg-migration-lint/commit/68f82860c6b59fcaf7dd2e2dd6aa5a4b25be4301))

## [2.5.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.4.0...v2.5.0) (2026-02-18)


### Features

* **rules:** add PGM301, PGM302, PGM303, PGM506 — DML-in-migration and UNLOGGED TABLE rules ([4bca1a6](https://github.com/robert-sjoblom/pg-migration-lint/commit/4bca1a661a006fe47245b344f5ff4886177228fb))

## [2.4.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.3.0...v2.4.0) (2026-02-18)


### Features

* **rules:** add PGM203 + PGM204 — TRUNCATE TABLE rules ([4c06336](https://github.com/robert-sjoblom/pg-migration-lint/commit/4c06336c2ceb179207a819a678ce3a078e62b16a))
* **rules:** add PGM403 — CREATE TABLE IF NOT EXISTS for already-existing table ([ed1ce17](https://github.com/robert-sjoblom/pg-migration-lint/commit/ed1ce179969c75050631a99f9059d724d2bef881))


### Bug Fixes

* **bridge:** resolve XML line numbers for Liquibase changesets ([310f828](https://github.com/robert-sjoblom/pg-migration-lint/commit/310f828b9b2f6666eb3a8e7bd7bc4e897871e215))

## [2.3.0](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.2.2...v2.3.0) (2026-02-18)


### Features

* **rules:** add PGM202 — DROP TABLE CASCADE on existing table ([fd75c6e](https://github.com/robert-sjoblom/pg-migration-lint/commit/fd75c6ee397dc6d80a79a0e04a71178346fae7a5))

## [2.2.2](https://github.com/robert-sjoblom/pg-migration-lint/compare/v2.2.1...v2.2.2) (2026-02-18)


### Bug Fixes

* reorganize rule IDs into coherent numeric ranges [coverage] ([634f14f](https://github.com/robert-sjoblom/pg-migration-lint/commit/634f14f493f034ce95f9cd43d1316956b998059c))

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
