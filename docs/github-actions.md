---
layout: default
title: GitHub Actions Integration
---

# GitHub Actions Integration

## Basic: Lint changed migrations on pull requests

This workflow detects changed SQL migration files, runs pg-migration-lint, and uploads SARIF results so that findings appear as inline annotations on the pull request.

Copy this file to `.github/workflows/migration-lint.yml` in your repository:

```yaml
name: Migration Lint

on:
  pull_request:
    paths:
      - 'db/migrations/**'

jobs:
  lint:
    runs-on: ubuntu-latest
    permissions:
      security-events: write  # Required for uploading SARIF to GitHub Code Scanning
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0  # Full history so git diff can compare against the base branch

      - name: Get changed migration files
        id: changes
        run: |
          files=$(git diff --name-only origin/${{ github.base_ref }}...HEAD -- 'db/migrations/*.sql' | tr '\n' ',')
          echo "files=$files" >> "$GITHUB_OUTPUT"

      - name: Download pg-migration-lint
        run: |
          curl -LO https://github.com/robert-sjoblom/pg-migration-lint/releases/latest/download/pg-migration-lint-x86_64-linux.tar.gz
          tar xzf pg-migration-lint-x86_64-linux.tar.gz
          chmod +x pg-migration-lint

      - name: Run linter
        if: steps.changes.outputs.files != ''
        run: |
          ./pg-migration-lint \
            --changed-files "${{ steps.changes.outputs.files }}" \
            --format sarif \
            --fail-on critical

      - name: Upload SARIF
        if: always() && steps.changes.outputs.files != ''
        uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: build/reports/migration-lint/findings.sarif
```

## What each step does

- **`fetch-depth: 0`** -- By default, `actions/checkout` performs a shallow clone (only the latest commit). The `git diff` step needs the full commit history to compare your PR branch against the base branch. Without this, the diff will fail or produce incorrect results.

- **`permissions: security-events: write`** -- GitHub requires this permission to upload SARIF files to the Code Scanning API. Without it, the "Upload SARIF" step will fail with a 403 error.

- **Get changed migration files** -- This step runs `git diff --name-only` to list only the SQL files that changed between the base branch and the PR head. The filenames are joined with commas because `--changed-files` expects a comma-separated list. If no migration files changed, the output is empty and subsequent steps are skipped.

- **`--fail-on critical`** -- The linter exits with code 1 if any finding has Critical severity or higher. This blocks the PR (when branch protection requires the check to pass). Set to `major`, `minor`, or `info` to be stricter, or `none` to never fail.

- **`if: always()`** on the SARIF upload step -- The "Run linter" step exits with code 1 when it finds Critical issues, which normally causes subsequent steps to be skipped. The `if: always()` condition ensures the SARIF file is still uploaded so that findings appear as inline annotations on the PR, even when the lint step "fails."
