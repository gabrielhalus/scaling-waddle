import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { WorkflowsPage } from './workflows'

function renderPage(workflows: unknown[], onOpenRuns = vi.fn()) {
  const client = { workflows: vi.fn().mockResolvedValue(workflows) } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <WorkflowsPage onOpenRuns={onOpenRuns} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { onOpenRuns }
}

const wf = (over: object = {}) => ({
  id: 'w1',
  project: 'ops',
  name: 'notify-on-deploy',
  active_version: 2,
  latest_version: 3,
  ...over,
})

describe('WorkflowsPage', () => {
  it('lists workflows with project and versions', async () => {
    renderPage([wf()])
    expect(await screen.findByText('notify-on-deploy')).toBeInTheDocument()
    expect(screen.getByText('ops')).toBeInTheDocument()
    expect(screen.getByText('v2 active · v3 dernière')).toBeInTheDocument()
  })

  it('flags a workflow with no active version', async () => {
    renderPage([wf({ active_version: null })])
    expect(await screen.findByText('inactif · v3 dernière')).toBeInTheDocument()
  })

  it('shows an empty state', async () => {
    renderPage([])
    expect(await screen.findByText('Aucun workflow pour le moment.')).toBeInTheDocument()
  })

  it('opens the runs of a workflow', async () => {
    const { onOpenRuns } = renderPage([wf()])
    await userEvent.click(await screen.findByRole('button', { name: 'Voir les runs' }))
    expect(onOpenRuns).toHaveBeenCalledWith('w1')
  })
})
