import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { OverviewPage } from './overview'

function renderPage(overrides: Partial<ApiClient> = {}, onLogout = vi.fn()) {
  const client = {
    me: vi.fn().mockResolvedValue({ id: '1', email: 'ada@example.com', role: 'admin' }),
    health: vi.fn().mockResolvedValue({ status: 'ok', database: 'ok' }),
    logout: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <OverviewPage onLogout={onLogout} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onLogout }
}

describe('OverviewPage', () => {
  it('shows the signed-in user and the role', async () => {
    renderPage()
    expect(await screen.findByText('ada@example.com')).toBeInTheDocument()
    expect(screen.getByText('admin')).toBeInTheDocument()
  })

  it('shows a healthy database', async () => {
    renderPage()
    expect(await screen.findByText('Base de données : opérationnelle')).toBeInTheDocument()
  })

  it('shows an unavailable database', async () => {
    renderPage({ health: vi.fn().mockResolvedValue({ status: 'unavailable', database: 'down' }) })
    expect(await screen.findByText('Base de données : indisponible')).toBeInTheDocument()
  })

  it('logs out through the client then notifies the parent', async () => {
    const { client, onLogout } = renderPage()
    await userEvent.click(await screen.findByRole('button', { name: 'Se déconnecter' }))

    await waitFor(() => expect(onLogout).toHaveBeenCalled())
    expect(client.logout).toHaveBeenCalled()
  })
})
