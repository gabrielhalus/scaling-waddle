export type TriggerType = 'manual' | 'webhook' | 'gitlab' | 'uptime-kuma' | 'cron'

export type Trigger = { type: TriggerType; config?: Record<string, unknown> }

export type Retry = { max_attempts: number; backoff_seconds: number }

export type Step = {
  id: string
  type: string
  needs?: string[]
  if?: string
  with: Record<string, unknown>
  retry?: Retry
}

export type WorkflowDefinition = {
  apiVersion: 'scaling-waddle.dev/v1'
  kind: 'Workflow'
  metadata: { name: string; project: string }
  spec: { trigger: Trigger; concurrency?: unknown; steps: Step[] }
}

export type StepField = { key: string; label: string; kind: 'text' | 'number' | 'textarea' }

export type StepTypeInfo = { type: string; label: string; fields: StepField[]; implemented: boolean }

const httpFields: StepField[] = [
  { key: 'integration', label: 'Intégration', kind: 'text' },
  { key: 'method', label: 'Méthode', kind: 'text' },
  { key: 'path', label: 'Chemin', kind: 'text' },
  { key: 'body', label: 'Corps', kind: 'textarea' },
  { key: 'timeout_secs', label: 'Délai max (s)', kind: 'number' },
]

export const STEP_TYPES: StepTypeInfo[] = [
  { type: 'http.request', label: 'Requête HTTP', fields: httpFields, implemented: true },
  {
    type: 'condition',
    label: 'Condition',
    fields: [{ key: 'expression', label: 'Expression', kind: 'textarea' }],
    implemented: true,
  },
  {
    type: 'transform',
    label: 'Transformation',
    fields: [{ key: 'value', label: 'Valeur', kind: 'textarea' }],
    implemented: true,
  },
  {
    type: 'delay',
    label: 'Délai',
    fields: [{ key: 'seconds', label: 'Secondes', kind: 'number' }],
    implemented: true,
  },
  {
    type: 'notification',
    label: 'Notification',
    fields: [...httpFields, { key: 'message', label: 'Message', kind: 'textarea' }],
    implemented: true,
  },
  {
    type: 'approval',
    label: 'Approbation',
    fields: [
      { key: 'message', label: 'Message', kind: 'textarea' },
      { key: 'timeout_seconds', label: 'Délai max (s)', kind: 'number' },
    ],
    implemented: true,
  },
  {
    type: 'agent.run',
    label: 'Agent',
    fields: [
      { key: 'prompt', label: 'Prompt', kind: 'textarea' },
      { key: 'model', label: 'Modèle', kind: 'text' },
      { key: 'timeout_seconds', label: 'Délai max (s)', kind: 'number' },
    ],
    implemented: true,
  },
  { type: 'script.run', label: 'Script', fields: [], implemented: false },
]

export function stepTypeInfo(type: string): StepTypeInfo | undefined {
  const known = STEP_TYPES.find((t) => t.type === type)
  if (known) return known
  if (type.startsWith('integration.')) return { type, label: type, fields: [], implemented: false }
  return undefined
}
