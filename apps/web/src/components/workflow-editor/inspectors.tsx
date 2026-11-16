import { useId, useState } from 'react'

import {
  STEP_TYPES,
  stepTypeInfo,
  type Retry,
  type Step,
  type Trigger,
  type TriggerType,
} from '../../lib/workflow-definition'
import { dangerButton, inputClass } from '../ui/styles'

const TRIGGER_TYPES: TriggerType[] = ['manual', 'webhook', 'gitlab', 'uptime-kuma', 'cron']

function display(value: unknown): string {
  if (value === undefined || value === null) return ''
  return typeof value === 'string' ? value : typeof value === 'number' ? String(value) : JSON.stringify(value, null, 2)
}

function parseText(text: string, multiline: boolean): unknown {
  const trimmed = text.trim()
  if (!trimmed) return undefined
  if (multiline && (trimmed.startsWith('{') || trimmed.startsWith('['))) {
    try {
      return JSON.parse(trimmed)
    } catch {
      return text
    }
  }
  return text
}

function Field({
  label,
  value,
  kind = 'text',
  mono,
  onCommit,
}: {
  label: string
  value: unknown
  kind?: 'text' | 'number' | 'textarea'
  mono?: boolean
  onCommit: (value: unknown) => void
}) {
  const id = useId()
  const [text, setText] = useState(display(value))
  const commit = () => {
    if (kind === 'number') onCommit(text.trim() === '' ? undefined : Number(text))
    else onCommit(parseText(text, kind === 'textarea'))
  }
  return (
    <div className="flex flex-col gap-1 text-xs">
      <label htmlFor={id}>{label}</label>
      {kind === 'textarea' ? (
        <textarea
          id={id}
          className={`${inputClass} h-24 ${mono ? 'font-mono' : ''}`}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onBlur={commit}
        />
      ) : (
        <input
          id={id}
          type={kind === 'number' ? 'number' : 'text'}
          className={`${inputClass} ${mono ? 'font-mono' : ''}`}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onBlur={commit}
        />
      )}
    </div>
  )
}

function withKey(current: Record<string, unknown>, key: string, value: unknown) {
  const next = { ...current }
  if (value === undefined) delete next[key]
  else next[key] = value
  return next
}

export function StepInspector({
  step,
  others,
  error,
  onChange,
  onToggleNeed,
  onRemove,
}: {
  step: Step
  others: string[]
  error: string | null
  onChange: (patch: Partial<Step>) => void
  onToggleNeed: (id: string, on: boolean) => void
  onRemove: () => void
}) {
  const typeId = useId()
  const info = stepTypeInfo(step.type)
  const knownType = STEP_TYPES.some((t) => t.type === step.type)
  const setRetry = (patch: Partial<Retry>) => {
    const next = { ...step.retry, ...patch }
    const empty = next.max_attempts === undefined && next.backoff_seconds === undefined
    onChange({
      retry: empty ? undefined : { max_attempts: next.max_attempts ?? 1, backoff_seconds: next.backoff_seconds ?? 1 },
    })
  }

  return (
    <div className="flex flex-col gap-3">
      <h2 className="text-sm font-semibold">Étape</h2>
      {error && (
        <p role="alert" className="text-xs text-red-600">
          {error}
        </p>
      )}
      <Field label="Identifiant" value={step.id} mono onCommit={(id) => id !== step.id && onChange({ id: String(id ?? '') })} />
      <div className="flex flex-col gap-1 text-xs">
        <label htmlFor={typeId}>Type</label>
        <select
          id={typeId}
          className={inputClass}
          value={step.type}
          onChange={(event) => onChange({ type: event.target.value, with: {} })}
        >
          {!knownType && <option value={step.type}>{step.type}</option>}
          {STEP_TYPES.map((t) => (
            <option key={t.type} value={t.type}>
              {t.label}
            </option>
          ))}
        </select>
      </div>
      {info && !info.implemented && (
        <p className="text-xs text-amber-700">Ce type passe la validation mais échouera à l'exécution.</p>
      )}
      {info?.fields.length ? (
        info.fields.map((field) => (
          <Field
            key={`${step.type}-${field.key}`}
            label={field.label}
            kind={field.kind}
            value={step.with[field.key]}
            onCommit={(value) => onChange({ with: withKey(step.with, field.key, value) })}
          />
        ))
      ) : (
        <Field
          key={step.type}
          label="Paramètres (JSON)"
          kind="textarea"
          mono
          value={step.with}
          onCommit={(value) =>
            onChange({ with: value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {} })
          }
        />
      )}
      <Field
        label="Condition (if)"
        value={step.if}
        mono
        onCommit={(value) => onChange({ if: value === undefined ? undefined : String(value) })}
      />
      <fieldset className="flex flex-col gap-1 text-xs">
        <legend className="mb-1">Dépend de</legend>
        {others.length === 0 && <span className="text-muted-foreground">Aucune autre étape.</span>}
        {others.map((id) => (
          <label key={id} className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={step.needs?.includes(id) ?? false}
              onChange={(event) => onToggleNeed(id, event.target.checked)}
            />
            {id}
          </label>
        ))}
      </fieldset>
      <div className="grid grid-cols-2 gap-2">
        <Field
          label="Tentatives max"
          kind="number"
          value={step.retry?.max_attempts}
          onCommit={(value) => setRetry({ max_attempts: value as number | undefined })}
        />
        <Field
          label="Délai entre tentatives (s)"
          kind="number"
          value={step.retry?.backoff_seconds}
          onCommit={(value) => setRetry({ backoff_seconds: value as number | undefined })}
        />
      </div>
      <button type="button" className={`${dangerButton} self-start`} onClick={onRemove}>
        Supprimer l'étape
      </button>
    </div>
  )
}

export function TriggerInspector({
  trigger,
  onChange,
}: {
  trigger: Trigger
  onChange: (trigger: Trigger) => void
}) {
  const typeId = useId()
  const setConfig = (key: string, value: unknown) => {
    const config = withKey(trigger.config ?? {}, key, value)
    onChange({ type: trigger.type, ...(Object.keys(config).length ? { config } : {}) })
  }
  return (
    <div className="flex flex-col gap-3">
      <h2 className="text-sm font-semibold">Déclencheur</h2>
      <div className="flex flex-col gap-1 text-xs">
        <label htmlFor={typeId}>Type de déclencheur</label>
        <select
          id={typeId}
          className={inputClass}
          value={trigger.type}
          onChange={(event) => onChange({ type: event.target.value as TriggerType })}
        >
          {TRIGGER_TYPES.map((type) => (
            <option key={type} value={type}>
              {type}
            </option>
          ))}
        </select>
      </div>
      {trigger.type === 'cron' && (
        <Field
          key="schedule"
          label="Planification (cron)"
          mono
          value={trigger.config?.schedule}
          onCommit={(value) => setConfig('schedule', value)}
        />
      )}
      {trigger.type !== 'cron' && trigger.type !== 'manual' && (
        <Field
          key="events"
          label="Événements (séparés par des virgules)"
          mono
          value={Array.isArray(trigger.config?.events) ? trigger.config.events.join(', ') : undefined}
          onCommit={(value) => {
            const events = String(value ?? '')
              .split(',')
              .map((e) => e.trim())
              .filter(Boolean)
            setConfig('events', events.length ? events : undefined)
          }}
        />
      )}
    </div>
  )
}
