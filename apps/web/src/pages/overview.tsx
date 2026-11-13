import { useMutation, useQuery } from '@tanstack/react-query'

import { useClient } from '../lib/client-context'

export function OverviewPage({ onLogout }: { onLogout: () => void }) {
  const client = useClient()
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  const health = useQuery({ queryKey: ['health'], queryFn: client.health, refetchInterval: 15_000 })
  const logout = useMutation({ mutationFn: client.logout, onSettled: onLogout })

  const databaseUp = health.data?.database === 'ok'

  return (
    <div className="flex flex-col gap-8">
      <header className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Overview</h1>
        <button
          type="button"
          onClick={() => logout.mutate()}
          className="rounded border border-neutral-300 px-3 py-1.5 text-sm"
        >
          Se déconnecter
        </button>
      </header>
      <section className="flex flex-col gap-1 text-sm">
        {me.data && (
          <p className="flex items-center gap-2">
            <span>{me.data.email}</span>
            <span className="rounded bg-neutral-100 px-2 py-0.5 text-xs">{me.data.role}</span>
          </p>
        )}
        {health.data && (
          <p className={databaseUp ? 'text-green-700' : 'text-red-600'}>
            Base de données : {databaseUp ? 'opérationnelle' : 'indisponible'}
          </p>
        )}
      </section>
    </div>
  )
}
