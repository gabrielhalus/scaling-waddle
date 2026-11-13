import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { useClient } from '../lib/client-context'
import { inputClass } from '../components/ui/styles'

function Trail() {
  const client = useClient()
  const [action, setAction] = useState('')
  const events = useQuery({
    queryKey: ['audit', action],
    queryFn: () => client.audit({ action: action || undefined }),
  })

  return (
    <section className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Audit</h1>
        <label className="flex items-center gap-2 text-sm">
          Action
          <input
            value={action}
            onChange={(event) => setAction(event.target.value)}
            placeholder="run.cancel"
            className={inputClass}
          />
        </label>
      </div>
      {events.isError && <p className="text-red-600">Chargement impossible.</p>}
      {events.data?.length === 0 && <p className="text-sm text-muted-foreground">Aucun événement.</p>}
      <ul className="flex flex-col divide-y divide-border rounded-lg border bg-card">
        {events.data?.map((item) => (
          <li key={item.id} className="flex flex-col gap-1 p-3 text-sm">
            <div className="flex items-center justify-between gap-4">
              <span className="font-medium">{item.action}</span>
              <span className="text-xs whitespace-nowrap text-muted-foreground">{item.at}</span>
            </div>
            <div className="flex gap-3 text-xs text-muted-foreground">
              <span>{item.actor}</span>
              <span>
                {item.target_type} · {item.target_id}
              </span>
            </div>
            <code className="overflow-x-auto text-xs whitespace-nowrap text-muted-foreground">
              {JSON.stringify(item.details)}
            </code>
          </li>
        ))}
      </ul>
    </section>
  )
}

export function AuditPage() {
  const client = useClient()
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  if (!me.data) return null
  if (me.data.role !== 'admin') {
    return <p className="text-sm text-muted-foreground">Réservé aux administrateurs.</p>
  }
  return <Trail />
}
