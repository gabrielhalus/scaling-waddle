import { useQueryClient } from '@tanstack/react-query'
import { useId, useState } from 'react'

import { WorkflowEditor } from '../components/workflow-editor/workflow-editor'
import { inputClass, primaryButton } from '../components/ui/styles'
import { useClient } from '../lib/client-context'
import type { WorkflowDefinition } from '../lib/workflow-definition'

function skeleton(name: string, project: string): WorkflowDefinition {
  return {
    apiVersion: 'scaling-waddle.dev/v1',
    kind: 'Workflow',
    metadata: { name, project },
    spec: {
      trigger: { type: 'manual' },
      steps: [{ id: 'start', type: 'transform', with: { value: true } }],
    },
  }
}

export function WorkflowNewPage({ onCreated }: { onCreated: (id: string) => void }) {
  const client = useClient()
  const queryClient = useQueryClient()
  const nameId = useId()
  const projectId = useId()
  const [name, setName] = useState('')
  const [project, setProject] = useState('')
  const [initial, setInitial] = useState<WorkflowDefinition | null>(null)

  return (
    <section className="flex flex-col gap-3">
      <h1 className="text-2xl font-semibold">Nouveau workflow</h1>
      {initial ? (
        <>
          <p className="text-xs text-muted-foreground">
            {initial.metadata.project} / {initial.metadata.name}
          </p>
          <WorkflowEditor
            initial={initial}
            allowUnchanged
            submit={async (def) => (await client.createWorkflow(def)).id}
            onSaved={async (id) => {
              await queryClient.invalidateQueries({ queryKey: ['workflows'] })
              onCreated(id)
            }}
          />
        </>
      ) : (
        <form
          className="flex max-w-sm flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault()
            setInitial(skeleton(name.trim(), project.trim()))
          }}
        >
          <div className="flex flex-col gap-1 text-xs">
            <label htmlFor={nameId}>Nom</label>
            <input id={nameId} className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="flex flex-col gap-1 text-xs">
            <label htmlFor={projectId}>Projet</label>
            <input
              id={projectId}
              className={inputClass}
              value={project}
              onChange={(e) => setProject(e.target.value)}
            />
          </div>
          <button
            type="submit"
            className={`${primaryButton} self-start`}
            disabled={!name.trim() || !project.trim()}
          >
            Continuer
          </button>
        </form>
      )}
    </section>
  )
}
