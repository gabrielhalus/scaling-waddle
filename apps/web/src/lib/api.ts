export type Tokens = { access: string; refresh: string }

export type User = { id: string; email: string; role: 'admin' | 'operator' | 'viewer' }

type TokenResponse = { access_token: string; refresh_token: string; expires_in: number }

export type TokenStorage = {
  get: () => Tokens | null
  set: (tokens: Tokens | null) => void
}

export class ApiError extends Error {
  readonly status: number

  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

export function memoryStorage(initial: Tokens | null = null): TokenStorage {
  let current = initial
  return { get: () => current, set: (tokens) => (current = tokens) }
}

const KEY = 'sw.tokens'

export function browserStorage(): TokenStorage {
  return {
    get: () => {
      try {
        const raw = localStorage.getItem(KEY)
        return raw ? (JSON.parse(raw) as Tokens) : null
      } catch {
        return null
      }
    },
    set: (tokens) => {
      try {
        if (tokens) localStorage.setItem(KEY, JSON.stringify(tokens))
        else localStorage.removeItem(KEY)
      } catch {
        // stockage indisponible : la session ne survivra pas au rechargement
      }
    },
  }
}

type Options = { fetch?: typeof fetch; storage: TokenStorage }

export function createClient({ fetch: fetcher = (...args) => fetch(...args), storage }: Options) {
  let inflightRefresh: Promise<boolean> | null = null

  async function send(path: string, init: RequestInit = {}, withAuth = true) {
    const headers = new Headers(init.headers)
    if (init.body) headers.set('content-type', 'application/json')
    const access = storage.get()?.access
    if (withAuth && access) headers.set('authorization', `Bearer ${access}`)
    return fetcher(path, { ...init, headers })
  }

  async function failure(res: Response): Promise<ApiError> {
    const body = (await res.json().catch(() => null)) as { error?: string } | null
    return new ApiError(res.status, body?.error ?? res.statusText)
  }

  function refresh(): Promise<boolean> {
    inflightRefresh ??= (async () => {
      const current = storage.get()
      if (!current) return false
      const res = await send(
        '/api/v1/auth/refresh',
        { method: 'POST', body: JSON.stringify({ refresh_token: current.refresh }) },
        false,
      )
      if (!res.ok) {
        storage.set(null)
        return false
      }
      const body = (await res.json()) as TokenResponse
      storage.set({ access: body.access_token, refresh: body.refresh_token })
      return true
    })().finally(() => {
      inflightRefresh = null
    })
    return inflightRefresh
  }

  async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
    let res = await send(path, init)
    if (res.status === 401 && storage.get() && (await refresh())) {
      res = await send(path, init)
    }
    if (!res.ok) throw await failure(res)
    return res.status === 204 ? (undefined as T) : ((await res.json()) as T)
  }

  async function authenticate(path: string, email: string, password: string) {
    const res = await send(
      path,
      { method: 'POST', body: JSON.stringify({ email, password }) },
      false,
    )
    if (!res.ok) throw await failure(res)
    const body = (await res.json()) as TokenResponse
    storage.set({ access: body.access_token, refresh: body.refresh_token })
  }

  return {
    login: (email: string, password: string) => authenticate('/api/v1/auth/login', email, password),
    setup: (email: string, password: string) => authenticate('/api/v1/auth/setup', email, password),
    setupRequired: async () => {
      const body = await request<{ required: boolean }>('/api/v1/auth/setup')
      return body.required
    },
    me: () => request<User>('/api/v1/auth/me'),
    logout: async () => {
      try {
        await request<void>('/api/v1/auth/logout', { method: 'POST' })
      } finally {
        storage.set(null)
      }
    },
    isAuthenticated: () => storage.get() !== null,
  }
}

export type ApiClient = ReturnType<typeof createClient>
