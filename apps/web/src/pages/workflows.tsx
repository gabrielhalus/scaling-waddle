import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { useClient } from '../lib/client-context'

function ImportForm({ onOpen }: { onOpen: (workflowId: string) => void }) {
  const client = useClient()
  const queryClient = useQueryClient()
  const [text, setText] = useState('')
  const send = useMutation({
    mutationFn: () => client.importWorkflow(text),
    onSuccess: async ({ id }) => {
      setText('')
      await queryClient.invalidateQueries({ queryKey: ['workflows'] })
      onOpen(id)
    },
  })
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault()
        send.mutate()
      }}
    >
      <label className="flex flex-col gap-1 text-xs">
        Définition (YAML)
        <textarea
          className="h-40 rounded border border-neutral-300 px-2 py-1 font-mono text-sm"
          value={text}
          onChange={(event) => setText(event.target.value)}
        />
      </label>
      <button
        type="submit"
        className="self-start rounded border border-neutral-300 px-3 py-1 text-xs"
      >
        Importer
      </button>
      {send.isError && <p className="text-xs text-red-600">Définition refusée.</p>}
    </form>
  )
}

type Props = {
  onOpenRuns: (workflowId: string) => void
  onOpen: (workflowId: string) => void
}

export function WorkflowsPage({ onOpenRuns, onOpen }: Props) {
  const client = useClient()
  const workflows = useQuery({ queryKey: ['workflows'], queryFn: client.workflows })
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  const canWrite = me.data?.role === 'admin' || me.data?.role === 'operator'

  return (
    <section className="flex flex-col gap-6">
      <h1 className="text-2xl font-semibold">Workflows</h1>
      {workflows.isError && <p className="text-red-600">Chargement impossible.</p>}
      {workflows.data?.length === 0 && (
        <p className="text-sm text-neutral-600">Aucun workflow pour le moment.</p>
      )}
      <ul className="flex flex-col divide-y divide-neutral-200 rounded border border-neutral-200">
        {workflows.data?.map((workflow) => (
          <li key={workflow.id} className="flex items-center justify-between gap-4 p-3 text-sm">
            <div className="flex flex-col gap-0.5 text-left">
              <button type="button" onClick={() => onOpen(workflow.id)} className="text-left font-medium">
                {workflow.name}
              </button>
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
      {canWrite && <ImportForm onOpen={onOpen} />}
    </section>
  )
}
