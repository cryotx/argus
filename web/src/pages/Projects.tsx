import { useEffect, useState } from 'react';
import { Card, CardContent } from '../components/Card';
import { Button } from '../components/Button';
import {
  FolderKanban,
  Plus,
  Search,
  Calendar,
} from 'lucide-react';
import { useAppStore } from '../hooks/useAppStore';
import api from '../services/api';
import type { ProjectStatus } from '../types';

export function ProjectsPage() {
  const { projects, setProjects, loading, setLoading, error, setError } =
    useAppStore();
  const [searchQuery, setSearchQuery] = useState('');
  const [showCreateModal, setShowCreateModal] = useState(false);

  useEffect(() => {
    const fetchProjects = async () => {
      setLoading(true);
      try {
        const data = await api.getProjects();
        setProjects(data);
      } catch (err) {
        setError('Failed to fetch projects');
        console.error(err);
      } finally {
        setLoading(false);
      }
    };

    fetchProjects();
  }, [setProjects, setLoading, setError]);

  const filteredProjects = projects.filter(
    (p) =>
      p.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
      p.description.toLowerCase().includes(searchQuery.toLowerCase())
  );

  const statusColors: Record<ProjectStatus, string> = {
    active: 'bg-green-50 text-green-600',
    inactive: 'bg-red-50 text-red-600',
    paused: 'bg-yellow-50 text-yellow-600',
    completed: 'bg-blue-50 text-blue-600',
    archived: 'bg-gray-50 text-gray-600',
  };

  const formatDate = (dateStr: string) => {
    return new Date(dateStr).toLocaleDateString('zh-CN', {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    });
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
          <h1 className="text-2xl font-bold text-gray-900">项目</h1>
          <p className="text-gray-500 mt-1">管理您的所有项目</p>
        </div>
        <Button variant="primary" onClick={() => setShowCreateModal(true)}>
          <Plus className="w-4 h-4 mr-2" />
          新建项目
        </Button>
      </div>

      {/* Search */}
      <div className="relative max-w-md">
        <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
        <input
          type="text"
          placeholder="搜索项目..."
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          className="w-full pl-10 pr-4 py-2 text-sm bg-white border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500 focus:border-transparent"
        />
      </div>

      {/* Projects Grid */}
      {error && (
        <div className="p-4 bg-red-50 text-red-600 rounded-lg">{error}</div>
      )}

      {filteredProjects.length === 0 ? (
        <Card>
          <CardContent className="py-12 text-center">
            <FolderKanban className="w-12 h-12 text-gray-300 mx-auto mb-4" />
            <p className="text-gray-500">暂无项目</p>
            <Button
              variant="secondary"
              className="mt-4"
              onClick={() => setShowCreateModal(true)}
            >
              创建第一个项目
            </Button>
          </CardContent>
        </Card>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {filteredProjects.map((project) => (
            <Card key={project.id} className="hover:shadow-md transition-shadow">
              <CardContent className="p-5">
                <div className="flex items-start justify-between">
                  <div className="flex items-center gap-3">
                    <div className="w-10 h-10 rounded-lg bg-primary-50 flex items-center justify-center">
                      <FolderKanban className="w-5 h-5 text-primary-600" />
                    </div>
                    <div>
                      <h3 className="font-semibold text-gray-900">
                        {project.name}
                      </h3>
                      <span
                        className={`text-xs px-2 py-0.5 rounded-full ${statusColors[project.status]}`}
                      >
                        {project.status}
                      </span>
                    </div>
                  </div>
                </div>

                <p className="mt-3 text-sm text-gray-500 line-clamp-2">
                  {project.description || '暂无描述'}
                </p>

                <div className="mt-4 flex items-center gap-4 text-xs text-gray-400">
                  <span className="flex items-center gap-1">
                    <Calendar className="w-3 h-3" />
                    {formatDate(project.created_at)}
                  </span>
                </div>

                <div className="mt-4 text-xs text-gray-400">项目操作暂未开放</div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      {/* Create Modal Placeholder */}
      {showCreateModal && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
          <Card className="w-full max-w-md">
            <CardContent className="p-6">
              <h2 className="text-lg font-semibold mb-4">新建项目</h2>
              <p className="text-gray-500 text-sm mb-4">
                项目创建功能将通过对话界面完成。请在聊天中输入您的需求。
              </p>
              <div className="flex justify-end">
                <Button variant="secondary" onClick={() => setShowCreateModal(false)}>
                  关闭
                </Button>
              </div>
            </CardContent>
          </Card>
        </div>
      )}
    </div>
  );
}
