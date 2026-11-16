import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
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

const definition = {
  apiVersion: 'scaling-waddle.dev/v1',
  kind: 'Workflow',
  metadata: { name: 'notify', project: 'ops' },
  spec: {
    trigger: { type: 'manual' },
    steps: [
      { id: 'check', type: 'condition', with: {} },
      { id: 'b', type: 'delay', needs: ['check'], with: {} },
    ],
  },
}

function renderPage(status: string, role = 'admin', steps: object[] = [], withGraph = false) {
  const client = {
    ...(withGraph ? { workflowDefinition: vi.fn().mockResolvedValue(definition) } : {}),
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

  describe('graph', () => {
    const node = (id: string) => document.querySelector(`.react-flow__node[data-id="${id}"]`) as HTMLElement

    it('draws the run version with the status of each step', async () => {
      const { client } = renderPage('failed', 'admin', [step(), step({ step_id: 'b', status: 'failed' })], true)
      await waitFor(() => expect(node('b')).not.toBeNull())
      expect(client.workflowDefinition).toHaveBeenCalledWith('w1', 2)
      expect(within(node('check')).getByText('succeeded')).toBeInTheDocument()
      expect(within(node('b')).getByText('failed')).toBeInTheDocument()
    })

    it('highlights the step card matching the clicked node', async () => {
      renderPage('failed', 'admin', [step(), step({ step_id: 'b', status: 'failed' })], true)
      await waitFor(() => expect(node('b')).not.toBeNull())
      fireEvent.click(node('b'))
      const card = screen.getAllByRole('listitem').find((li) => li.textContent?.startsWith('b'))
      expect(card).toHaveAttribute('aria-current', 'true')
    })

    it('still shows the run when the definition cannot be loaded', async () => {
      renderPage('succeeded', 'admin', [step()])
      expect(await screen.findByText('webhook.received')).toBeInTheDocument()
      expect(document.querySelector('.react-flow')).toBeNull()
    })
  })
})
