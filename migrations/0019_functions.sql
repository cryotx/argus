-- 0019: Helper functions and triggers

-- Trigger to auto-update updated_at columns
CREATE OR REPLACE FUNCTION update_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply the trigger to all tables with updated_at
CREATE TRIGGER trg_projects_updated_at
    BEFORE UPDATE ON projects FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_milestones_updated_at
    BEFORE UPDATE ON milestones FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_tasks_updated_at
    BEFORE UPDATE ON tasks FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_mcp_servers_updated_at
    BEFORE UPDATE ON mcp_servers FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_skills_updated_at
    BEFORE UPDATE ON skills FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_env_configs_updated_at
    BEFORE UPDATE ON environment_configs FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_owner_profiles_updated_at
    BEFORE UPDATE ON owner_profiles FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_llm_providers_updated_at
    BEFORE UPDATE ON llm_providers FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- Memory decay function (run daily via scheduler)
CREATE OR REPLACE FUNCTION decay_memories()
RETURNS INTEGER AS $$
DECLARE
    affected INTEGER;
BEGIN
    -- Reduce importance based on decay_rate and time since last access
    UPDATE memories
    SET importance = GREATEST(
        0.0,
        importance - decay_rate * EXTRACT(EPOCH FROM (NOW() - COALESCE(last_accessed, created_at))) / 86400.0
    )
    WHERE layer IN ('short_term', 'long_term')
      AND importance > 0.0;

    GET DIAGNOSTICS affected = ROW_COUNT;

    -- Delete expired memories
    DELETE FROM memories
    WHERE expires_at IS NOT NULL AND expires_at < NOW();

    -- Delete memories with importance below threshold (except permanent)
    DELETE FROM memories
    WHERE layer != 'permanent'
      AND importance < 0.01
      AND last_accessed < NOW() - INTERVAL '7 days';

    RETURN affected;
END;
$$ LANGUAGE plpgsql;

-- Promote important short-term memories to long-term
CREATE OR REPLACE FUNCTION promote_memories()
RETURNS INTEGER AS $$
DECLARE
    affected INTEGER;
BEGIN
    UPDATE memories
    SET layer = 'long_term',
        expires_at = NULL
    WHERE layer = 'short_term'
      AND importance > 0.7
      AND access_count >= 3;

    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected;
END;
$$ LANGUAGE plpgsql;

-- Clean up old deployment artifacts
CREATE OR REPLACE FUNCTION cleanup_old_artifacts(max_age_days INTEGER DEFAULT 30)
RETURNS INTEGER AS $$
DECLARE
    affected INTEGER;
BEGIN
    DELETE FROM deployment_artifacts
    WHERE created_at < NOW() - (max_age_days || ' days')::INTERVAL;

    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected;
END;
$$ LANGUAGE plpgsql;

-- Refresh LLM usage materialized view
CREATE OR REPLACE FUNCTION refresh_llm_usage_daily()
RETURNS VOID AS $$
BEGIN
    REFRESH MATERIALIZED VIEW CONCURRENTLY llm_usage_daily;
END;
$$ LANGUAGE plpgsql;
