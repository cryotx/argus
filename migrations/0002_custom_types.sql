-- 0002: Custom ENUM types
CREATE TYPE task_state AS ENUM (
    'pending', 'ready', 'queued', 'assigned', 'running',
    'paused', 'completed', 'failed', 'cancelled', 'timed_out'
);

CREATE TYPE task_priority AS ENUM (
    'critical', 'high', 'medium', 'low', 'background'
);

CREATE TYPE environment AS ENUM (
    'development', 'staging', 'production', 'custom'
);

CREATE TYPE deployment_status AS ENUM (
    'queued', 'running', 'waiting_approval', 'approved',
    'succeeded', 'failed', 'rolled_back', 'cancelled'
);

CREATE TYPE worker_state AS ENUM (
    'idle', 'busy', 'starting', 'crashed', 'shutting_down'
);

CREATE TYPE worker_type AS ENUM (
    'opencode', 'claude_code', 'aider', 'generic_cli'
);

CREATE TYPE memory_layer AS ENUM (
    'working', 'short_term', 'long_term', 'permanent'
);

CREATE TYPE memory_type AS ENUM (
    'fact', 'decision', 'experience', 'preference',
    'error_pattern', 'skill_learned', 'project_context',
    'conversation_summary', 'safety_rule'
);

CREATE TYPE trust_level AS ENUM (
    'builtin', 'verified', 'third_party', 'untrusted'
);

CREATE TYPE role AS ENUM (
    'owner', 'admin', 'developer', 'viewer'
);

CREATE TYPE platform AS ENUM (
    'telegram', 'discord', 'wechat', 'slack', 'web_dashboard'
);

CREATE TYPE audit_outcome AS ENUM (
    'success', 'failure', 'blocked', 'warned'
);

CREATE TYPE evolution_status AS ENUM (
    'requested', 'safety_checking', 'safety_approved', 'safety_rejected',
    'branch_created', 'in_development', 'testing', 'tests_passed',
    'tests_failed', 'test_instance_running', 'awaiting_approval',
    'approved', 'rejected', 'deploying', 'deployed',
    'health_check_passed', 'health_check_failed', 'rolled_back',
    'completed', 'failed'
);

CREATE TYPE collaboration_mode AS ENUM (
    'solo', 'pair_review', 'full_team'
);

CREATE TYPE collaboration_phase AS ENUM (
    'architecture', 'implementation', 'review', 'testing',
    'security_audit', 'merging', 'completed', 'failed'
);

CREATE TYPE collaboration_role AS ENUM (
    'architect', 'developer', 'reviewer', 'tester',
    'security_auditor', 'devops', 'tech_lead'
);
