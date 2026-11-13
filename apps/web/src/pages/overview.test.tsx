import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { OverviewPage } from './overview'

function renderPage(overrides: Partial<ApiClient> = {}, onLogout = vi.fn(), onOpenRun = vi.fn()) {
  const client = {
    me: vi.fn().mockResolvedValue({ id: '1', email: 'ada@example.com', role: 'admin' }),
    health: vi.fn().mockResolvedValue({ status: 'ok', database: 'ok' }),
    logout: vi.fn().mockResolvedValue(undefined),
    workflows: vi.fn().mockResolvedValue([
      { id: 'w1', project: 'ops', name: 'notify', active_version: 1, latest_version: 1 },
      { id: 'w2', project: 'ops', name: 'idle', active_version: null, latest_version: 1 },
    ]),
    runs: vi.fn().mockResolvedValue([
      { id: 'r1', workflow_id: 'w1', version: 1, event_id: 'e', status: 'succeeded', error: null, created_at: '2026-11-13T10:00:00.000Z', finished_at: null },
      { id: 'r2', workflow_id: 'w1', version: 1, event_id: 'e', status: 'failed', error: 'x', created_at: '2026-11-13T09:00:00.000Z', finished_at: null },
      { id: 'r3', workflow_id: 'w1', version: 1, event_id: 'e', status: 'failed', error: 'y', created_at: '2026-11-13T08:00:00.000Z', finished_at: null },
    ]),
    approvals: vi.fn().mockResolvedValue([{ id: 'a1' }]),
    ...overrides,
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <OverviewPage onLogout={onLogout} onOpenRun={onOpenRun} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onLogout, onOpenRun }
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

  it('summarises active workflows, pending approvals and runs by status', async () => {
    renderPage()
    expect(await screen.findByText('Workflows actifs : 1 / 2')).toBeInTheDocument()
    expect(await screen.findByText('Approbations en attente : 1')).toBeInTheDocument()
    expect(await screen.findByText('succeeded : 1')).toBeInTheDocument()
    expect(screen.getByText('failed : 2')).toBeInTheDocument()
  })

  it('lists the latest runs and opens one', async () => {
    const { onOpenRun } = renderPage()
    await userEvent.click(await screen.findByRole('button', { name: /r1/ }))
    expect(onOpenRun).toHaveBeenCalledWith('r1')
  })
})
