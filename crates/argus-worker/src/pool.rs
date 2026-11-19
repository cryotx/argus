//! Worker pool management

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::drivers::{WorkerDriver, get_driver};
use crate::types::{
    ResourceLimits, TaskPayload, WorkerConfig, WorkerHandle, WorkerState, WorkerStats, WorkerType,
};

const ASSIGN_RETRY_ATTEMPTS: u32 = 3;
const ASSIGN_RETRY_BASE_BACKOFF_MS: u64 = 200;

/// A managed worker
pub struct Worker {
    pub id: Uuid,
    pub worker_type: WorkerType,
    pub state: WorkerState,
    pub handle: WorkerHandle,
    pub current_task_id: Option<Uuid>,
    pub config: WorkerConfig,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub spawned_at: chrono::DateTime<chrono::Utc>,
    pub last_activity_at: chrono::DateTime<chrono::Utc>,
}

impl Worker {
    pub fn is_idle(&self) -> bool {
        self.state == WorkerState::Idle
    }

    pub fn is_busy(&self) -> bool {
        self.state == WorkerState::Busy
    }

    pub fn touch(&mut self) {
        self.last_activity_at = chrono::Utc::now();
    }

    pub fn idle_duration(&self) -> Duration {
        let elapsed = chrono::Utc::now() - self.last_activity_at;
        Duration::from_secs(elapsed.num_seconds().max(0) as u64)
    }
}

/// Worker pool
pub struct WorkerPool {
    workers: Arc<RwLock<HashMap<Uuid, Worker>>>,
    max_workers: usize,
    _max_workers_per_project: usize,
    drivers: HashMap<WorkerType, Box<dyn WorkerDriver>>,
    total_spawned: Arc<RwLock<u64>>,
}

impl WorkerPool {
    pub fn new(max_workers: usize, max_workers_per_project: usize) -> Self {
        let mut drivers = HashMap::new();
        drivers.insert(WorkerType::OpenCode, get_driver(WorkerType::OpenCode));
        drivers.insert(WorkerType::ClaudeCode, get_driver(WorkerType::ClaudeCode));
        drivers.insert(WorkerType::Aider, get_driver(WorkerType::Aider));
        drivers.insert(WorkerType::GenericCli, get_driver(WorkerType::GenericCli));

        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            max_workers,
            _max_workers_per_project: max_workers_per_project,
            drivers,
            total_spawned: Arc::new(RwLock::new(0)),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(5, 3)
    }

    pub async fn worker_count(&self) -> usize {
        let workers = self.workers.read().await;
        workers.len()
    }

    pub async fn worker_ids(&self) -> Vec<Uuid> {
        let workers = self.workers.read().await;
        workers.keys().copied().collect()
    }

    pub async fn worker_count_for_project(&self, _project_id: Uuid) -> usize {
        let workers = self.workers.read().await;
        workers
            .values()
            .filter(|w| w.current_task_id.is_some())
            .count()
    }

    pub async fn find_idle_worker(&self, worker_type: WorkerType) -> Option<Uuid> {
        let workers = self.workers.read().await;
        for (id, worker) in workers.iter() {
            if worker.worker_type == worker_type && worker.is_idle() {
                return Some(*id);
            }
        }
        None
    }

    pub async fn spawn_worker(
        &self,
        worker_type: WorkerType,
        workspace_dir: &Path,
    ) -> anyhow::Result<Uuid> {
        let current_count = self.worker_count().await;
        if current_count >= self.max_workers {
            anyhow::bail!("Worker pool at capacity");
        }

        let driver = self
            .drivers
            .get(&worker_type)
            .ok_or_else(|| anyhow::anyhow!("No driver for {:?}", worker_type))?;

        let config = WorkerConfig {
            workspace_dir: workspace_dir.to_path_buf(),
            sandbox_type: worker_type.default_sandbox(),
            resource_limits: ResourceLimits::default(),
            ..Default::default()
        };

        let handle = driver.spawn(&config).await?;

        let worker_id = Uuid::new_v4();
        let now = chrono::Utc::now();

        let worker = Worker {
            id: worker_id,
            worker_type,
            state: WorkerState::Idle,
            handle,
            current_task_id: None,
            config,
            tasks_completed: 0,
            tasks_failed: 0,
            spawned_at: now,
            last_activity_at: now,
        };

        {
            let mut workers = self.workers.write().await;
            workers.insert(worker_id, worker);
        }

        {
            let mut total = self.total_spawned.write().await;
            *total += 1;
        }

        info!(worker_id = %worker_id, worker_type = ?worker_type, "Spawned new worker");
        Ok(worker_id)
    }

    pub async fn assign_task(
        &self,
        worker_id: Uuid,
        task_id: Uuid,
        task: &TaskPayload,
    ) -> anyhow::Result<()> {
        let mut worker = {
            let mut workers = self.workers.write().await;
            let worker = workers
                .remove(&worker_id)
                .ok_or_else(|| anyhow::anyhow!("Worker not found"))?;
            if !worker.is_idle() {
                workers.insert(worker_id, worker);
                anyhow::bail!("Worker is not idle");
            }
            worker
        };

        let driver = match self.drivers.get(&worker.worker_type) {
            Some(driver) => driver,
            None => {
                let mut workers = self.workers.write().await;
                workers.insert(worker_id, worker);
                anyhow::bail!("Driver not found");
            }
        };

        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=ASSIGN_RETRY_ATTEMPTS {
            worker.state = WorkerState::Busy;
            worker.current_task_id = Some(task_id);
            worker.touch();

            match driver.send_task(&mut worker.handle, task).await {
                Ok(()) => {
                    info!(
                        worker_id = %worker_id,
                        task_id = %task_id,
                        attempt,
                        "Assigned task to worker"
                    );
                    let mut workers = self.workers.write().await;
                    workers.insert(worker_id, worker);
                    return Ok(());
                }
                Err(err) => {
                    worker.state = WorkerState::Idle;
                    worker.current_task_id = None;
                    worker.touch();

                    warn!(
                        worker_id = %worker_id,
                        task_id = %task_id,
                        attempt,
                        error = %err,
                        "Worker task assignment failed"
                    );
                    last_error = Some(err);

                    if attempt < ASSIGN_RETRY_ATTEMPTS {
                        let backoff_ms = ASSIGN_RETRY_BASE_BACKOFF_MS * (1u64 << (attempt - 1));
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    }
                }
            }
        }

        worker.tasks_failed += 1;
        let mut workers = self.workers.write().await;
        workers.insert(worker_id, worker);

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("task assignment failed")))
    }

    pub async fn complete_task(&self, worker_id: Uuid, success: bool) -> anyhow::Result<()> {
        let mut workers = self.workers.write().await;
        let worker = workers
            .get_mut(&worker_id)
            .ok_or_else(|| anyhow::anyhow!("Worker not found"))?;

        worker.state = WorkerState::Idle;
        worker.current_task_id = None;
        worker.touch();

        if success {
            worker.tasks_completed += 1;
        } else {
            worker.tasks_failed += 1;
        }

        Ok(())
    }

    pub async fn stop_worker(&self, worker_id: Uuid) -> anyhow::Result<()> {
        let mut workers = self.workers.write().await;
        let worker = workers
            .get_mut(&worker_id)
            .ok_or_else(|| anyhow::anyhow!("Worker not found"))?;

        let driver = self
            .drivers
            .get(&worker.worker_type)
            .ok_or_else(|| anyhow::anyhow!("Driver not found"))?;

        worker.state = WorkerState::ShuttingDown;
        driver.stop(&mut worker.handle).await?;
        workers.remove(&worker_id);

        info!(worker_id = %worker_id, "Stopped worker");
        Ok(())
    }

    pub async fn kill_worker(&self, worker_id: Uuid) -> anyhow::Result<()> {
        let mut workers = self.workers.write().await;
        let worker = workers
            .get_mut(&worker_id)
            .ok_or_else(|| anyhow::anyhow!("Worker not found"))?;

        let driver = self
            .drivers
            .get(&worker.worker_type)
            .ok_or_else(|| anyhow::anyhow!("Driver not found"))?;

        driver.kill(&mut worker.handle).await?;
        workers.remove(&worker_id);

        warn!(worker_id = %worker_id, "Killed worker");
        Ok(())
    }

    pub async fn stats(&self) -> WorkerStats {
        let workers = self.workers.read().await;

        let mut idle = 0;
        let mut busy = 0;
        let mut crashed = 0;
        let mut total_completed = 0u64;
        let mut total_failed = 0u64;

        for worker in workers.values() {
            match worker.state {
                WorkerState::Idle => idle += 1,
                WorkerState::Busy => busy += 1,
                WorkerState::Crashed => crashed += 1,
                _ => {}
            }
            total_completed += worker.tasks_completed;
            total_failed += worker.tasks_failed;
        }

        let total_tasks = total_completed + total_failed;
        let success_rate = if total_tasks > 0 {
            total_completed as f64 / total_tasks as f64
        } else {
            1.0
        };

        WorkerStats {
            total_workers: workers.len(),
            idle_workers: idle,
            busy_workers: busy,
            crashed_workers: crashed,
            total_tasks_completed: total_completed,
            total_tasks_failed: total_failed,
            success_rate,
        }
    }

    pub async fn cleanup_idle_workers(&self, max_idle_duration: Duration) -> Vec<Uuid> {
        let workers = self.workers.read().await;

        let idle_workers: Vec<Uuid> = workers
            .iter()
            .filter(|(_, w)| w.is_idle())
            .filter(|(_, w)| w.idle_duration() > max_idle_duration)
            .map(|(id, _)| *id)
            .collect();

        drop(workers);

        let mut removed = Vec::new();
        for worker_id in idle_workers {
            if self.worker_count().await > 1 {
                if let Err(e) = self.stop_worker(worker_id).await {
                    warn!(worker_id = %worker_id, error = %e, "Failed to stop idle worker");
                } else {
                    removed.push(worker_id);
                }
            }
        }

        removed
    }
}

impl Default for WorkerPool {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_worker_pool_creation() {
        let pool = WorkerPool::new(5, 3);
        assert_eq!(pool.worker_count().await, 0);
    }

    #[tokio::test]
    async fn test_worker_pool_spawn() {
        let pool = WorkerPool::new(5, 3);
        let workspace = std::env::temp_dir();

        let worker_id = pool
            .spawn_worker(WorkerType::GenericCli, &workspace)
            .await
            .unwrap();
        assert_eq!(pool.worker_count().await, 1);

        let _ = pool.stop_worker(worker_id).await;
    }

    #[tokio::test]
    async fn test_worker_pool_stats() {
        let pool = WorkerPool::new(5, 3);
        let workspace = std::env::temp_dir();

        let worker_id = pool
            .spawn_worker(WorkerType::GenericCli, &workspace)
            .await
            .unwrap();
        let stats = pool.stats().await;

        assert_eq!(stats.total_workers, 1);
        assert_eq!(stats.idle_workers, 1);

        let _ = pool.stop_worker(worker_id).await;
    }

    #[tokio::test]
    async fn test_assign_task_unknown_worker_returns_error() {
        let pool = WorkerPool::new(5, 3);
        let payload = TaskPayload::default();

        let result = pool
            .assign_task(Uuid::new_v4(), Uuid::new_v4(), &payload)
            .await;
        assert!(result.is_err());
    }
}
