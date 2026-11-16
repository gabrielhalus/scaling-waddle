import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useMemo, useState } from 'react'

import { StatusBadge } from '../components/status-badge'
import { WorkflowCanvas } from '../components/workflow-editor/workflow-canvas'
import { useClient } from '../lib/client-context'
import { outlineButton, dangerButton } from '../components/ui/styles'

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
  const run = detail.data?.run
  const definition = useQuery({
    queryKey: ['workflow-definition', run?.workflow_id, run?.version],
    queryFn: () => client.workflowDefinition(run?.workflow_id as string, run?.version as number),
    enabled: run !== undefined,
    retry: false,
  })
  const [selectedStep, setSelectedStep] = useState<string | null>(null)
  const statuses = useMemo(
    () => Object.fromEntries((detail.data?.steps ?? []).map((s) => [s.step_id, s.status])),
    [detail.data?.steps],
  )
  const refresh = () => queryClient.invalidateQueries({ queryKey: ['run', id] })
  const cancel = useMutation({ mutationFn: () => client.cancelRun(id), onSuccess: refresh })
  const retry = useMutation({
    mutationFn: () => client.retryRun(id),
    onSuccess: ({ run_id }) => onOpenRun(run_id),
  })

  if (detail.isError) return <p className="text-red-600">Run introuvable.</p>
  if (!detail.data || !run) return null
  const { event, steps } = detail.data
  const canWrite = me.data?.role === 'admin' || me.data?.role === 'operator'

  return (
    <section className="flex flex-col gap-6">
      <header className="flex items-center justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="text-2xl font-semibold">Run</h1>
          <span className="font-mono text-xs text-muted-foreground">{run.id}</span>
        </div>
        <div className="flex items-center gap-3">
          <StatusBadge status={run.status} />
          {canWrite && ACTIVE.includes(run.status) && (
            <button
              type="button"
              onClick={() => cancel.mutate()}
              className={dangerButton}
            >
              Annuler
            </button>
          )}
          {canWrite && RETRYABLE.includes(run.status) && (
            <button
              type="button"
              onClick={() => retry.mutate()}
              className={outlineButton}
            >
              Relancer
            </button>
          )}
        </div>
      </header>
      {run.error && <p className="rounded bg-red-50 p-3 text-sm text-red-700">{run.error}</p>}
      {definition.data && (
        <WorkflowCanvas
          definition={definition.data}
          statuses={statuses}
          selectedId={selectedStep}
          onSelectStep={(stepId) => {
            setSelectedStep(stepId)
            document
              .getElementById(`step-${stepId}`)
              ?.scrollIntoView?.({ behavior: 'smooth', block: 'nearest' })
          }}
        />
      )}
      <div className="flex flex-col gap-1 text-sm">
        <h2 className="font-medium">Événement</h2>
        <p className="flex gap-2 text-muted-foreground">
          <span>{event.source}</span>
          <span>{event.type}</span>
        </p>
        <pre className="overflow-x-auto rounded bg-muted p-3 text-xs">
          {JSON.stringify(event.payload, null, 2)}
        </pre>
      </div>
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">Étapes</h2>
        <ol className="flex flex-col gap-2">
          {steps.map((step) => (
            <li
              key={step.step_id}
              id={`step-${step.step_id}`}
              aria-current={step.step_id === selectedStep ? 'true' : undefined}
              className={`flex flex-col gap-2 rounded-lg border bg-card p-3 text-sm ${step.step_id === selectedStep ? 'ring-2 ring-ring' : ''}`}
            >
              <div className="flex items-center justify-between">
                <span className="font-medium">{step.step_id}</span>
                <span className="flex items-center gap-2">
                  {step.attempts > 1 && (
                    <span className="text-xs text-muted-foreground">{step.attempts} tentatives</span>
                  )}
                  <StatusBadge status={step.status} />
                </span>
              </div>
              {step.wake_at !== null && step.status === 'waiting' && (
                <p className="text-xs text-muted-foreground">
                  reprise à {new Date(step.wake_at).toLocaleString()}
                </p>
              )}
              {step.error && <p className="text-xs text-red-700">{step.error}</p>}
              {step.output !== null && (
                <pre className="overflow-x-auto rounded bg-muted p-2 text-xs">
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
