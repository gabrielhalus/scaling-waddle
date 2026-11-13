import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { StatusBadge } from '../components/status-badge'
import type { RunStatus } from '../lib/api'
import { useClient } from '../lib/client-context'

const STATUSES: RunStatus[] = ['queued', 'running', 'waiting', 'succeeded', 'failed', 'cancelled']

type Props = { workflowId?: string; onOpenRun: (id: string) => void }

export function RunsPage({ workflowId, onOpenRun }: Props) {
  const client = useClient()
  const [status, setStatus] = useState<RunStatus | ''>('')
  const runs = useQuery({
    queryKey: ['runs', workflowId, status],
    queryFn: () => client.runs({ workflowId, status: status || undefined }),
    refetchInterval: 5_000,
  })
  const workflows = useQuery({ queryKey: ['workflows'], queryFn: client.workflows })
  const names = new Map(workflows.data?.map((workflow) => [workflow.id, workflow.name]))

  return (
    <section className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Runs</h1>
        <label className="flex items-center gap-2 text-sm">
          Statut
          <select
            value={status}
            onChange={(event) => setStatus(event.target.value as RunStatus | '')}
            className="rounded border border-neutral-300 px-2 py-1"
          >
            <option value="">Tous</option>
            {STATUSES.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </select>
        </label>
      </div>
      {runs.isError && <p className="text-red-600">Chargement impossible.</p>}
      {runs.data?.length === 0 && <p className="text-sm text-neutral-600">Aucun run.</p>}
      <ul className="flex flex-col divide-y divide-neutral-200 rounded border border-neutral-200">
        {runs.data?.map((run) => (
          <li key={run.id} className="flex items-center justify-between gap-4 p-3 text-sm">
            <button
              type="button"
              onClick={() => onOpenRun(run.id)}
              className="flex flex-col gap-0.5 text-left"
            >
              <span className="font-medium">{names.get(run.workflow_id) ?? run.workflow_id}</span>
              <span className="font-mono text-xs text-neutral-500">{run.id}</span>
            </button>
            <span className="text-xs whitespace-nowrap text-neutral-500">{run.created_at}</span>
            <StatusBadge status={run.status} />
          </li>
        ))}
      </ul>
    </section>
  )
}
