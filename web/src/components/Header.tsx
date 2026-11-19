import { Bell, Search, User } from 'lucide-react';
import { useAppStore } from '../hooks/useAppStore';

export function Header() {
  const { systemStatus } = useAppStore();

  return (
    <header className="h-16 bg-white border-b border-gray-200 flex items-center justify-between px-6">
      {/* Search */}
      <div className="flex items-center gap-4 flex-1">
        <div className="relative max-w-md flex-1">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
          <input
            type="text"
            placeholder="搜索项目、任务、日志..."
            className="w-full pl-10 pr-4 py-2 text-sm bg-gray-50 border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500 focus:border-transparent"
          />
        </div>
      </div>

      {/* Right side */}
      <div className="flex items-center gap-4">
        {/* Status indicator */}
        <div className="flex items-center gap-2 text-sm">
          <span
            className={`w-2 h-2 rounded-full ${
              systemStatus?.database_connected
                ? 'bg-green-500'
                : 'bg-red-500'
            }`}
          />
          <span className="text-gray-500">
            {systemStatus?.database_connected ? '已连接' : '未连接'}
          </span>
        </div>

        {/* Notifications */}
        <button className="relative p-2 text-gray-400 hover:text-gray-600 transition-colors">
          <Bell className="w-5 h-5" />
          <span className="absolute top-1 right-1 w-2 h-2 bg-red-500 rounded-full" />
        </button>

        {/* User */}
        <button className="flex items-center gap-2 p-1.5 text-gray-400 hover:text-gray-600 transition-colors">
          <User className="w-5 h-5" />
        </button>
      </div>
    </header>
  );
}
