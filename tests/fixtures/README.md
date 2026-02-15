# Test Fixtures

This directory contains test fixtures for pg-migration-lint.

## Structure

- `repos/` - Full fixture repositories for integration tests
  - `clean/` - All migrations correct, expect 0 findings
  - `all-rules/` - One violation per rule (PGM001-PGM011), expect 11 findings
  - `suppressed/` - All violations suppressed, expect 0 findings
  - `fk-with-later-index/` - Tests PGM003 FK detection across migration boundaries
  - `liquibase-xml/` - Liquibase XML fixture for bridge tests
  - `schema-qualified/` - Tests schema-qualified names and cross-schema references

## Usage

Component tests will use individual SQL snippets.
Integration tests will use the full repositories in `repos/`.

Phase 1 agents will populate these directories with actual test cases.
