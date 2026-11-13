import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { AuditPage } from './audit'

const event = (over: object = {}) => ({
  id: 1,
  at: '2026-11-13T10:00:00.000Z',
  actor: 'user-1',
  action: 'run.cancel',
  target_type: 'run',
  target_id: 'run-1',
  details: { status: 'cancelled' },
  ...over,
})

function renderPage(items: unknown[], role = 'admin') {
  const client = {
    audit: vi.fn().mockResolvedValue(items),
    me: vi.fn().mockResolvedValue({ id: '1', email: 'a@b.c', role }),
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <AuditPage />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return client
}

describe('AuditPage', () => {
  it('lists events with actor, target and details', async () => {
    renderPage([event()])
    expect(await screen.findByText('run.cancel')).toBeInTheDocument()
    expect(screen.getByText('user-1')).toBeInTheDocument()
    expect(screen.getByText('run · run-1')).toBeInTheDocument()
    expect(screen.getByText('{"status":"cancelled"}')).toBeInTheDocument()
  })

  it('filters by action', async () => {
    const client = renderPage([event()])
    await userEvent.type(await screen.findByLabelText('Action'), 'run.retry')
    await waitFor(() => expect(client.audit).toHaveBeenLastCalledWith({ action: 'run.retry' }))
  })

  it('shows an empty state', async () => {
    renderPage([])
    expect(await screen.findByText('Aucun événement.')).toBeInTheDocument()
  })

  it('tells non-admins the trail is restricted without querying it', async () => {
    const client = renderPage([event()], 'viewer')
    expect(await screen.findByText('Réservé aux administrateurs.')).toBeInTheDocument()
    expect(client.audit).not.toHaveBeenCalled()
  })
})
