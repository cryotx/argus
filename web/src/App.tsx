import { useState } from 'react';
import { Layout } from './components/Layout';
import {
  DashboardPage,
  ProjectsPage,
  TasksPage,
  DeploymentsPage,
  LogsPage,
  LlmUsagePage,
  ConfigPage,
} from './pages';

type Page = 'dashboard' | 'projects' | 'tasks' | 'deployments' | 'logs' | 'llm-usage' | 'config';

function App() {
  const [currentPage, setCurrentPage] = useState<Page>('dashboard');

  const renderPage = () => {
    switch (currentPage) {
      case 'dashboard':
        return <DashboardPage />;
      case 'projects':
        return <ProjectsPage />;
      case 'tasks':
        return <TasksPage />;
      case 'deployments':
        return <DeploymentsPage />;
      case 'logs':
        return <LogsPage />;
      case 'llm-usage':
        return <LlmUsagePage />;
      case 'config':
        return <ConfigPage />;
      default:
        return <DashboardPage />;
    }
  };

  return (
    <Layout currentPage={currentPage} onNavigate={setCurrentPage}>
      {renderPage()}
    </Layout>
  );
}

export default App;
