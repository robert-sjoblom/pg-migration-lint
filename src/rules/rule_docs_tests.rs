//! Contract for the per-rule markdown in `src/rules/docs/`.
//!
//! Each file is at once a rustdoc module doc, the `--explain` body printed
//! verbatim, and the body of the rule's `docs/rules.md` section, so it has
//! to satisfy rustdoc, a terminal, and Jekyll. Rustdoc is the strict one:
//! an untagged fence or a four-space-indented block is a doctest it will
//! try to compile.

use strum::IntoEnumIterator;

use crate::rules::{DOCS_BASE_URL, Rule, RuleId};

fn violations(text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if text.is_empty() {
        problems.push("empty".to_string());
        return problems;
    }
    if !text.ends_with('\n') {
        problems.push("missing trailing newline".to_string());
    }
    if text.ends_with("\n\n") {
        problems.push("more than one trailing newline".to_string());
    }

    let mut in_fence = false;
    for (idx, line) in text.lines().enumerate() {
        let n = idx + 1;
        if let Some(info) = line.strip_prefix("```") {
            let tagged = !info.is_empty() && info.chars().all(|c| c.is_ascii_lowercase());
            if !in_fence && !tagged {
                problems.push(format!("line {n}: fence opener without a language tag"));
            }
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if line.starts_with('#') {
            problems.push(format!("line {n}: heading outside a fence"));
        }
        if line.starts_with("    ") || line.starts_with('\t') {
            problems.push(format!(
                "line {n}: indented block outside a fence (rustdoc runs it as a doctest)"
            ));
        }
        for marker in ["{:", "{{", "{%"] {
            if line.contains(marker) {
                problems.push(format!("line {n}: Jekyll/Liquid syntax `{marker}`"));
            }
        }
        for target in link_targets(line) {
            if target.starts_with('#') {
                problems.push(format!("line {n}: relative anchor link `{target}`"));
            } else if target.contains("#pgm") && !target.starts_with(DOCS_BASE_URL) {
                problems.push(format!(
                    "line {n}: rule link `{target}` does not start with DOCS_BASE_URL"
                ));
            }
        }
    }
    if in_fence {
        problems.push("unterminated fence".to_string());
    }
    problems
}

fn link_targets(line: &str) -> Vec<&str> {
    line.match_indices("](")
        .filter_map(|(pos, _)| {
            let rest = &line[pos + 2..];
            rest.find(')').map(|end| &rest[..end])
        })
        .collect()
}

#[test]
fn every_rule_doc_satisfies_the_markdown_contract() {
    let mut failures = Vec::new();
    for id in RuleId::iter() {
        for problem in violations(id.explain()) {
            failures.push(format!("{id}: {problem}"));
        }
    }
    assert!(
        failures.is_empty(),
        "rule docs violate the contract:\n{}",
        failures.join("\n")
    );
}

#[test]
fn checker_accepts_the_dialect_the_bodies_use() {
    let ok = format!(
        "Intro paragraph.\n\n**Example**:\n```sql\nSELECT 1;\n```\n\n- item\n1. step\n\nSee [PGM004]({DOCS_BASE_URL}#pgm004).\n"
    );
    assert_eq!(violations(&ok), Vec::<String>::new());
}

#[test]
fn checker_flags_empty_text() {
    assert_eq!(violations(""), vec!["empty".to_string()]);
}

#[test]
fn checker_flags_trailing_newline_mistakes() {
    assert!(
        violations("no newline")
            .iter()
            .any(|p| p.contains("missing trailing newline"))
    );
    assert!(
        violations("two\n\n")
            .iter()
            .any(|p| p.contains("more than one trailing newline"))
    );
}

#[test]
fn checker_flags_untagged_and_unterminated_fences() {
    assert!(
        violations("```\nx\n```\n")
            .iter()
            .any(|p| p.contains("language tag"))
    );
    assert!(
        violations("```sql\nSELECT 1;\n")
            .iter()
            .any(|p| p.contains("unterminated fence"))
    );
}

#[test]
fn checker_ignores_content_inside_fences() {
    assert_eq!(
        violations("```text\n# not a heading\n    indented\n{{ liquid }}\n```\n"),
        Vec::<String>::new()
    );
}

#[test]
fn checker_flags_headings_indented_blocks_and_jekyll_syntax() {
    assert!(
        violations("# Title\n")
            .iter()
            .any(|p| p.contains("heading"))
    );
    assert!(
        violations("    code\n")
            .iter()
            .any(|p| p.contains("indented block"))
    );
    assert!(
        violations("\tcode\n")
            .iter()
            .any(|p| p.contains("indented block"))
    );
    assert!(
        violations("{: #anchor}\n")
            .iter()
            .any(|p| p.contains("Jekyll"))
    );
}

#[test]
fn checker_flags_relative_and_foreign_rule_links() {
    assert!(
        violations("see [PGM004](#pgm004)\n")
            .iter()
            .any(|p| p.contains("relative anchor"))
    );
    assert!(
        violations("see [PGM004](https://example.com/rules#pgm004)\n")
            .iter()
            .any(|p| p.contains("DOCS_BASE_URL"))
    );
}
