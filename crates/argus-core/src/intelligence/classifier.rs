//! V0.42 deterministic, explainable task classifier.
//!
//! Rule-based by design: reproducible, testable, explainable, zero-cost, and a
//! baseline a future learned router can be compared against. It never uses
//! prompt length as a difficulty signal and never calls a model.

use super::types::*;

pub struct TaskClassifier;

/// Booleans for the risk/change-type flags a task can carry.
#[derive(Default, Clone, Copy)]
struct Flags {
    security: bool,
    authentication: bool,
    secrets: bool,
    financial: bool,
    migration: bool,
    schema: bool,
    data_loss: bool,
    destructive: bool,
    concurrency: bool,
    external_side_effects: bool,
    production_infra: bool,
    public_api: bool,
}

impl Flags {
    fn names(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.security {
            out.push("security");
        }
        if self.authentication {
            out.push("authentication");
        }
        if self.secrets {
            out.push("secrets");
        }
        if self.financial {
            out.push("financial_correctness");
        }
        if self.migration {
            out.push("migration");
        }
        if self.schema {
            out.push("schema");
        }
        if self.data_loss {
            out.push("data_loss");
        }
        if self.destructive {
            out.push("irreversible");
        }
        if self.concurrency {
            out.push("concurrency");
        }
        if self.external_side_effects {
            out.push("external_side_effects");
        }
        if self.production_infra {
            out.push("production_infrastructure");
        }
        if self.public_api {
            out.push("public_api");
        }
        out.into_iter().map(str::to_string).collect()
    }
}

impl TaskClassifier {
    pub fn classify(input: &TaskIntelligenceInput) -> NewTaskProfile {
        let text = input.normalized_text();
        let flags = detect_flags(&text);
        let (kind, kind_reason) = infer_kind(input, &text);
        let (base_scope, base_reasoning, base_verification, base_context) = kind_baseline(kind);

        let cross_crate = contains_any(
            &text,
            &[
                "multiple crates",
                "cross-crate",
                "cross crate",
                "across crates",
                "system-wide",
                "system wide",
                "all modules",
                "whole repository",
                "multiple modules",
            ],
        );
        let root_cause = contains_any(
            &text,
            &[
                "root cause",
                "root-cause",
                "unknown cause",
                "race condition",
                "deadlock",
                "flaky",
                "intermittent",
                "distributed",
            ],
        );

        let mut scope = base_scope;
        if cross_crate {
            scope += 0.3;
        }
        if input.dependency_count > 2 {
            scope += 0.15;
        }
        if flags.public_api || flags.schema {
            scope += 0.15;
        }
        let scope = clamp01(scope);

        let mut reasoning = base_reasoning;
        if root_cause {
            reasoning += 0.15;
        }
        let reasoning = clamp01(reasoning);

        let mut verification = base_verification;
        if input.test_commands.iter().any(|c| {
            contains_any(
                &c.to_lowercase(),
                &["postgres", "docker", "e2e", "restart", "integration"],
            )
        }) {
            verification += 0.15;
        }
        if flags.security || flags.authentication || flags.financial || flags.data_loss {
            verification += 0.2;
        }
        let verification = clamp01(verification);

        let mut context = base_context;
        if input.context_truncated {
            context += 0.1;
        }
        if cross_crate {
            context += 0.15;
        }
        let context = clamp01(context);

        let ambiguity = ambiguity_score(input, &text);
        let reversibility = reversibility_score(kind, &flags, &text);
        let risk = risk_score(kind, &flags);

        let complexity = combine_max_mean(&[scope, reasoning, verification, context, ambiguity]);

        // Base tier from complexity, then reasoning/ambiguity/risk floors.
        let mut tier = match complexity {
            c if c < 0.25 => ModelTier::T0,
            c if c < 0.5 => ModelTier::T1,
            c if c < 0.72 => ModelTier::T2,
            _ => ModelTier::T3,
        };
        let mut reasons = vec![format!("task kind {kind} ({kind_reason})")];

        if reasoning >= 0.55 {
            reasons.push(format!("reasoning requirement {reasoning:.2}"));
        }
        if reasoning >= 0.85 {
            tier = tier.max(ModelTier::T2);
        }
        if ambiguity >= 0.75 {
            tier = tier.max(ModelTier::T2);
            reasons.push(format!("high ambiguity {ambiguity:.2}"));
        }

        let risk_floor = risk_floor(risk);
        if risk_floor.rank() > tier.rank() {
            reasons.push(format!(
                "risk floor {} applied (risk {risk:.2})",
                risk_floor
            ));
        }
        tier = tier.max(risk_floor);

        if scope >= 0.6 {
            reasons.push(format!("broad scope {scope:.2}"));
        }
        for name in flags.names() {
            reasons.push(format!("flag: {name}"));
        }
        if input.prior_attempts > 0 {
            reasons.push(format!(
                "{} prior attempt(s); prior tier {}",
                input.prior_attempts,
                input
                    .prior_tier
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ));
        }
        if input.context_truncated {
            reasons.push("context was truncated".to_string());
        }

        let verification_requirement =
            verification_requirement(kind, &flags, risk, reversibility, verification, complexity);
        reasons.push(format!(
            "verification requirement {verification_requirement}"
        ));

        let capabilities = capabilities(
            kind,
            &flags,
            scope,
            reasoning,
            context,
            risk,
            tier,
            verification_requirement,
            !input.test_commands.is_empty(),
        );

        let confidence = confidence(input, kind);

        NewTaskProfile {
            project_id: input.project_id,
            work_item_id: input.work_item_id,
            task_id: input.task_id,
            task_kind: kind,
            secondary_flags: flags.names(),
            complexity_score: complexity,
            risk_score: risk,
            ambiguity_score: ambiguity,
            reasoning_score: reasoning,
            scope_score: scope,
            verification_score: verification,
            context_score: context,
            reversibility_score: reversibility,
            recommended_tier: tier,
            verification_requirement,
            required_capabilities: capabilities,
            confidence,
            reasons,
            classifier_version: CLASSIFIER_VERSION.to_string(),
            input_hash: input.input_hash(),
        }
    }
}

/// The routing tier floor implied by risk, independent of complexity.
pub fn risk_floor(risk: f32) -> ModelTier {
    if risk >= 0.85 {
        ModelTier::T3
    } else if risk >= 0.65 {
        ModelTier::T2
    } else if risk >= 0.45 {
        ModelTier::T1
    } else {
        ModelTier::T0
    }
}

fn detect_flags(text: &str) -> Flags {
    Flags {
        security: contains_any(
            text,
            &[
                "security",
                "vulnerability",
                "cve",
                "exploit",
                "injection",
                "xss",
            ],
        ),
        authentication: contains_any(
            text,
            &[
                "authentication",
                "authorize",
                "authorization",
                "oauth",
                "jwt",
                "login",
                "session token",
            ],
        ),
        secrets: contains_any(
            text,
            &["secret", "credential", "api key", "private key", "password"],
        ),
        financial: contains_any(
            text,
            &[
                "billing",
                "payment",
                "financial",
                "accounting",
                "ledger",
                "spend",
                "invoice",
                "refund",
                "cost_usd",
            ],
        ),
        migration: contains_any(text, &["migration", "migrate"]),
        schema: contains_any(
            text,
            &["schema", "alter table", "create table", "table structure"],
        ),
        data_loss: contains_any(
            text,
            &[
                "data loss",
                "delete data",
                "drop table",
                "truncate",
                "purge",
            ],
        ),
        destructive: contains_any(
            text,
            &[
                "destructive",
                "irreversible",
                "drop column",
                "cannot be undone",
            ],
        ),
        concurrency: contains_any(
            text,
            &[
                "concurrency",
                "concurrent",
                "race condition",
                "deadlock",
                "lock contention",
                "parallel",
            ],
        ),
        external_side_effects: contains_any(
            text,
            &[
                "webhook",
                "third-party",
                "third party",
                "external service",
                "send email",
                "payment provider",
            ],
        ),
        production_infra: contains_any(
            text,
            &[
                "production",
                "infrastructure",
                "kubernetes",
                "docker",
                "terraform",
                "deploy",
                "ci/cd",
                "pipeline",
            ],
        ),
        public_api: contains_any(
            text,
            &[
                "public api",
                "breaking change",
                "api compatibility",
                "backwards compatible",
                "schema change",
            ],
        ),
    }
}

fn infer_kind(input: &TaskIntelligenceInput, text: &str) -> (TaskKind, &'static str) {
    if let Some(explicit) = &input.task_type {
        if let Some(kind) = parse_task_type(explicit) {
            return (kind, "explicit task_type");
        }
    }

    let checks: &[(&[&str], TaskKind)] = &[
        (&["migration", "migrate"], TaskKind::Migration),
        (
            &[
                "authentication",
                "oauth",
                "jwt",
                "login",
                "permission",
                "secret",
                "credential",
            ],
            TaskKind::Authentication,
        ),
        (
            &[
                "security",
                "vulnerability",
                "cve",
                "exploit",
                "injection",
                "authorization",
            ],
            TaskKind::Security,
        ),
        (
            &[
                "incident",
                "outage",
                "postmortem",
                "production down",
                "root cause",
            ],
            TaskKind::IncidentDebugging,
        ),
        (
            &["database", "postgres", "sql", "schema", "query"],
            TaskKind::DatabaseChange,
        ),
        (
            &[
                "infrastructure",
                "kubernetes",
                "docker",
                "terraform",
                "deploy",
                "pipeline",
            ],
            TaskKind::Infrastructure,
        ),
        (
            &[
                "architecture",
                "system design",
                "redesign",
                "cross-system",
                "system-wide",
            ],
            TaskKind::Architecture,
        ),
        (
            &["refactor", "restructure", "cleanup", "technical debt"],
            TaskKind::Refactor,
        ),
        (
            &[
                "integration",
                "webhook",
                "third-party",
                "third party",
                "external service",
            ],
            TaskKind::Integration,
        ),
        (
            &[
                "research",
                "investigate",
                "spike",
                "explore",
                "evaluate",
                "survey",
            ],
            TaskKind::Research,
        ),
        (
            &[
                "test suite",
                "unit test",
                "integration test",
                "e2e",
                "coverage",
            ],
            TaskKind::Testing,
        ),
        (
            &["format", "typo", "lint", "whitespace", "rename", "reformat"],
            TaskKind::Formatting,
        ),
        (
            &[
                "documentation",
                "readme",
                "docs",
                "guide",
                "comment",
                "changelog",
            ],
            TaskKind::Documentation,
        ),
        (
            &["bug", "fix", "regression", "broken", "crash", "error"],
            TaskKind::BugFix,
        ),
        (
            &[
                "add",
                "implement",
                "feature",
                "create",
                "build",
                "endpoint",
                "support",
                "introduce",
            ],
            TaskKind::FeatureImplementation,
        ),
    ];
    for (needles, kind) in checks {
        if contains_any(text, needles) {
            return (*kind, "keyword rule");
        }
    }
    (TaskKind::Unknown, "no kind signal")
}

fn parse_task_type(raw: &str) -> Option<TaskKind> {
    let s = raw.trim().to_ascii_lowercase();
    Some(match s.as_str() {
        "documentation" | "docs" | "doc" => TaskKind::Documentation,
        "formatting" | "format" | "lint" => TaskKind::Formatting,
        "simple_code_change" | "simple" => TaskKind::SimpleCodeChange,
        "feature" | "feature_implementation" | "generic_feature" => TaskKind::FeatureImplementation,
        "bug_fix" | "bugfix" | "bug" => TaskKind::BugFix,
        "refactor" => TaskKind::Refactor,
        "database_change" | "database" | "db" => TaskKind::DatabaseChange,
        "migration" => TaskKind::Migration,
        "architecture" | "design" => TaskKind::Architecture,
        "security" => TaskKind::Security,
        "authentication" | "auth" => TaskKind::Authentication,
        "infrastructure" | "infra" => TaskKind::Infrastructure,
        "integration" => TaskKind::Integration,
        "testing" | "test" => TaskKind::Testing,
        "research" => TaskKind::Research,
        "incident_debugging" | "incident" | "debugging" => TaskKind::IncidentDebugging,
        _ => return None,
    })
}

/// (scope, reasoning, verification, context) baselines per kind.
fn kind_baseline(kind: TaskKind) -> (f32, f32, f32, f32) {
    match kind {
        TaskKind::Formatting => (0.03, 0.03, 0.05, 0.05),
        TaskKind::Documentation => (0.06, 0.08, 0.10, 0.15),
        TaskKind::SimpleCodeChange => (0.12, 0.20, 0.20, 0.20),
        TaskKind::Testing => (0.25, 0.25, 0.45, 0.30),
        TaskKind::BugFix => (0.30, 0.40, 0.45, 0.40),
        TaskKind::FeatureImplementation => (0.35, 0.45, 0.40, 0.40),
        TaskKind::Refactor => (0.50, 0.60, 0.55, 0.60),
        TaskKind::DatabaseChange => (0.45, 0.55, 0.75, 0.60),
        TaskKind::Migration => (0.45, 0.60, 0.80, 0.65),
        TaskKind::Integration => (0.55, 0.60, 0.80, 0.70),
        TaskKind::Infrastructure => (0.60, 0.55, 0.75, 0.60),
        TaskKind::Architecture => (0.80, 0.90, 0.60, 0.85),
        TaskKind::Security => (0.50, 0.70, 0.80, 0.70),
        TaskKind::Authentication => (0.50, 0.70, 0.85, 0.70),
        TaskKind::Research => (0.40, 0.50, 0.40, 0.60),
        TaskKind::IncidentDebugging => (0.60, 0.80, 0.75, 0.70),
        TaskKind::Unknown => (0.40, 0.45, 0.50, 0.50),
    }
}

fn ambiguity_score(input: &TaskIntelligenceInput, text: &str) -> f32 {
    let mut score = match (
        input.has_definition_of_done,
        input.acceptance_criteria.is_empty(),
    ) {
        (true, false) => 0.10,
        (true, true) | (false, false) => 0.25,
        (false, true) => 0.50,
    };
    if contains_any(
        text,
        &[
            "investigate",
            "explore",
            "unknown",
            "unclear",
            "figure out",
            "maybe",
            "research",
        ],
    ) {
        score += 0.2;
    }
    if !input.test_commands.is_empty() {
        score -= 0.1;
    }
    clamp01(score)
}

fn reversibility_score(kind: TaskKind, flags: &Flags, text: &str) -> f32 {
    let mut score = 0.9f32;
    if matches!(kind, TaskKind::Migration | TaskKind::DatabaseChange) {
        score = score.min(0.4);
    }
    if flags.data_loss {
        score = score.min(0.1);
    }
    if flags.destructive {
        score = score.min(0.2);
    }
    if flags.financial {
        score = score.min(0.3);
    }
    if flags.production_infra {
        score = score.min(0.4);
    }
    if flags.security || flags.authentication || flags.secrets {
        score = score.min(0.6);
    }
    if flags.external_side_effects {
        score = score.min(0.3);
    }
    if matches!(kind, TaskKind::Documentation | TaskKind::Formatting)
        || contains_any(text, &["documentation", "readme", "comment", "typo"])
    {
        score = 0.98;
    }
    clamp01(score)
}

fn risk_score(kind: TaskKind, flags: &Flags) -> f32 {
    let mut risk: f32 = match kind {
        TaskKind::Documentation | TaskKind::Formatting => 0.05,
        TaskKind::Architecture => 0.4,
        TaskKind::IncidentDebugging => 0.45,
        TaskKind::Migration | TaskKind::DatabaseChange => 0.6,
        TaskKind::Security | TaskKind::Authentication => 0.7,
        _ => 0.2,
    };
    let mut bump = |v: f32| risk = risk.max(v);
    if flags.authentication {
        bump(0.9);
    }
    if flags.security {
        bump(0.85);
    }
    if flags.secrets {
        bump(0.85);
    }
    if flags.financial {
        bump(0.9);
    }
    if flags.data_loss {
        bump(0.9);
    }
    if flags.destructive {
        bump(0.85);
    }
    if flags.migration || flags.schema {
        bump(0.7);
    }
    if flags.production_infra {
        bump(0.7);
    }
    if flags.public_api {
        bump(0.6);
    }
    if flags.concurrency {
        bump(0.6);
    }
    if flags.external_side_effects {
        bump(0.6);
    }
    clamp01(risk)
}

fn verification_requirement(
    kind: TaskKind,
    flags: &Flags,
    risk: f32,
    reversibility: f32,
    verification_score: f32,
    complexity: f32,
) -> VerificationRequirement {
    let mut req = if risk >= 0.85
        || flags.data_loss
        || flags.destructive
        || matches!(kind, TaskKind::Authentication) && flags.secrets
    {
        VerificationRequirement::Critical
    } else if matches!(
        kind,
        TaskKind::Migration
            | TaskKind::DatabaseChange
            | TaskKind::Integration
            | TaskKind::Infrastructure
    ) || risk >= 0.65
        || verification_score >= 0.7
        || reversibility <= 0.4
        || complexity >= 0.6
    {
        VerificationRequirement::Strong
    } else if matches!(kind, TaskKind::Documentation | TaskKind::Formatting) {
        VerificationRequirement::Basic
    } else {
        VerificationRequirement::Standard
    };
    // Verification floor: a migration is never verified "Basic/Standard".
    if matches!(kind, TaskKind::Migration | TaskKind::DatabaseChange) {
        req = req.max(VerificationRequirement::Strong);
    }
    if risk >= 0.85 {
        req = VerificationRequirement::Critical;
    }
    req
}

#[allow(clippy::too_many_arguments)]
fn capabilities(
    kind: TaskKind,
    flags: &Flags,
    scope: f32,
    reasoning: f32,
    context: f32,
    risk: f32,
    tier: ModelTier,
    verification: VerificationRequirement,
    has_test_commands: bool,
) -> Vec<CapabilityRequirement> {
    use CapabilityRequirement as C;
    let mut set = std::collections::BTreeSet::new();

    let code_kind = matches!(
        kind,
        TaskKind::SimpleCodeChange
            | TaskKind::FeatureImplementation
            | TaskKind::BugFix
            | TaskKind::Refactor
            | TaskKind::DatabaseChange
            | TaskKind::Migration
            | TaskKind::Architecture
            | TaskKind::Security
            | TaskKind::Authentication
            | TaskKind::Infrastructure
            | TaskKind::Integration
            | TaskKind::Testing
    );
    if code_kind {
        set.insert(C::CodeGeneration);
    }
    if reasoning >= 0.55 || tier >= ModelTier::T2 {
        set.insert(C::StrongReasoning);
    }
    if scope >= 0.4 {
        set.insert(C::RepositoryNavigation);
        set.insert(C::MultiFileEditing);
    }
    if matches!(kind, TaskKind::DatabaseChange | TaskKind::Migration) || flags.schema {
        set.insert(C::DatabaseReasoning);
    }
    if matches!(kind, TaskKind::Architecture) || reasoning >= 0.85 {
        set.insert(C::ArchitectureReasoning);
    }
    if flags.security || flags.authentication || flags.secrets {
        set.insert(C::SecurityReasoning);
    }
    if matches!(kind, TaskKind::BugFix | TaskKind::IncidentDebugging) {
        set.insert(C::Debugging);
    }
    if matches!(kind, TaskKind::Testing)
        || has_test_commands
        || verification >= VerificationRequirement::Standard
    {
        set.insert(C::TestGeneration);
    }
    if context >= 0.6 {
        set.insert(C::LongContext);
    }
    if matches!(kind, TaskKind::Infrastructure | TaskKind::Integration) || flags.production_infra {
        set.insert(C::ToolUse);
    }
    if risk >= 0.6 || tier >= ModelTier::T3 || flags.financial || flags.data_loss {
        set.insert(C::HighReliability);
    }
    set.into_iter().collect()
}

fn confidence(input: &TaskIntelligenceInput, kind: TaskKind) -> f32 {
    let mut c = 0.5f32;
    if input.task_type.is_some() {
        c += 0.15;
    }
    if input.has_definition_of_done {
        c += 0.1;
    }
    if !input.acceptance_criteria.is_empty() {
        c += 0.1;
    }
    if !input.test_commands.is_empty() {
        c += 0.1;
    }
    if kind != TaskKind::Unknown {
        c += 0.05;
    }
    if c > 0.5 && input.prior_failures.len() > 1 {
        c -= 0.1;
    }
    c.clamp(0.1, 0.95)
}

fn combine_max_mean(values: &[f32]) -> f32 {
    let max = values.iter().copied().fold(0.0f32, f32::max);
    let mean = values.iter().sum::<f32>() / values.len().max(1) as f32;
    clamp01(0.6 * max + 0.4 * mean)
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

/// Convenience for tests: classify and return just the tier.
pub fn classify_tier(input: &TaskIntelligenceInput) -> ModelTier {
    TaskClassifier::classify(input).recommended_tier
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(input: TaskIntelligenceInput) -> NewTaskProfile {
        TaskClassifier::classify(&input)
    }

    #[test]
    fn simple_task_is_t0_with_basic_verification() {
        let profile = p(TaskIntelligenceInput::new("Change README typo")
            .with_description("Fix a spelling mistake in the documentation.")
            .with_definition_of_done(true));
        assert_eq!(profile.task_kind, TaskKind::Formatting);
        assert_eq!(profile.recommended_tier, ModelTier::T0);
        assert!(
            profile.complexity_score < 0.25,
            "complexity {}",
            profile.complexity_score
        );
        assert!(!profile.reasons.is_empty());
    }

    #[test]
    fn normal_task_is_t1() {
        let profile = p(TaskIntelligenceInput::new(
            "Add a bounded REST endpoint using the existing repository pattern",
        )
        .with_description("Implement a new read-only API endpoint.")
        .with_acceptance(vec!["endpoint returns 200".to_string()])
        .with_test_commands(vec!["cargo test -p argus-cli".to_string()]));
        assert_eq!(profile.task_kind, TaskKind::FeatureImplementation);
        assert_eq!(profile.recommended_tier, ModelTier::T1);
    }

    #[test]
    fn complex_refactor_is_at_least_t2_with_strong_verification() {
        let profile = p(TaskIntelligenceInput::new(
            "Refactor scheduler recovery across multiple crates while preserving restart semantics",
        )
        .with_description("Move recovery logic and keep restart behavior identical.")
        .with_test_commands(vec!["cargo test -p argus-core -- --ignored".to_string()]));
        assert_eq!(profile.task_kind, TaskKind::Refactor);
        assert!(
            profile.recommended_tier >= ModelTier::T2,
            "got {}",
            profile.recommended_tier
        );
        assert!(
            profile
                .required_capabilities
                .contains(&CapabilityRequirement::MultiFileEditing)
        );
    }

    #[test]
    fn critical_auth_task_respects_risk_floor() {
        let profile = p(TaskIntelligenceInput::new(
            "Modify authentication and secret permission boundaries",
        )
        .with_description("Tighten the auth checks and rotate the secret handling.")
        .with_definition_of_done(true));
        assert!(matches!(
            profile.task_kind,
            TaskKind::Authentication | TaskKind::Security
        ));
        assert!(
            profile.recommended_tier >= ModelTier::T2,
            "risk floor must apply, got {}",
            profile.recommended_tier
        );
        assert!(
            profile
                .required_capabilities
                .contains(&CapabilityRequirement::SecurityReasoning)
        );
        assert!(
            profile
                .required_capabilities
                .contains(&CapabilityRequirement::HighReliability)
        );
    }

    #[test]
    fn short_financial_prompt_is_not_t0() {
        let profile = p(TaskIntelligenceInput::new("Fix duplicate billing."));
        assert!(
            profile.recommended_tier >= ModelTier::T2,
            "financial risk floor must apply, got {}",
            profile.recommended_tier
        );
        assert!(profile.risk_score >= 0.85);
    }

    #[test]
    fn long_documentation_prompt_stays_low() {
        let long = "Rewrite the documentation prose, improve wording, adjust headings, \
                    fix comma splices, rewrite the introduction, add examples to the guide, \
                    clarify terminology, and reorganize the table of contents. "
            .repeat(20);
        let profile = p(
            TaskIntelligenceInput::new("Reformat the documentation guide").with_description(long),
        );
        assert!(matches!(
            profile.task_kind,
            TaskKind::Documentation | TaskKind::Formatting
        ));
        assert!(
            profile.recommended_tier <= ModelTier::T1,
            "prompt length must not raise the tier, got {}",
            profile.recommended_tier
        );
    }

    #[test]
    fn every_non_trivial_decision_has_a_reason() {
        let profile = p(TaskIntelligenceInput::new("Implement cross-crate routing")
            .with_description("New routing policy across multiple crates."));
        assert!(profile.recommended_tier > ModelTier::T0);
        assert!(!profile.reasons.is_empty(), "T1+ must be explainable");
    }

    #[test]
    fn determinism() {
        let input = TaskIntelligenceInput::new("Implement a migration")
            .with_description("Add a database migration for spend tracking.");
        let a = TaskClassifier::classify(&input);
        let b = TaskClassifier::classify(&input);
        assert_eq!(a.input_hash, b.input_hash);
        assert_eq!(a.recommended_tier, b.recommended_tier);
        assert_eq!(a.complexity_score, b.complexity_score);
    }
}
