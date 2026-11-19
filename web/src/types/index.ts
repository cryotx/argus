// Core Types for Argus Dashboard

export interface Project {
  id: string;
  name: string;
  description: string;
  git_repo_url?: string;
  language?: string;
  framework?: string;
  status: ProjectStatus;
  priority: string;
  created_at: string;
  updated_at: string;
  owner_id?: string;
}

export type ProjectStatus = 'active' | 'inactive' | 'paused' | 'completed' | 'archived';

export interface Task {
  id: string;
  project_id: string;
  project_name?: string;
  title: string;
  description: string;
  task_type: string;
  status: TaskStatus;
  priority: TaskPriority;
  assignee?: string;
  dependencies: string[];
  created_at: string;
  updated_at: string;
  completed_at?: string;
  result?: TaskResult;
}

export type TaskStatus = 'pending' | 'ready' | 'running' | 'completed' | 'failed' | 'cancelled' | 'spawning' | 'stopping' | 'crashed';
export type TaskPriority = 'low' | 'medium' | 'high' | 'urgent';

export interface TaskResult {
  output: string;
  duration_ms: number;
  exit_code: number;
}

export interface Deployment {
  id: string;
  project_id: string;
  project_name?: string;
  environment: DeploymentEnvironment;
  status: DeploymentStatus;
  version: string;
  commit_sha?: string;
  created_at: string;
  completed_at?: string;
  health_check_passed?: boolean;
}

export type DeploymentEnvironment = 'development' | 'staging' | 'production';
export type DeploymentStatus = 'pending' | 'building' | 'deploying' | 'success' | 'failed' | 'rolled_back';

export interface LogEntry {
  id: string;
  timestamp: string;
  level: LogLevel;
  source: string;
  message: string;
  metadata?: Record<string, unknown>;
}

export type LogLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error';

export interface LlmUsage {
  id: string;
  project_id?: string;
  provider: string;
  model: string;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  timestamp: string;
}

export interface SystemStatus {
  version: string;
  status: string;
  uptime_seconds: number;
  database_connected: boolean;
  workers_active: number;
  workers_total?: number;
  memory_usage_mb?: number;
  cpu_usage_percent?: number;
}

export interface ChatMessage {
  session_id: string;
  user_id: string;
  text: string;
  direction: 'incoming' | 'outgoing';
  timestamp: string;
}

export interface Config {
  database?: Record<string, unknown>;
  server?: Record<string, unknown>;
  worker?: Record<string, unknown>;
  llm?: Record<string, unknown>;
  memory?: Record<string, unknown>;
  notification?: Record<string, unknown>;
  security?: Record<string, unknown>;
  scheduler?: Record<string, unknown>;
  [key: string]: Record<string, unknown> | undefined;
}
