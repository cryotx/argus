import { useEffect, useRef, useCallback, useState } from 'react';

type WebSocketStatus = 'connecting' | 'connected' | 'disconnected' | 'error';

interface UseWebSocketOptions {
  url?: string;
  onMessage?: (data: unknown) => void;
  onConnect?: () => void;
  onDisconnect?: () => void;
  onError?: (error: Event) => void;
  reconnectInterval?: number;
  reconnectAttempts?: number;
}

interface WebSocketMessage {
  type: string;
  payload: unknown;
}

export function useWebSocket(options: UseWebSocketOptions = {}) {
  const {
    onMessage,
    onConnect,
    onDisconnect,
    onError,
    reconnectInterval = 3000,
    reconnectAttempts = 5,
  } = options;

  const [status, setStatus] = useState<WebSocketStatus>('disconnected');
  const wsRef = useRef<WebSocket | null>(null);
  const attemptsRef = useRef(0);
  const reconnectTimeoutRef = useRef<number>();

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      return;
    }

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/ws`;

    try {
      setStatus('connecting');
      const ws = new WebSocket(wsUrl);

      ws.onopen = () => {
        setStatus('connected');
        attemptsRef.current = 0;
        onConnect?.();
      };

      ws.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data) as WebSocketMessage;
          onMessage?.(data);
        } catch {
          onMessage?.(event.data);
        }
      };

      ws.onerror = (error) => {
        setStatus('error');
        onError?.(error);
      };

      ws.onclose = () => {
        setStatus('disconnected');
        onDisconnect?.();

        // Auto-reconnect
        if (attemptsRef.current < reconnectAttempts) {
          attemptsRef.current += 1;
          reconnectTimeoutRef.current = setTimeout(connect, reconnectInterval);
        }
      };

      wsRef.current = ws;
    } catch {
      setStatus('error');
    }
  }, [onMessage, onConnect, onDisconnect, onError, reconnectInterval, reconnectAttempts]);

  const disconnect = useCallback(() => {
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
    }
    attemptsRef.current = reconnectAttempts; // Prevent reconnection

    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }
    setStatus('disconnected');
  }, [reconnectAttempts]);

  const send = useCallback((message: unknown) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(message));
    }
  }, []);

  useEffect(() => {
    connect();
    return () => disconnect();
  }, [connect, disconnect]);

  return {
    status,
    connect,
    disconnect,
    send,
    isConnected: status === 'connected',
  };
}

// Event types for WebSocket messages
export interface TaskEvent {
  type: 'task_created' | 'task_updated' | 'task_completed' | 'task_failed';
  payload: {
    task_id: string;
    project_id: string;
    status: string;
  };
}

export interface LogEvent {
  type: 'log';
  payload: {
    entry: {
      timestamp: string;
      level: string;
      source: string;
      message: string;
    };
  };
}

export interface SystemEvent {
  type: 'system_status' | 'worker_status' | 'deployment_status';
  payload: unknown;
}

export type ArgusEvent = TaskEvent | LogEvent | SystemEvent;
