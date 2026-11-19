import { useCallback, useEffect, useState } from 'react';
import { Card, CardContent } from '../components/Card';
import { Button } from '../components/Button';
import {
  Rocket,
  Search,
  RefreshCw,
  CheckCircle2,
  XCircle,
  Clock,
  PlayCircle,
} from 'lucide-react';
import { useAppStore } from '../hooks/useAppStore';
import api from '../services/api';
import type { DeploymentStatus, DeploymentEnvironment } from '../types';
import { clsx } from 'clsx';
import { format } from 'date-fns';
import { useEventStream } from '../hooks/useEventStream';

const envColors: Record<DeploymentEnvironment, string> = {
  development: 'bg-blue-100 text-blue-600',
  staging: 'bg-yellow-100 text-yellow-600',
  production: 'bg-red-100 text-red-600',
};

const statusColors: Record<DeploymentStatus, string> = {
  pending: 'bg-gray-100 text-gray-600',
  building: 'bg-purple-100 text-purple-600',
  deploying: 'bg-yellow-100 text-yellow-600',
  success: 'bg-green-100 text-green-600',
  failed: 'bg-red-100 text-red-600',
  rolled_back: 'bg-orange-100 text-orange-600',
};

export function DeploymentsPage() {
  const { deployments, setDeployments, loading, setLoading, error, setError } =
    useAppStore();
  const [searchQuery, setSearchQuery] = useState('');
  const [filterEnv, setFilterEnv] = useState<string>('');

  const fetchDeployments = useCallback(async () => {
    setLoading(true);
    try {
      const data = await api.getDeployments(filterEnv || undefined);
      setDeployments(data);
    } catch (err) {
      setError('Failed to fetch deployments');
      console.error(err);
    } finally {
      setLoading(false);
    }
  }, [filterEnv, setDeployments, setError, setLoading]);

  useEffect(() => {
    fetchDeployments();
  }, [fetchDeployments]);

  const onRealtimeEvent = useCallback((event: { type: string }) => {
    if (event.type.startsWith('Deploy') || event.type.startsWith('HealthCheck')) {
      void fetchDeployments();
    }
  }, [fetchDeployments]);
  useEventStream(onRealtimeEvent);

  const filteredDeployments = deployments.filter(
    (d) =>
      d.version.toLowerCase().includes(searchQuery.toLowerCase()) ||
      (d.commit_sha && d.commit_sha.toLowerCase().includes(searchQuery.toLowerCase()))
  );

  const getStatusIcon = (status: DeploymentStatus) => {
    switch (status) {
      case 'success':
        return <CheckCircle2 className="w-4 h-4 text-green-500" />;
      case 'failed':
        return <XCircle className="w-4 h-4 text-red-500" />;
      case 'deploying':
        return <PlayCircle className="w-4 h-4 text-yellow-500 animate-pulse" />;
      case 'building':
        return <RefreshCw className="w-4 h-4 text-purple-500 animate-spin" />;
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
          <h1 className="text-2xl font-bold text-gray-900">部署</h1>
          <p className="text-gray-500 mt-1">查看部署历史和状态</p>
        </div>
        <Button variant="primary" onClick={fetchDeployments}>
          <RefreshCw className="w-4 h-4 mr-2" />
          刷新
        </Button>
      </div>

      {/* Filters */}
      <div className="flex items-center gap-4">
        <div className="relative flex-1 max-w-md">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
          <input
            type="text"
            placeholder="搜索部署版本或提交..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full pl-10 pr-4 py-2 text-sm bg-white border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500"
          />
        </div>
        <div className="flex gap-2">
          {(['development', 'staging', 'production'] as const).map((env) => (
            <Button
              key={env}
              variant={filterEnv === env ? 'primary' : 'secondary'}
              size="sm"
              onClick={() => setFilterEnv(filterEnv === env ? '' : env)}
            >
              {env}
            </Button>
          ))}
        </div>
      </div>

      {error && (
        <div className="p-4 bg-red-50 text-red-600 rounded-lg">{error}</div>
      )}

      {/* Deployments List */}
      {filteredDeployments.length === 0 ? (
        <Card>
          <CardContent className="py-12 text-center">
            <Rocket className="w-12 h-12 text-gray-300 mx-auto mb-4" />
            <p className="text-gray-500">暂无部署记录</p>
          </CardContent>
        </Card>
      ) : (
        <Card padding="none">
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead className="bg-gray-50">
                <tr>
                  <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                    状态
                  </th>
                  <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                    环境
                  </th>
                  <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                    版本
                  </th>
                  <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                    提交
                  </th>
                  <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                    时间
                  </th>
                  <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                    健康检查
                  </th>
                  <th className="px-5 py-3 text-right text-xs font-medium text-gray-500 uppercase">
                    操作
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-100">
                {filteredDeployments.map((deployment) => (
                  <tr key={deployment.id} className="hover:bg-gray-50">
                    <td className="px-5 py-4">
                      <div className="flex items-center gap-2">
                        {getStatusIcon(deployment.status)}
                        <span
                          className={clsx(
                            'text-xs px-2 py-1 rounded-full',
                            statusColors[deployment.status]
                          )}
                        >
                          {deployment.status}
                        </span>
                      </div>
                    </td>
                    <td className="px-5 py-4">
                      <span
                        className={clsx(
                          'text-xs px-2 py-1 rounded-full',
                          envColors[deployment.environment]
                        )}
                      >
                        {deployment.environment}
                      </span>
                    </td>
                    <td className="px-5 py-4 text-sm text-gray-900">
                      {deployment.version}
                    </td>
                    <td className="px-5 py-4">
                      <code className="text-xs bg-gray-100 px-2 py-1 rounded">
                        {(deployment.commit_sha || '').slice(0, 7)}
                      </code>
                    </td>
                    <td className="px-5 py-4 text-sm text-gray-500">
                      {format(new Date(deployment.created_at), 'yyyy-MM-dd HH:mm')}
                    </td>
                    <td className="px-5 py-4">
                      {deployment.health_check_passed ? (
                        <CheckCircle2 className="w-4 h-4 text-green-500" />
                      ) : (
                        <XCircle className="w-4 h-4 text-red-500" />
                      )}
                    </td>
                    <td className="px-5 py-4 text-right text-xs text-gray-400">
                      只读
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}
    </div>
  );
}
