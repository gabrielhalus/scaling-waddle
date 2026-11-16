import { useMutation } from '@tanstack/react-query'
import { useMemo, useState } from 'react'

import { ApiError, type ValidationIssue } from '../../lib/api'
import { useClient } from '../../lib/client-context'
import { STEP_TYPES, type Step, type WorkflowDefinition } from '../../lib/workflow-definition'
import {
  addStep,
  connect,
  disconnect,
  removeStep,
  TRIGGER_NODE_ID,
  updateStep,
} from '../../lib/workflow-graph'
import { outlineButton, primaryButton } from '../ui/styles'
import { StepInspector, TriggerInspector } from './inspectors'
import { WorkflowCanvas } from './workflow-canvas'

export function WorkflowEditor<T>({
  initial,
  allowUnchanged = false,
  submit,
  onSaved,
}: {
  initial: WorkflowDefinition
  allowUnchanged?: boolean
  submit: (def: WorkflowDefinition) => Promise<T>
  onSaved: (result: T) => void
}) {
  const client = useClient()
  const [def, setDef] = useState(initial)
  const [selected, setSelected] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [issues, setIssues] = useState<ValidationIssue[]>([])
  const dirty = useMemo(() => JSON.stringify(def) !== JSON.stringify(initial), [def, initial])
  const errorIds = useMemo(
    () => issues.flatMap((i) => (i.step_id ? [i.step_id] : [])),
    [issues],
  )

  const apply = (fn: (current: WorkflowDefinition) => WorkflowDefinition): boolean => {
    try {
      setDef(fn(def))
      setNotice(null)
      return true
    } catch (err) {
      setNotice(err instanceof Error ? err.message : String(err))
      return false
    }
  }

  const save = useMutation({
    mutationFn: async () => {
      const result = await client.validateWorkflow(def)
      if (!result.valid) return { issues: result.issues }
      return submit(def).then((saved) => ({ saved }))
    },
    onSuccess: (result) => {
      if ('saved' in result) {
        setIssues([])
        onSaved(result.saved)
      } else setIssues(result.issues)
    },
    onError: (err) =>
      setIssues(
        err instanceof ApiError
          ? err.issues.length
            ? err.issues
            : [{ message: err.message, step_id: null }]
          : [{ message: 'Enregistrement impossible.', step_id: null }],
      ),
  })

  const step: Step | undefined = def.spec.steps.find((s) => s.id === selected)
  const select = (id: string | null) => {
    setSelected(id)
    setNotice(null)
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        {STEP_TYPES.map((t) => (
          <button
            key={t.type}
            type="button"
            className={outlineButton}
            aria-label={`Ajouter ${t.label}`}
            onClick={() => {
              const next = addStep(def, t.type)
              setDef(next)
              select(next.spec.steps.at(-1)?.id ?? null)
            }}
          >
            + {t.label}
          </button>
        ))}
        <button
          type="button"
          className={`${primaryButton} ml-auto`}
          disabled={(!dirty && !allowUnchanged) || save.isPending}
          onClick={() => save.mutate()}
        >
          Enregistrer une version
        </button>
      </div>
      {issues.length > 0 && (
        <ul className="flex flex-col gap-1 rounded-lg border border-destructive p-3 text-xs">
          {issues.map((issue, index) => (
            <li key={`${index}-${issue.message}`}>
              {issue.step_id ? (
                <button
                  type="button"
                  className="text-left text-red-600 underline"
                  onClick={() => select(issue.step_id)}
                >
                  {issue.message}
                </button>
              ) : (
                <span className="text-red-600">{issue.message}</span>
              )}
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-col gap-3 lg:flex-row">
        <WorkflowCanvas
          className="h-[560px] min-w-0 flex-1"
          definition={def}
          selectedId={selected}
          errorIds={errorIds}
          onSelectStep={select}
          onConnect={(source, target) => apply((d) => connect(d, source, target))}
          onDisconnect={(source, target) => apply((d) => disconnect(d, source, target))}
          onRemoveStep={(id) => {
            if (apply((d) => removeStep(d, id))) select(null)
          }}
        />
        <aside className="w-full shrink-0 rounded-lg border bg-card p-3 lg:w-80">
          {selected === TRIGGER_NODE_ID ? (
            <TriggerInspector
              trigger={def.spec.trigger}
              onChange={(trigger) => apply((d) => ({ ...d, spec: { ...d.spec, trigger } }))}
            />
          ) : step ? (
            <StepInspector
              key={step.id}
              step={step}
              others={def.spec.steps.filter((s) => s.id !== step.id).map((s) => s.id)}
              error={notice}
              onChange={(patch) => {
                if (apply((d) => updateStep(d, step.id, patch)) && patch.id) setSelected(patch.id)
              }}
              onToggleNeed={(id, on) =>
                apply((d) => (on ? connect(d, id, step.id) : disconnect(d, id, step.id)))
              }
              onRemove={() => {
                if (apply((d) => removeStep(d, step.id))) select(null)
              }}
            />
          ) : (
            <p className="text-xs text-muted-foreground">
              Sélectionnez un nœud pour le modifier, ou ajoutez une étape.
            </p>
          )}
        </aside>
      </div>
    </div>
  )
}
