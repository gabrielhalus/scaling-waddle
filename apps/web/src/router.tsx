import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { useState } from 'react'
import {
  Link,
  Outlet,
  RouterProvider,
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
  useNavigate,
} from '@tanstack/react-router'

import type { ApiClient } from './lib/api'
import { ClientProvider } from './lib/client-context'
import { LoginPage } from './pages/login'
import { ApprovalsPage } from './pages/approvals'
import { AuditPage } from './pages/audit'
import { IntegrationsPage } from './pages/integrations'
import { OverviewPage } from './pages/overview'
import { RunDetailPage } from './pages/run-detail'
import { RunsPage } from './pages/runs'
import { SettingsPage } from './pages/settings'
import { WorkflowDetailPage } from './pages/workflow-detail'
import { WorkflowsPage } from './pages/workflows'

export function createAppRouter(client: ApiClient, queryClient: QueryClient) {
  const root = createRootRoute({ component: Outlet })

  const shell = createRoute({
    getParentRoute: () => root,
    id: 'shell',
    beforeLoad: () => {
      if (!client.isAuthenticated()) throw redirect({ to: '/login' })
    },
    component: function Shell() {
      return (
        <div className="min-h-screen">
          <header className="border-b bg-card">
            <div className="mx-auto flex max-w-5xl items-center gap-6 px-6 py-3">
              <span className="text-sm font-semibold">scaling-waddle</span>
              <nav className="flex flex-wrap gap-1 text-sm">
                <Link
                  to="/"
                  activeOptions={{ exact: true }}
                  className="rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:text-foreground [&.active]:bg-muted [&.active]:font-medium [&.active]:text-foreground"
                >
                  Overview
                </Link>
                <Link
                  to="/workflows"
                  className="rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:text-foreground [&.active]:bg-muted [&.active]:font-medium [&.active]:text-foreground"
                >
                  Workflows
                </Link>
                <Link
                  to="/runs"
                  className="rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:text-foreground [&.active]:bg-muted [&.active]:font-medium [&.active]:text-foreground"
                >
                  Runs
                </Link>
                <Link
                  to="/approvals"
                  className="rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:text-foreground [&.active]:bg-muted [&.active]:font-medium [&.active]:text-foreground"
                >
                  Approbations
                </Link>
                <Link
                  to="/integrations"
                  className="rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:text-foreground [&.active]:bg-muted [&.active]:font-medium [&.active]:text-foreground"
                >
                  Intégrations
                </Link>
                <Link
                  to="/audit"
                  className="rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:text-foreground [&.active]:bg-muted [&.active]:font-medium [&.active]:text-foreground"
                >
                  Audit
                </Link>
                <Link
                  to="/settings"
                  className="rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:text-foreground [&.active]:bg-muted [&.active]:font-medium [&.active]:text-foreground"
                >
                  Réglages
                </Link>
              </nav>
            </div>
          </header>
          <main className="mx-auto max-w-5xl p-6">
            <Outlet />
          </main>
        </div>
      )
    },
  })

  const overview = createRoute({
    getParentRoute: () => shell,
    path: '/',
    component: function Overview() {
      const navigate = useNavigate()
      return (
        <OverviewPage
          onOpenRun={(runId) => void navigate({ to: '/runs/$runId', params: { runId } })}
          onLogout={() => {
            queryClient.clear()
            void navigate({ to: '/login' })
          }}
        />
      )
    },
  })

  const workflows = createRoute({
    getParentRoute: () => shell,
    path: '/workflows',
    component: function Workflows() {
      const navigate = useNavigate()
      return (
        <WorkflowsPage
          onOpenRuns={(workflowId) =>
            void navigate({ to: '/runs', search: { workflow_id: workflowId } })
          }
          onOpen={(workflowId) =>
            void navigate({
              to: '/workflows/$workflowId',
              params: { workflowId },
            })
          }
        />
      )
    },
  })

  const workflowDetail = createRoute({
    getParentRoute: () => shell,
    path: '/workflows/$workflowId',
    component: function WorkflowDetail() {
      const navigate = useNavigate()
      const { workflowId } = workflowDetail.useParams()
      return (
        <WorkflowDetailPage
          key={workflowId}
          id={workflowId}
          onOpenRun={(runId) => void navigate({ to: '/runs/$runId', params: { runId } })}
        />
      )
    },
  })

  const runs = createRoute({
    getParentRoute: () => shell,
    path: '/runs',
    validateSearch: (search: Record<string, unknown>): { workflow_id?: string } =>
      typeof search.workflow_id === 'string' ? { workflow_id: search.workflow_id } : {},
    component: function Runs() {
      const navigate = useNavigate()
      const { workflow_id } = runs.useSearch()
      return (
        <RunsPage
          workflowId={workflow_id}
          onOpenRun={(runId) => void navigate({ to: '/runs/$runId', params: { runId } })}
        />
      )
    },
  })

  const approvals = createRoute({
    getParentRoute: () => shell,
    path: '/approvals',
    component: function Approvals() {
      const navigate = useNavigate()
      return (
        <ApprovalsPage
          onOpenRun={(runId) => void navigate({ to: '/runs/$runId', params: { runId } })}
        />
      )
    },
  })

  const integrations = createRoute({
    getParentRoute: () => shell,
    path: '/integrations',
    component: IntegrationsPage,
  })

  const audit = createRoute({
    getParentRoute: () => shell,
    path: '/audit',
    component: AuditPage,
  })

  const settings = createRoute({
    getParentRoute: () => shell,
    path: '/settings',
    component: SettingsPage,
  })

  const runDetail = createRoute({
    getParentRoute: () => shell,
    path: '/runs/$runId',
    component: function RunDetail() {
      const navigate = useNavigate()
      const { runId } = runDetail.useParams()
      return (
        <RunDetailPage
          key={runId}
          id={runId}
          onOpenRun={(id) => void navigate({ to: '/runs/$runId', params: { runId: id } })}
        />
      )
    },
  })

  const login = createRoute({
    getParentRoute: () => root,
    path: '/login',
    beforeLoad: () => {
      if (client.isAuthenticated()) throw redirect({ to: '/' })
    },
    component: function Login() {
      const navigate = useNavigate()
      return <LoginPage onDone={() => void navigate({ to: '/' })} />
    },
  })

  return createRouter({
    routeTree: root.addChildren([
      shell.addChildren([
        overview,
        workflows,
        workflowDetail,
        runs,
        runDetail,
        approvals,
        integrations,
        audit,
        settings,
      ]),
      login,
    ]),
  })
}

export function App({ client }: { client: ApiClient }) {
  const [{ queryClient, router }] = useState(() => {
    const queryClient = new QueryClient()
    return { queryClient, router: createAppRouter(client, queryClient) }
  })
  return (
    <QueryClientProvider client={queryClient}>
      <ClientProvider value={client}>
        <RouterProvider router={router} />
      </ClientProvider>
    </QueryClientProvider>
  )
}

declare module '@tanstack/react-router' {
  interface Register {
    router: ReturnType<typeof createAppRouter>
  }
}
