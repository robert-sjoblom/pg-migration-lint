//! Documentation generator for `docs/rules.md`.
//!
//! Feature-gated behind `--features docgen`. Reads rule metadata from
//! [`RuleId`] and per-rule content from `docs/examples/`, renders
//! them through a minijinja template, and exposes an insta snapshot test
//! that fails when the generated output drifts.

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

/// Build the template context from all rule IDs and example files on disk.
///
/// `examples_dir` should point to `docs/examples/` relative to the project root.
pub fn build_context(examples_dir: &Path) -> Result<DocsContext, DocgenError> {
    let mut all_rules = Vec::new();

    for id in RuleId::iter() {
        let id_str = id.to_string();
        let anchor = id_str.to_lowercase();

        // Read body file
        let body_path = examples_dir.join(format!("{}_body.md", anchor));
        let body = std::fs::read_to_string(&body_path).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "missing body file for {id_str} — create {path}\n\
                     (original error: {e})",
                    path = body_path.display(),
                ),
            )
        })?;

        all_rules.push(RuleEntry {
            id: id_str,
            anchor,
            description: id.description().to_string(),
            severity: id.default_severity().title_case().to_string(),
            body: body.trim_end().to_string(),
        });
    }

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

    Ok(DocsContext {
        rule_count,
        families,
        all_rules,
    })
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
        let examples_dir = project_root().join("docs/examples");
        let template_path = project_root().join("docs/rules.md.j2");

        let ctx = build_context(&examples_dir).expect("build_context should succeed");
        let rendered = render(&ctx, &template_path).expect("render should succeed");

        insta::assert_snapshot!("rules_md", rendered);
    }
}
