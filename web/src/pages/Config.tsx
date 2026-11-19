import { useEffect, useState } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '../components/Card';
import api from '../services/api';
import type { Config } from '../types';

export function ConfigPage() {
  const [config, setConfig] = useState<Config | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    api.getConfig().then((value) => { if (active) setConfig(value); })
      .catch(() => { if (active) setError('无法读取运行配置'); });
    return () => { active = false; };
  }, []);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-gray-900">运行配置</h1>
        <p className="mt-2 text-sm text-gray-500">
          当前进程的配置快照，敏感值已隐藏。配置暂不支持在线修改，请修改配置文件并重启服务。
        </p>
      </div>
      {error && <p role="alert" className="text-red-600">{error}</p>}
      {!config && !error && <p className="text-gray-500">正在加载配置…</p>}
      {config && Object.entries(config).map(([section, values]) => (
        <Card key={section}>
          <CardHeader><CardTitle>{section}</CardTitle></CardHeader>
          <CardContent>
            <dl className="space-y-3">
              {Object.entries(values ?? {}).map(([key, value]) => (
                <div key={key} className="grid grid-cols-1 gap-1 sm:grid-cols-2">
                  <dt className="text-sm text-gray-500">{key}</dt>
                  <dd className="break-all font-mono text-sm text-gray-900">
                    {typeof value === 'object' ? JSON.stringify(value) : String(value)}
                  </dd>
                </div>
              ))}
            </dl>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}
