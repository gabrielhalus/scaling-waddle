import { useQuery } from '@tanstack/react-query'

import { useClient } from '../lib/client-context'

export function WorkflowsPage({ onOpenRuns }: { onOpenRuns: (workflowId: string) => void }) {
  const client = useClient()
  const workflows = useQuery({ queryKey: ['workflows'], queryFn: client.workflows })

  return (
    <section className="flex flex-col gap-4">
      <h1 className="text-2xl font-semibold">Workflows</h1>
      {workflows.isError && <p className="text-red-600">Chargement impossible.</p>}
      {workflows.data?.length === 0 && (
        <p className="text-sm text-neutral-600">Aucun workflow pour le moment.</p>
      )}
      <ul className="flex flex-col divide-y divide-neutral-200 rounded border border-neutral-200">
        {workflows.data?.map((workflow) => (
          <li key={workflow.id} className="flex items-center justify-between gap-4 p-3 text-sm">
            <div className="flex flex-col gap-0.5">
              <span className="font-medium">{workflow.name}</span>
              <span className="text-xs text-neutral-500">{workflow.project}</span>
            </div>
            <span className="text-xs whitespace-nowrap text-neutral-600">
              {workflow.active_version === null
                ? `inactif · v${workflow.latest_version} dernière`
                : `v${workflow.active_version} active · v${workflow.latest_version} dernière`}
            </span>
            <button
              type="button"
              onClick={() => onOpenRuns(workflow.id)}
              className="rounded border border-neutral-300 px-3 py-1 text-xs whitespace-nowrap"
            >
              Voir les runs
            </button>
          </li>
        ))}
      </ul>
    </section>
  )
}
