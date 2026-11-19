import { useEffect, useState } from 'react';

export interface RealtimeEvent {
  type: string;
  payload: unknown;
  id: string;
  timestamp: string;
  source: string;
  correlation_id?: string;
}

export function useEventStream(onEvent: (event: RealtimeEvent) => void) {
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    const source = new EventSource('/api/events');
    source.onopen = () => setConnected(true);
    source.onerror = () => setConnected(false);
    source.onmessage = (message) => {
      try {
        onEvent(JSON.parse(message.data) as RealtimeEvent);
      } catch {
        // Ignore malformed server events; EventSource reconnects automatically.
      }
    };
    return () => source.close();
  }, [onEvent]);

  return connected;
}
