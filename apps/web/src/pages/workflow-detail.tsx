import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { useClient } from '../lib/client-context'

const input = 'rounded border border-neutral-300 px-2 py-1 text-sm'
const button = 'rounded border border-neutral-300 px-3 py-1 text-xs whitespace-nowrap'

function Versions({ id, canWrite }: { id: string; canWrite: boolean }) {
  const client = useClient()
  const queryClient = useQueryClient()
  const detail = useQuery({ queryKey: ['workflow', id], queryFn: () => client.workflow(id) })
  const [shown, setShown] = useState<{ version: number; yaml: string } | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: ['workflow', id] })
  const activate = useMutation({
    mutationFn: (version: number) => client.activateWorkflow(id, version),
    onSuccess: refresh,
  })
  const deactivate = useMutation({ mutationFn: () => client.deactivateWorkflow(id), onSuccess: refresh })
  const view = useMutation({
    mutationFn: async (version: number) => ({
      version,
      yaml: await client.exportWorkflow(id, version),
    }),
    onSuccess: setShown,
  })
  if (!detail.data) return detail.isError ? <p className="text-red-600">Workflow introuvable.</p> : null
  const { workflow, versions } = detail.data

  return (
    <section className="flex flex-col gap-3">
      <header className="flex items-center justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="text-2xl font-semibold">{workflow.name}</h1>
          <span className="text-xs text-neutral-500">{workflow.project}</span>
        </div>
        {canWrite && workflow.active_version !== null && (
          <button type="button" className={button} onClick={() => deactivate.mutate()}>
            Désactiver
          </button>
        )}
      </header>
      <ul className="flex flex-col divide-y divide-neutral-200 rounded border border-neutral-200">
        {versions.map((item) => {
          const active = item.version === workflow.active_version
          return (
            <li key={item.version} className="flex items-center justify-between gap-3 p-3 text-sm">
              <span className="font-medium">{active ? `v${item.version} · active` : `v${item.version}`}</span>
              <span className="font-mono text-xs text-neutral-500">{item.hash.slice(0, 8)}</span>
              <span className="text-xs whitespace-nowrap text-neutral-500">{item.created_at}</span>
              <span className="flex gap-2">
                <button
                  type="button"
                  className={button}
                  aria-label={`Voir le YAML v${item.version}`}
                  onClick={() => view.mutate(item.version)}
                >
                  YAML
                </button>
                {canWrite && !active && (
                  <button
                    type="button"
                    className={button}
                    aria-label={`Activer v${item.version}`}
                    onClick={() => activate.mutate(item.version)}
                  >
                    Activer
                  </button>
                )}
              </span>
            </li>
          )
        })}
      </ul>
      {shown && (
        <pre className="overflow-x-auto rounded bg-neutral-50 p-3 text-xs">{shown.yaml}</pre>
      )}
    </section>
  )
}

function ImportVersion({ id }: { id: string }) {
  const client = useClient()
  const queryClient = useQueryClient()
  const [text, setText] = useState('')
  const send = useMutation({
    mutationFn: () => client.importWorkflow(text),
    onSuccess: () => {
      setText('')
      return queryClient.invalidateQueries({ queryKey: ['workflow', id] })
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
        Nouvelle version (YAML)
        <textarea
          className={`${input} h-40 font-mono`}
          value={text}
          onChange={(event) => setText(event.target.value)}
        />
      </label>
      <button type="submit" className={`${button} self-start`}>
        Importer la version
      </button>
      {send.isError && <p className="text-xs text-red-600">Définition refusée.</p>}
    </form>
  )
}

function ManualRun({ id, onOpenRun }: { id: string; onOpenRun: (id: string) => void }) {
  const client = useClient()
  const [payload, setPayload] = useState('{}')
  const [invalid, setInvalid] = useState(false)
  const run = useMutation({
    mutationFn: (body: unknown) => client.triggerRun(id, body),
    onSuccess: ({ run_id }) => onOpenRun(run_id),
  })
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault()
        try {
          const parsed: unknown = JSON.parse(payload)
          setInvalid(false)
          run.mutate(parsed)
        } catch {
          setInvalid(true)
        }
      }}
    >
      <label className="flex flex-col gap-1 text-xs">
        Payload JSON
        <textarea
          className={`${input} h-24 font-mono`}
          value={payload}
          onChange={(event) => setPayload(event.target.value)}
        />
      </label>
      {invalid && <p className="text-xs text-red-600">Payload JSON invalide.</p>}
      {run.isError && <p className="text-xs text-red-600">Lancement refusé (workflow actif ?).</p>}
      <button type="submit" className={`${button} self-start`}>
        Lancer
      </button>
    </form>
  )
}

function WebhookTokens({ id, canWrite }: { id: string; canWrite: boolean }) {
  const client = useClient()
  const queryClient = useQueryClient()
  const tokens = useQuery({ queryKey: ['webhook-tokens', id], queryFn: () => client.webhookTokens(id) })
  const [signed, setSigned] = useState(false)
  const [fresh, setFresh] = useState<{ token: string; secret: string | null } | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: ['webhook-tokens', id] })
  const issue = useMutation({
    mutationFn: () => client.issueWebhookToken(id, signed),
    onSuccess: (issued) => {
      setFresh({ token: issued.token, secret: issued.signing_secret })
      return refresh()
    },
  })
  const revoke = useMutation({
    mutationFn: (tokenId: string) => client.revokeWebhookToken(id, tokenId),
    onSuccess: refresh,
  })

  return (
    <div className="flex flex-col gap-3">
      <ul className="flex flex-col divide-y divide-neutral-200 rounded border border-neutral-200">
        {tokens.data?.length === 0 && <li className="p-3 text-sm text-neutral-600">Aucun jeton.</li>}
        {tokens.data?.map((item) => (
          <li key={item.id} className="flex items-center justify-between gap-3 p-3 text-sm">
            <span className="font-mono text-xs">{item.id}</span>
            {item.signed && <span className="text-xs">signé</span>}
            <span className="text-xs whitespace-nowrap text-neutral-500">
              {item.revoked_at ? `révoqué ${item.revoked_at}` : item.created_at}
            </span>
            {canWrite && !item.revoked_at && (
              <button
                type="button"
                className={button}
                aria-label={`Révoquer ${item.id}`}
                onClick={() => revoke.mutate(item.id)}
              >
                Révoquer
              </button>
            )}
          </li>
        ))}
      </ul>
      {canWrite && (
        <div className="flex items-center gap-3">
          <label className="flex items-center gap-2 text-xs">
            <input type="checkbox" checked={signed} onChange={(e) => setSigned(e.target.checked)} />
            Signé (HMAC)
          </label>
          <button type="button" className={button} onClick={() => issue.mutate()}>
            Créer un jeton
          </button>
        </div>
      )}
      {fresh && (
        <div className="flex flex-col gap-1 rounded bg-amber-50 p-3 text-xs">
          <p>Copiez ces valeurs maintenant : elles ne seront plus affichées.</p>
          <code className="break-all">{fresh.token}</code>
          {fresh.secret && <code className="break-all">{fresh.secret}</code>}
          <code className="break-all text-neutral-600">
            URL : {window.location.origin}/api/v1/triggers/webhook/{fresh.token}
          </code>
        </div>
      )}
    </div>
  )
}

export function WorkflowDetailPage({ id, onOpenRun }: { id: string; onOpenRun: (id: string) => void }) {
  const client = useClient()
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  const canWrite = me.data?.role === 'admin' || me.data?.role === 'operator'

  return (
    <div className="flex flex-col gap-10">
      <Versions id={id} canWrite={canWrite} />
      {canWrite && (
        <>
          <section className="flex flex-col gap-3">
            <h2 className="text-xl font-semibold">Nouvelle version</h2>
            <ImportVersion id={id} />
          </section>
          <section className="flex flex-col gap-3">
            <h2 className="text-xl font-semibold">Lancement manuel</h2>
            <ManualRun id={id} onOpenRun={onOpenRun} />
          </section>
        </>
      )}
      <section className="flex flex-col gap-3">
        <h2 className="text-xl font-semibold">Jetons de webhook</h2>
        <WebhookTokens id={id} canWrite={canWrite} />
      </section>
    </div>
  )
}
