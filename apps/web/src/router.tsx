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
import { OverviewPage } from './pages/overview'
import { RunDetailPage } from './pages/run-detail'
import { RunsPage } from './pages/runs'
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
        <div className="mx-auto flex min-h-screen max-w-3xl flex-col gap-6 p-6">
          <nav className="flex gap-4 text-sm">
            <Link to="/" activeOptions={{ exact: true }} className="[&.active]:font-semibold">
              Overview
            </Link>
            <Link to="/workflows" className="[&.active]:font-semibold">
              Workflows
            </Link>
            <Link to="/runs" className="[&.active]:font-semibold">
              Runs
            </Link>
            <Link to="/approvals" className="[&.active]:font-semibold">
              Approbations
            </Link>
          </nav>
          <main>
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
          onOpenRuns={(workflowId) => void navigate({ to: '/runs', search: { workflow_id: workflowId } })}
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

  return createRouter({ routeTree: root.addChildren([shell.addChildren([overview, workflows, runs, runDetail, approvals]), login]) })
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
