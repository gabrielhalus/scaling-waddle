import { describe, expect, it, vi } from 'vitest'

import { ApiError, createClient, memoryStorage } from './api'

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })

const tokens = (n: number) => ({ access_token: `a${n}`, refresh_token: `r${n}`, expires_in: 900 })

describe('api client', () => {
  it('stores tokens on login', async () => {
    const storage = memoryStorage()
    const fetchMock = vi.fn().mockResolvedValue(json(200, tokens(1)))
    const client = createClient({ fetch: fetchMock, storage })

    await client.login('ada@example.com', 'secret')

    expect(storage.get()).toEqual({ access: 'a1', refresh: 'r1' })
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/v1/auth/login',
      expect.objectContaining({ method: 'POST' }),
    )
  })

  it('sends the access token as a bearer', async () => {
    const storage = memoryStorage({ access: 'a1', refresh: 'r1' })
    const fetchMock = vi.fn().mockResolvedValue(json(200, { id: '1', email: 'x', role: 'admin' }))
    const client = createClient({ fetch: fetchMock, storage })

    await client.me()

    const init = fetchMock.mock.calls[0][1] as RequestInit
    expect(new Headers(init.headers).get('authorization')).toBe('Bearer a1')
  })

  it('refreshes once on 401 then retries with the new token', async () => {
    const storage = memoryStorage({ access: 'old', refresh: 'r1' })
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(json(401, { error: 'x' }))
      .mockResolvedValueOnce(json(200, tokens(2)))
      .mockResolvedValueOnce(json(200, { id: '1', email: 'x', role: 'admin' }))
    const client = createClient({ fetch: fetchMock, storage })

    const me = await client.me()

    expect(me.role).toBe('admin')
    expect(storage.get()).toEqual({ access: 'a2', refresh: 'r2' })
    const retry = fetchMock.mock.calls[2][1] as RequestInit
    expect(new Headers(retry.headers).get('authorization')).toBe('Bearer a2')
  })

  it('shares one refresh between concurrent 401s', async () => {
    const storage = memoryStorage({ access: 'old', refresh: 'r1' })
    const fetchMock = vi.fn(async (url: RequestInfo | URL) => {
      if (url === '/api/v1/auth/refresh') return json(200, tokens(2))
      const bearer = storage.get()?.access
      return bearer === 'a2' ? json(200, { id: '1', email: 'x', role: 'admin' }) : json(401, {})
    })
    const client = createClient({ fetch: fetchMock, storage })

    await Promise.all([client.me(), client.me()])

    const refreshes = fetchMock.mock.calls.filter(([url]) => url === '/api/v1/auth/refresh')
    expect(refreshes).toHaveLength(1)
  })

  it('clears tokens when the refresh fails', async () => {
    const storage = memoryStorage({ access: 'old', refresh: 'r1' })
    const fetchMock = vi.fn().mockResolvedValue(json(401, { error: 'x' }))
    const client = createClient({ fetch: fetchMock, storage })

    await expect(client.me()).rejects.toBeInstanceOf(ApiError)
    expect(storage.get()).toBeNull()
  })

  it('does not try to refresh a failed login', async () => {
    const storage = memoryStorage()
    const fetchMock = vi.fn().mockResolvedValue(json(401, { error: 'identifiants invalides' }))
    const client = createClient({ fetch: fetchMock, storage })

    await expect(client.login('a@b.c', 'x')).rejects.toMatchObject({
      status: 401,
      message: 'identifiants invalides',
    })
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('clears tokens on logout even if the server fails', async () => {
    const storage = memoryStorage({ access: 'a1', refresh: 'r1' })
    const fetchMock = vi.fn().mockResolvedValue(json(500, { error: 'boom' }))
    const client = createClient({ fetch: fetchMock, storage })

    await client.logout().catch(() => undefined)

    expect(storage.get()).toBeNull()
  })

  it('reads readiness without credentials', async () => {
    const storage = memoryStorage({ access: 'a1', refresh: 'r1' })
    const fetchMock = vi.fn().mockResolvedValue(json(200, { status: 'ok', database: 'ok' }))
    const client = createClient({ fetch: fetchMock, storage })

    expect(await client.health()).toEqual({ status: 'ok', database: 'ok' })
    const init = fetchMock.mock.calls[0][1] as RequestInit
    expect(new Headers(init.headers).has('authorization')).toBe(false)
  })

  it('reports a 503 readiness as unavailable instead of throwing', async () => {
    const fetchMock = vi.fn().mockResolvedValue(json(503, { status: 'unavailable', database: 'down' }))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    expect(await client.health()).toEqual({ status: 'unavailable', database: 'down' })
  })

  it('lists runs with the filters in the query string', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => json(200, []))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    await client.runs({ workflowId: 'w1', status: 'failed' })
    await client.runs()

    expect(fetchMock.mock.calls[0][0]).toBe('/api/v1/runs?workflow_id=w1&status=failed')
    expect(fetchMock.mock.calls[1][0]).toBe('/api/v1/runs')
  })

  it('reads workflows and a run detail', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => json(200, []))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    await client.workflows()
    await client.run('r 1')

    expect(fetchMock.mock.calls[0][0]).toBe('/api/v1/workflows')
    expect(fetchMock.mock.calls[1][0]).toBe('/api/v1/runs/r%201')
  })

  it('posts cancel and retry', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => json(200, { run_id: 'x', status: 'cancelled' }))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    await client.cancelRun('r1')
    await client.retryRun('r1')

    expect(fetchMock.mock.calls[0]).toEqual(['/api/v1/runs/r1/cancel', expect.objectContaining({ method: 'POST' })])
    expect(fetchMock.mock.calls[1]).toEqual(['/api/v1/runs/r1/retry', expect.objectContaining({ method: 'POST' })])
  })

  it('lists approvals by status and posts a decision', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => json(200, []))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    await client.approvals('pending')
    await client.approvals()
    await client.decideApproval('a 1', 'approve', 'ok')

    expect(fetchMock.mock.calls[0][0]).toBe('/api/v1/approvals?status=pending')
    expect(fetchMock.mock.calls[1][0]).toBe('/api/v1/approvals')
    expect(fetchMock.mock.calls[2][0]).toBe('/api/v1/approvals/a%201/decision')
    const init = fetchMock.mock.calls[2][1] as RequestInit
    expect(JSON.parse(init.body as string)).toEqual({ decision: 'approve', comment: 'ok' })
  })

  it('manages integrations and secrets', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => json(200, []))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    await client.integrations()
    await client.createIntegration({
      project: 'p',
      name: 'hook',
      base_url: 'https://x.example.com',
      headers: { authorization: 'Bearer ${secret:t}' },
    })
    await client.deleteIntegration('i 1')
    await client.secrets()
    await client.setSecret('p', 'tok', 'v')
    await client.deleteSecret('p', 'tok')

    const calls = fetchMock.mock.calls.map((call) => [call[0], (call[1] as RequestInit | undefined)?.method])
    expect(calls).toEqual([
      ['/api/v1/integrations', undefined],
      ['/api/v1/integrations', 'POST'],
      ['/api/v1/integrations/i%201', 'DELETE'],
      ['/api/v1/secrets', undefined],
      ['/api/v1/secrets/p/tok', 'PUT'],
      ['/api/v1/secrets/p/tok', 'DELETE'],
    ])
  })

  it('reads the audit trail with filters', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => json(200, []))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    await client.audit()
    await client.audit({ action: 'run.cancel' })

    expect(fetchMock.mock.calls[0][0]).toBe('/api/v1/audit')
    expect(fetchMock.mock.calls[1][0]).toBe('/api/v1/audit?action=run.cancel')
  })

  it('imports a workflow as YAML with the right content type', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => json(201, { id: 'w1', version: 1, created: true }))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    const result = await client.importWorkflow('apiVersion: x')

    expect(result).toEqual({ id: 'w1', version: 1, created: true })
    const init = fetchMock.mock.calls[0][1] as RequestInit
    expect(fetchMock.mock.calls[0][0]).toBe('/api/v1/workflows/import')
    expect(new Headers(init.headers).get('content-type')).toBe('application/yaml')
    expect(init.body).toBe('apiVersion: x')
  })

  it('reads a workflow, activates, deactivates and exports it as text', async () => {
    const fetchMock = vi
      .fn()
      .mockImplementationOnce(async () => json(200, { workflow: {}, versions: [] }))
      .mockImplementationOnce(async () => new Response(null, { status: 204 }))
      .mockImplementationOnce(async () => new Response(null, { status: 204 }))
      .mockImplementationOnce(async () => new Response('name: demo\n', { status: 200 }))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    await client.workflow('w 1')
    await client.activateWorkflow('w1', 2)
    await client.deactivateWorkflow('w1')
    const yaml = await client.exportWorkflow('w1', 2)

    expect(yaml).toBe('name: demo\n')
    const calls = fetchMock.mock.calls.map((c) => [c[0], (c[1] as RequestInit | undefined)?.method])
    expect(calls).toEqual([
      ['/api/v1/workflows/w%201', undefined],
      ['/api/v1/workflows/w1/activate', 'POST'],
      ['/api/v1/workflows/w1/deactivate', 'POST'],
      ['/api/v1/workflows/w1/export?version=2&format=yaml', undefined],
    ])
    expect(JSON.parse((fetchMock.mock.calls[1][1] as RequestInit).body as string)).toEqual({ version: 2 })
  })

  it('triggers a manual run and manages webhook tokens', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => json(200, []))
    const client = createClient({ fetch: fetchMock, storage: memoryStorage() })

    await client.triggerRun('w1', { a: 1 })
    await client.webhookTokens('w1')
    await client.issueWebhookToken('w1', true)
    await client.revokeWebhookToken('w1', 't 1')

    const calls = fetchMock.mock.calls.map((c) => [c[0], (c[1] as RequestInit | undefined)?.method])
    expect(calls).toEqual([
      ['/api/v1/workflows/w1/runs', 'POST'],
      ['/api/v1/workflows/w1/webhook-tokens', undefined],
      ['/api/v1/workflows/w1/webhook-tokens', 'POST'],
      ['/api/v1/workflows/w1/webhook-tokens/t%201', 'DELETE'],
    ])
    expect(JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string)).toEqual({ a: 1 })
    expect(JSON.parse((fetchMock.mock.calls[2][1] as RequestInit).body as string)).toEqual({ signed: true })
  })
})
