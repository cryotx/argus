//! Tests for V0.35 recovery safety requirements.

use std::sync::Arc;
use uuid::Uuid;

use argus_core::task_repository::{CreateTaskResult, TaskRepository};
use argus_core::task_scheduler::TaskScheduler;
use argus_core::task_types::{TaskBuilder, TaskState};
use argus_core::types::ArgusResult;
use async_trait::async_trait;

/// Mock repository for testing restore behavior
struct TestRepository {
    tasks: std::sync::Mutex<Vec<(Uuid, TaskState)>>,
}

#[async_trait]
impl TaskRepository for TestRepository {
    async fn create_task(
        &self,
        task: &argus_core::task_types::Task,
        _request_key: Option<&str>,
    ) -> ArgusResult<CreateTaskResult> {
        self.tasks.lock().unwrap().push((task.id, task.state));
        Ok(CreateTaskResult::Created(task.id))
    }

    async fn transition_task(
        &self,
        _expected_state: TaskState,
        task: &argus_core::task_types::Task,
        _checkpoint_context: serde_json::Value,
    ) -> ArgusResult<bool> {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(pos) = tasks.iter().position(|(id, _)| id == &task.id) {
            tasks[pos].1 = task.state;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn load_tasks(&self) -> ArgusResult<Vec<argus_core::task_types::Task>> {
        // This should not be called during restore
        panic!("load_tasks() should not be called during recovery; use load_active_tasks()");
    }

    async fn load_active_tasks(&self) -> ArgusResult<Vec<argus_core::task_types::Task>> {
        let tasks = self.tasks.lock().unwrap();
        let project_id = Uuid::new_v4();

        Ok(tasks
            .iter()
            .filter(|(_, state)| !state.is_terminal())
            .map(|(id, state)| {
                let mut task = TaskBuilder::new(project_id, format!("Task {}", id)).build();
                task.id = *id;
                task.state = *state;
                task
            })
            .collect())
    }

    async fn find_task_id_by_request_key(&self, _request_key: &str) -> ArgusResult<Option<Uuid>> {
        Ok(None)
    }
}

#[tokio::test]
async fn test_restore_excludes_terminal_tasks() {
    let repo = Arc::new(TestRepository {
        tasks: std::sync::Mutex::new(vec![
            (Uuid::new_v4(), TaskState::Completed),
            (Uuid::new_v4(), TaskState::Failed),
            (Uuid::new_v4(), TaskState::Cancelled),
            (Uuid::new_v4(), TaskState::TimedOut),
            (Uuid::new_v4(), TaskState::Ready),
            (Uuid::new_v4(), TaskState::Pending),
        ]),
    });

    let scheduler = TaskScheduler::new_with_repository(repo.clone(), None);
    let report = scheduler.restore().await.unwrap();

    // Should only restore 2 active tasks (Ready + Pending), not 4 terminal tasks
    assert_eq!(
        report.active_tasks, 2,
        "Should only restore non-terminal tasks"
    );

    let all_tasks = scheduler.get_all_tasks().await;
    assert_eq!(all_tasks.len(), 2);
    assert!(all_tasks.iter().all(|t| !t.state.is_terminal()));
}

#[tokio::test]
async fn test_restore_resumes_interrupted_tasks() {
    let repo = Arc::new(TestRepository {
        tasks: std::sync::Mutex::new(vec![
            (Uuid::new_v4(), TaskState::Running),
            (Uuid::new_v4(), TaskState::Testing),
            (Uuid::new_v4(), TaskState::Ready),
        ]),
    });

    let scheduler = TaskScheduler::new_with_repository(repo.clone(), None);
    let report = scheduler.restore().await.unwrap();

    assert_eq!(report.active_tasks, 3);
    assert_eq!(report.interrupted_resumed, 2);

    let all_tasks = scheduler.get_all_tasks().await;
    // All should be in Ready state after recovery
    assert!(all_tasks.iter().all(|t| t.state == TaskState::Ready));
}

#[tokio::test]
async fn test_restore_with_many_historical_tasks() {
    let mut task_states = vec![];

    // Add many completed tasks (historical)
    for _ in 0..1000 {
        task_states.push((Uuid::new_v4(), TaskState::Completed));
    }
    for _ in 0..500 {
        task_states.push((Uuid::new_v4(), TaskState::Failed));
    }

    // Add few active tasks
    task_states.push((Uuid::new_v4(), TaskState::Ready));
    task_states.push((Uuid::new_v4(), TaskState::Ready));
    task_states.push((Uuid::new_v4(), TaskState::Running));

    let repo = Arc::new(TestRepository {
        tasks: std::sync::Mutex::new(task_states),
    });

    let scheduler = TaskScheduler::new_with_repository(repo.clone(), None);
    let report = scheduler.restore().await.unwrap();

    // Should only restore 3 active tasks, not 1500 historical ones
    assert_eq!(report.active_tasks, 3);

    let all_tasks = scheduler.get_all_tasks().await;
    assert_eq!(all_tasks.len(), 3);
}
