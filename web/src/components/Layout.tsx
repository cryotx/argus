import { Sidebar } from './Sidebar';
import { Header } from './Header';
import { useAppStore } from '../hooks/useAppStore';
import { clsx } from 'clsx';
import type { ReactNode } from 'react';

type Page = 'dashboard' | 'projects' | 'tasks' | 'deployments' | 'logs' | 'llm-usage' | 'config';

interface LayoutProps {
  children: ReactNode;
  currentPage: Page;
  onNavigate: (page: Page) => void;
}

export function Layout({ children, currentPage, onNavigate }: LayoutProps) {
  const { sidebarCollapsed } = useAppStore();

  return (
    <div className="min-h-screen bg-gray-50">
      <Sidebar currentPage={currentPage} onNavigate={onNavigate} />
      <div
        className={clsx(
          'transition-all duration-300',
          sidebarCollapsed ? 'ml-16' : 'ml-56'
        )}
      >
        <Header />
        <main className="p-6">
          {children}
        </main>
      </div>
    </div>
  );
}