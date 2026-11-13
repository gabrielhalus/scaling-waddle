import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../lib/api'
import { ClientProvider } from '../lib/client-context'
import { ApprovalsPage } from './approvals'

const approval = (over: object = {}) => ({
  id: 'ap1',
  run_id: 'run-1',
  step_id: 'ask',
  status: 'pending',
  message: 'Déployer ?',
  requested_at: '2026-11-13T10:00:00.000Z',
  decided_by: null,
  decided_at: null,
  comment: null,
  ...over,
})

function renderPage(items: unknown[], role = 'admin', onOpenRun = vi.fn()) {
  const client = {
    approvals: vi.fn().mockResolvedValue(items),
    me: vi.fn().mockResolvedValue({ id: '1', email: 'a@b.c', role }),
    decideApproval: vi.fn().mockResolvedValue(approval({ status: 'approved' })),
  } as unknown as ApiClient
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClientProvider value={client}>
        <ApprovalsPage onOpenRun={onOpenRun} />
      </ClientProvider>
    </QueryClientProvider>,
  )
  return { client, onOpenRun }
}

describe('ApprovalsPage', () => {
  it('lists pending approvals with their message', async () => {
    const { client } = renderPage([approval()])
    expect(await screen.findByText('Déployer ?')).toBeInTheDocument()
    expect(client.approvals).toHaveBeenCalledWith('pending')
  })

  it('shows an empty state', async () => {
    renderPage([])
    expect(await screen.findByText('Aucune demande.')).toBeInTheDocument()
  })

  it('approves with an optional comment', async () => {
    const { client } = renderPage([approval()])
    await userEvent.type(await screen.findByLabelText('Commentaire'), 'go')
    await userEvent.click(screen.getByRole('button', { name: 'Approuver' }))
    await waitFor(() => expect(client.decideApproval).toHaveBeenCalledWith('ap1', 'approve', 'go'))
  })

  it('rejects without a comment', async () => {
    const { client } = renderPage([approval()])
    await userEvent.click(await screen.findByRole('button', { name: 'Refuser' }))
    await waitFor(() =>
      expect(client.decideApproval).toHaveBeenCalledWith('ap1', 'reject', undefined),
    )
  })

  it('hides the decision buttons from viewers', async () => {
    renderPage([approval()], 'viewer')
    await screen.findByText('Déployer ?')
    expect(screen.queryByRole('button', { name: 'Approuver' })).toBeNull()
  })

  it('opens the run of a request', async () => {
    const { onOpenRun } = renderPage([approval()])
    await userEvent.click(await screen.findByRole('button', { name: /run-1/ }))
    expect(onOpenRun).toHaveBeenCalledWith('run-1')
  })

  it('switches to the history', async () => {
    const { client } = renderPage([approval()])
    await userEvent.click(await screen.findByRole('button', { name: 'Historique' }))
    await waitFor(() => expect(client.approvals).toHaveBeenLastCalledWith(undefined))
  })
})
