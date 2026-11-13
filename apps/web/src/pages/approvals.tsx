import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { StatusBadge } from '../components/status-badge'
import type { Approval } from '../lib/api'
import { useClient } from '../lib/client-context'
import { inputClass, outlineButton, dangerButton } from '../components/ui/styles'

function Request({ approval, canDecide, onOpenRun }: {
  approval: Approval
  canDecide: boolean
  onOpenRun: (id: string) => void
}) {
  const client = useClient()
  const queryClient = useQueryClient()
  const [comment, setComment] = useState('')
  const decide = useMutation({
    mutationFn: (decision: 'approve' | 'reject') =>
      client.decideApproval(approval.id, decision, comment || undefined),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['approvals'] }),
  })

  return (
    <li className="flex flex-col gap-2 p-3 text-sm">
      <div className="flex items-center justify-between gap-4">
        <span className="font-medium">{approval.message ?? approval.step_id}</span>
        <StatusBadge status={approval.status} />
      </div>
      <div className="flex items-center gap-3 text-xs text-muted-foreground">
        <button type="button" onClick={() => onOpenRun(approval.run_id)} className="font-mono">
          {approval.run_id}
        </button>
        <span className="whitespace-nowrap">{approval.requested_at}</span>
        {approval.decided_by && <span>par {approval.decided_by}</span>}
      </div>
      {approval.comment && <p className="text-xs text-muted-foreground">{approval.comment}</p>}
      {canDecide && approval.status === 'pending' && (
        <div className="flex items-center gap-2">
          <label className="flex flex-1 items-center gap-2 text-xs">
            Commentaire
            <input
              value={comment}
              onChange={(event) => setComment(event.target.value)}
              className={`${inputClass} flex-1`}
            />
          </label>
          <button
            type="button"
            onClick={() => decide.mutate('approve')}
            className={outlineButton}
          >
            Approuver
          </button>
          <button
            type="button"
            onClick={() => decide.mutate('reject')}
            className={dangerButton}
          >
            Refuser
          </button>
        </div>
      )}
    </li>
  )
}

export function ApprovalsPage({ onOpenRun }: { onOpenRun: (id: string) => void }) {
  const client = useClient()
  const [history, setHistory] = useState(false)
  const status = history ? undefined : ('pending' as const)
  const approvals = useQuery({
    queryKey: ['approvals', status],
    queryFn: () => client.approvals(status),
    refetchInterval: 5_000,
  })
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  const canDecide = me.data?.role === 'admin' || me.data?.role === 'operator'

  return (
    <section className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Approbations</h1>
        <button
          type="button"
          onClick={() => setHistory((value) => !value)}
          className={outlineButton}
        >
          {history ? 'En attente' : 'Historique'}
        </button>
      </div>
      {approvals.isError && <p className="text-red-600">Chargement impossible.</p>}
      {approvals.data?.length === 0 && <p className="text-sm text-muted-foreground">Aucune demande.</p>}
      <ul className="flex flex-col divide-y divide-border rounded-lg border bg-card">
        {approvals.data?.map((approval) => (
          <Request
            key={approval.id}
            approval={approval}
            canDecide={canDecide}
            onOpenRun={onOpenRun}
          />
        ))}
      </ul>
    </section>
  )
}
