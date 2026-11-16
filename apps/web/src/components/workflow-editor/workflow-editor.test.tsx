import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { ApiError, type ApiClient } from '../../lib/api'
import { ClientProvider } from '../../lib/client-context'
import type { WorkflowDefinition } from '../../lib/workflow-definition'
import { WorkflowEditor } from './workflow-editor'

const initial: WorkflowDefinition = {
  apiVersion: 'scaling-waddle.dev/v1',
  kind: 'Workflow',
  metadata: { name: 'demo', project: 'p' },
  spec: {
    trigger: { type: 'manual' },
    steps: [
      { id: 'check', type: 'condition', with: { expression: 'true' } },
      { id: 'notify', type: 'delay', needs: ['check'], with: { seconds: 5 } },
    ],
  },
}

function setup(
  overrides: Partial<Record<'validateWorkflow' | 'addVersion', unknown>> = {},
  allowUnchanged = false,
) {
  const client = {
    validateWorkflow: vi.fn().mockResolvedValue({ valid: true, issues: [] }),
    addVersion: vi.fn().mockResolvedValue({ version: 3 }),
    ...overrides,
  } as unknown as ApiClient
  const onSaved = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}>
      <ClientProvider value={client}>
        <WorkflowEditor
          initial={initial}
          allowUnchanged={allowUnchanged}
          submit={async (def) => (await client.addVersion('w1', def)).version}
          onSaved={onSaved}
        />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onSaved }
}

// fireEvent : le mousedown synthétique de user-event n'a pas de `view`, ce qui fait planter d3-zoom dans jsdom
const selectNode = (id: string) => {
  fireEvent.click(document.querySelector(`.react-flow__node[data-id="${id}"]`) as HTMLElement)
}

const lastSaved = (client: ApiClient) =>
  (client.addVersion as ReturnType<typeof vi.fn>).mock.calls[0][1] as WorkflowDefinition

describe('WorkflowEditor', () => {
  it('keeps save disabled until something changes', async () => {
    setup()
    expect(screen.getByRole('button', { name: 'Enregistrer une version' })).toBeDisabled()
    await userEvent.click(screen.getByRole('button', { name: 'Ajouter Délai' }))
    expect(screen.getByRole('button', { name: 'Enregistrer une version' })).toBeEnabled()
  })

  it('can save an unchanged definition when creating a workflow', async () => {
    const { client, onSaved } = setup({}, true)
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(3))
    expect(client.addVersion).toHaveBeenCalled()
  })

  it('adds a step, selects it and saves the edited definition as a new version', async () => {
    const { client, onSaved } = setup()
    await userEvent.click(screen.getByRole('button', { name: 'Ajouter Délai' }))
    expect(screen.getByLabelText('Identifiant')).toHaveValue('delay-1')

    await userEvent.type(screen.getByLabelText('Secondes'), '30')
    await userEvent.tab()
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))

    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(3))
    expect(client.validateWorkflow).toHaveBeenCalledTimes(1)
    expect(client.addVersion).toHaveBeenCalledWith('w1', expect.anything())
    expect(lastSaved(client).spec.steps.at(-1)).toEqual({
      id: 'delay-1',
      type: 'delay',
      with: { seconds: 30 },
    })
  })

  it('renames a step and updates the dependencies that point to it', async () => {
    const { client } = setup()
    selectNode('check')
    const id = screen.getByLabelText('Identifiant')
    await userEvent.clear(id)
    await userEvent.type(id, 'verify')
    await userEvent.tab()
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))

    await waitFor(() => expect(client.addVersion).toHaveBeenCalled())
    const steps = lastSaved(client).spec.steps
    expect(steps.map((s) => s.id)).toEqual(['verify', 'notify'])
    expect(steps[1]?.needs).toEqual(['verify'])
  })

  it('refuses an invalid or duplicate identifier', async () => {
    setup()
    selectNode('check')
    const id = screen.getByLabelText('Identifiant')
    await userEvent.clear(id)
    await userEvent.type(id, 'notify')
    await userEvent.tab()
    expect(screen.getByRole('alert')).toHaveTextContent('déjà utilisé')
  })

  it('toggles dependencies and refuses cycles', async () => {
    const { client } = setup()
    selectNode('check')
    await userEvent.click(screen.getByRole('checkbox', { name: 'notify' }))
    expect(screen.getByRole('alert')).toHaveTextContent('cycle')

    selectNode('notify')
    await userEvent.click(screen.getByRole('checkbox', { name: 'check' }))
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))
    await waitFor(() => expect(client.addVersion).toHaveBeenCalled())
    expect(lastSaved(client).spec.steps[1]?.needs).toBeUndefined()
  })

  it('removes the selected step', async () => {
    const { client } = setup()
    selectNode('check')
    await userEvent.click(screen.getByRole('button', { name: "Supprimer l'étape" }))
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))
    await waitFor(() => expect(client.addVersion).toHaveBeenCalled())
    expect(lastSaved(client).spec.steps.map((s) => s.id)).toEqual(['notify'])
    expect(lastSaved(client).spec.steps[0]?.needs).toBeUndefined()
  })

  it('edits the trigger', async () => {
    const { client } = setup()
    selectNode('__trigger__')
    await userEvent.selectOptions(screen.getByLabelText('Type de déclencheur'), 'cron')
    await userEvent.type(screen.getByLabelText('Planification (cron)'), '0 8 * * *')
    await userEvent.tab()
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))
    await waitFor(() => expect(client.addVersion).toHaveBeenCalled())
    expect(lastSaved(client).spec.trigger).toEqual({ type: 'cron', config: { schedule: '0 8 * * *' } })
  })

  it('shows validation issues, selects the faulty step and does not save', async () => {
    const { client } = setup({
      validateWorkflow: vi.fn().mockResolvedValue({
        valid: false,
        issues: [{ message: "l'étape notify référence x", step_id: 'notify' }],
      }),
    })
    await userEvent.click(screen.getByRole('button', { name: 'Ajouter Délai' }))
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))

    const issue = await screen.findByRole('button', { name: "l'étape notify référence x" })
    await userEvent.click(issue)
    expect(screen.getByLabelText('Identifiant')).toHaveValue('notify')
    expect(client.addVersion).not.toHaveBeenCalled()
  })

  it('shows the issues returned by the server when saving is refused', async () => {
    setup({
      addVersion: vi
        .fn()
        .mockRejectedValue(new ApiError(400, 'définition invalide', [{ message: 'refusée', step_id: null }])),
    })
    await userEvent.click(screen.getByRole('button', { name: 'Ajouter Délai' }))
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer une version' }))
    expect(await screen.findByText('refusée')).toBeInTheDocument()
  })
})
