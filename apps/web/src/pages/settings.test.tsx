import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { SettingsPage } from './settings'

const users = [
  { id: 'u1', email: 'ada@example.com', role: 'admin', created_at: '2026-11-13T10:00:00.000Z' },
  { id: 'u2', email: 'bob@example.com', role: 'viewer', created_at: '2026-11-13T11:00:00.000Z' },
]

function renderPage(role = 'admin') {
  const client = {
    me: vi.fn().mockResolvedValue({ id: 'u1', email: 'ada@example.com', role }),
    users: vi.fn().mockResolvedValue(users),
    createUser: vi.fn().mockResolvedValue({ id: 'u3', email: 'c@d.co', role: 'operator' }),
    updateUser: vi.fn().mockResolvedValue(undefined),
    deleteUser: vi.fn().mockResolvedValue(undefined),
    rotateSecrets: vi.fn().mockResolvedValue({ rotated: 3 }),
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <SettingsPage />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return client
}

describe('SettingsPage', () => {
  it('lists users with their role', async () => {
    renderPage()
    expect(await screen.findByText('bob@example.com')).toBeInTheDocument()
    expect(screen.getByLabelText('Rôle de bob@example.com')).toHaveValue('viewer')
  })

  it('creates a user', async () => {
    const client = renderPage()
    await screen.findByText('bob@example.com')
    await userEvent.type(screen.getByLabelText('E-mail'), 'c@d.co')
    await userEvent.type(screen.getByLabelText('Mot de passe'), 'un-mot-de-passe-long')
    await userEvent.selectOptions(screen.getByLabelText('Rôle'), 'operator')
    await userEvent.click(screen.getByRole('button', { name: 'Créer l’utilisateur' }))
    await waitFor(() =>
      expect(client.createUser).toHaveBeenCalledWith({
        email: 'c@d.co',
        password: 'un-mot-de-passe-long',
        role: 'operator',
      }),
    )
  })

  it('changes the role of another user', async () => {
    const client = renderPage()
    await userEvent.selectOptions(await screen.findByLabelText('Rôle de bob@example.com'), 'operator')
    await waitFor(() => expect(client.updateUser).toHaveBeenCalledWith('u2', { role: 'operator' }))
  })

  it('resets a password', async () => {
    const client = renderPage()
    await userEvent.type(await screen.findByLabelText('Nouveau mot de passe de bob@example.com'), 'encore-un-long-mot-de-passe')
    await userEvent.click(screen.getByRole('button', { name: 'Changer le mot de passe de bob@example.com' }))
    await waitFor(() =>
      expect(client.updateUser).toHaveBeenCalledWith('u2', { password: 'encore-un-long-mot-de-passe' }),
    )
  })

  it('deletes another user but never offers it for the current one', async () => {
    const client = renderPage()
    await userEvent.click(await screen.findByRole('button', { name: 'Supprimer bob@example.com' }))
    await waitFor(() => expect(client.deleteUser).toHaveBeenCalledWith('u2'))
    expect(screen.queryByRole('button', { name: 'Supprimer ada@example.com' })).toBeNull()
    expect(screen.getByLabelText('Rôle de ada@example.com')).toBeDisabled()
  })

  it('rotates the secret keys and reports the count', async () => {
    const client = renderPage()
    await userEvent.click(await screen.findByRole('button', { name: 'Rechiffrer les secrets' }))
    await waitFor(() => expect(client.rotateSecrets).toHaveBeenCalled())
    expect(await screen.findByText('3 secret(s) rechiffré(s).')).toBeInTheDocument()
  })

  it('is restricted to administrators', async () => {
    const client = renderPage('operator')
    expect(await screen.findByText('Réservé aux administrateurs.')).toBeInTheDocument()
    expect(client.users).not.toHaveBeenCalled()
  })
})
