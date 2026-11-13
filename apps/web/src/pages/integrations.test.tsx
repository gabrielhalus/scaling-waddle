import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { IntegrationsPage } from './integrations'

const integration = {
  id: 'i1',
  project: 'ops',
  name: 'chat',
  base_url: 'https://chat.example.com',
  headers: { authorization: 'Bearer ${secret:tok}', 'x-key': '***' },
  created_at: '2026-11-13T10:00:00.000Z',
}
const secret = {
  project: 'ops',
  name: 'tok',
  key_id: 'k1',
  created_at: '2026-11-13T10:00:00.000Z',
  updated_at: '2026-11-13T10:00:00.000Z',
}

function renderPage(role = 'admin') {
  const client = {
    integrations: vi.fn().mockResolvedValue([integration]),
    secrets: vi.fn().mockResolvedValue([secret]),
    me: vi.fn().mockResolvedValue({ id: '1', email: 'a@b.c', role }),
    createIntegration: vi.fn().mockResolvedValue(integration),
    deleteIntegration: vi.fn().mockResolvedValue(undefined),
    setSecret: vi.fn().mockResolvedValue(undefined),
    deleteSecret: vi.fn().mockResolvedValue(undefined),
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <IntegrationsPage />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return client
}

describe('IntegrationsPage', () => {
  it('lists integrations with masked headers and secrets without values', async () => {
    renderPage()
    expect(await screen.findByText('chat')).toBeInTheDocument()
    expect(screen.getByText('https://chat.example.com')).toBeInTheDocument()
    expect(screen.getByText('authorization: Bearer ${secret:tok}')).toBeInTheDocument()
    expect(screen.getByText('x-key: ***')).toBeInTheDocument()
    expect(await screen.findByText('tok')).toBeInTheDocument()
    expect(screen.getByText('clé k1')).toBeInTheDocument()
  })

  it('creates an integration from the form', async () => {
    const client = renderPage()
    await screen.findByText('chat')
    await userEvent.type(screen.getByLabelText('Projet'), 'ops')
    await userEvent.type(screen.getByLabelText('Nom'), 'ci')
    await userEvent.type(screen.getByLabelText('URL de base'), 'https://ci.example.com')
    await userEvent.click(screen.getByRole('button', { name: 'Créer l’intégration' }))
    await waitFor(() =>
      expect(client.createIntegration).toHaveBeenCalledWith({
        project: 'ops',
        name: 'ci',
        base_url: 'https://ci.example.com',
        headers: {},
      }),
    )
  })

  it('sets a secret then clears the value field', async () => {
    const client = renderPage()
    await screen.findByText('tok')
    await userEvent.type(screen.getByLabelText('Projet du secret'), 'ops')
    await userEvent.type(screen.getByLabelText('Nom du secret'), 'new')
    await userEvent.type(screen.getByLabelText('Valeur'), 'hunter2')
    await userEvent.click(screen.getByRole('button', { name: 'Enregistrer le secret' }))
    await waitFor(() => expect(client.setSecret).toHaveBeenCalledWith('ops', 'new', 'hunter2'))
    await waitFor(() => expect(screen.getByLabelText('Valeur')).toHaveValue(''))
  })

  it('deletes an integration and a secret', async () => {
    const client = renderPage()
    await userEvent.click(await screen.findByRole('button', { name: 'Supprimer chat' }))
    await waitFor(() => expect(client.deleteIntegration).toHaveBeenCalledWith('i1'))
    await userEvent.click(await screen.findByRole('button', { name: 'Supprimer le secret tok' }))
    await waitFor(() => expect(client.deleteSecret).toHaveBeenCalledWith('ops', 'tok'))
  })

  it('is read-only for viewers', async () => {
    renderPage('viewer')
    await screen.findByText('chat')
    expect(screen.queryByRole('button', { name: 'Supprimer chat' })).toBeNull()
    expect(screen.queryByLabelText('Valeur')).toBeNull()
  })
})
