import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { useState } from 'react'
import {
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
import { OverviewPage } from './pages/overview'

export function createAppRouter(client: ApiClient, queryClient: QueryClient) {
  const root = createRootRoute({ component: Outlet })

  const overview = createRoute({
    getParentRoute: () => root,
    path: '/',
    beforeLoad: () => {
      if (!client.isAuthenticated()) throw redirect({ to: '/login' })
    },
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

  return createRouter({ routeTree: root.addChildren([overview, login]) })
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
