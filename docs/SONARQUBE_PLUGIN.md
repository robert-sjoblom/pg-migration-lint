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

### 1b. Add `--stdout` flag for report output

**File**: `src/main.rs`

Add a `--stdout` flag. When set, write the report to stdout instead of a file. This lets the SonarQube sensor capture output without needing to coordinate file paths.

Implementation: In the emit loop, when `--stdout` is set, call `reporter.render()` and print the result instead of calling `reporter.emit()`.

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

Entry point. Registers all extensions:

- `PgMigrationLanguage.class`
- `PgMigrationLintRulesDefinition.class`
- `PgMigrationLintSensor.class`
- `PgMigrationLintQualityProfile.class`
- Property definitions for `sonar.pgmigrationlint.configFile` (default: `pg-migration-lint.toml`) and `sonar.pgmigrationlint.binaryPath` (optional override)

```java
public class PgMigrationLintPlugin implements Plugin {
    @Override
    public void define(Context context) {
        context.addExtensions(
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
                .build()
        );
    }
}
```

#### `PgMigrationLanguage`

Registers `"pgmigration"` language with `.sql` and `.xml` file suffixes (matching the tool's default `include = ["*.sql", "*.xml"]`). Configurable via `sonar.pgmigrationlint.file.suffixes` property so users can add/remove extensions.

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
                .setDebtRemediationFunction(
                    newRule.debtRemediationFunctions()
                        .constantPerIssue(rule.effortMinutes() + "min")
                )
                .setTags(rule.tags().toArray(String[]::new));
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
- Detect indented SQL blocks → `<pre><code>` blocks
- Append link to GitHub Pages docs

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

The analysis engine:

1. `describe()`: only runs on `"pgmigration"` language files, creates issues for `"pgmigrationlint"` repository
2. `execute()`:
   - Skip if no pgmigration-language files indexed (`.sql` or `.xml`)
   - Resolve binary (user override → bundled extraction)
   - Run: `pg-migration-lint --config <path> --format sonarqube --stdout --fail-on none`
   - Capture stdout, parse as `SonarQubeReport` JSON
   - For each issue: look up `InputFile` by relative path, create `NewIssue` with `RuleKey.of("pgmigrationlint", ruleId)`, set location + message + line range
   - Log summary

```java
public class PgMigrationLintSensor implements Sensor {
    private static final Logger LOG = LoggerFactory.getLogger(PgMigrationLintSensor.class);

    private final Configuration config;
    private final BinaryExtractor binaryExtractor;

    public PgMigrationLintSensor(Configuration config) {
        this.config = config;
        this.binaryExtractor = new BinaryExtractor();
    }

    @Override
    public void describe(SensorDescriptor descriptor) {
        descriptor.name("pg-migration-lint")
            .onlyOnLanguage(PgMigrationLanguage.KEY)
            .createIssuesForRuleRepository("pgmigrationlint");
    }

    @Override
    public void execute(SensorContext context) {
        FileSystem fs = context.fileSystem();
        if (!fs.hasFiles(fs.predicates().hasLanguage(PgMigrationLanguage.KEY))) {
            LOG.info("No pgmigration files found, skipping analysis");
            return;
        }

        String binary = resolveBinary();
        String configFile = config.get("sonar.pgmigrationlint.configFile")
            .orElse("pg-migration-lint.toml");

        ProcessBuilder pb = new ProcessBuilder(
            binary, "--config", configFile,
            "--format", "sonarqube", "--stdout", "--fail-on", "none"
        );
        pb.directory(fs.baseDir());
        pb.redirectErrorStream(false);

        // Execute, capture stdout, parse JSON, create issues...
        SonarQubeReport report = runAndParse(pb);

        int count = 0;
        for (SonarQubeReport.Issue issue : report.issues()) {
            InputFile inputFile = fs.inputFile(
                fs.predicates().hasRelativePath(issue.primaryLocation().filePath()));
            if (inputFile == null) {
                LOG.warn("File not found: {}", issue.primaryLocation().filePath());
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

        LOG.info("pg-migration-lint reported {} issues", count);
    }

    private String resolveBinary() {
        return config.get("sonar.pgmigrationlint.binaryPath")
            .orElseGet(binaryExtractor::extract);
    }
}
```

#### `BinaryExtractor`

Platform-aware binary extraction:

- Detect arch via `os.arch` (amd64/x86_64 → `x86_64`, aarch64/arm64 → `aarch64`)
- Reject non-Linux (error message pointing to `sonar.pgmigrationlint.binaryPath`)
- Extract from `/binaries/<arch>/pg-migration-lint` to `$TMPDIR/pg-migration-lint-sonar/pg-migration-lint`
- Set executable permission
- Cache extracted path in instance field (avoid re-extraction within same analysis)

```java
public class BinaryExtractor {
    private String cachedPath;

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

```java
public record SonarQubeReport(
    List<Rule> rules,
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

    public record TextRange(int startLine, int endLine) {}
}
```

---

## Part 3: CI Pipeline Changes

**File**: `.github/workflows/ci.yml`

### 3a. Add aarch64 build job

New job `build-aarch64` (parallel with existing `build`):
- Install `aarch64-unknown-linux-musl` target + cross-linker (`gcc-aarch64-linux-gnu`)
- `cargo build --release --target aarch64-unknown-linux-musl`
- Upload artifact `pg-migration-lint-aarch64`

Note: `pg_query` crate uses `cc` to build libpg_query. Cross-compilation needs `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc` and possibly `CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc`. If this proves difficult, fall back to `cross` tool or QEMU-based Docker build.

### 3b. Add SonarQube plugin build job

New job `build-sonarqube-plugin` (depends on `build` + `build-aarch64`):

1. Download x86_64 and aarch64 binary artifacts
2. Copy binaries to `sonarqube-plugin/src/main/resources/binaries/{x86_64,aarch64}/`
3. Generate `rules.json`: run x86_64 binary with `--dump-rules-json > sonarqube-plugin/src/main/resources/com/pgmigrationlint/sonar/rules.json`
4. Set up Java 17
5. `mvn -f sonarqube-plugin/pom.xml package`
6. Upload plugin JAR artifact

---

## Part 4: Build Helpers

### `sonarqube-plugin/Makefile`

Following bridge pattern:

- `package` — `mvn package -q`
- `clean` — `mvn clean`

### Release workflow

When a release is tagged, the plugin JAR should be attached as a release artifact alongside the Rust binaries. This is a follow-up task (not in initial scope).

---

## Verification

1. **Rust changes**: `cargo test` — all existing tests pass. New test for `--dump-rules-json` output (snapshot test verifying all 39 rules are present with correct structure).
2. **Plugin unit tests**: `mvn -f sonarqube-plugin/pom.xml test` — verify rules registration (38 rules, excluding PGM901), quality profile activation, binary extraction logic, sensor issue reporting with mock data.
3. **Manual integration test**: Install plugin JAR in a local SonarQube 10.x instance, run `sonar-scanner` against a project with migration files, verify rules appear in Quality Profiles and issues appear on dashboard.

---

## Implementation Order

1. Rust: make `sonarqube_meta()` and `effort_minutes()` `pub(crate)`
2. Rust: add `--dump-rules-json` flag + handler
3. Rust: add `--stdout` flag + handler
4. Rust: tests for new flags
5. Java: scaffold `sonarqube-plugin/` with `pom.xml`
6. Java: `RuleMetadata`, `RuleMetadataLoader`, `RuleDescriptionFormatter`
7. Java: `PgMigrationLanguage`
8. Java: `PgMigrationLintRulesDefinition`
9. Java: `PgMigrationLintQualityProfile`
10. Java: `BinaryExtractor`
11. Java: `SonarQubeReport` DTOs
12. Java: `PgMigrationLintSensor`
13. Java: `PgMigrationLintPlugin` (wires everything together)
14. Java: unit tests
15. CI: aarch64 build job
16. CI: plugin build job
