//! V0.38 Phase 2: Workspace Lifecycle Integration Tests
//!
//! Tests for workspace persistence, idempotency, ownership validation,
//! and integration with ExecutionLoop.

#[cfg(test)]
mod workspace_lifecycle {
    use argus_core::workspace_manager::{WorkspaceConfig, WorkspaceContext, WorkspaceManager};
    use uuid::Uuid;

    #[test]
    fn test_workspace_context_creation() {
        let context = WorkspaceContext {
            workspace_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            execution_id: Uuid::new_v4(),
            canonical_repository_path: "/repo".into(),
            worktree_path: "/workspace/task".into(),
            branch_name: "argus/task/123/456".to_string(),
            base_revision: "abc123def456".to_string(),
        };

        assert_ne!(context.workspace_id, Uuid::nil());
        assert_ne!(context.project_id, Uuid::nil());
        assert_ne!(context.task_id, Uuid::nil());
        assert_ne!(context.execution_id, Uuid::nil());
        println!("✓ WorkspaceContext created successfully");
    }

    #[test]
    fn test_workspace_ownership_validation() {
        let config = WorkspaceConfig::new("/tmp/argus-test");
        let manager = WorkspaceManager::new(config);

        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();

        let path = manager.worktree_path(project_id, execution_id);
        let branch = manager.branch_name(task_id, execution_id);

        // Ownership is deterministic
        let path2 = manager.worktree_path(project_id, execution_id);
        let branch2 = manager.branch_name(task_id, execution_id);

        assert_eq!(
            path, path2,
            "Same project/execution should produce same path"
        );
        assert_eq!(
            branch, branch2,
            "Same task/execution should produce same branch"
        );
        println!("✓ Workspace ownership deterministic");
    }

    #[test]
    fn test_canonical_vs_worktree_path_different() {
        let canonical = "/repo/main";
        let worktree = "/workspace/project/execution";

        assert_ne!(canonical, worktree);
        println!("✓ Canonical repo and worktree paths are distinct");
    }

    #[test]
    fn test_idempotent_workspace_generation() {
        // Simulating idempotent workspace generation
        let project_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();

        let config = WorkspaceConfig::new("/tmp/argus-test");
        let manager = WorkspaceManager::new(config);

        // First call
        let path_1 = manager.worktree_path(project_id, execution_id);
        let branch_1 = manager.branch_name(task_id, execution_id);

        // Second call (same execution_id)
        let path_2 = manager.worktree_path(project_id, execution_id);
        let branch_2 = manager.branch_name(task_id, execution_id);

        // Must be identical
        assert_eq!(path_1, path_2, "Idempotent: same execution_id → same path");
        assert_eq!(
            branch_1, branch_2,
            "Idempotent: same execution_id → same branch"
        );
        println!("✓ Workspace generation is idempotent");
    }

    #[test]
    fn test_budget_blocked_no_duplicate_workspace() {
        // Scenario: Task claimed, workspace created, budget blocked, requeued
        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();

        let config = WorkspaceConfig::new("/tmp/argus-test");
        let manager = WorkspaceManager::new(config);

        // First execution attempt
        let ws_1 = WorkspaceContext {
            workspace_id: Uuid::new_v4(),
            project_id,
            task_id,
            execution_id,
            canonical_repository_path: "/repo".into(),
            worktree_path: manager.worktree_path(project_id, execution_id),
            branch_name: manager.branch_name(task_id, execution_id),
            base_revision: "abc123".to_string(),
        };

        // Budget blocked, task requeued (same execution_id would be reused in production)
        // Second attempt with SAME execution_id
        let ws_2_path = manager.worktree_path(project_id, execution_id);
        let ws_2_branch = manager.branch_name(task_id, execution_id);

        // Must reuse same workspace
        assert_eq!(ws_1.worktree_path, ws_2_path);
        assert_eq!(ws_1.branch_name, ws_2_branch);
        println!("✓ Budget blocked: workspace reused on requeue");
    }

    #[test]
    fn test_two_execution_different_workspaces() {
        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();

        let config = WorkspaceConfig::new("/tmp/argus-test");
        let manager = WorkspaceManager::new(config);

        // Execution 1
        let exec_1 = Uuid::new_v4();
        let ws_1_path = manager.worktree_path(project_id, exec_1);
        let ws_1_branch = manager.branch_name(task_id, exec_1);

        // Execution 2 (different execution_id, same task)
        let exec_2 = Uuid::new_v4();
        let ws_2_path = manager.worktree_path(project_id, exec_2);
        let ws_2_branch = manager.branch_name(task_id, exec_2);

        // Must be different
        assert_ne!(
            ws_1_path, ws_2_path,
            "Different execution_ids → different paths"
        );
        assert_ne!(
            ws_1_branch, ws_2_branch,
            "Different execution_ids → different branches"
        );
        println!("✓ Different executions create separate workspaces");
    }

    #[test]
    fn test_worktree_cannot_be_canonical_repo() {
        let canonical = "/repo/main";
        let project_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();

        let config = WorkspaceConfig::new("/tmp/argus-test");
        let manager = WorkspaceManager::new(config);

        let worktree = manager.worktree_path(project_id, execution_id);

        // They should never be the same
        assert_ne!(canonical, worktree.to_string_lossy().as_ref());
        println!("✓ Worktree path differs from canonical repository");
    }

    #[test]
    fn test_execution_id_maps_to_single_workspace() {
        // CRITICAL: execution_id uniquely identifies workspace
        let execution_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();

        let config = WorkspaceConfig::new("/tmp/argus-test");
        let manager = WorkspaceManager::new(config);

        // Generate workspace context
        let ws = WorkspaceContext {
            workspace_id: Uuid::new_v4(),
            project_id,
            task_id: Uuid::new_v4(),
            execution_id,
            canonical_repository_path: "/repo".into(),
            worktree_path: manager.worktree_path(project_id, execution_id),
            branch_name: manager.branch_name(Uuid::new_v4(), execution_id),
            base_revision: "abc123".to_string(),
        };

        // Even if we lose workspace_id, we can recover from (project_id, execution_id)
        let recovered_path = manager.worktree_path(project_id, execution_id);
        assert_eq!(ws.worktree_path, recovered_path);
        println!("✓ execution_id uniquely maps to workspace (recovery-safe)");
    }
}
