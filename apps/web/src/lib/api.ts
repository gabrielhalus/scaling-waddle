export type Tokens = { access: string; refresh: string }

export type User = { id: string; email: string; role: 'admin' | 'operator' | 'viewer' }

export type Health = { status: 'ok' | 'unavailable'; database: 'ok' | 'down' }

export type Workflow = {
  id: string
  project: string
  name: string
  active_version: number | null
  latest_version: number
}

export type RunStatus = 'queued' | 'running' | 'waiting' | 'succeeded' | 'failed' | 'cancelled'

export type Run = {
  id: string
  workflow_id: string
  version: number
  event_id: string
  status: RunStatus
  error: string | null
  created_at: string
  finished_at: string | null
}

export type StepRun = {
  step_id: string
  status: string
  attempts: number
  output: unknown
  error: string | null
  started_at: string | null
  finished_at: string | null
  wake_at: number | null
}

export type RunDetail = {
  run: Run
  event: { source: string; type: string; payload: unknown; correlation_id: string }
  steps: StepRun[]
}

export type ApprovalStatus = 'pending' | 'approved' | 'rejected' | 'expired' | 'cancelled'

export type Approval = {
  id: string
  run_id: string
  step_id: string
  status: ApprovalStatus
  message: string | null
  requested_at: string
  decided_by: string | null
  decided_at: string | null
  comment: string | null
}

export type Integration = {
  id: string
  project: string
  name: string
  base_url: string
  headers: Record<string, string>
  created_at: string
}

export type NewIntegration = Omit<Integration, 'id' | 'created_at'>

export type Secret = {
  project: string
  name: string
  key_id: string
  created_at: string
  updated_at: string
}

export type AuditEvent = {
  id: number
  at: string
  actor: string
  action: string
  target_type: string
  target_id: string
  details: unknown
}

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
    health: async (): Promise<Health> => {
      const res = await send('/health/ready', {}, false)
      return (await res.json()) as Health
    },
    workflows: () => request<Workflow[]>('/api/v1/workflows'),
    runs: (filter: { workflowId?: string; status?: RunStatus } = {}) => {
      const query = new URLSearchParams()
      if (filter.workflowId) query.set('workflow_id', filter.workflowId)
      if (filter.status) query.set('status', filter.status)
      const text = query.toString()
      return request<Run[]>(`/api/v1/runs${text ? `?${text}` : ''}`)
    },
    run: (id: string) => request<RunDetail>(`/api/v1/runs/${encodeURIComponent(id)}`),
    cancelRun: (id: string) =>
      request<{ run_id: string; status: string }>(`/api/v1/runs/${encodeURIComponent(id)}/cancel`, {
        method: 'POST',
      }),
    retryRun: (id: string) =>
      request<{ run_id: string }>(`/api/v1/runs/${encodeURIComponent(id)}/retry`, { method: 'POST' }),
    approvals: (status?: ApprovalStatus) =>
      request<Approval[]>(`/api/v1/approvals${status ? `?status=${status}` : ''}`),
    decideApproval: (id: string, decision: 'approve' | 'reject', comment?: string) =>
      request<Approval>(`/api/v1/approvals/${encodeURIComponent(id)}/decision`, {
        method: 'POST',
        body: JSON.stringify({ decision, comment }),
      }),
    integrations: () => request<Integration[]>('/api/v1/integrations'),
    createIntegration: (input: NewIntegration) =>
      request<Integration>('/api/v1/integrations', { method: 'POST', body: JSON.stringify(input) }),
    deleteIntegration: (id: string) =>
      request<void>(`/api/v1/integrations/${encodeURIComponent(id)}`, { method: 'DELETE' }),
    secrets: () => request<Secret[]>('/api/v1/secrets'),
    setSecret: (project: string, name: string, value: string) =>
      request<void>(`/api/v1/secrets/${encodeURIComponent(project)}/${encodeURIComponent(name)}`, {
        method: 'PUT',
        body: JSON.stringify({ value }),
      }),
    deleteSecret: (project: string, name: string) =>
      request<void>(`/api/v1/secrets/${encodeURIComponent(project)}/${encodeURIComponent(name)}`, {
        method: 'DELETE',
      }),
    audit: (filter: { action?: string } = {}) =>
      request<AuditEvent[]>(
        `/api/v1/audit${filter.action ? `?action=${encodeURIComponent(filter.action)}` : ''}`,
      ),
    isAuthenticated: () => storage.get() !== null,
  }
}

export type ApiClient = ReturnType<typeof createClient>
