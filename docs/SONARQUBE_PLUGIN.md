# SonarQube Native Plugin for pg-migration-lint

## Context

pg-migration-lint currently outputs SonarQube Generic Issue Import JSON. While functional, this means rules appear as "external issues" in SonarQube — no Quality Profile management, no per-rule severity overrides, no browsable rule documentation. A native SonarQube plugin solves all of this.

The plugin will be a Java wrapper that bundles the Rust binary (x86_64 + aarch64 musl), extracts the right one at runtime, runs it, and reports findings as native SonarQube issues.

## Decisions

- **Language key**: `"pgmigration"` (avoids conflicts with other SQL plugins)
- **SonarQube version**: 10.x LTS (Clean Code Taxonomy API available)
- **Rule metadata**: Generated from Rust at build time via `--dump-rules-json` (no duplication)
- **Binary delivery**: Bundled inside the plugin JAR, extracted at runtime
- **Report capture**: Rust binary writes to stdout (new `--stdout` flag), sensor captures it directly
- **Sensor run-gate**: Config file presence, not language file index (see [Part 5](#part-5-file-indexing-and-migration-scoping))

---

## Part 1: Rust CLI Changes

Two small additions to the Rust binary. No changes to existing behavior.

### 1a. Add `--dump-rules-json` flag

**File**: `src/main.rs`

Add a new clap arg `--dump-rules-json` (bool flag). When set, print a JSON array of all rule metadata to stdout and exit. The JSON includes fields needed by the Java plugin: `id`, `name`, `description` (explain text), `severity`, `cleanCodeAttribute`, `issueType`, `softwareQuality`, `impactSeverity`, `effortMinutes`, `docsUrl`, `tags`.

Implementation:
- Add field to `Args` struct
- Handle early exit in `run()` (like `--explain`)
- Create a serializable struct that combines data from `RuleRegistry`, `RuleInfo::from_registry()`, `sonarqube_meta()`, and `effort_minutes()`
- Reuse the existing `DOCS_BASE_URL` constant from `src/output/sonarqube.rs`
- Tags derived from rule family (0xx→`["postgresql","migration","ddl","locking"]`, 1xx→`["postgresql","migration","type"]`, etc.)

The functions `sonarqube_meta()` and `effort_minutes()` are currently private to `src/output/sonarqube.rs`. They need to be made `pub(crate)` so `main.rs` can call them.

#### Explain string format contract

The `explain()` output from each rule follows a consistent structure that `RuleDescriptionFormatter` (Java) relies on for HTML conversion. The expected format is:

```
What it detects:
  Prose description of the rule.

Why it's dangerous:
  Prose description of the risk.

Example (bad):
    CREATE INDEX idx_foo ON bar (col);

Fix:
    CREATE INDEX CONCURRENTLY idx_foo ON bar (col);
```

Sections are identified by a line ending in `:` with no leading indentation. SQL blocks are indented by 4+ spaces. This contract must be documented in the Rust codebase (e.g. as a doc comment on the `Rule::explain()` trait method) before the Java formatter is implemented.

#### Example output

```json
[
  {
    "id": "PGM001",
    "name": "Missing CONCURRENTLY on CREATE INDEX",
    "description": "What it detects:\n  CREATE INDEX on an existing table ...",
    "severity": "CRITICAL",
    "cleanCodeAttribute": "COMPLETE",
    "issueType": "BUG",
    "softwareQuality": "RELIABILITY",
    "impactSeverity": "HIGH",
    "effortMinutes": 5,
    "docsUrl": "https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm001",
    "tags": ["postgresql", "migration", "ddl", "locking"]
  }
]
```

#### Test note

The `--dump-rules-json` snapshot test emits all 39 rules (including PGM901). The 38-rule count referenced in the Java tests is correct because PGM901 exclusion is a Java-side concern — the Rust binary dumps everything, and `PgMigrationLintRulesDefinition` filters it out during registration.

### 1b. Add `--stdout` flag for report output

**File**: `src/main.rs`

Add a `--stdout` flag. When set, write the report to stdout instead of a file. This lets the SonarQube sensor capture output without needing to coordinate file paths.

`--stdout` works with all `--format` values (sonarqube, sarif, text), not just sonarqube. The flag controls output destination, not format. Implementation: In the emit loop, when `--stdout` is set, call `reporter.render()` and print the result instead of calling `reporter.emit()`.

---

## Part 2: SonarQube Plugin (Java)

New Maven module at `sonarqube-plugin/` alongside `bridge/`.

### Project Structure

```
sonarqube-plugin/
├── pom.xml
├── Makefile
└── src/
    ├── main/
    │   ├── java/com/pgmigrationlint/sonar/
    │   │   ├── PgMigrationLintPlugin.java
    │   │   ├── PgMigrationLintRulesDefinition.java
    │   │   ├── PgMigrationLintSensor.java
    │   │   ├── PgMigrationLintQualityProfile.java
    │   │   ├── PgMigrationLanguage.java
    │   │   ├── BinaryExtractor.java
    │   │   ├── RuleMetadata.java
    │   │   ├── RuleMetadataLoader.java
    │   │   ├── RuleDescriptionFormatter.java
    │   │   └── SonarQubeReport.java
    │   └── resources/
    │       ├── com/pgmigrationlint/sonar/rules.json  (generated at build time)
    │       └── binaries/
    │           ├── x86_64/pg-migration-lint           (copied by CI)
    │           └── aarch64/pg-migration-lint          (copied by CI)
    └── test/
        ├── java/com/pgmigrationlint/sonar/
        │   ├── PgMigrationLintPluginTest.java
        │   ├── PgMigrationLintRulesDefinitionTest.java
        │   ├── PgMigrationLintSensorTest.java
        │   ├── BinaryExtractorTest.java
        │   └── RuleMetadataLoaderTest.java
        └── resources/fixtures/
            └── sample-report.json
```

### Maven Configuration (`pom.xml`)

- **Packaging**: `sonar-plugin` (via `sonar-packaging-maven-plugin 1.23.0.740`)
- **Plugin key**: `pgmigrationlint`
- **Plugin class**: `com.pgmigrationlint.sonar.PgMigrationLintPlugin`
- **Min SonarQube**: `10.0`
- **Java**: 17 (matching bridge)
- **Dependencies**:
  - `org.sonarsource.api.plugin:sonar-plugin-api:10.14.0.2599` (scope: `provided`)
  - `com.google.code.gson:gson:2.13.2` (bundled)
  - `org.junit.jupiter:junit-jupiter:5.11.4` (test)
  - `org.assertj:assertj-core:3.27.3` (test)
  - `org.mockito:mockito-core:5.14.2` (test)

### Java Classes

#### `PgMigrationLintPlugin`

Entry point. Registers all extensions including `BinaryExtractor` as a singleton (so the extracted path is cached across sensor re-instantiations within a single SonarQube analysis):

- `BinaryExtractor.class` (singleton — injected into sensor)
- `PgMigrationLanguage.class`
- `PgMigrationLintRulesDefinition.class`
- `PgMigrationLintSensor.class`
- `PgMigrationLintQualityProfile.class`
- Property definitions for `sonar.pgmigrationlint.configFile`, `sonar.pgmigrationlint.binaryPath`, `sonar.pgmigrationlint.file.suffixes`, and `sonar.pgmigrationlint.changedFiles`

```java
public class PgMigrationLintPlugin implements Plugin {
    @Override
    public void define(Context context) {
        context.addExtensions(
            BinaryExtractor.class,
            PgMigrationLanguage.class,
            PgMigrationLintRulesDefinition.class,
            PgMigrationLintSensor.class,
            PgMigrationLintQualityProfile.class
        );
        context.addExtensions(
            PropertyDefinition.builder("sonar.pgmigrationlint.configFile")
                .name("Config file path")
                .description("Path to pg-migration-lint.toml")
                .defaultValue("pg-migration-lint.toml")
                .category("pg-migration-lint")
                .build(),
            PropertyDefinition.builder("sonar.pgmigrationlint.binaryPath")
                .name("Binary path override")
                .description("Path to pg-migration-lint binary (overrides bundled)")
                .category("pg-migration-lint")
                .build(),
            PropertyDefinition.builder("sonar.pgmigrationlint.file.suffixes")
                .name("File suffixes")
                .description("Comma-separated list of file suffixes for migration files")
                .defaultValue(".sql,.xml")
                .category("pg-migration-lint")
                .build(),
            PropertyDefinition.builder("sonar.pgmigrationlint.changedFiles")
                .name("Changed files")
                .description("Comma-separated list of changed migration files (set by CI)")
                .category("pg-migration-lint")
                .build()
        );
    }
}
```

#### `PgMigrationLanguage`

Registers `"pgmigration"` language with `.sql` and `.xml` file suffixes (matching the tool's default `include = ["*.sql", "*.xml"]`). Configurable via `sonar.pgmigrationlint.file.suffixes` property (registered in plugin, configurable from the SonarQube UI).

```java
public class PgMigrationLanguage extends AbstractLanguage {
    public static final String KEY = "pgmigration";
    public static final String NAME = "PG Migration";
    private static final String[] DEFAULT_SUFFIXES = { ".sql", ".xml" };

    private final Configuration config;

    public PgMigrationLanguage(Configuration config) {
        super(KEY, NAME);
        this.config = config;
    }

    @Override
    public String[] getFileSuffixes() {
        String[] suffixes = config.getStringArray("sonar.pgmigrationlint.file.suffixes");
        return suffixes.length > 0 ? suffixes : DEFAULT_SUFFIXES;
    }
}
```

#### `PgMigrationLintRulesDefinition`

Loads `rules.json` via `RuleMetadataLoader`, creates a repository `"pgmigrationlint"` for language `"pgmigration"`, and registers each rule with:

- Name (short description)
- HTML description (converted from plain-text explain via `RuleDescriptionFormatter`)
- Severity, type, status
- Clean code attribute + default impact (SonarQube 10.x API)
- Debt remediation function (constant effort in minutes)
- Tags
- PGM901 is excluded (meta-behavior, not a real rule)

Note: `setDebtRemediationFunction` requires a two-step assignment — `NewRule` must be fully constructed before calling `debtRemediationFunctions()` on it.

```java
public class PgMigrationLintRulesDefinition implements RulesDefinition {
    private static final String REPO_KEY = "pgmigrationlint";

    @Override
    public void define(Context context) {
        List<RuleMetadata> rules = RuleMetadataLoader.load();
        NewRepository repo = context.createRepository(REPO_KEY, PgMigrationLanguage.KEY)
            .setName("pg-migration-lint");

        for (RuleMetadata rule : rules) {
            if (rule.id().equals("PGM901")) continue; // meta-behavior, not a real rule

            NewRule newRule = repo.createRule(rule.id())
                .setName(rule.name())
                .setHtmlDescription(RuleDescriptionFormatter.toHtml(rule))
                .setSeverity(rule.sonarSeverity())
                .setType(rule.sonarRuleType())
                .setStatus(RuleStatus.READY)
                .addDefaultImpact(rule.sonarSoftwareQuality(), rule.sonarImpactSeverity())
                .setCleanCodeAttribute(rule.sonarCleanCodeAttribute())
                .setTags(rule.tags().toArray(String[]::new));

            // Must be set after NewRule is constructed
            newRule.setDebtRemediationFunction(
                newRule.debtRemediationFunctions()
                    .constantPerIssue(rule.effortMinutes() + "min")
            );
        }

        repo.done();
    }
}
```

#### `RuleMetadata`

Java record deserializing the JSON entries. Provides conversion methods to SonarQube API types (`CleanCodeAttribute`, `RuleType`, `SoftwareQuality`, `Severity`).

```java
public record RuleMetadata(
    String id,
    String name,
    String description,
    String severity,
    String cleanCodeAttribute,
    String issueType,
    String softwareQuality,
    String impactSeverity,
    int effortMinutes,
    String docsUrl,
    List<String> tags
) {
    public org.sonar.api.rules.RuleType sonarRuleType() {
        return switch (issueType) {
            case "BUG" -> org.sonar.api.rules.RuleType.BUG;
            case "CODE_SMELL" -> org.sonar.api.rules.RuleType.CODE_SMELL;
            default -> org.sonar.api.rules.RuleType.CODE_SMELL;
        };
    }

    public String sonarSeverity() {
        return switch (severity) {
            case "CRITICAL" -> "CRITICAL";
            case "MAJOR" -> "MAJOR";
            case "WARNING" -> "MINOR";
            case "INFO" -> "INFO";
            default -> "MINOR";
        };
    }

    // ... similar conversion methods for CleanCodeAttribute,
    //     SoftwareQuality, ImpactSeverity
}
```

#### `RuleMetadataLoader`

Reads `/com/pgmigrationlint/sonar/rules.json` from classpath via Gson.

```java
public final class RuleMetadataLoader {
    private RuleMetadataLoader() {}

    public static List<RuleMetadata> load() {
        try (var stream = RuleMetadataLoader.class.getResourceAsStream(
                "/com/pgmigrationlint/sonar/rules.json")) {
            var reader = new InputStreamReader(stream, StandardCharsets.UTF_8);
            return new Gson().fromJson(reader,
                new TypeToken<List<RuleMetadata>>() {}.getType());
        }
    }
}
```

#### `RuleDescriptionFormatter`

Converts plain-text explain strings to HTML for SonarQube rule pages:

- Escape HTML entities
- Detect section headers ("What it detects:", "Why it's dangerous:", "Example (bad):", "Fix:") → `<h3>` tags
- Detect indented SQL blocks (4+ spaces) → `<pre><code>` blocks
- Append link to GitHub Pages docs

This relies on the explain string format contract defined in [Part 1a](#explain-string-format-contract). The contract must be enforced in Rust before this formatter is implemented.

```java
public final class RuleDescriptionFormatter {
    private RuleDescriptionFormatter() {}

    public static String toHtml(RuleMetadata rule) {
        StringBuilder html = new StringBuilder();
        // Parse plain text explain into sections
        // Convert sections to HTML with <h3>, <p>, <pre><code> blocks
        // Append docs link
        html.append("<p>Full documentation: <a href=\"")
            .append(rule.docsUrl())
            .append("\">")
            .append(rule.id())
            .append("</a></p>");
        return html.toString();
    }
}
```

#### `PgMigrationLintQualityProfile`

Registers a default quality profile `"pg-migration-lint way"` for the `"pgmigration"` language with all rules activated at their default severities.

```java
public class PgMigrationLintQualityProfile implements BuiltInQualityProfilesDefinition {
    @Override
    public void define(Context context) {
        List<RuleMetadata> rules = RuleMetadataLoader.load();
        NewBuiltInQualityProfile profile = context.createBuiltInQualityProfile(
            "pg-migration-lint way", PgMigrationLanguage.KEY);

        for (RuleMetadata rule : rules) {
            if (rule.id().equals("PGM901")) continue;
            profile.activateRule("pgmigrationlint", rule.id());
        }

        profile.done();
    }
}
```

#### `PgMigrationLintSensor`

The analysis engine. Run-gate is config file presence, not language file index (see [Part 5](#part-5-file-indexing-and-migration-scoping)).

1. `describe()`: creates issues for `"pgmigrationlint"` repository
2. `execute()`:
   - Skip if config file (`pg-migration-lint.toml`) does not exist at the configured path
   - Resolve binary (user override → bundled extraction via injected `BinaryExtractor` singleton)
   - Build command args: `pg-migration-lint --config <path> --format sonarqube --stdout --fail-on none`
   - If `sonar.pgmigrationlint.changedFiles` is set, append `--changed-files <value>`
   - Capture stdout, parse as `SonarQubeReport` JSON
   - For each issue: look up `InputFile` by relative path, create `NewIssue` with `RuleKey.of("pgmigrationlint", ruleId)`, set location + message + line range
   - When `fs.inputFile()` returns null for a reported file, log a warning (not silent drop) and continue
   - Log summary

**Error handling for binary execution**: `--fail-on none` prevents non-zero exit from lint findings, but the binary can still fail (config not found, parse errors). On non-zero exit:
- Log stderr at WARN level
- If stdout contains valid JSON, process it (partial results are still useful)
- If stdout is empty or not valid JSON, log an error and return without failing the analysis (do not throw `SonarRunnerException`)

**Note on `fs.baseDir()`**: This method is deprecated in SonarQube 10.x. Verify the correct replacement in the 10.x API before implementation. Candidates include `context.project().baseDir()` or the module filesystem API.

```java
public class PgMigrationLintSensor implements Sensor {
    private static final Logger LOG = LoggerFactory.getLogger(PgMigrationLintSensor.class);

    private final Configuration config;
    private final BinaryExtractor binaryExtractor;

    // BinaryExtractor injected as a singleton registered in PgMigrationLintPlugin
    public PgMigrationLintSensor(Configuration config, BinaryExtractor binaryExtractor) {
        this.config = config;
        this.binaryExtractor = binaryExtractor;
    }

    @Override
    public void describe(SensorDescriptor descriptor) {
        descriptor.name("pg-migration-lint")
            .createIssuesForRuleRepository("pgmigrationlint");
        // No .onlyOnLanguage() — run-gate is config file presence, not file index
    }

    @Override
    public void execute(SensorContext context) {
        String configFile = config.get("sonar.pgmigrationlint.configFile")
            .orElse("pg-migration-lint.toml");

        // Gate on config file presence, not language file index
        Path configPath = context.fileSystem().baseDir().toPath().resolve(configFile);
        if (!Files.exists(configPath)) {
            LOG.info("Config file {} not found, skipping pg-migration-lint", configFile);
            return;
        }

        String binary = resolveBinary();
        List<String> command = new ArrayList<>(List.of(
            binary, "--config", configFile,
            "--format", "sonarqube", "--stdout", "--fail-on", "none"
        ));

        // Forward changed-files from CI if present
        config.get("sonar.pgmigrationlint.changedFiles")
            .ifPresent(files -> {
                command.add("--changed-files");
                command.add(files);
            });

        ProcessBuilder pb = new ProcessBuilder(command);
        pb.directory(context.fileSystem().baseDir());
        pb.redirectErrorStream(false);

        SonarQubeReport report = runAndParse(pb);
        if (report == null) return; // logged in runAndParse

        FileSystem fs = context.fileSystem();
        int count = 0;
        int skipped = 0;
        for (SonarQubeReport.Issue issue : report.issues()) {
            InputFile inputFile = fs.inputFile(
                fs.predicates().hasRelativePath(issue.primaryLocation().filePath()));
            if (inputFile == null) {
                LOG.warn("File not indexed by SonarQube, issue skipped: {} ({})",
                    issue.primaryLocation().filePath(), issue.ruleId());
                skipped++;
                continue;
            }

            NewIssue newIssue = context.newIssue()
                .forRule(RuleKey.of("pgmigrationlint", issue.ruleId()));
            NewIssueLocation location = newIssue.newLocation()
                .on(inputFile)
                .message(issue.primaryLocation().message())
                .at(inputFile.selectLine(issue.primaryLocation().textRange().startLine()));
            newIssue.at(location).save();
            count++;
        }

        LOG.info("pg-migration-lint reported {} issues ({} skipped — files not indexed)",
            count, skipped);
    }

    private String resolveBinary() {
        return config.get("sonar.pgmigrationlint.binaryPath")
            .orElseGet(binaryExtractor::extract);
    }
}
```

#### `BinaryExtractor`

Platform-aware binary extraction. Registered as a singleton extension via `PgMigrationLintPlugin` so the extracted path is cached across sensor re-instantiations within a single SonarQube analysis.

- Detect arch via `os.arch` (amd64/x86_64 → `x86_64`, aarch64/arm64 → `aarch64`)
- Reject non-Linux (error message pointing to `sonar.pgmigrationlint.binaryPath`)
- Extract from `/binaries/<arch>/pg-migration-lint` to `$TMPDIR/pg-migration-lint-sonar/pg-migration-lint`
- Set executable permission
- Cache extracted path in static field (survives sensor re-instantiation)

```java
public class BinaryExtractor {
    private static volatile String cachedPath;

    public String extract() {
        if (cachedPath != null) return cachedPath;

        String os = System.getProperty("os.name", "").toLowerCase(Locale.ROOT);
        if (!os.contains("linux")) {
            throw new IllegalStateException(
                "Bundled binary only supports Linux. " +
                "Set sonar.pgmigrationlint.binaryPath for other platforms.");
        }

        String arch = normalizeArch(System.getProperty("os.arch", ""));
        String resourcePath = "/binaries/" + arch + "/pg-migration-lint";

        Path target = Path.of(System.getProperty("java.io.tmpdir"),
            "pg-migration-lint-sonar", "pg-migration-lint");
        // Extract from classpath, set executable, cache path
        // ...

        cachedPath = target.toString();
        return cachedPath;
    }

    private static String normalizeArch(String arch) {
        return switch (arch) {
            case "amd64", "x86_64" -> "x86_64";
            case "aarch64", "arm64" -> "aarch64";
            default -> throw new IllegalStateException(
                "Unsupported architecture: " + arch +
                ". Set sonar.pgmigrationlint.binaryPath manually.");
        };
    }
}
```

#### `SonarQubeReport`

DTOs for deserializing the Rust binary's JSON output. Reuses the existing format — no changes needed on the Rust output side.

The `rules` field is present in the Rust JSON output but intentionally ignored by the sensor. The sensor only reads the `issues` array. Rule metadata for SonarQube comes from the build-time `rules.json`, not from the runtime report. The `rules` field is deserialized to avoid parse errors but never accessed.

`TextRange` includes `startColumn`/`endColumn` for forward-compatibility with column-level highlighting, even though the Rust binary currently only emits line-level ranges.

```java
public record SonarQubeReport(
    List<Rule> rules,  // present in JSON but intentionally unused by sensor
    List<Issue> issues
) {
    public record Rule(String id, String name) {}

    public record Issue(
        String ruleId,
        int effortMinutes,
        PrimaryLocation primaryLocation
    ) {}

    public record PrimaryLocation(
        String message,
        String filePath,
        TextRange textRange
    ) {}

    public record TextRange(
        int startLine,
        int endLine,
        @Nullable Integer startColumn,  // reserved for future column-level highlighting
        @Nullable Integer endColumn     // reserved for future column-level highlighting
    ) {}
}
```

---

## Part 3: CI Pipeline Changes

**File**: `.github/workflows/ci.yml`

### 3a. Add aarch64 build job

New job `build-aarch64` (parallel with existing `build`).

`libpg_query` cross-compilation via standard cargo + cross-linker is likely to fail because the `pg_query` crate uses `cc` to build libpg_query from C source. Use the [`cross`](https://github.com/cross-rs/cross) tool as the primary approach:

- Install `cross` (`cargo install cross`)
- `cross build --release --target aarch64-unknown-linux-musl`
- Upload artifact `pg-migration-lint-aarch64`

Fallback if `cross` proves insufficient: QEMU-based Docker build with a multi-arch Dockerfile.

### 3b. Add SonarQube plugin build job

New job `build-sonarqube-plugin` (depends on `build` + `build-aarch64`).

**Assumption**: This job runs on an x86_64 Linux runner (ubuntu-latest). The `--dump-rules-json` step executes the x86_64 binary directly. Add an explicit check that the binary is executable after artifact download.

1. Download x86_64 and aarch64 binary artifacts
2. `chmod +x` on x86_64 binary, verify it runs: `./pg-migration-lint --version || { echo "Binary not executable"; exit 1; }`
3. Copy binaries to `sonarqube-plugin/src/main/resources/binaries/{x86_64,aarch64}/`
4. Generate `rules.json`: `./pg-migration-lint --dump-rules-json > sonarqube-plugin/src/main/resources/com/pgmigrationlint/sonar/rules.json`
5. Set up Java 17
6. `mvn -f sonarqube-plugin/pom.xml package`
7. Upload plugin JAR artifact

---

## Part 4: Build Helpers

### `sonarqube-plugin/Makefile`

Following bridge pattern:

- `package` — `mvn package -q`
- `clean` — `mvn clean`

### Release workflow

When a release is tagged, the plugin JAR should be attached as a release artifact alongside the Rust binaries. This is a follow-up task (not in initial scope).

---

## Part 5: File Indexing and Migration Scoping

### The Problem

The sensor must not use SonarQube's pgmigration language index as the gate for running analysis. SonarQube indexes files by suffix and source directory configuration; pg-migration-lint identifies migrations by path patterns in `pg-migration-lint.toml`. These two sets do not necessarily align.

Failure modes if language index is used as gate:
- Binary reports issues on files SonarQube did not index. `fs.inputFile()` returns null, issues are silently dropped.
- No pgmigration files are indexed (misconfigured suffixes, wrong source directory) so the sensor exits early and the binary never runs, producing no output and no error.

### Resolution

- Gate on config file presence (`pg-migration-lint.toml` exists at the configured path), not on language file index.
- Run the binary unconditionally when the config is present. Map issues back to `InputFile` after the fact.
- When `fs.inputFile()` returns null, log a warning per file (including the rule ID) rather than dropping silently. Do not fail the analysis.
- Migration directories must be listed under `sonar.sources` in `sonar-project.properties`. This is a user configuration requirement, not a plugin concern, but should be documented in the plugin's README.
- The toml already specifies migration paths. No additional `sonar.pgmigrationlint.migrations` property is needed.

---

## Part 6: PR Analysis and Changed-File Scoping

### Reporting Scope

For PR decoration (showing issues only on changed lines), SonarQube handles this natively via `sonar.pullrequest.*` configuration. The binary can run on all migrations; SonarQube scopes the results to changed lines automatically. No plugin changes required for this case.

### Performance Scope (linting only changed files)

pg-migration-lint is stateful. Correctly linting a migration in a PR requires:

1. **Catalog** — schema state derived by replaying all migrations on the base branch (pre-PR).
2. **Delta** — the migrations introduced in the PR, linted against that catalog.

This model is already implemented in the binary. A `CREATE INDEX WITHOUT CONCURRENTLY` on a table created in the same PR is correctly identified as safe because the catalog shows the table does not exist in production yet.

### CI Integration

Git work stays in CI, not in the binary. The CI step computes the delta:

```sh
git diff --name-only --diff-filter=A origin/main
```

`--diff-filter=A` restricts to added files. Migrations are append-only; modified existing migrations are a separate concern.

The file list is passed to the binary via the existing `--changed-files` flag (comma-separated). The binary derives the catalog from all migration files in the configured paths that are not in the changed-files list, replays those to build schema state, then lints only the changed files.

The sensor reads the `sonar.pgmigrationlint.changedFiles` property and forwards it to the binary via `--changed-files`. The CI step populates this property, either in `sonar-project.properties` or as a `-D` flag to `sonar-scanner`.

### PR Reporting vs. Full Analysis

In non-PR (full branch) analysis, `sonar.pgmigrationlint.changedFiles` is not set. The binary lints all migrations in discovery order. This is existing behavior and requires no changes.

---

## Verification

1. **Rust changes**: `cargo test` — all existing tests pass. New test for `--dump-rules-json` output (snapshot test verifying all 39 rules are present with correct structure, including PGM901).
2. **Plugin unit tests**: `mvn -f sonarqube-plugin/pom.xml test` — verify rules registration (38 rules, excluding PGM901), quality profile activation, binary extraction logic, sensor issue reporting with mock data.
3. **Manual integration test**: Install plugin JAR in a local SonarQube 10.x instance, run `sonar-scanner` against a project with migration files, verify rules appear in Quality Profiles and issues appear on dashboard.

---

## Implementation Order

1. Rust: make `sonarqube_meta()` and `effort_minutes()` `pub(crate)`
2. Rust: document explain string format contract on `Rule::explain()` trait method
3. Rust: add `--dump-rules-json` flag + handler
4. Rust: add `--stdout` flag + handler (all formats)
5. Rust: tests for new flags
6. Java: scaffold `sonarqube-plugin/` with `pom.xml`
7. Java: `RuleMetadata`, `RuleMetadataLoader`, `RuleDescriptionFormatter`
8. Java: `PgMigrationLanguage`
9. Java: `PgMigrationLintRulesDefinition`
10. Java: `PgMigrationLintQualityProfile`
11. Java: `BinaryExtractor` (singleton)
12. Java: `SonarQubeReport` DTOs (with column fields reserved)
13. Java: `PgMigrationLintSensor` (config-file gate, changed-files forwarding, error handling)
14. Java: `PgMigrationLintPlugin` (wires everything, registers all properties)
15. Java: unit tests
16. CI: aarch64 build job (via `cross`)
17. CI: plugin build job (with binary executability check)
