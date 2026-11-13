import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { RunDetailPage } from './run-detail'

const detail = (status: string, steps: object[] = []) => ({
  run: {
    id: 'run-1',
    workflow_id: 'w1',
    version: 2,
    event_id: 'e1',
    status,
    error: status === 'failed' ? 'étape b : boom' : null,
    created_at: '2026-11-13T10:00:00.000Z',
    finished_at: null,
  },
  event: { source: 'webhook', type: 'webhook.received', payload: { a: 1 }, correlation_id: 'c1' },
  steps,
})

function renderPage(status: string, role = 'admin', steps: object[] = []) {
  const client = {
    run: vi.fn().mockResolvedValue(detail(status, steps)),
    me: vi.fn().mockResolvedValue({ id: '1', email: 'a@b.c', role }),
    cancelRun: vi.fn().mockResolvedValue({ run_id: 'run-1', status: 'cancelled' }),
    retryRun: vi.fn().mockResolvedValue({ run_id: 'run-2' }),
  } as unknown as ApiClient
  const onOpenRun = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <RunDetailPage id="run-1" onOpenRun={onOpenRun} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onOpenRun }
}

const step = (over: object = {}) => ({
  step_id: 'check',
  status: 'succeeded',
  attempts: 1,
  output: { result: true },
  error: null,
  started_at: null,
  finished_at: null,
  wake_at: null,
  ...over,
})

describe('RunDetailPage', () => {
  it('shows the run error, the event and the steps', async () => {
    renderPage('failed', 'admin', [step(), step({ step_id: 'b', status: 'failed', error: 'boom' })])
    expect(await screen.findByText('étape b : boom')).toBeInTheDocument()
    expect(screen.getByText('webhook.received')).toBeInTheDocument()
    expect(screen.getByText('check')).toBeInTheDocument()
    expect(screen.getByText('boom')).toBeInTheDocument()
  })

  it('cancels an active run', async () => {
    const { client } = renderPage('queued')
    await userEvent.click(await screen.findByRole('button', { name: 'Annuler' }))
    await waitFor(() => expect(client.cancelRun).toHaveBeenCalledWith('run-1'))
  })

  it('retries a failed run and opens the new one', async () => {
    const { client, onOpenRun } = renderPage('failed')
    await userEvent.click(await screen.findByRole('button', { name: 'Relancer' }))
    await waitFor(() => expect(client.retryRun).toHaveBeenCalledWith('run-1'))
    await waitFor(() => expect(onOpenRun).toHaveBeenCalledWith('run-2'))
  })

  it('offers no action on a succeeded run', async () => {
    renderPage('succeeded')
    await screen.findByText('webhook.received')
    expect(screen.queryByRole('button', { name: 'Annuler' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Relancer' })).toBeNull()
  })

  it('hides actions from viewers', async () => {
    renderPage('failed', 'viewer')
    await screen.findByText('webhook.received')
    expect(screen.queryByRole('button', { name: 'Relancer' })).toBeNull()
  })

  it('shows when a waiting step resumes', async () => {
    const wake = Date.UTC(2026, 10, 13, 12, 0, 0)
    renderPage('waiting', 'admin', [step({ step_id: 'pause', status: 'waiting', wake_at: wake })])
    expect(await screen.findByText(`reprise à ${new Date(wake).toLocaleString()}`)).toBeInTheDocument()
  })
})
