import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { ApiError, type ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { LoginPage } from './login'

function renderPage(overrides: Partial<ApiClient>, onDone = vi.fn()) {
  const client = {
    setupRequired: vi.fn().mockResolvedValue(false),
    login: vi.fn().mockResolvedValue(undefined),
    setup: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <LoginPage onDone={onDone} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onDone }
}

describe('LoginPage', () => {
  it('shows the login form when an admin already exists', async () => {
    renderPage({})
    expect(await screen.findByRole('button', { name: 'Se connecter' })).toBeInTheDocument()
  })

  it('shows the admin creation form on first run', async () => {
    renderPage({ setupRequired: vi.fn().mockResolvedValue(true) })
    expect(await screen.findByRole('button', { name: "Créer l'administrateur" })).toBeInTheDocument()
  })

  it('logs in and notifies the parent', async () => {
    const { client, onDone } = renderPage({})
    await userEvent.type(await screen.findByLabelText('E-mail'), 'ada@example.com')
    await userEvent.type(screen.getByLabelText('Mot de passe'), 'correct horse battery')
    await userEvent.click(screen.getByRole('button', { name: 'Se connecter' }))

    await waitFor(() => expect(onDone).toHaveBeenCalled())
    expect(client.login).toHaveBeenCalledWith('ada@example.com', 'correct horse battery')
  })

  it('creates the admin on first run', async () => {
    const { client, onDone } = renderPage({ setupRequired: vi.fn().mockResolvedValue(true) })
    await userEvent.type(await screen.findByLabelText('E-mail'), 'ada@example.com')
    await userEvent.type(screen.getByLabelText('Mot de passe'), 'correct horse battery')
    await userEvent.click(screen.getByRole('button', { name: "Créer l'administrateur" }))

    await waitFor(() => expect(onDone).toHaveBeenCalled())
    expect(client.setup).toHaveBeenCalledWith('ada@example.com', 'correct horse battery')
  })

  it('shows the server error and does not navigate', async () => {
    const { onDone } = renderPage({
      login: vi.fn().mockRejectedValue(new ApiError(401, 'identifiants invalides')),
    })
    await userEvent.type(await screen.findByLabelText('E-mail'), 'ada@example.com')
    await userEvent.type(screen.getByLabelText('Mot de passe'), 'mauvais mot de passe')
    await userEvent.click(screen.getByRole('button', { name: 'Se connecter' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('identifiants invalides')
    expect(onDone).not.toHaveBeenCalled()
  })
})
