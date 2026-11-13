import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { WorkflowDetailPage } from './workflow-detail'

const detail = (active: number | null = 1) => ({
  workflow: { id: 'w1', project: 'ops', name: 'notify', active_version: active, latest_version: 2 },
  versions: [
    { version: 2, hash: 'bbbbbbbbbbbb1234', created_at: '2026-11-13T11:00:00.000Z' },
    { version: 1, hash: 'aaaaaaaaaaaa1234', created_at: '2026-11-13T10:00:00.000Z' },
  ],
})

function renderPage(role = 'admin', active: number | null = 1) {
  const client = {
    workflow: vi.fn().mockResolvedValue(detail(active)),
    me: vi.fn().mockResolvedValue({ id: '1', email: 'a@b.c', role }),
    webhookTokens: vi
      .fn()
      .mockResolvedValue([
        { id: 't1', created_at: '2026-11-13T10:00:00.000Z', revoked_at: null, signed: true },
      ]),
    activateWorkflow: vi.fn().mockResolvedValue(undefined),
    deactivateWorkflow: vi.fn().mockResolvedValue(undefined),
    exportWorkflow: vi.fn().mockResolvedValue('name: notify\n'),
    importWorkflow: vi.fn().mockResolvedValue({ id: 'w1', version: 3, created: false }),
    triggerRun: vi.fn().mockResolvedValue({ run_id: 'run-9' }),
    issueWebhookToken: vi
      .fn()
      .mockResolvedValue({ id: 't2', token: 'swh_abc', signing_secret: 'whsec_xyz' }),
    revokeWebhookToken: vi.fn().mockResolvedValue(undefined),
  } as unknown as ApiClient
  const onOpenRun = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <WorkflowDetailPage id="w1" onOpenRun={onOpenRun} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onOpenRun }
}

describe('WorkflowDetailPage', () => {
  it('shows the workflow, its versions and which one is active', async () => {
    renderPage()
    expect(await screen.findByText('notify')).toBeInTheDocument()
    expect(screen.getByText('v2')).toBeInTheDocument()
    expect(screen.getByText('v1 · active')).toBeInTheDocument()
  })

  it('activates another version', async () => {
    const { client } = renderPage()
    await userEvent.click(await screen.findByRole('button', { name: 'Activer v2' }))
    await waitFor(() => expect(client.activateWorkflow).toHaveBeenCalledWith('w1', 2))
  })

  it('deactivates the workflow', async () => {
    const { client } = renderPage()
    await userEvent.click(await screen.findByRole('button', { name: 'Désactiver' }))
    await waitFor(() => expect(client.deactivateWorkflow).toHaveBeenCalledWith('w1'))
  })

  it('shows the YAML of a version', async () => {
    const { client } = renderPage()
    await userEvent.click(await screen.findByRole('button', { name: 'Voir le YAML v1' }))
    expect(await screen.findByText(/name: notify/)).toBeInTheDocument()
    expect(client.exportWorkflow).toHaveBeenCalledWith('w1', 1)
  })

  it('imports a new version from YAML', async () => {
    const { client } = renderPage()
    await userEvent.type(await screen.findByLabelText('Nouvelle version (YAML)'), 'apiVersion: x')
    await userEvent.click(screen.getByRole('button', { name: 'Importer la version' }))
    await waitFor(() => expect(client.importWorkflow).toHaveBeenCalledWith('apiVersion: x'))
  })

  it('runs the workflow with a JSON payload and opens the run', async () => {
    const { client, onOpenRun } = renderPage()
    const box = await screen.findByLabelText('Payload JSON')
    await userEvent.clear(box)
    await userEvent.type(box, '{{"env":"prod"}')
    await userEvent.click(screen.getByRole('button', { name: 'Lancer' }))
    await waitFor(() => expect(client.triggerRun).toHaveBeenCalledWith('w1', { env: 'prod' }))
    await waitFor(() => expect(onOpenRun).toHaveBeenCalledWith('run-9'))
  })

  it('refuses an invalid JSON payload without calling the API', async () => {
    const { client } = renderPage()
    const box = await screen.findByLabelText('Payload JSON')
    await userEvent.clear(box)
    await userEvent.type(box, 'pas du json')
    await userEvent.click(screen.getByRole('button', { name: 'Lancer' }))
    expect(await screen.findByText('Payload JSON invalide.')).toBeInTheDocument()
    expect(client.triggerRun).not.toHaveBeenCalled()
  })

  it('issues a signed webhook token and shows the secrets once', async () => {
    const { client } = renderPage()
    await userEvent.click(await screen.findByLabelText('Signé (HMAC)'))
    await userEvent.click(screen.getByRole('button', { name: 'Créer un jeton' }))
    await waitFor(() => expect(client.issueWebhookToken).toHaveBeenCalledWith('w1', true))
    expect(await screen.findByText('swh_abc')).toBeInTheDocument()
    expect(screen.getByText('whsec_xyz')).toBeInTheDocument()
  })

  it('lists tokens and revokes one', async () => {
    const { client } = renderPage()
    expect(await screen.findByText('signé')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Révoquer t1' }))
    await waitFor(() => expect(client.revokeWebhookToken).toHaveBeenCalledWith('w1', 't1'))
  })

  it('is read-only for viewers', async () => {
    renderPage('viewer')
    await screen.findByText('notify')
    for (const name of ['Activer v2', 'Désactiver', 'Lancer', 'Importer la version', 'Créer un jeton']) {
      expect(screen.queryByRole('button', { name })).toBeNull()
    }
  })
})
