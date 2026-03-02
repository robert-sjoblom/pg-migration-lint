---
layout: default
title: SonarQube Integration
---

# SonarQube Integration

pg-migration-lint can produce SonarQube Generic Issue Import JSON alongside SARIF. Since the `--format` CLI flag accepts only a single format, use a configuration file to produce multiple formats simultaneously.

## Configuration

Create or update your `pg-migration-lint.toml`:

```toml
[output]
formats = ["sarif", "sonarqube"]
```

The SonarQube JSON file will be written to `build/reports/migration-lint/findings.json`.

## SonarQube scanner setup

Configure your SonarQube scanner to import the findings. Add this to your `sonar-project.properties`:

```properties
sonar.externalIssuesReportPaths=build/reports/migration-lint/findings.json
```

> **Note:** In Java projects (and some other language plugins), SonarQube does not index `.xml` or `.sql` files by default. If findings reference files that SonarQube has not indexed, they will be silently dropped from the report. Add the relevant extensions to `sonar.sources` or use `sonar.inclusions` to ensure your migration files are covered -- for example: `sonar.inclusions=src/**,db/migrations/**`.

## GitHub Actions example

In your GitHub Actions workflow, run the linter before the SonarQube scanner step:

```yaml
      - name: Run migration linter
        if: steps.changes.outputs.files != ''
        run: |
          ./pg-migration-lint \
            --changed-files "${{ steps.changes.outputs.files }}" \
            --fail-on critical

      - name: SonarQube Scan
        uses: SonarSource/sonarqube-scan-action@v3
        env:
          SONAR_TOKEN: ${{ secrets.SONAR_TOKEN }}
```

When using the config file, the `--format` flag is not needed -- the tool reads formats from `[output].formats` in the config.
