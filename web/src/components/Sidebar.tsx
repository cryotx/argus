import { clsx } from 'clsx';
import {
  LayoutDashboard,
  FolderKanban,
  ListTodo,
  Rocket,
  Terminal,
  Settings,
  Activity,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import { useAppStore } from '../hooks/useAppStore';

type Page = 'dashboard' | 'projects' | 'tasks' | 'deployments' | 'logs' | 'llm-usage' | 'config';

const navItems: { page: Page; icon: typeof LayoutDashboard; label: string }[] = [
  { page: 'dashboard', icon: LayoutDashboard, label: '仪表盘' },
  { page: 'projects', icon: FolderKanban, label: '项目' },
  { page: 'tasks', icon: ListTodo, label: '任务' },
  { page: 'deployments', icon: Rocket, label: '部署' },
  { page: 'logs', icon: Terminal, label: '日志' },
  { page: 'llm-usage', icon: Activity, label: 'LLM 使用' },
  { page: 'config', icon: Settings, label: '配置' },
];

interface SidebarProps {
  currentPage: Page;
  onNavigate: (page: Page) => void;
}

export function Sidebar({ currentPage, onNavigate }: SidebarProps) {
  const { sidebarCollapsed, toggleSidebar } = useAppStore();

  return (
    <aside
      className={clsx(
        'fixed left-0 top-0 h-screen bg-white border-r border-gray-200 transition-all duration-300 z-40',
        sidebarCollapsed ? 'w-16' : 'w-56'
      )}
    >
      <div className="flex flex-col h-full">
        {/* Logo */}
        <div className="flex items-center h-16 px-4 border-b border-gray-100">
          <div className="flex items-center gap-3">
            <div className="w-8 h-8 rounded-lg bg-gradient-to-br from-primary-500 to-primary-700 flex items-center justify-center">
              <span className="text-white font-bold text-sm">A</span>
            </div>
            {!sidebarCollapsed && (
              <span className="font-bold text-gray-900">Argus</span>
            )}
          </div>
        </div>

        {/* Navigation */}
        <nav className="flex-1 py-4 px-2 space-y-1">
          {navItems.map((item) => (
            <button
              key={item.page}
              onClick={() => onNavigate(item.page)}
              className={clsx(
                'w-full flex items-center gap-3 px-3 py-2.5 rounded-lg transition-colors',
                'hover:bg-gray-50',
                currentPage === item.page
                  ? 'bg-primary-50 text-primary-700'
                  : 'text-gray-600'
              )}
            >
              <item.icon className="w-5 h-5 flex-shrink-0" />
              {!sidebarCollapsed && (
                <span className="text-sm font-medium">{item.label}</span>
              )}
            </button>
          ))}
        </nav>

        {/* Collapse button */}
        <div className="p-2 border-t border-gray-100">
          <button
            onClick={toggleSidebar}
            className="w-full flex items-center justify-center p-2 rounded-lg text-gray-400 hover:text-gray-600 hover:bg-gray-50 transition-colors"
          >
            {sidebarCollapsed ? (
              <ChevronRight className="w-5 h-5" />
            ) : (
              <ChevronLeft className="w-5 h-5" />
            )}
          </button>
        </div>
      </div>
    </aside>
  );
}
