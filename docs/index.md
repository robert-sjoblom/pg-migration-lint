---
layout: default
title: Home
---

# pg-migration-lint

Static analysis for PostgreSQL migration files. Catches unsafe DDL, type anti-patterns, destructive operations, and schema design issues before they hit production.

## Quick links

- [Rule Reference](rules) — all 42 lint rules with examples and fixes
- [GitHub Repository](https://github.com/robert-sjoblom/pg-migration-lint)

## Installation

Download the latest release binary from the [releases page](https://github.com/robert-sjoblom/pg-migration-lint/releases), or build from source:

```bash
git clone https://github.com/robert-sjoblom/pg-migration-lint.git
cd pg-migration-lint
cargo build --release
# Binary is at target/release/pg-migration-lint
```

## Usage

```bash
# Lint all migrations
pg-migration-lint --config pg-migration-lint.toml

# Lint only changed files (typical CI usage)
pg-migration-lint --changed-files V042__add_index.sql,V043__add_fk.sql

# Explain a specific rule
pg-migration-lint --explain PGM001
```
