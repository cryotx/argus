import { useEffect } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '../components/Card';
import { Button } from '../components/Button';
import {
  FolderKanban,
  ListTodo,
  Rocket,
  Activity,
  Clock,
  CheckCircle2,
  AlertCircle,
} from 'lucide-react';
import { useAppStore } from '../hooks/useAppStore';
import api from '../services/api';

export function DashboardPage() {
  const {
    projects,
    tasks,
    deployments,
    systemStatus,
    setProjects,
    setTasks,
    setDeployments,
    setSystemStatus,
    setLoading,
    setError,
  } = useAppStore();

  useEffect(() => {
    const fetchData = async () => {
      setLoading(true);
      try {
        const [status, projectsData, tasksData, deploymentsData] =
          await Promise.all([
            api.getSystemStatus().catch(() => null),
            api.getProjects().catch(() => []),
            api.getTasks().catch(() => []),
            api.getDeployments().catch(() => []),
          ]);

        setSystemStatus(status);
        setProjects(projectsData);
        setTasks(tasksData);
        setDeployments(deploymentsData);
      } catch (err) {
        setError('Failed to fetch dashboard data');
        console.error(err);
      } finally {
        setLoading(false);
      }
    };

    fetchData();
  }, [setProjects, setTasks, setDeployments, setSystemStatus, setLoading, setError]);

  const activeProjects = projects.filter((p) => p.status === 'active').length;
  const pendingTasks = tasks.filter((t) => t.status === 'pending' || t.status === 'ready').length;
  const runningTasks = tasks.filter((t) => t.status === 'running').length;
  const recentDeployments = deployments.slice(-5).reverse();

  const stats = [
    {
      label: '活跃项目',
      value: activeProjects,
      icon: FolderKanban,
      color: 'text-blue-500',
      bg: 'bg-blue-50',
    },
    {
      label: '待处理任务',
      value: pendingTasks,
      icon: ListTodo,
      color: 'text-yellow-500',
      bg: 'bg-yellow-50',
    },
    {
      label: '运行中',
      value: runningTasks,
      icon: Activity,
      color: 'text-green-500',
      bg: 'bg-green-50',
    },
    {
      label: '最近部署',
      value: deployments.length,
      icon: Rocket,
      color: 'text-purple-500',
      bg: 'bg-purple-50',
    },
  ];

  return (
    <div className="space-y-6 animate-fadeIn">
      {/* Welcome */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gray-900">仪表盘</h1>
          <p className="text-gray-500 mt-1">欢迎回来，这是 Argus 的运行概览</p>
        </div>
        <Button variant="primary">
          <FolderKanban className="w-4 h-4 mr-2" />
          新建项目
        </Button>
      </div>

      {/* Stats Grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        {stats.map((stat) => (
          <Card key={stat.label} padding="md">
            <CardContent className="flex items-center gap-4">
              <div className={`p-3 rounded-lg ${stat.bg}`}>
                <stat.icon className={`w-6 h-6 ${stat.color}`} />
              </div>
              <div>
                <p className="text-2xl font-bold text-gray-900">{stat.value}</p>
                <p className="text-sm text-gray-500">{stat.label}</p>
              </div>
            </CardContent>
          </Card>
        ))}
      </div>

      {/* Main Content Grid */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Recent Tasks */}
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <ListTodo className="w-5 h-5 text-primary-500" />
              最近任务
            </CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <div className="divide-y divide-gray-100">
              {tasks.slice(0, 5).map((task) => (
                <div
                  key={task.id}
                  className="px-5 py-3 flex items-center justify-between hover:bg-gray-50"
                >
                  <div className="flex items-center gap-3">
                    {task.status === 'completed' ? (
                      <CheckCircle2 className="w-4 h-4 text-green-500" />
                    ) : task.status === 'failed' ? (
                      <AlertCircle className="w-4 h-4 text-red-500" />
                    ) : (
                      <Clock className="w-4 h-4 text-gray-400" />
                    )}
                    <span className="text-sm text-gray-700">{task.title}</span>
                  </div>
                  <span
                    className={`text-xs px-2 py-1 rounded-full ${
                      task.status === 'completed'
                        ? 'bg-green-50 text-green-600'
                        : task.status === 'failed'
                        ? 'bg-red-50 text-red-600'
                        : task.status === 'running'
                        ? 'bg-blue-50 text-blue-600'
                        : 'bg-gray-50 text-gray-600'
                    }`}
                  >
                    {task.status}
                  </span>
                </div>
              ))}
              {tasks.length === 0 && (
                <div className="px-5 py-8 text-center text-gray-400">
                  暂无任务
                </div>
              )}
            </div>
          </CardContent>
        </Card>

        {/* Recent Deployments */}
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Rocket className="w-5 h-5 text-primary-500" />
              最近部署
            </CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <div className="divide-y divide-gray-100">
              {recentDeployments.slice(0, 5).map((deployment) => (
                <div
                  key={deployment.id}
                  className="px-5 py-3 flex items-center justify-between hover:bg-gray-50"
                >
                  <div>
                    <p className="text-sm text-gray-700">
                      {deployment.environment}
                    </p>
                    <p className="text-xs text-gray-400">{deployment.version}</p>
                  </div>
                  <span
                    className={`text-xs px-2 py-1 rounded-full ${
                      deployment.status === 'success'
                        ? 'bg-green-50 text-green-600'
                        : deployment.status === 'failed'
                        ? 'bg-red-50 text-red-600'
                        : deployment.status === 'deploying'
                        ? 'bg-blue-50 text-blue-600'
                        : 'bg-gray-50 text-gray-600'
                    }`}
                  >
                    {deployment.status}
                  </span>
                </div>
              ))}
              {deployments.length === 0 && (
                <div className="px-5 py-8 text-center text-gray-400">
                  暂无部署记录
                </div>
              )}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* System Status */}
      {systemStatus && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Activity className="w-5 h-5 text-primary-500" />
              系统状态
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
              <div className="text-center p-4 bg-gray-50 rounded-lg">
                <p className="text-2xl font-bold text-gray-900">
                  {systemStatus.version}
                </p>
                <p className="text-sm text-gray-500">版本</p>
              </div>
              <div className="text-center p-4 bg-gray-50 rounded-lg">
                <p className="text-2xl font-bold text-gray-900">
                  {Math.floor(systemStatus.uptime_seconds / 3600)}h
                </p>
                <p className="text-sm text-gray-500">运行时长</p>
              </div>
              <div className="text-center p-4 bg-gray-50 rounded-lg">
                <p className="text-2xl font-bold text-gray-900">
                  {systemStatus.workers_active}/{systemStatus.workers_total}
                </p>
                <p className="text-sm text-gray-500">Worker</p>
              </div>
              <div className="text-center p-4 bg-gray-50 rounded-lg">
                <p className="text-2xl font-bold text-gray-900">
                  {systemStatus.memory_usage_mb}MB
                </p>
                <p className="text-sm text-gray-500">内存</p>
              </div>
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
