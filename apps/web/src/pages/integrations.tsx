import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { useClient } from '../lib/client-context'
import { inputClass, dangerButton, primaryButton } from '../components/ui/styles'


function IntegrationForm() {
  const client = useClient()
  const queryClient = useQueryClient()
  const [project, setProject] = useState('')
  const [name, setName] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const create = useMutation({
    mutationFn: () =>
      client.createIntegration({ project, name, base_url: baseUrl, headers: {} }),
    onSuccess: () => {
      setName('')
      setBaseUrl('')
      return queryClient.invalidateQueries({ queryKey: ['integrations'] })
    },
  })

  return (
    <form
      className="flex flex-wrap items-end gap-2"
      onSubmit={(event) => {
        event.preventDefault()
        create.mutate()
      }}
    >
      <label className="flex flex-col gap-1 text-xs">
        Projet
        <input className={inputClass} value={project} onChange={(e) => setProject(e.target.value)} />
      </label>
      <label className="flex flex-col gap-1 text-xs">
        Nom
        <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="flex flex-1 flex-col gap-1 text-xs">
        URL de base
        <input className={inputClass} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} />
      </label>
      <button type="submit" className={primaryButton}>
        Créer l’intégration
      </button>
      {create.isError && <p className="w-full text-xs text-red-600">Création refusée.</p>}
    </form>
  )
}

function SecretForm() {
  const client = useClient()
  const queryClient = useQueryClient()
  const [project, setProject] = useState('')
  const [name, setName] = useState('')
  const [value, setValue] = useState('')
  const save = useMutation({
    mutationFn: () => client.setSecret(project, name, value),
    onSuccess: () => {
      setValue('')
      return queryClient.invalidateQueries({ queryKey: ['secrets'] })
    },
  })

  return (
    <form
      className="flex flex-wrap items-end gap-2"
      onSubmit={(event) => {
        event.preventDefault()
        save.mutate()
      }}
    >
      <label className="flex flex-col gap-1 text-xs">
        Projet du secret
        <input className={inputClass} value={project} onChange={(e) => setProject(e.target.value)} />
      </label>
      <label className="flex flex-col gap-1 text-xs">
        Nom du secret
        <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="flex flex-1 flex-col gap-1 text-xs">
        Valeur
        <input
          className={inputClass}
          type="password"
          autoComplete="off"
          value={value}
          onChange={(e) => setValue(e.target.value)}
        />
      </label>
      <button type="submit" className={primaryButton}>
        Enregistrer le secret
      </button>
      {save.isError && <p className="w-full text-xs text-red-600">Enregistrement refusé.</p>}
    </form>
  )
}

export function IntegrationsPage() {
  const client = useClient()
  const queryClient = useQueryClient()
  const integrations = useQuery({ queryKey: ['integrations'], queryFn: client.integrations })
  const secrets = useQuery({ queryKey: ['secrets'], queryFn: client.secrets })
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  const canWrite = me.data?.role === 'admin' || me.data?.role === 'operator'
  const removeIntegration = useMutation({
    mutationFn: (id: string) => client.deleteIntegration(id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['integrations'] }),
  })
  const removeSecret = useMutation({
    mutationFn: (secret: { project: string; name: string }) =>
      client.deleteSecret(secret.project, secret.name),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['secrets'] }),
  })

  return (
    <div className="flex flex-col gap-10">
      <section className="flex flex-col gap-4">
        <h1 className="text-2xl font-semibold">Intégrations</h1>
        {integrations.data?.length === 0 && (
          <p className="text-sm text-muted-foreground">Aucune intégration.</p>
        )}
        <ul className="flex flex-col divide-y divide-border rounded-lg border bg-card">
          {integrations.data?.map((item) => (
            <li key={item.id} className="flex items-start justify-between gap-4 p-3 text-sm">
              <div className="flex flex-col gap-1">
                <span className="font-medium">{item.name}</span>
                <span className="text-xs text-muted-foreground">{item.project}</span>
                <span className="font-mono text-xs">{item.base_url}</span>
                {Object.entries(item.headers).map(([header, shown]) => (
                  <span key={header} className="font-mono text-xs text-muted-foreground">
                    {header}: {shown}
                  </span>
                ))}
              </div>
              {canWrite && (
                <button
                  type="button"
                  aria-label={`Supprimer ${item.name}`}
                  onClick={() => removeIntegration.mutate(item.id)}
                  className={dangerButton}
                >
                  Supprimer
                </button>
              )}
            </li>
          ))}
        </ul>
        {canWrite && <IntegrationForm />}
      </section>

      <section className="flex flex-col gap-4">
        <h2 className="text-xl font-semibold">Secrets</h2>
        <p className="text-xs text-muted-foreground">
          Référencés dans les en-têtes par <code>{'${secret:nom}'}</code> ; les valeurs ne sont jamais
          affichées.
        </p>
        {secrets.data?.length === 0 && <p className="text-sm text-muted-foreground">Aucun secret.</p>}
        <ul className="flex flex-col divide-y divide-border rounded-lg border bg-card">
          {secrets.data?.map((item) => (
            <li
              key={`${item.project}/${item.name}`}
              className="flex items-center justify-between gap-4 p-3 text-sm"
            >
              <div className="flex flex-col gap-0.5">
                <span className="font-medium">{item.name}</span>
                <span className="text-xs text-muted-foreground">{item.project}</span>
              </div>
              <span className="text-xs whitespace-nowrap text-muted-foreground">clé {item.key_id}</span>
              {canWrite && (
                <button
                  type="button"
                  aria-label={`Supprimer le secret ${item.name}`}
                  onClick={() => removeSecret.mutate(item)}
                  className={dangerButton}
                >
                  Supprimer
                </button>
              )}
            </li>
          ))}
        </ul>
        {canWrite && <SecretForm />}
      </section>
    </div>
  )
}
