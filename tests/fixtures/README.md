# Test Fixtures

This directory contains test fixtures for pg-migration-lint.

## Structure

- `repos/` - Full fixture repositories for integration tests
  - `all-rules/` - One violation per rule, expect findings for all rules
  - `catalog-ops/` - Tests catalog operations: DROP NOT NULL, DROP CONSTRAINT, SET/DROP DEFAULT, hash indexes
  - `clean/` - All migrations correct, expect 0 findings
  - `enterprise/` - Realistic 31-file migration history based on anonymized production schema
  - `fk-with-later-index/` - Tests PGM501 FK detection across migration boundaries
  - `go-migrate/` - Tests go-migrate convention (.up.sql / .down.sql pairs)
  - `liquibase-multi-schema/` - Tests Liquibase bridge with multi-schema changelogs
  - `liquibase-xml/` - Liquibase XML fixture for bridge tests
  - `multi-schema/` - Tests catalog tracking across multiple schemas (cross-schema FKs, name isolation, drop isolation)
  - `schema-qualified/` - Tests schema-qualified names and cross-schema references
  - `suppressed/` - All violations suppressed via inline comments, expect 0 findings

## Usage

Component tests use individual SQL snippets.
Integration tests use the full repositories in `repos/`.
