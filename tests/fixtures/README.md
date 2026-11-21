# Test Fixtures

This directory contains test fixtures for pg-migration-lint.

## Structure

- `repos/` - Full fixture repositories for integration tests
  - `all-rules/` - One violation per rule, expect findings for all rules
  - `bridge-table/` - Tests PGM501 on a bridge table whose composite PK `(x_id, y_id)` covers both FKs, expect no PGM501 finding
  - `catalog-ops/` - Tests catalog operations: DROP NOT NULL, DROP CONSTRAINT, SET/DROP DEFAULT, hash indexes, an `ADD PRIMARY KEY USING INDEX` of another table's index and a `CREATE UNIQUE INDEX` onto another table's index name, which replay leaves out
  - `clean/` - All migrations correct, expect 0 findings
  - `down-migrations/` - Tests that a down migration never changes the replayed schema: `V001__create_users_down.sql` drops the table `V001__create_users.sql` creates, and `V002__index_users_email.sql` still indexes an existing table (PGM001), linted or replayed
  - `enterprise/` - Realistic 31-file migration history based on anonymized production schema
  - `fk-with-later-index/` - Tests PGM501 FK detection across migration boundaries: an FK added before its covering index, the index dropped again later, created again, and then renamed and dropped under its new name in one file
  - `go-migrate/` - Tests go-migrate convention (.up.sql / .down.sql pairs)
  - `liquibase-fresh/` - Liquibase fixture for the bridge fresh-repo test (every changeset changed, expect only PGM402)
  - `liquibase-multi-schema/` - Tests Liquibase bridge with multi-schema changelogs
  - `liquibase-rollback/` - Liquibase changesets with `<rollback>` blocks (inline SQL, nested changes, `<sqlFile>`, a `changeSetId` reference, empty, rollback only) and a derived rollback; both loaders must emit only the forward SQL
  - `liquibase-xml/` - Liquibase XML fixture for bridge tests
  - `multi-schema/` - Tests catalog tracking across multiple schemas (cross-schema FKs, name isolation, drop isolation)
  - `partition-lock-hazard/` - Tests PGM024 and PGM026 on a repartition of 5 partitioned parents in one transaction (PGM024: one finding per parent; PGM026: one finding naming all five), and the safe DETACH CONCURRENTLY then DROP
  - `repeated-findings/` - Grouping: 20 `DROP TABLE` without IF EXISTS in one file (PGM201 and PGM401 floods), and a file where one suppressed statement keeps PGM401 below `group_threshold`
  - `schema-qualified/` - Tests schema-qualified names and cross-schema references
  - `suppressed/` - All violations suppressed via inline comments, expect 0 findings

## Usage

Component tests use individual SQL snippets.
Integration tests use the full repositories in `repos/`.
