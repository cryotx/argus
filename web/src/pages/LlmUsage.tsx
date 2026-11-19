import { useCallback, useEffect, useState } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '../components/Card';
import { Button } from '../components/Button';
import {
  Activity,
  Search,
  TrendingUp,
  DollarSign,
  Clock,
} from 'lucide-react';
import { useAppStore } from '../hooks/useAppStore';
import api from '../services/api';
import { format } from 'date-fns';

export function LlmUsagePage() {
  const { llmUsage, setLlmUsage, loading, setLoading, error, setError } =
    useAppStore();
  const [searchQuery, setSearchQuery] = useState('');
  const [dateRange, setDateRange] = useState<string>('');

  const fetchUsage = useCallback(async () => {
    setLoading(true);
    try {
      const data = await api.getLlmUsage(
        dateRange ? { start_date: dateRange } : undefined
      );
      setLlmUsage(data);
    } catch (err) {
      setError('Failed to fetch LLM usage');
      console.error(err);
    } finally {
      setLoading(false);
    }
  }, [dateRange, setError, setLoading, setLlmUsage]);

  useEffect(() => {
    fetchUsage();
  }, [fetchUsage]);

  const filteredUsage = llmUsage.filter(
    (u) =>
      u.provider.toLowerCase().includes(searchQuery.toLowerCase()) ||
      u.model.toLowerCase().includes(searchQuery.toLowerCase())
  );

  // Calculate totals
  const totalCost = filteredUsage.reduce((sum, u) => sum + u.cost_usd, 0);
  const totalInputTokens = filteredUsage.reduce((sum, u) => sum + u.input_tokens, 0);
  const totalOutputTokens = filteredUsage.reduce((sum, u) => sum + u.output_tokens, 0);
  const totalTokens = totalInputTokens + totalOutputTokens;

  // Group by provider
  const byProvider = filteredUsage.reduce(
    (acc, u) => {
      if (!acc[u.provider]) {
        acc[u.provider] = { cost: 0, tokens: 0 };
      }
      acc[u.provider].cost += u.cost_usd;
      acc[u.provider].tokens += u.input_tokens + u.output_tokens;
      return acc;
    },
    {} as Record<string, { cost: number; tokens: number }>
  );

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
          <h1 className="text-2xl font-bold text-gray-900">LLM 使用</h1>
          <p className="text-gray-500 mt-1">追踪 LLM 调用和成本</p>
        </div>
        <Button variant="primary" onClick={fetchUsage}>
          <Activity className="w-4 h-4 mr-2" />
          刷新
        </Button>
      </div>

      {/* Filters */}
      <div className="flex items-center gap-4">
        <div className="relative flex-1 max-w-md">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
          <input
            type="text"
            placeholder="搜索提供商或模型..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full pl-10 pr-4 py-2 text-sm bg-white border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500"
          />
        </div>
        <select
          value={dateRange}
          onChange={(e) => setDateRange(e.target.value)}
          className="px-3 py-2 text-sm bg-white border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-primary-500"
        >
          <option value="">所有时间</option>
          <option value="today">今天</option>
          <option value="7days">最近 7 天</option>
          <option value="30days">最近 30 天</option>
        </select>
      </div>

      {error && (
        <div className="p-4 bg-red-50 text-red-600 rounded-lg">{error}</div>
      )}

      {/* Stats */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <Card padding="md">
          <CardContent className="flex items-center gap-4">
            <div className="p-3 rounded-lg bg-green-50">
              <DollarSign className="w-6 h-6 text-green-600" />
            </div>
            <div>
              <p className="text-2xl font-bold text-gray-900">
                ${totalCost.toFixed(4)}
              </p>
              <p className="text-sm text-gray-500">总成本</p>
            </div>
          </CardContent>
        </Card>

        <Card padding="md">
          <CardContent className="flex items-center gap-4">
            <div className="p-3 rounded-lg bg-blue-50">
              <TrendingUp className="w-6 h-6 text-blue-600" />
            </div>
            <div>
              <p className="text-2xl font-bold text-gray-900">
                {totalTokens.toLocaleString()}
              </p>
              <p className="text-sm text-gray-500">总 Token</p>
            </div>
          </CardContent>
        </Card>

        <Card padding="md">
          <CardContent className="flex items-center gap-4">
            <div className="p-3 rounded-lg bg-purple-50">
              <Activity className="w-6 h-6 text-purple-600" />
            </div>
            <div>
              <p className="text-2xl font-bold text-gray-900">
                {filteredUsage.length}
              </p>
              <p className="text-sm text-gray-500">调用次数</p>
            </div>
          </CardContent>
        </Card>

        <Card padding="md">
          <CardContent className="flex items-center gap-4">
            <div className="p-3 rounded-lg bg-yellow-50">
              <Clock className="w-6 h-6 text-yellow-600" />
            </div>
            <div>
              <p className="text-2xl font-bold text-gray-900">
                ${totalTokens > 0 ? (totalCost / totalTokens).toFixed(6) : '0'}
              </p>
              <p className="text-sm text-gray-500">每 Token 成本</p>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Provider Breakdown */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <Card>
          <CardHeader>
            <CardTitle>按提供商统计</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="space-y-4">
              {Object.entries(byProvider).map(([provider, stats]) => (
                <div key={provider} className="flex items-center justify-between">
                  <div>
                    <p className="font-medium text-gray-900">{provider}</p>
                    <p className="text-sm text-gray-500">
                      {stats.tokens.toLocaleString()} tokens
                    </p>
                  </div>
                  <div className="text-right">
                    <p className="font-semibold text-gray-900">
                      ${stats.cost.toFixed(4)}
                    </p>
                    <p className="text-sm text-gray-500">
                      {((stats.cost / totalCost) * 100).toFixed(1)}%
                    </p>
                  </div>
                </div>
              ))}
              {Object.keys(byProvider).length === 0 && (
                <p className="text-center text-gray-400">暂无数据</p>
              )}
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>使用趋势</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-center text-gray-400 py-8">
              图表功能开发中...
            </p>
          </CardContent>
        </Card>
      </div>

      {/* Usage List */}
      <Card padding="none">
        <CardHeader className="border-b">
          <CardTitle>使用详情</CardTitle>
        </CardHeader>
        <div className="overflow-x-auto">
          <table className="w-full">
            <thead className="bg-gray-50">
              <tr>
                <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  时间
                </th>
                <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  提供商
                </th>
                <th className="px-5 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  模型
                </th>
                <th className="px-5 py-3 text-right text-xs font-medium text-gray-500 uppercase">
                  输入
                </th>
                <th className="px-5 py-3 text-right text-xs font-medium text-gray-500 uppercase">
                  输出
                </th>
                <th className="px-5 py-3 text-right text-xs font-medium text-gray-500 uppercase">
                  成本
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-100">
              {filteredUsage.slice(0, 50).map((usage) => (
                <tr key={usage.id} className="hover:bg-gray-50">
                  <td className="px-5 py-4 text-sm text-gray-500">
                    {format(new Date(usage.timestamp), 'MM-dd HH:mm')}
                  </td>
                  <td className="px-5 py-4 text-sm text-gray-900">
                    {usage.provider}
                  </td>
                  <td className="px-5 py-4 text-sm text-gray-600">
                    <code className="text-xs bg-gray-100 px-2 py-1 rounded">
                      {usage.model}
                    </code>
                  </td>
                  <td className="px-5 py-4 text-sm text-gray-600 text-right">
                    {usage.input_tokens.toLocaleString()}
                  </td>
                  <td className="px-5 py-4 text-sm text-gray-600 text-right">
                    {usage.output_tokens.toLocaleString()}
                  </td>
                  <td className="px-5 py-4 text-sm text-gray-900 text-right font-medium">
                    ${usage.cost_usd.toFixed(4)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}
