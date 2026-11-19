import { useCallback, useEffect, useState } from 'react';
import { Card, CardContent } from '../components/Card';
import {
  Search,
  Clock,
  CheckCircle2,
  XCircle,
  AlertCircle,
  PlayCircle,
} from 'lucide-react';
import { useAppStore } from '../hooks/useAppStore';
import api from '../services/api';
import type { TaskStatus } from '../types';
import { clsx } from 'clsx';
import { useEventStream } from '../hooks/useEventStream';

const statusColumns: { status: TaskStatus; label: string; color: string }[] = [
  { status: 'pending', label: '待处理', color: 'border-gray-300' },
  { status: 'ready', label: '就绪', color: 'border-blue-300' },
  { status: 'running', label: '进行中', color: 'border-yellow-300' },
  { status: 'completed', label: '已完成', color: 'border-green-300' },
  { status: 'failed', label: '失败', color: 'border-red-300' },
];

export function TasksPage() {
  const { tasks, setTasks, loading, setLoading, error, setError } = useAppStore();
  const [searchQuery, setSearchQuery] = useState('');
  const [filterProject, setFilterProject] = useState<string>('');

  // Suppress unused variable warning
  void setFilterProject;

  const fetchTasks = useCallback(async () => {
      setLoading(true);
      try {
        const data = await api.getTasks(filterProject || undefined);
        setTasks(data);
      } catch (err) {
        setError('Failed to fetch tasks');
        console.error(err);
      } finally {
        setLoading(false);
      }
  }, [filterProject, setTasks, setLoading, setError]);

  useEffect(() => {
    fetchTasks();
  }, [fetchTasks]);

  const onRealtimeEvent = useCallback((event: { type: string }) => {
    if (event.type.startsWith('Task')) void fetchTasks();
  }, [fetchTasks]);
  useEventStream(onRealtimeEvent);

  const filteredTasks = tasks.filter(
    (t) =>
      t.title.toLowerCase().includes(searchQuery.toLowerCase()) ||
      t.description.toLowerCase().includes(searchQuery.toLowerCase())
  );

  const getStatusIcon = (status: TaskStatus) => {
    switch (status) {
      case 'pending':
        return <Clock className="w-4 h-4 text-gray-400" />;
      case 'ready':
        return <AlertCircle className="w-4 h-4 text-blue-400" />;
      case 'running':
        return <PlayCircle className="w-4 h-4 text-yellow-400 animate-pulse" />;
      case 'completed':
        return <CheckCircle2 className="w-4 h-4 text-green-400" />;
      case 'failed':
        return <XCircle className="w-4 h-4 text-red-400" />;
      default:
        return <Clock className="w-4 h-4 text-gray-400" />;
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary-500" />
      </div>
    );
  }

  return (
    <div className="space-y-6 animate-fadeIn">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gray-900">任务</h1>
          <p className="text-gray-500 mt-1">查看和管理所有任务</p>
        </div>
      </div>

      {/* Filters */}
      <div className="flex items-center gap-4">
        <div className="relative flex-1 max-w-md">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
          <input
            type="text"
            placeholder="搜索任务..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full pl-10 pr-4 py-2 text-sm bg-white border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500"
          />
        </div>
      </div>

      {error && (
        <div className="p-4 bg-red-50 text-red-600 rounded-lg">{error}</div>
      )}

      {/* Kanban Board */}
      <div className="flex gap-4 overflow-x-auto pb-4">
        {statusColumns.map((column) => {
          const columnTasks = filteredTasks.filter(
            (t) => t.status === column.status
          );

          return (
            <div
              key={column.status}
              className="flex-shrink-0 w-72 bg-gray-100 rounded-lg p-3"
            >
              {/* Column Header */}
              <div className="flex items-center justify-between mb-3">
                <div className="flex items-center gap-2">
                  <div
                    className={clsx('w-2 h-2 rounded-full', {
                      'bg-gray-400': column.status === 'pending',
                      'bg-blue-400': column.status === 'ready',
                      'bg-yellow-400': column.status === 'running',
                      'bg-green-400': column.status === 'completed',
                      'bg-red-400': column.status === 'failed',
                    })}
                  />
                  <span className="font-medium text-sm text-gray-700">
                    {column.label}
                  </span>
                  <span className="text-xs text-gray-400">
                    ({columnTasks.length})
                  </span>
                </div>
              </div>

              {/* Tasks */}
              <div className="space-y-2">
                {columnTasks.map((task) => (
                  <Card key={task.id} padding="sm" className="hover:shadow-md">
                    <CardContent className="p-3">
                      <div className="flex items-start justify-between gap-2">
                        <div className="flex items-center gap-2">
                          {getStatusIcon(task.status)}
                          <span className="text-xs px-1.5 py-0.5 rounded bg-gray-100 text-gray-500">
                            {task.priority}
                          </span>
                        </div>
                      </div>
                      <h4 className="mt-2 text-sm font-medium text-gray-900">
                        {task.title}
                      </h4>
                      {task.description && (
                        <p className="mt-1 text-xs text-gray-500 line-clamp-2">
                          {task.description}
                        </p>
                      )}
                      {task.result && (
                        <div className="mt-2 pt-2 border-t border-gray-100">
                          <span className="text-xs text-gray-400">
                            耗时: {(task.result.duration_ms / 1000).toFixed(1)}s
                          </span>
                        </div>
                      )}
                    </CardContent>
                  </Card>
                ))}

                {columnTasks.length === 0 && (
                  <div className="text-center py-8 text-gray-400 text-sm">
                    暂无任务
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
