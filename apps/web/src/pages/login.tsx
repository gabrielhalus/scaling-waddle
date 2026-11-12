import { useMutation, useQuery } from '@tanstack/react-query'
import { type FormEvent, useState } from 'react'

import { useClient } from '../lib/client-context'

export function LoginPage({ onDone }: { onDone: () => void }) {
  const client = useClient()
  const setupRequired = useQuery({ queryKey: ['setup-required'], queryFn: client.setupRequired })
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')

  const submit = useMutation({
    mutationFn: () => (setupRequired.data ? client.setup(email, password) : client.login(email, password)),
    onSuccess: onDone,
  })

  if (setupRequired.isPending) return <p className="p-8 text-sm text-neutral-500">Chargement…</p>

  const first = setupRequired.data === true
  const onSubmit = (event: FormEvent) => {
    event.preventDefault()
    submit.mutate()
  }

  return (
    <main className="mx-auto flex min-h-screen max-w-sm flex-col justify-center gap-6 p-6">
      <header>
        <h1 className="text-2xl font-semibold">scaling-waddle</h1>
        <p className="text-sm text-neutral-500">
          {first ? 'Créez le compte administrateur.' : 'Connectez-vous à votre instance.'}
        </p>
      </header>
      <form onSubmit={onSubmit} className="flex flex-col gap-4">
        <label className="flex flex-col gap-1 text-sm">
          E-mail
          <input
            type="email"
            required
            autoComplete="username"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            className="rounded border border-neutral-300 px-3 py-2"
          />
        </label>
        <label className="flex flex-col gap-1 text-sm">
          Mot de passe
          <input
            type="password"
            required
            minLength={12}
            autoComplete={first ? 'new-password' : 'current-password'}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            className="rounded border border-neutral-300 px-3 py-2"
          />
        </label>
        {submit.isError && (
          <p role="alert" className="text-sm text-red-600">
            {submit.error.message}
          </p>
        )}
        <button
          type="submit"
          disabled={submit.isPending}
          className="rounded bg-neutral-900 px-3 py-2 text-sm font-medium text-white disabled:opacity-50"
        >
          {first ? "Créer l'administrateur" : 'Se connecter'}
        </button>
      </form>
    </main>
  )
}
