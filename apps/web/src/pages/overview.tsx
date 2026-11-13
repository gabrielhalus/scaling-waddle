import { useMutation, useQuery } from '@tanstack/react-query'

import { StatusBadge } from '../components/status-badge'
import { useClient } from '../lib/client-context'
import { outlineButton } from '../components/ui/styles'

export function OverviewPage({
  onLogout,
  onOpenRun,
}: {
  onLogout: () => void
  onOpenRun: (id: string) => void
}) {
  const client = useClient()
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  const health = useQuery({ queryKey: ['health'], queryFn: client.health, refetchInterval: 15_000 })
  const workflows = useQuery({ queryKey: ['workflows'], queryFn: client.workflows })
  const runs = useQuery({
    queryKey: ['runs', undefined, ''],
    queryFn: () => client.runs(),
    refetchInterval: 10_000,
  })
  const approvals = useQuery({ queryKey: ['approvals', 'pending'], queryFn: () => client.approvals('pending') })
  const logout = useMutation({ mutationFn: client.logout, onSettled: onLogout })

  const active = workflows.data?.filter((workflow) => workflow.active_version !== null).length
  const counts = new Map<string, number>()
  runs.data?.forEach((run) => counts.set(run.status, (counts.get(run.status) ?? 0) + 1))
  const databaseUp = health.data?.database === 'ok'

  return (
    <div className="flex flex-col gap-8">
      <header className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Overview</h1>
        <button
          type="button"
          onClick={() => logout.mutate()}
          className={outlineButton}
        >
          Se déconnecter
        </button>
      </header>
      <section className="flex flex-col gap-1 text-sm">
        {me.data && (
          <p className="flex items-center gap-2">
            <span>{me.data.email}</span>
            <span className="rounded bg-muted px-2 py-0.5 text-xs">{me.data.role}</span>
          </p>
        )}
        {health.data && (
          <p className={databaseUp ? 'text-green-700' : 'text-red-600'}>
            Base de données : {databaseUp ? 'opérationnelle' : 'indisponible'}
          </p>
        )}
      </section>
      <section className="flex flex-col gap-1 text-sm">
        {workflows.data && (
          <p>
            Workflows actifs : {active} / {workflows.data.length}
          </p>
        )}
        {approvals.data && <p>Approbations en attente : {approvals.data.length}</p>}
      </section>
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-semibold">Activité récente</h2>
        <div className="flex flex-wrap gap-3 text-xs">
          {[...counts.entries()].map(([status, count]) => (
            <span key={status} className="rounded bg-muted px-2 py-1">
              {status} : {count}
            </span>
          ))}
        </div>
        <ul className="flex flex-col divide-y divide-border rounded-lg border bg-card">
          {runs.data?.slice(0, 5).map((run) => (
            <li key={run.id} className="flex items-center justify-between gap-3 p-3 text-sm">
              <button type="button" onClick={() => onOpenRun(run.id)} className="font-mono text-xs">
                {run.id}
              </button>
              <span className="text-xs whitespace-nowrap text-muted-foreground">{run.created_at}</span>
              <StatusBadge status={run.status} />
            </li>
          ))}
        </ul>
      </section>
    </div>
  )
}
