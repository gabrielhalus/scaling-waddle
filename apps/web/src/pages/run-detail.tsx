import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { StatusBadge } from '../components/status-badge'
import { useClient } from '../lib/client-context'

const ACTIVE = ['queued', 'running', 'waiting']
const RETRYABLE = ['failed', 'cancelled']

export function RunDetailPage({ id, onOpenRun }: { id: string; onOpenRun: (id: string) => void }) {
  const client = useClient()
  const queryClient = useQueryClient()
  const detail = useQuery({
    queryKey: ['run', id],
    queryFn: () => client.run(id),
    refetchInterval: (query) =>
      query.state.data && ACTIVE.includes(query.state.data.run.status) ? 2_000 : false,
  })
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  const refresh = () => queryClient.invalidateQueries({ queryKey: ['run', id] })
  const cancel = useMutation({ mutationFn: () => client.cancelRun(id), onSuccess: refresh })
  const retry = useMutation({
    mutationFn: () => client.retryRun(id),
    onSuccess: ({ run_id }) => onOpenRun(run_id),
  })

  if (detail.isError) return <p className="text-red-600">Run introuvable.</p>
  if (!detail.data) return null
  const { run, event, steps } = detail.data
  const canWrite = me.data?.role === 'admin' || me.data?.role === 'operator'

  return (
    <section className="flex flex-col gap-6">
      <header className="flex items-center justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="text-2xl font-semibold">Run</h1>
          <span className="font-mono text-xs text-neutral-500">{run.id}</span>
        </div>
        <div className="flex items-center gap-3">
          <StatusBadge status={run.status} />
          {canWrite && ACTIVE.includes(run.status) && (
            <button
              type="button"
              onClick={() => cancel.mutate()}
              className="rounded border border-neutral-300 px-3 py-1.5 text-sm"
            >
              Annuler
            </button>
          )}
          {canWrite && RETRYABLE.includes(run.status) && (
            <button
              type="button"
              onClick={() => retry.mutate()}
              className="rounded border border-neutral-300 px-3 py-1.5 text-sm"
            >
              Relancer
            </button>
          )}
        </div>
      </header>
      {run.error && <p className="rounded bg-red-50 p-3 text-sm text-red-700">{run.error}</p>}
      <div className="flex flex-col gap-1 text-sm">
        <h2 className="font-medium">Événement</h2>
        <p className="flex gap-2 text-neutral-600">
          <span>{event.source}</span>
          <span>{event.type}</span>
        </p>
        <pre className="overflow-x-auto rounded bg-neutral-50 p-3 text-xs">
          {JSON.stringify(event.payload, null, 2)}
        </pre>
      </div>
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">Étapes</h2>
        <ol className="flex flex-col gap-2">
          {steps.map((step) => (
            <li key={step.step_id} className="flex flex-col gap-2 rounded border border-neutral-200 p-3 text-sm">
              <div className="flex items-center justify-between">
                <span className="font-medium">{step.step_id}</span>
                <span className="flex items-center gap-2">
                  {step.attempts > 1 && (
                    <span className="text-xs text-neutral-500">{step.attempts} tentatives</span>
                  )}
                  <StatusBadge status={step.status} />
                </span>
              </div>
              {step.wake_at !== null && step.status === 'waiting' && (
                <p className="text-xs text-neutral-600">
                  reprise à {new Date(step.wake_at).toLocaleString()}
                </p>
              )}
              {step.error && <p className="text-xs text-red-700">{step.error}</p>}
              {step.output !== null && (
                <pre className="overflow-x-auto rounded bg-neutral-50 p-2 text-xs">
                  {JSON.stringify(step.output, null, 2)}
                </pre>
              )}
            </li>
          ))}
        </ol>
      </div>
    </section>
  )
}
