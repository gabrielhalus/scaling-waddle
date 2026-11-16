import { useQuery, useQueryClient } from '@tanstack/react-query'

import { WorkflowCanvas } from '../components/workflow-editor/workflow-canvas'
import { WorkflowEditor } from '../components/workflow-editor/workflow-editor'
import { useClient } from '../lib/client-context'

export function WorkflowEditPage({ id, onDone }: { id: string; onDone: () => void }) {
  const client = useClient()
  const queryClient = useQueryClient()
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  const detail = useQuery({ queryKey: ['workflow', id], queryFn: () => client.workflow(id) })
  const workflow = detail.data?.workflow
  const version = workflow ? (workflow.active_version ?? workflow.latest_version) : null
  const definition = useQuery({
    queryKey: ['workflow-definition', id, version],
    queryFn: () => client.workflowDefinition(id, version as number),
    enabled: version !== null,
  })
  const canWrite = me.data?.role === 'admin' || me.data?.role === 'operator'

  if (detail.isError || definition.isError) return <p className="text-red-600">Workflow introuvable.</p>
  if (!workflow || !definition.data || !me.data) return null

  return (
    <section className="flex flex-col gap-3">
      <header className="flex flex-col gap-1">
        <h1 className="text-2xl font-semibold">{workflow.name}</h1>
        <span className="text-xs text-muted-foreground">
          {workflow.project} · Basé sur la version v{version}
        </span>
      </header>
      {canWrite ? (
        <WorkflowEditor
          key={version}
          initial={definition.data}
          submit={async (def) => (await client.addVersion(id, def)).version}
          onSaved={async () => {
            await queryClient.invalidateQueries({ queryKey: ['workflow', id] })
            onDone()
          }}
        />
      ) : (
        <WorkflowCanvas className="h-[560px]" definition={definition.data} />
      )}
    </section>
  )
}
