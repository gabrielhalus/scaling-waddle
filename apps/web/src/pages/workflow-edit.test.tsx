import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { WorkflowEditPage } from './workflow-edit'

const definition = {
  apiVersion: 'scaling-waddle.dev/v1',
  kind: 'Workflow',
  metadata: { name: 'notify', project: 'ops' },
  spec: { trigger: { type: 'manual' }, steps: [{ id: 'check', type: 'condition', with: { expression: 'true' } }] },
}

function renderPage(role = 'admin') {
  const client = {
    workflow: vi.fn().mockResolvedValue({
      workflow: { id: 'w1', project: 'ops', name: 'notify', active_version: 1, latest_version: 2 },
      versions: [],
    }),
    me: vi.fn().mockResolvedValue({ id: '1', email: 'a@b.c', role }),
    workflowDefinition: vi.fn().mockResolvedValue(definition),
    validateWorkflow: vi.fn().mockResolvedValue({ valid: true, issues: [] }),
    addVersion: vi.fn().mockResolvedValue({ version: 3 }),
  } as unknown as ApiClient
  const onDone = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <WorkflowEditPage id="w1" onDone={onDone} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onDone }
}

describe('WorkflowEditPage', () => {
  it('starts from the active version and goes back after saving', async () => {
    const { client, onDone } = renderPage()
    expect(await screen.findByText('notify')).toBeInTheDocument()
    expect(screen.getByText(/Basé sur la version v1/)).toBeInTheDocument()
    expect(client.workflowDefinition).toHaveBeenCalledWith('w1', 1)

    await userEvent.click(await screen.findByRole('button', { name: 'Ajouter Délai' }))
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))
    await waitFor(() => expect(onDone).toHaveBeenCalled())
  })

  it('is read-only for viewers', async () => {
    renderPage('viewer')
    expect(await screen.findByText('check')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Enregistrer une version' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Ajouter Délai' })).not.toBeInTheDocument()
  })
})
