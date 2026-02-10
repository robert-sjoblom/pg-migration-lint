# Liquibase Bridge JAR

A minimal Java program (~100 LOC) that embeds Liquibase as a library and produces structured JSON output mapping changesets to their generated SQL statements. This enables `pg-migration-lint` to lint Liquibase XML changelogs with exact changeset-to-SQL traceability and line number mapping.

## What it does

The bridge takes a Liquibase changelog XML file as input, iterates through all changesets, generates the SQL that Liquibase would produce for PostgreSQL, and outputs a JSON array to stdout:

```json
[
  {
    "changeset_id": "20240315-1",
    "author": "robert",
    "sql": "CREATE TABLE orders (id INTEGER, status TEXT);",
    "xml_file": "db/changelog/20240315-create-orders.xml",
    "xml_line": 1,
    "run_in_transaction": true
  }
]
```

The Rust side (`src/input/liquibase_bridge.rs`) parses this JSON and feeds the SQL into the standard linting pipeline.

## Building

No local Java or Maven installation is required. Everything is built inside Docker.

### Using the build script

```bash
./bridge/build.sh
```

This builds the Docker image and extracts the fat JAR to `tools/liquibase-bridge.jar`.

### Using Make

```bash
cd bridge/
make            # Build and extract JAR to ../tools/
make image-only # Build Docker image only
make clean      # Remove JAR and Docker artifacts
```

### Manual Docker commands

```bash
# Build the image
docker build -t pg-migration-lint-bridge bridge/

# Extract the JAR
docker create --name bridge-extract pg-migration-lint-bridge
docker cp bridge-extract:/app/liquibase-bridge.jar tools/
docker rm bridge-extract
```

## Usage

Once the JAR is built, `pg-migration-lint` invokes it automatically when configured:

```toml
# pg-migration-lint.toml
[liquibase]
bridge_jar_path = "tools/liquibase-bridge.jar"
strategy = "auto"  # or "bridge" to use only this strategy
```

You can also invoke it directly for testing:

```bash
java -jar tools/liquibase-bridge.jar --changelog db/changelog/changelog-master.xml
```

## How it works

1. Initializes Liquibase with an **offline PostgreSQL connection** (no actual database required).
2. Parses the changelog XML using Liquibase's own parser, which resolves `<include>` directives and preconditions.
3. For each changeset, calls `generateStatements()` on every change and converts them to SQL using Liquibase's SQL generator for PostgreSQL.
4. Outputs the results as a JSON array where each entry contains the changeset ID, author, generated SQL, source file path, and transaction mode.

## Requirements

- Docker (for building)
- Java 11+ runtime (for running the JAR; included in the Docker image)

## Project structure

```
bridge/
  pom.xml         Maven project with Liquibase + Gson dependencies and shade plugin
  Dockerfile      Multi-stage build: Maven build then slim JRE runtime
  build.sh        Build script (Docker-based, no local Java needed)
  Makefile         Alternative build via Make
  src/main/java/
    com/pgmigrationlint/
      LiquibaseBridge.java   The bridge program (~100 LOC)
```
