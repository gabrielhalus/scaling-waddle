const COLORS: Record<string, string> = {
  succeeded: 'bg-green-100 text-green-800',
  failed: 'bg-red-100 text-red-800',
  running: 'bg-blue-100 text-blue-800',
  queued: 'bg-neutral-100 text-neutral-700',
  waiting: 'bg-amber-100 text-amber-800',
  cancelled: 'bg-neutral-200 text-neutral-600',
  skipped: 'bg-neutral-100 text-neutral-500',
  pending: 'bg-neutral-100 text-neutral-500',
}

export function StatusBadge({ status }: { status: string }) {
  return (
    <span
      className={`rounded px-2 py-0.5 text-xs whitespace-nowrap ${COLORS[status] ?? 'bg-neutral-100'}`}
    >
      {status}
    </span>
  )
}
