import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { RunsPage } from './runs'

const run = (over: object = {}) => ({
  id: 'run-1',
  workflow_id: 'w1',
  version: 1,
  event_id: 'e1',
  status: 'succeeded',
  error: null,
  created_at: '2026-11-13T10:00:00.000Z',
  finished_at: null,
  ...over,
})

function renderPage(runs: unknown[], workflowId?: string, onOpenRun = vi.fn()) {
  const client = {
    runs: vi.fn().mockResolvedValue(runs),
    workflows: vi.fn().mockResolvedValue([{ id: 'w1', name: 'notify', project: 'p' }]),
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <RunsPage workflowId={workflowId} onOpenRun={onOpenRun} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onOpenRun }
}

describe('RunsPage', () => {
  it('lists runs with the workflow name and status', async () => {
    renderPage([run({ status: 'failed' })])
    expect(await screen.findByText('notify')).toBeInTheDocument()
    expect(screen.getByText('failed', { selector: 'span' })).toBeInTheDocument()
  })

  it('shows an empty state', async () => {
    renderPage([])
    expect(await screen.findByText('Aucun run.')).toBeInTheDocument()
  })

  it('filters by status through the client', async () => {
    const { client } = renderPage([run()])
    await userEvent.selectOptions(await screen.findByLabelText('Statut'), 'failed')
    expect(client.runs).toHaveBeenLastCalledWith({ workflowId: undefined, status: 'failed' })
  })

  it('scopes the list to a workflow', async () => {
    const { client } = renderPage([run()], 'w1')
    await screen.findByText('notify')
    expect(client.runs).toHaveBeenCalledWith({ workflowId: 'w1', status: undefined })
  })

  it('opens a run', async () => {
    const { onOpenRun } = renderPage([run()])
    await userEvent.click(await screen.findByRole('button', { name: /run-1/ }))
    expect(onOpenRun).toHaveBeenCalledWith('run-1')
  })
})
