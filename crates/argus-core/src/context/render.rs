//! Deterministic renderers for a [`ContextPacket`].
//!
//! These are template rendering only — no summarization, no model. The same
//! packet always renders identically (the JSON includes `compiled_at` /
//! `packet_id`, but the Markdown view intentionally omits them so it is a
//! pure function of the structural content).

use serde::{Deserialize, Serialize};

use super::types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextFormat {
    Json,
    Markdown,
}

impl ContextFormat {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "json" => Self::Json,
            "markdown" | "md" => Self::Markdown,
            _ => return None,
        })
    }
}

pub fn render_json(packet: &ContextPacket) -> String {
    serde_json::to_string_pretty(packet).unwrap_or_else(|_| "{}".to_string())
}

pub fn render_markdown(packet: &ContextPacket) -> String {
    let mut out = String::new();
    out.push_str("# ARGUS Context Packet\n\n");
    out.push_str(&format!("- Project: {}\n", packet.project_identity.name));
    out.push_str(&format!("- Role: {}\n", packet.role));
    out.push_str(&format!("- Purpose: {}\n", packet.purpose));
    out.push_str(&format!("- Compiler: {}\n", packet.compiler_version));
    out.push_str(&format!("- Context Hash: {}\n", packet.context_hash));
    out.push_str(&format!("- Items: {}\n", packet.estimated_size.item_count));
    out.push_str(&format!(
        "- Est. size: {} bytes (~{} tokens)\n",
        packet.estimated_size.estimated_bytes, packet.estimated_size.estimated_tokens
    ));
    out.push_str(&format!("- Truncated: {}\n", packet.truncation.truncated));
    if !packet.truncation.omitted_sections.is_empty() {
        let omitted: Vec<&str> = packet
            .truncation
            .omitted_sections
            .iter()
            .map(|s| s.as_str())
            .collect();
        out.push_str(&format!("- Omitted sections: {}\n", omitted.join(", ")));
    }

    // Project
    out.push_str("\n## Project\n");
    out.push_str(&format!(
        "{} (`{}`) — {}\n",
        packet.project_identity.name, packet.project_identity.slug, packet.project_identity.status
    ));
    if !packet.project_identity.description.is_empty() {
        out.push_str(&format!("{}\n", packet.project_identity.description));
    }

    // Charter
    out.push_str("\n## Charter\n");
    match &packet.charter {
        None => out.push_str("None\n"),
        Some(c) => {
            out.push_str(&format!("Vision: {}\n", c.vision));
            out.push_str(&format!("Goal: {}\n", c.goal));
            push_list(&mut out, "Success criteria", &c.success_criteria);
            push_list(&mut out, "Constraints", &c.constraints);
            push_list(&mut out, "Non-goals", &c.non_goals);
            out.push_str(&format!("Revision: {}\n", c.revision));
        }
    }

    // Current
    out.push_str("\n## Current\n");
    match &packet.current_milestone {
        Some(m) => out.push_str(&format!(
            "Milestone: {} ({}, {:.0}%)\n",
            m.title,
            m.status,
            m.completion_ratio * 100.0
        )),
        None => out.push_str("Milestone: None\n"),
    }
    if let Some(p) = &packet.current_phase {
        out.push_str(&format!("Phase: {} ({})\n", p.title, p.status));
    }
    match &packet.current_work_item {
        Some(w) => {
            out.push_str(&format!("Work Item: {} ({})\n", w.title, w.status));
            if let Some(dod) = &w.definition_of_done {
                out.push_str(&format!("Definition of Done: {dod}\n"));
            }
        }
        None => out.push_str("Work Item: None\n"),
    }

    // Roadmap
    out.push_str("\n## Roadmap\n");
    match &packet.roadmap_context {
        None => out.push_str("None\n"),
        Some(r) => {
            if r.milestones.is_empty() {
                out.push_str("None\n");
            } else {
                for m in &r.milestones {
                    out.push_str(&format!(
                        "- {} ({}, {:.0}%)\n",
                        m.title,
                        m.status,
                        m.completion_ratio * 100.0
                    ));
                }
            }
        }
    }

    // Active work: current focus plus any active/blocked WorkItems.
    out.push_str("\n## Active Work\n");
    let mut work: Vec<&WorkItemContext> = Vec::new();
    if let Some(w) = &packet.current_work_item {
        work.push(w);
    }
    for w in packet
        .active_work_items
        .iter()
        .chain(packet.blocked_work_items.iter())
    {
        if !work.iter().any(|x| x.work_item_id == w.work_item_id) {
            work.push(w);
        }
    }
    if work.is_empty() {
        out.push_str("None\n");
    } else {
        for w in work {
            out.push_str(&format!("- {} ({})\n", w.title, w.status));
        }
    }

    // Decisions
    out.push_str("\n## Relevant Decisions\n");
    if packet.relevant_decisions.is_empty() {
        out.push_str("None\n");
    } else {
        for d in &packet.relevant_decisions {
            out.push_str(&format!("- [{}] {}: {}\n", d.status, d.title, d.decision));
        }
    }

    // Risks
    out.push_str("\n## Risks\n");
    if packet.relevant_risks.is_empty() {
        out.push_str("None\n");
    } else {
        for r in &packet.relevant_risks {
            out.push_str(&format!("- [{}] {}\n", r.severity, r.title));
        }
    }

    // Blockers
    out.push_str("\n## Blockers\n");
    if packet.relevant_blockers.is_empty() {
        out.push_str("None\n");
    } else {
        for b in &packet.relevant_blockers {
            out.push_str(&format!("- [{}] {}\n", b.status, b.title));
        }
    }

    // Artifacts
    out.push_str("\n## Recent Artifacts\n");
    if packet.relevant_artifacts.is_empty() {
        out.push_str("None\n");
    } else {
        for a in &packet.relevant_artifacts {
            let short = &a.commit_sha[..8.min(a.commit_sha.len())];
            out.push_str(&format!(
                "- {} ({} file(s), quality={})\n",
                short,
                a.files_changed.len(),
                a.quality_status.as_deref().unwrap_or("unknown")
            ));
        }
    }

    // Dependencies
    out.push_str("\n## Dependencies\n");
    if packet.dependencies.is_empty() {
        out.push_str("None\n");
    } else {
        for d in &packet.dependencies {
            out.push_str(&format!(
                "- {} depends on {} (satisfied: {})\n",
                d.work_item_id,
                d.depends_on_title
                    .clone()
                    .unwrap_or_else(|| d.depends_on_id.to_string()),
                d.satisfied
            ));
        }
    }

    // Deferred
    out.push_str("\n## Deferred Improvements\n");
    if packet.deferred_improvements.is_empty() {
        out.push_str("None\n");
    } else {
        for i in &packet.deferred_improvements {
            match &i.external_key {
                Some(k) => out.push_str(&format!("- {} {} ({})\n", k, i.title, i.status)),
                None => out.push_str(&format!("- {} ({})\n", i.title, i.status)),
            }
        }
    }

    // Constraints
    out.push_str("\n## Constraints\n");
    if packet.constraints.is_empty() {
        out.push_str("None\n");
    } else {
        for c in &packet.constraints {
            out.push_str(&format!("- {}\n", c.text));
        }
    }

    // Recent activity
    out.push_str("\n## Recent Activity\n");
    if packet.recent_activity.is_empty() {
        out.push_str("None\n");
    } else {
        for a in &packet.recent_activity {
            out.push_str(&format!(
                "- [{}] {} ({})\n",
                a.activity_type,
                a.summary,
                a.occurred_at.to_rfc3339()
            ));
        }
    }

    // Provenance
    out.push_str("\n## Provenance\n");
    if packet.source_refs.is_empty() {
        out.push_str("None\n");
    } else {
        for r in &packet.source_refs {
            match r.revision {
                Some(rev) => out.push_str(&format!(
                    "- {} {} (rev {})\n",
                    r.source_type.as_str(),
                    r.source_id,
                    rev
                )),
                None => out.push_str(&format!("- {} {}\n", r.source_type.as_str(), r.source_id)),
            }
        }
    }

    out
}

fn push_list(out: &mut String, label: &str, items: &[String]) {
    if items.is_empty() {
        out.push_str(&format!("{label}: None\n"));
    } else {
        out.push_str(&format!("{label}:\n"));
        for item in items {
            out.push_str(&format!("- {item}\n"));
        }
    }
}
