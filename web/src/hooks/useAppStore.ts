import { create } from 'zustand';
import type { Project, Task, Deployment, LogEntry, SystemStatus, LlmUsage, Config } from '../types';

interface AppState {
  // UI State
  sidebarCollapsed: boolean;
  toggleSidebar: () => void;

  // Data
  projects: Project[];
  setProjects: (projects: Project[]) => void;
  addProject: (project: Project) => void;
  updateProject: (id: string, updates: Partial<Project>) => void;
  removeProject: (id: string) => void;

  tasks: Task[];
  setTasks: (tasks: Task[]) => void;
  addTask: (task: Task) => void;
  updateTask: (id: string, updates: Partial<Task>) => void;

  deployments: Deployment[];
  setDeployments: (deployments: Deployment[]) => void;
  addDeployment: (deployment: Deployment) => void;

  logs: LogEntry[];
  setLogs: (logs: LogEntry[]) => void;
  appendLog: (log: LogEntry) => void;

  llmUsage: LlmUsage[];
  setLlmUsage: (usage: LlmUsage[]) => void;

  config: Config | null;
  setConfig: (config: Config) => void;

  systemStatus: SystemStatus | null;
  setSystemStatus: (status: SystemStatus | null) => void;

  // Selected items
  selectedProjectId: string | null;
  setSelectedProjectId: (id: string | null) => void;

  // Loading states
  loading: boolean;
  setLoading: (loading: boolean) => void;

  // Error state
  error: string | null;
  setError: (error: string | null) => void;
}

export const useAppStore = create<AppState>((set) => ({
  // UI State
  sidebarCollapsed: false,
  toggleSidebar: () => set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),

  // Data
  projects: [],
  setProjects: (projects) => set({ projects }),
  addProject: (project) => set((state) => ({ projects: [...state.projects, project] })),
  updateProject: (id, updates) => set((state) => ({
    projects: state.projects.map((p) => p.id === id ? { ...p, ...updates } : p),
  })),
  removeProject: (id) => set((state) => ({
    projects: state.projects.filter((p) => p.id !== id),
  })),

  tasks: [],
  setTasks: (tasks) => set({ tasks }),
  addTask: (task) => set((state) => ({ tasks: [...state.tasks, task] })),
  updateTask: (id, updates) => set((state) => ({
    tasks: state.tasks.map((t) => t.id === id ? { ...t, ...updates } : t),
  })),

  deployments: [],
  setDeployments: (deployments) => set({ deployments }),
  addDeployment: (deployment) => set((state) => ({
    deployments: [...state.deployments, deployment],
  })),

  logs: [],
  setLogs: (logs) => set({ logs }),
  appendLog: (log) => set((state) => ({
    logs: [...state.logs.slice(-999), log], // Keep last 1000 logs
  })),

  llmUsage: [],
  setLlmUsage: (usage) => set({ llmUsage: usage }),

  config: null,
  setConfig: (config) => set({ config }),

  systemStatus: null,
  setSystemStatus: (status) => set({ systemStatus: status }),

  // Selected items
  selectedProjectId: null,
  setSelectedProjectId: (id) => set({ selectedProjectId: id }),

  // Loading states
  loading: false,
  setLoading: (loading) => set({ loading }),

  // Error state
  error: null,
  setError: (error) => set({ error }),
}));
