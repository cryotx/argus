import { useCallback, useEffect, useRef, useState } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '../components/Card';
import { Button } from '../components/Button';
import {
  Terminal,
  Search,
  RefreshCw,
  Download,
} from 'lucide-react';
import { useAppStore } from '../hooks/useAppStore';
import api from '../services/api';
import type { LogLevel } from '../types';
import { clsx } from 'clsx';
import { format } from 'date-fns';

const levelColors: Record<LogLevel, string> = {
  trace: 'text-gray-400',
  debug: 'text-blue-400',
  info: 'text-green-400',
  warn: 'text-yellow-400',
  error: 'text-red-400',
};

const levelBgColors: Record<LogLevel, string> = {
  trace: 'bg-gray-50',
  debug: 'bg-blue-50',
  info: 'bg-green-50',
  warn: 'bg-yellow-50',
  error: 'bg-red-50',
};

export function LogsPage() {
  const { logs, setLogs, setLoading, error, setError } =
    useAppStore();
  const [searchQuery, setSearchQuery] = useState('');
  const [filterLevel, setFilterLevel] = useState<string>('');
  const [filterSource, setFilterSource] = useState<string>('');
  const [autoRefresh, setAutoRefresh] = useState(true);
  const logsEndRef = useRef<HTMLDivElement>(null);

  const fetchLogs = useCallback(async () => {
    try {
      const data = await api.getLogs({
        level: filterLevel || undefined,
        source: filterSource || undefined,
        limit: 500,
      });
      setLogs(data);
    } catch (err) {
      setError('Failed to fetch logs');
      console.error(err);
    }
  }, [filterLevel, filterSource, setError, setLogs]);

  useEffect(() => {
    setLoading(true);
    fetchLogs().finally(() => setLoading(false));
  }, [fetchLogs, setLoading]);

  // Auto-refresh
  useEffect(() => {
    if (!autoRefresh) return;
    const interval = setInterval(fetchLogs, 5000);
    return () => clearInterval(interval);
  }, [autoRefresh, fetchLogs]);

  // Auto-scroll to bottom
  useEffect(() => {
    logsEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [logs]);

  const filteredLogs = logs.filter(
    (log) =>
      log.message.toLowerCase().includes(searchQuery.toLowerCase()) ||
      log.source.toLowerCase().includes(searchQuery.toLowerCase())
  );

  const uniqueSources = [...new Set(logs.map((l) => l.source))];

  const downloadLogs = () => {
    const content = filteredLogs
      .map(
        (log) =>
          `${format(new Date(log.timestamp), 'yyyy-MM-dd HH:mm:ss')} [${log.level.toUpperCase()}] [${log.source}] ${log.message}`
      )
      .join('\n');

    const blob = new Blob([content], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `argus-logs-${format(new Date(), 'yyyy-MM-dd-HH-mm')}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="space-y-6 animate-fadeIn">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gray-900">日志</h1>
          <p className="text-gray-500 mt-1">查看已持久化的操作审计日志</p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant={autoRefresh ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => setAutoRefresh(!autoRefresh)}
          >
            <RefreshCw
              className={clsx('w-4 h-4 mr-2', autoRefresh && 'animate-spin')}
            />
            {autoRefresh ? '实时' : '已暂停'}
          </Button>
          <Button variant="secondary" size="sm" onClick={downloadLogs}>
            <Download className="w-4 h-4 mr-2" />
            导出
          </Button>
          <Button variant="ghost" size="sm" onClick={fetchLogs}>
            <RefreshCw className="w-4 h-4" />
          </Button>
        </div>
      </div>

      {/* Filters */}
      <div className="flex items-center gap-4 flex-wrap">
        <div className="relative flex-1 max-w-md">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
          <input
            type="text"
            placeholder="搜索日志内容..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full pl-10 pr-4 py-2 text-sm bg-white border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500"
          />
        </div>
        <select
          value={filterLevel}
          onChange={(e) => setFilterLevel(e.target.value)}
          className="px-3 py-2 text-sm bg-white border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500"
        >
          <option value="">所有级别</option>
          <option value="error">Error</option>
          <option value="warn">Warn</option>
          <option value="info">Info</option>
          <option value="debug">Debug</option>
          <option value="trace">Trace</option>
        </select>
        <select
          value={filterSource}
          onChange={(e) => setFilterSource(e.target.value)}
          className="px-3 py-2 text-sm bg-white border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500"
        >
          <option value="">所有来源</option>
          {uniqueSources.map((source) => (
            <option key={source} value={source}>
              {source}
            </option>
          ))}
        </select>
      </div>

      {error && (
        <div className="p-4 bg-red-50 text-red-600 rounded-lg">{error}</div>
      )}

      {/* Logs Terminal */}
      <Card padding="none" className="bg-gray-900">
        <CardHeader className="bg-gray-800 border-b border-gray-700">
          <CardTitle className="text-gray-100 flex items-center gap-2">
            <Terminal className="w-4 h-4" />
            日志终端
          </CardTitle>
        </CardHeader>
        <CardContent className="p-0">
          <div className="h-[600px] overflow-y-auto font-mono text-sm">
            {filteredLogs.length === 0 ? (
              <div className="p-8 text-center text-gray-500">暂无日志</div>
            ) : (
              <div className="p-4 space-y-1">
                {filteredLogs.map((log) => (
                  <div
                    key={log.id}
                    className={clsx(
                      'flex gap-3 py-1 rounded',
                      levelBgColors[log.level]
                    )}
                  >
                    <span className="text-gray-400 text-xs whitespace-nowrap">
                      {format(new Date(log.timestamp), 'HH:mm:ss.SSS')}
                    </span>
                    <span
                      className={clsx(
                        'text-xs font-medium uppercase w-12',
                        levelColors[log.level]
                      )}
                    >
                      {log.level}
                    </span>
                    <span className="text-purple-400 text-xs w-24 truncate">
                      [{log.source}]
                    </span>
                    <span
                      className={clsx(
                        'flex-1',
                        levelColors[log.level]
                      )}
                    >
                      {log.message}
                    </span>
                  </div>
                ))}
                <div ref={logsEndRef} />
              </div>
            )}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
