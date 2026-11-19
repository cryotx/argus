import axios, { AxiosInstance } from 'axios';
import type {
  Project,
  Task,
  Deployment,
  LogEntry,
  LlmUsage,
  SystemStatus,
  Config,
} from '../types';
import { API_OPERATIONS } from './openapi.generated';

const API_BASE = '/api';

class ApiService {
  private client: AxiosInstance;

  constructor() {
    this.client = axios.create({
      baseURL: API_BASE,
      timeout: 30000,
      headers: {
        'Content-Type': 'application/json',
      },
    });
  }

  // Chat
  async sendChat(sessionId: string, userId: string, text: string): Promise<{ reply: string }> {
    const response = await this.client.post('/chat', { session_id: sessionId, user_id: userId, text });
    return response.data;
  }

  // Projects
  async getProjects(): Promise<Project[]> {
    const response = await this.client.get(API_OPERATIONS.listProjects.path);
    return response.data;
  }

  async createProject(project: Partial<Project>): Promise<Project> {
    const response = await this.client.post(API_OPERATIONS.createProject.path, project);
    return response.data;
  }

  // Tasks
  async getTasks(projectId?: string): Promise<Task[]> {
    const params: Record<string, string> = {};
    if (projectId) params.project_id = projectId;
    const response = await this.client.get(API_OPERATIONS.listTasks.path, { params });
    return response.data;
  }

  async getTask(id: string): Promise<Task> {
    const response = await this.client.get(
      API_OPERATIONS.getTask.path.replace('{task_id}', id),
    );
    return response.data;
  }

  async createTask(task: Partial<Task>): Promise<Task> {
    const response = await this.client.post(API_OPERATIONS.createTask.path, task);
    return response.data;
  }

  // Deployments
  async getDeployments(projectId?: string): Promise<Deployment[]> {
    const params = projectId ? { project_id: projectId } : {};
    const response = await this.client.get(API_OPERATIONS.listDeployments.path, { params });
    return response.data;
  }

  // Logs
  async getLogs(options?: {
    level?: string;
    source?: string;
    limit?: number;
    offset?: number;
  }): Promise<LogEntry[]> {
    const response = await this.client.get('/logs', { params: options });
    return response.data;
  }

  // LLM Usage
  async getLlmUsage(options?: {
    project_id?: string;
    start_date?: string;
    end_date?: string;
  }): Promise<LlmUsage[]> {
    const response = await this.client.get('/llm-usage', { params: options });
    return response.data;
  }

  // System
  async getSystemStatus(): Promise<SystemStatus> {
    const response = await this.client.get('/status');
    return response.data;
  }

  // Config
  async getConfig(): Promise<Config> {
    const response = await this.client.get('/config');
    return response.data;
  }

  async updateConfig(config: Config): Promise<Config> {
    const response = await this.client.put('/config', config);
    return response.data;
  }
}

export const api = new ApiService();
export default api;
