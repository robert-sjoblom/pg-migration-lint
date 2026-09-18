//! Documentation generator for `docs/rules.md`.
//!
//! Feature-gated behind `--features docgen`. Reads rule metadata from
//! [`RuleId`] and each rule's body from [`Rule::explain`] (the markdown in
//! `src/rules/docs/`), renders them through a minijinja template, and
//! exposes an insta snapshot test that fails when the generated output
//! drifts. The same module also checks `SPEC.md` against the binary.

use std::path::Path;

use minijinja::Environment;
use serde::Serialize;
use strum::IntoEnumIterator;

use crate::rules::{Rule, RuleId};

/// Error type for documentation generation.
#[derive(Debug, thiserror::Error)]
pub enum DocgenError {
    /// Template rendering failed.
    #[error("template error: {0}")]
    Template(#[from] minijinja::Error),
    /// I/O error reading example files or templates.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Top-level context passed to the template.
#[derive(Debug, Serialize)]
pub struct DocsContext {
    /// Total number of rules (for the intro line).
    pub rule_count: usize,
    /// Rule families in display order.
    pub families: Vec<FamilyContext>,
    /// Flat list of all rules (for the quick-reference table).
    pub all_rules: Vec<RuleEntry>,
}

/// A family of related rules (e.g. "0xx — Unsafe DDL Rules").
#[derive(Debug, Serialize)]
pub struct FamilyContext {
    /// Full heading text (e.g. "0xx — Unsafe DDL Rules").
    pub heading: String,
    /// Optional intro paragraph below the family heading.
    pub intro: Option<String>,
    /// Rules in this family.
    pub rules: Vec<RuleEntry>,
}

/// A single rule entry.
#[derive(Debug, Serialize)]
pub struct RuleEntry {
    /// Rule ID (e.g. "PGM001").
    pub id: String,
    /// Lowercase anchor for Jekyll (e.g. "pgm001").
    pub anchor: String,
    /// Short description from the rule.
    pub description: String,
    /// Title-case severity (e.g. "Critical").
    pub severity: String,
    /// Full body content (markdown with examples inline).
    pub body: String,
}

/// Family metadata: heading text and optional intro paragraph.
struct FamilyMeta {
    prefix: &'static str,
    heading: &'static str,
    intro: Option<&'static str>,
}

/// Ordered list of families with their display metadata.
const FAMILIES: &[FamilyMeta] = &[
    FamilyMeta {
        prefix: "0xx",
        heading: "0xx — Unsafe DDL Rules",
        intro: None,
    },
    FamilyMeta {
        prefix: "1xx",
        heading: "1xx — Type Anti-pattern Rules",
        intro: Some(
            "These rules flag column types that should be avoided per the PostgreSQL wiki's [\"Don't Do This\"](https://wiki.postgresql.org/wiki/Don't_Do_This) recommendations.",
        ),
    },
    FamilyMeta {
        prefix: "2xx",
        heading: "2xx — Destructive Operation Rules",
        intro: None,
    },
    FamilyMeta {
        prefix: "3xx",
        heading: "3xx — DML in Migration Rules",
        intro: None,
    },
    FamilyMeta {
        prefix: "4xx",
        heading: "4xx — Idempotency Guard Rules",
        intro: None,
    },
    FamilyMeta {
        prefix: "5xx",
        heading: "5xx — Schema Design Rules",
        intro: None,
    },
    FamilyMeta {
        prefix: "9xx",
        heading: "9xx — Meta-behavior Rules",
        intro: None,
    },
];

/// Build the template context from all rule IDs and their markdown bodies.
pub fn build_context() -> DocsContext {
    let all_rules: Vec<RuleEntry> = RuleId::iter()
        .map(|id| {
            let id_str = id.to_string();
            RuleEntry {
                anchor: id_str.to_lowercase(),
                id: id_str,
                description: id.description().to_string(),
                severity: id.default_severity().title_case().to_string(),
                body: id.explain().trim_end().to_string(),
            }
        })
        .collect();

    // Group into families
    let mut families = Vec::new();
    for meta in FAMILIES {
        let rules: Vec<RuleEntry> = all_rules
            .iter()
            .filter(|r| {
                let prefix = &r.id[3..4]; // digit after "PGM"
                let family_digit = &meta.prefix[0..1];
                prefix == family_digit
            })
            .map(|r| RuleEntry {
                id: r.id.clone(),
                anchor: r.anchor.clone(),
                description: r.description.clone(),
                severity: r.severity.clone(),
                body: r.body.clone(),
            })
            .collect();

        if !rules.is_empty() {
            families.push(FamilyContext {
                heading: meta.heading.to_string(),
                intro: meta.intro.map(|s| s.to_string()),
                rules,
            });
        }
    }

    // PGM901 is a meta-behavior, not a standalone rule — exclude from count
    let rule_count = all_rules.iter().filter(|r| r.id != "PGM901").count();

    DocsContext {
        rule_count,
        families,
        all_rules,
    }
}

/// Render the docs context through the template.
pub fn render(context: &DocsContext, template_path: &Path) -> Result<String, DocgenError> {
    let template_source = std::fs::read_to_string(template_path)?;

    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.add_template("rules.md.j2", &template_source)?;

    let tmpl = env.get_template("rules.md.j2")?;
    let rendered = tmpl.render(context)?;

    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
    }

    #[test]
    fn docs_rules_md() {
        let template_path = project_root().join("docs/rules.md.j2");

        let ctx = build_context();
        let rendered = render(&ctx, &template_path).expect("render should succeed");

        insta::assert_snapshot!("rules_md", rendered);
    }

    /// Rule sections of `SPEC.md`, keyed by rule ID: heading title and the
    /// section body up to the next `###`/`####` heading.
    fn spec_rule_sections(spec: &str) -> std::collections::HashMap<&str, (&str, String)> {
        let mut sections = std::collections::HashMap::new();
        let mut current: Option<(&str, &str)> = None;
        let mut body = String::new();

        for line in spec.lines() {
            if line.starts_with("### ") || line.starts_with("#### ") {
                if let Some((id, title)) = current.take() {
                    let previous = sections.insert(id, (title, std::mem::take(&mut body)));
                    assert!(previous.is_none(), "SPEC.md has two `#### {id}` headings");
                }
                if let Some(rest) = line.strip_prefix("#### PGM")
                    && let (Some(digits), Some(tail)) = (rest.get(..3), rest.get(3..))
                    && digits.chars().all(|c| c.is_ascii_digit())
                    && let Some(title) = tail.strip_prefix(" — ")
                {
                    current = Some((&line[5..11], title));
                }
                continue;
            }
            if current.is_some() {
                body.push_str(line);
                body.push('\n');
            }
        }
        if let Some((id, title)) = current {
            let previous = sections.insert(id, (title, body));
            assert!(previous.is_none(), "SPEC.md has two `#### {id}` headings");
        }
        sections
    }

    #[test]
    fn spec_rule_sections_match_the_binary() {
        let spec =
            std::fs::read_to_string(project_root().join("SPEC.md")).expect("SPEC.md readable");
        let sections = spec_rule_sections(&spec);
        let mut failures = Vec::new();

        if sections.len() != RuleId::iter().count() {
            failures.push(format!(
                "SPEC.md has {} rule headings, RuleId has {} variants",
                sections.len(),
                RuleId::iter().count()
            ));
        }

        for id in RuleId::iter() {
            let Some((title, body)) = sections.get(id.as_str()) else {
                failures.push(format!("{id}: no `#### {id} — ...` heading in SPEC.md"));
                continue;
            };

            if body
                .lines()
                .any(|l| l.trim_start().starts_with("- **Why**"))
            {
                failures.push(format!(
                    "{id}: SPEC section has a **Why** bullet; rationale lives only in src/rules/docs/{}.md",
                    id.as_str().to_lowercase()
                ));
            }

            let plain_title = title.replace('`', "");
            if plain_title != id.description() {
                failures.push(format!(
                    "{id}: SPEC heading {plain_title:?} != DESCRIPTION {:?}",
                    id.description()
                ));
            }

            if id.is_meta() {
                // A meta-behaviour has no severity of its own.
                continue;
            }

            match body
                .lines()
                .find_map(|l| l.strip_prefix("- **Severity**: "))
            {
                None => failures.push(format!("{id}: no `- **Severity**:` bullet")),
                Some(rest) => {
                    let first = rest
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_end_matches(['.', ',', ';']);
                    let expected = id.default_severity().to_string();
                    if first != expected {
                        failures.push(format!(
                            "{id}: SPEC severity {first:?} != DEFAULT_SEVERITY {expected:?}"
                        ));
                    }
                }
            }
        }

        assert!(
            failures.is_empty(),
            "SPEC.md drifted from the binary:\n{}",
            failures.join("\n")
        );
    }
}
