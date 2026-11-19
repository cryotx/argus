//! System metrics collection

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// System-level metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub timestamp: DateTime<Utc>,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub disk: DiskMetrics,
    pub network: NetworkMetrics,
}

/// CPU metrics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CpuMetrics {
    pub usage_percent: f64,
    pub count: usize,
    pub load_avg: [f64; 3],
}

/// Memory metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
}

impl Default for MemoryMetrics {
    fn default() -> Self {
        Self {
            total_bytes: 0,
            used_bytes: 0,
            available_bytes: 0,
            usage_percent: 0.0,
        }
    }
}

/// Disk metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
}

impl Default for DiskMetrics {
    fn default() -> Self {
        Self {
            total_bytes: 0,
            used_bytes: 0,
            available_bytes: 0,
            usage_percent: 0.0,
        }
    }
}

/// Network metrics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkMetrics {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Process metrics for a specific worker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessMetrics {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub created_at: Option<DateTime<Utc>>,
}

/// Application metrics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppMetrics {
    pub workers_active: usize,
    pub workers_idle: usize,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub uptime_seconds: u64,
    pub custom: HashMap<String, f64>,
}

impl AppMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_worker_stats(
        workers_active: usize,
        workers_idle: usize,
        tasks_completed: u64,
        tasks_failed: u64,
    ) -> Self {
        Self {
            workers_active,
            workers_idle,
            tasks_completed,
            tasks_failed,
            uptime_seconds: 0,
            custom: HashMap::new(),
        }
    }

    pub fn set_custom(&mut self, key: impl Into<String>, value: f64) {
        self.custom.insert(key.into(), value);
    }
}

/// Metrics collector for aggregating multiple metric sources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub system: SystemMetrics,
    pub app: AppMetrics,
    pub processes: Vec<ProcessMetrics>,
}

impl SystemMetrics {
    /// Collect current system metrics from /proc
    pub async fn collect() -> anyhow::Result<Self> {
        let cpu = Self::collect_cpu().await?;
        let memory = Self::collect_memory().await?;
        let disk = Self::collect_disk().await?;
        let network = Self::collect_network().await?;

        Ok(Self {
            timestamp: Utc::now(),
            cpu,
            memory,
            disk,
            network,
        })
    }

    async fn collect_cpu() -> anyhow::Result<CpuMetrics> {
        let usage = Self::read_cpu_usage().await?;
        let count = Self::get_cpu_count()?;
        let load_avg = Self::get_load_avg()?;

        Ok(CpuMetrics {
            usage_percent: usage,
            count,
            load_avg,
        })
    }

    async fn read_cpu_usage() -> anyhow::Result<f64> {
        let stat = tokio::fs::read_to_string("/proc/stat").await?;
        let line = stat
            .lines()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No CPU line"))?;

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            return Ok(0.0);
        }

        let user: u64 = parts[1].parse().unwrap_or(0);
        let nice: u64 = parts[2].parse().unwrap_or(0);
        let system: u64 = parts[3].parse().unwrap_or(0);
        let idle: u64 = parts[4].parse().unwrap_or(0);
        let iowait: u64 = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
        let irq: u64 = parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
        let softirq: u64 = parts.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);

        let total = user + nice + system + idle + iowait + irq + softirq;
        let idle_time = idle + iowait;

        if total == 0 {
            return Ok(0.0);
        }

        let usage = ((total - idle_time) as f64 / total as f64) * 100.0;
        Ok(usage)
    }

    fn get_cpu_count() -> anyhow::Result<usize> {
        Ok(std::thread::available_parallelism()?.get())
    }

    fn get_load_avg() -> anyhow::Result<[f64; 3]> {
        let loadavg = std::fs::read_to_string("/proc/loadavg")?;
        let parts: Vec<&str> = loadavg.split_whitespace().collect();

        let load1: f64 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let load5: f64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let load15: f64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);

        Ok([load1, load5, load15])
    }

    async fn collect_memory() -> anyhow::Result<MemoryMetrics> {
        let meminfo = tokio::fs::read_to_string("/proc/meminfo").await?;

        let mut total: u64 = 0;
        let mut available: u64 = 0;
        let mut free: u64 = 0;
        let mut buffers: u64 = 0;
        let mut cached: u64 = 0;

        for line in meminfo.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let val = value
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0)
                    * 1024;

                match key.trim() {
                    "MemTotal" => total = val,
                    "MemAvailable" => available = val,
                    "MemFree" => free = val,
                    "Buffers" => buffers = val,
                    "Cached" => cached = val,
                    _ => {}
                }
            }
        }

        // Fallback: calculate available from free + buffers + cached
        if available == 0 {
            available = free + buffers + cached;
        }

        let used = total.saturating_sub(available);
        let usage_percent = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        Ok(MemoryMetrics {
            total_bytes: total,
            used_bytes: used,
            available_bytes: available,
            usage_percent,
        })
    }

    async fn collect_disk() -> anyhow::Result<DiskMetrics> {
        // Get root filesystem stats
        let statfs = unsafe {
            let mut stat: libc::statfs = std::mem::zeroed();
            let path = c"/";
            if libc::statfs(path.as_ptr(), &mut stat) != 0 {
                return Ok(DiskMetrics::default());
            }
            stat
        };

        let total = (statfs.f_blocks as u64) * (statfs.f_bsize as u64);
        let available = (statfs.f_bavail as u64) * (statfs.f_bsize as u64);
        let used = total.saturating_sub(available);
        let usage_percent = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        Ok(DiskMetrics {
            total_bytes: total,
            used_bytes: used,
            available_bytes: available,
            usage_percent,
        })
    }

    async fn collect_network() -> anyhow::Result<NetworkMetrics> {
        let net_dev = tokio::fs::read_to_string("/proc/net/dev").await?;

        let mut rx_bytes: u64 = 0;
        let mut tx_bytes: u64 = 0;

        for line in net_dev.lines().skip(2) {
            if let Some(colon_pos) = line.find(':') {
                let iface = &line[..colon_pos];
                // Skip loopback
                if iface.trim() == "lo" {
                    continue;
                }

                let values: Vec<&str> = line[colon_pos + 1..].split_whitespace().collect();
                if values.len() >= 8 {
                    rx_bytes += values[0].parse::<u64>().unwrap_or(0);
                    tx_bytes += values[8].parse::<u64>().unwrap_or(0);
                }
            }
        }

        Ok(NetworkMetrics { rx_bytes, tx_bytes })
    }
}
