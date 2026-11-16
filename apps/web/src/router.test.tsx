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
    approvals: vi.fn().mockResolvedValue([]),
    integrations: vi.fn().mockResolvedValue([]),
    secrets: vi.fn().mockResolvedValue([]),
    audit: vi.fn().mockResolvedValue([]),
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

  it('opens the approvals page from the navigation', async () => {
    render(<App client={fakeClient(true)} />)
    await userEvent.click(await screen.findByRole('link', { name: 'Approbations' }))
    expect(await screen.findByText('Aucune demande.')).toBeInTheDocument()
  })

  it('opens the integrations and audit pages from the navigation', async () => {
    render(<App client={fakeClient(true)} />)
    await userEvent.click(await screen.findByRole('link', { name: 'Intégrations' }))
    expect(await screen.findByText('Aucune intégration.')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('link', { name: 'Audit' }))
    expect(await screen.findByText('Aucun événement.')).toBeInTheDocument()
  })

  it('opens a workflow detail page from the list', async () => {
    const client = {
      ...fakeClient(true),
      workflows: vi.fn().mockResolvedValue([
        { id: 'w1', project: 'ops', name: 'notify', active_version: 1, latest_version: 1 },
      ]),
      workflow: vi.fn().mockResolvedValue({
        workflow: { id: 'w1', project: 'ops', name: 'notify', active_version: 1, latest_version: 1 },
        versions: [{ version: 1, hash: 'abcdef123456', created_at: '2026-11-13T10:00:00.000Z' }],
      }),
      webhookTokens: vi.fn().mockResolvedValue([]),
    } as unknown as ApiClient
    window.history.replaceState(null, '', '/workflows')
    render(<App client={client} />)
    await userEvent.click(await screen.findByRole('button', { name: 'notify' }))
    expect(await screen.findByText('v1 · active')).toBeInTheDocument()
    expect(window.location.pathname).toBe('/workflows/w1')
  })

  it('opens the visual editor in full width from a workflow', async () => {
    const detail = {
      workflow: { id: 'w1', project: 'ops', name: 'notify', active_version: 1, latest_version: 1 },
      versions: [{ version: 1, hash: 'abcdef123456', created_at: '2026-11-13T10:00:00.000Z' }],
    }
    const client = {
      ...fakeClient(true),
      workflow: vi.fn().mockResolvedValue(detail),
      webhookTokens: vi.fn().mockResolvedValue([]),
      workflowDefinition: vi.fn().mockResolvedValue({
        apiVersion: 'scaling-waddle.dev/v1',
        kind: 'Workflow',
        metadata: { name: 'notify', project: 'ops' },
        spec: { trigger: { type: 'manual' }, steps: [{ id: 'check', type: 'condition', with: {} }] },
      }),
    } as unknown as ApiClient
    window.history.replaceState(null, '', '/workflows/w1')
    render(<App client={client} />)
    await userEvent.click(await screen.findByRole('button', { name: 'Éditer' }))
    expect(await screen.findByRole('button', { name: 'Enregistrer une version' })).toBeInTheDocument()
    expect(window.location.pathname).toBe('/workflows/w1/edit')
    expect(screen.getByRole('main')).not.toHaveClass('max-w-5xl')
  })

  it('opens the new workflow page from the list', async () => {
    window.history.replaceState(null, '', '/workflows')
    render(<App client={fakeClient(true)} />)
    await userEvent.click(await screen.findByRole('button', { name: 'Nouveau workflow' }))
    expect(await screen.findByLabelText('Nom')).toBeInTheDocument()
    expect(window.location.pathname).toBe('/workflows/new')
  })
})
