import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { ApiError, type ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import type { WorkflowDefinition } from '../lib/workflow-definition'
import { WorkflowNewPage } from './workflow-new'

function renderPage(createWorkflow = vi.fn().mockResolvedValue({ id: 'w9' })) {
  const client = {
    validateWorkflow: vi.fn().mockResolvedValue({ valid: true, issues: [] }),
    createWorkflow,
  } as unknown as ApiClient
  const onCreated = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}>
      <ClientProvider value={client}>
        <WorkflowNewPage onCreated={onCreated} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onCreated }
}

async function start(name = 'deploy-notify', project = 'ops') {
  await userEvent.type(screen.getByLabelText('Nom'), name)
  await userEvent.type(screen.getByLabelText('Projet'), project)
  await userEvent.click(screen.getByRole('button', { name: 'Continuer' }))
}

describe('WorkflowNewPage', () => {
  it('needs a name and a project before opening the editor', async () => {
    renderPage()
    expect(screen.getByRole('button', { name: 'Continuer' })).toBeDisabled()
    await start()
    expect(screen.getByRole('button', { name: 'Enregistrer une version' })).toBeInTheDocument()
  })

  it('creates the workflow from a skeleton and opens it', async () => {
    const { client, onCreated } = renderPage()
    await start()
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith('w9'))
    const def = (client.createWorkflow as ReturnType<typeof vi.fn>).mock.calls[0][0] as WorkflowDefinition
    expect(def.metadata).toEqual({ name: 'deploy-notify', project: 'ops' })
    expect(def.spec.trigger).toEqual({ type: 'manual' })
    expect(def.spec.steps).toHaveLength(1)
  })

  it('shows the server message when the name is already taken', async () => {
    renderPage(vi.fn().mockRejectedValue(new ApiError(409, 'un workflow porte déjà ce nom dans ce projet')))
    await start()
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))
    expect(await screen.findByText('un workflow porte déjà ce nom dans ce projet')).toBeInTheDocument()
  })
})
