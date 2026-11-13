import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './lib/api'
import { App } from './router'

function fakeClient(authenticated: boolean) {
  return {
    isAuthenticated: () => authenticated,
    setupRequired: vi.fn().mockResolvedValue(false),
    me: vi.fn().mockResolvedValue({ id: '1', email: 'ada@example.com', role: 'admin' }),
    health: vi.fn().mockResolvedValue({ status: 'ok', database: 'ok' }),
    workflows: vi.fn().mockResolvedValue([]),
    runs: vi.fn().mockResolvedValue([]),
  } as unknown as ApiClient
}

describe('App routing', () => {
  beforeEach(() => window.history.replaceState(null, '', '/'))

  it('redirects an anonymous visitor to the login page', async () => {
    render(<App client={fakeClient(false)} />)
    expect(await screen.findByRole('button', { name: 'Se connecter' })).toBeInTheDocument()
    expect(window.location.pathname).toBe('/login')
  })

  it('shows the overview to an authenticated user', async () => {
    render(<App client={fakeClient(true)} />)
    expect(await screen.findByText('ada@example.com')).toBeInTheDocument()
  })

  it('sends an authenticated user away from the login page', async () => {
    window.history.replaceState(null, '', '/login')
    render(<App client={fakeClient(true)} />)
    expect(await screen.findByText('ada@example.com')).toBeInTheDocument()
  })

  it('navigates between the overview, workflows and runs', async () => {
    render(<App client={fakeClient(true)} />)
    await userEvent.click(await screen.findByRole('link', { name: 'Workflows' }))
    expect(await screen.findByText('Aucun workflow pour le moment.')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('link', { name: 'Runs' }))
    expect(await screen.findByText('Aucun run.')).toBeInTheDocument()
    expect(window.location.pathname).toBe('/runs')
  })

  it('redirects an anonymous visitor away from the runs page', async () => {
    window.history.replaceState(null, '', '/runs')
    render(<App client={fakeClient(false)} />)
    expect(await screen.findByRole('button', { name: 'Se connecter' })).toBeInTheDocument()
  })
})
