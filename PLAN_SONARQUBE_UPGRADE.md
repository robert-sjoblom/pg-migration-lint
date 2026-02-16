# Plan: Upgrade SonarQube output to 10.3+ Generic Issue Import format

## Context

Our SonarQube reporter emits the **deprecated** pre-10.3 format where each issue carries `engineId`, `ruleId`, `severity`, and `type`. Since SonarQube 10.3, the format has changed: rule metadata moves to a top-level `rules` array, and issues become slim (`ruleId` + `primaryLocation` only). The deprecated format still works but gets suboptimal defaults (`cleanCodeAttribute=CONVENTIONAL`, `softwareQuality=MAINTAINABILITY`, `severity=MEDIUM`).

Upgrading gives us proper control over clean-code attributes, software quality impacts, and per-rule metadata in the SonarQube UI.

## Approach

**Don't change the `Reporter` trait.** Instead, inject rule metadata into `SonarQubeReporter` at construction time. This keeps the change localized and doesn't affect SARIF or text reporters.

### Step 1: Add `RuleInfo` struct to `src/output/mod.rs`

A lightweight, SonarQube-agnostic struct extracted from the registry:

```rust
pub struct RuleInfo {
    pub id: String,
    pub name: String,            // from Rule::description()
    pub description: String,     // from Rule::explain()
    pub default_severity: Severity,
}
```

Add a helper to extract from the registry:
```rust
impl RuleInfo {
    pub fn from_registry(registry: &RuleRegistry) -> Vec<Self> { ... }
}
```

### Step 2: Change `SonarQubeReporter` to hold rule metadata

```rust
pub struct SonarQubeReporter {
    rules: Vec<RuleInfo>,
}

impl SonarQubeReporter {
    pub fn new(rules: Vec<RuleInfo>) -> Self { Self { rules } }
}
```

Remove the `Default` impl (it no longer makes sense without rules).

### Step 3: Update `src/main.rs` call site

At line ~265, change:
```rust
"sonarqube" => Box::new(SonarQubeReporter::new()),
```
to:
```rust
"sonarqube" => Box::new(SonarQubeReporter::new(RuleInfo::from_registry(&registry))),
```

### Step 4: Add SonarQube-specific mapping in `src/output/sonarqube.rs`

A static mapping from rule ID to `cleanCodeAttribute`, `type`, and `impacts`:

| Rule | cleanCodeAttribute | type | softwareQuality | impactSeverity |
|------|-------------------|------|-----------------|----------------|
| PGM001 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM002 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM003 | EFFICIENT | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM004 | COMPLETE | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM005 | CONVENTIONAL | CODE_SMELL | MAINTAINABILITY | LOW |
| PGM006 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM007 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM009 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM010 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM011 | COMPLETE | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM012 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM013 | COMPLETE | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM014 | COMPLETE | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM015 | COMPLETE | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM016 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM017 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM018 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM019 | COMPLETE | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM020 | COMPLETE | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM021 | COMPLETE | BUG | RELIABILITY | HIGH |
| PGM022 | COMPLETE | CODE_SMELL | MAINTAINABILITY | MEDIUM |
| PGM101–105, 108 | CONVENTIONAL | CODE_SMELL | MAINTAINABILITY | LOW |

Rationale:
- **BUG / RELIABILITY / HIGH** for rules that cause lock contention, table rewrites, or data issues on production (PGM001, 002, 006–010, 012, 016–018, 021)
- **CODE_SMELL / MAINTAINABILITY** for schema quality, side-effect warnings, and type conventions
- **COMPLETE** for safety-critical rules (migration is incomplete without the fix)
- **EFFICIENT** for PGM003 (missing index = performance)
- **CONVENTIONAL** for type-choice rules (PGM101–108)

### Step 5: Update the JSON structures in `src/output/sonarqube.rs`

**New top-level:**
```rust
struct SonarQubeReport {
    rules: Vec<SonarQubeRule>,
    issues: Vec<SonarQubeIssue>,
}
```

**New `rules` entry:**
```rust
struct SonarQubeRule {
    id: String,
    name: String,
    description: String,
    engine_id: &'static str,
    clean_code_attribute: &'static str,
    #[serde(rename = "type")]
    issue_type: &'static str,
    severity: String,
    impacts: Vec<SonarQubeImpact>,
}
```

**Slimmer `issues` entry:**
```rust
struct SonarQubeIssue {
    rule_id: String,
    effort_minutes: u32,
    primary_location: SonarQubePrimaryLocation,
}
```

Note: `engineId`, `severity`, and `type` move from issues to rules.

### Step 6: Update emit() logic

In `emit()`:
1. Collect the set of rule IDs that appear in findings
2. Build the `rules` array from stored `RuleInfo` + SonarQube-specific mapping, filtered to only rules that fired
3. Build the slim `issues` array
4. Serialize and write

### Step 7: Update tests and snapshots

All existing SonarQube snapshot tests need updating because:
- The JSON structure changes (new `rules` array, slimmer issues)
- Tests that construct `SonarQubeReporter` need to provide `RuleInfo`

Add a test helper to build `RuleInfo` for common test rules (PGM001, PGM003, etc.).

## PGM901 (down migration severity capping) trade-off

The new format has **no per-issue severity** — severity lives on the rule definition. This means PGM901's behavior (cap all findings to INFO in down migrations) won't be reflected in SonarQube severity. The finding will still appear with a message mentioning it's a down migration, but SonarQube will show the rule's default severity.

This is an inherent limitation of the new format, not something we can work around.

## Files to modify

1. **`src/output/mod.rs`** — Add `RuleInfo` struct, update `SonarQubeReporter` definition
2. **`src/output/sonarqube.rs`** — New JSON structures, SonarQube-specific mapping, updated emit()
3. **`src/main.rs`** — Pass rule info to SonarQubeReporter constructor
4. **`tests/snapshots/`** — Update all sonarqube snapshot files

## Verification

1. `cargo test` — all tests pass (snapshot tests updated with `INSTA_UPDATE=always`)
2. `cargo clippy` — no warnings
3. Manual: `cargo run -- --format sonarqube ...` and inspect `findings.json` structure
4. Verify the JSON has both `rules` and `issues` arrays at top level
