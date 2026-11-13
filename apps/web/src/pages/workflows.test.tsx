import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { WorkflowsPage } from './workflows'

function renderPage(workflows: unknown[], onOpenRuns = vi.fn(), onOpen = vi.fn(), role = 'admin') {
  const client = {
    workflows: vi.fn().mockResolvedValue(workflows),
    me: vi.fn().mockResolvedValue({ id: '1', email: 'a@b.c', role }),
    importWorkflow: vi.fn().mockResolvedValue({ id: 'w9', version: 1, created: true }),
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <WorkflowsPage onOpenRuns={onOpenRuns} onOpen={onOpen} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onOpenRuns, onOpen }
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

  it('opens the detail of a workflow from its name', async () => {
    const { onOpen } = renderPage([wf()])
    await userEvent.click(await screen.findByRole('button', { name: 'notify-on-deploy' }))
    expect(onOpen).toHaveBeenCalledWith('w1')
  })

  it('imports a workflow from YAML then opens it', async () => {
    const { client, onOpen } = renderPage([])
    await userEvent.type(await screen.findByLabelText('Définition (YAML)'), 'apiVersion: x')
    await userEvent.click(screen.getByRole('button', { name: 'Importer' }))
    await waitFor(() => expect(client.importWorkflow).toHaveBeenCalledWith('apiVersion: x'))
    await waitFor(() => expect(onOpen).toHaveBeenCalledWith('w9'))
  })

  it('hides the import form from viewers', async () => {
    renderPage([wf()], vi.fn(), vi.fn(), 'viewer')
    await screen.findByText('ops')
    expect(screen.queryByLabelText('Définition (YAML)')).toBeNull()
  })
})
