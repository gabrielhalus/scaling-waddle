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
})
