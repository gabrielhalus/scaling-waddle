import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import type { ManagedUser, User } from '../lib/api'
import { useClient } from '../lib/client-context'
import { inputClass, primaryButton, dangerButton } from '../components/ui/styles'

const ROLES: User['role'][] = ['admin', 'operator', 'viewer']

function UserRow({ user, self }: { user: ManagedUser; self: boolean }) {
  const client = useClient()
  const queryClient = useQueryClient()
  const [password, setPassword] = useState('')
  const refresh = () => queryClient.invalidateQueries({ queryKey: ['users'] })
  const changeRole = useMutation({
    mutationFn: (role: User['role']) => client.updateUser(user.id, { role }),
    onSuccess: refresh,
  })
  const changePassword = useMutation({
    mutationFn: () => client.updateUser(user.id, { password }),
    onSuccess: () => setPassword(''),
  })
  const remove = useMutation({ mutationFn: () => client.deleteUser(user.id), onSuccess: refresh })

  return (
    <li className="flex flex-col gap-2 p-3 text-sm">
      <div className="flex items-center justify-between gap-3">
        <span className="font-medium">{user.email}</span>
        <select
          aria-label={`Rôle de ${user.email}`}
          className={inputClass}
          value={user.role}
          disabled={self}
          onChange={(event) => changeRole.mutate(event.target.value as User['role'])}
        >
          {ROLES.map((role) => (
            <option key={role} value={role}>
              {role}
            </option>
          ))}
        </select>
        {!self && (
          <button
            type="button"
            className={dangerButton}
            aria-label={`Supprimer ${user.email}`}
            onClick={() => remove.mutate()}
          >
            Supprimer
          </button>
        )}
      </div>
      <form
        className="flex items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          changePassword.mutate()
        }}
      >
        <input
          type="password"
          autoComplete="new-password"
          aria-label={`Nouveau mot de passe de ${user.email}`}
          className={`${inputClass} flex-1`}
          value={password}
          onChange={(event) => setPassword(event.target.value)}
        />
        <button
          type="submit"
          className={primaryButton}
          aria-label={`Changer le mot de passe de ${user.email}`}
        >
          Changer
        </button>
      </form>
      {(changeRole.isError || changePassword.isError || remove.isError) && (
        <p className="text-xs text-red-600">Action refusée.</p>
      )}
    </li>
  )
}

function CreateUser() {
  const client = useClient()
  const queryClient = useQueryClient()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [role, setRole] = useState<User['role']>('viewer')
  const create = useMutation({
    mutationFn: () => client.createUser({ email, password, role }),
    onSuccess: () => {
      setEmail('')
      setPassword('')
      return queryClient.invalidateQueries({ queryKey: ['users'] })
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
      <label className="flex flex-1 flex-col gap-1 text-xs">
        E-mail
        <input className={inputClass} value={email} onChange={(e) => setEmail(e.target.value)} />
      </label>
      <label className="flex flex-1 flex-col gap-1 text-xs">
        Mot de passe
        <input
          className={inputClass}
          type="password"
          autoComplete="new-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </label>
      <label className="flex flex-col gap-1 text-xs">
        Rôle
        <select
          className={inputClass}
          value={role}
          onChange={(e) => setRole(e.target.value as User['role'])}
        >
          {ROLES.map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </select>
      </label>
      <button type="submit" className={primaryButton}>
        Créer l’utilisateur
      </button>
      {create.isError && (
        <p className="w-full text-xs text-red-600">
          Création refusée (e-mail déjà pris, mot de passe de 12 caractères minimum ?).
        </p>
      )}
    </form>
  )
}

function Rotation() {
  const client = useClient()
  const rotate = useMutation({ mutationFn: client.rotateSecrets })
  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs text-muted-foreground">
        À lancer après avoir changé <code>SW_SECRET_ACTIVE_KEY</code> : rechiffre les secrets avec la clé
        active.
      </p>
      <button type="button" className={`${primaryButton} self-start`} onClick={() => rotate.mutate()}>
        Rechiffrer les secrets
      </button>
      {rotate.data && <p className="text-xs">{rotate.data.rotated} secret(s) rechiffré(s).</p>}
      {rotate.isError && <p className="text-xs text-red-600">Rotation impossible (clés non configurées ?).</p>}
    </div>
  )
}

function Admin({ selfId }: { selfId: string }) {
  const client = useClient()
  const users = useQuery({ queryKey: ['users'], queryFn: client.users })
  return (
    <div className="flex flex-col gap-10">
      <section className="flex flex-col gap-4">
        <h1 className="text-2xl font-semibold">Utilisateurs</h1>
        <ul className="flex flex-col divide-y divide-border rounded-lg border bg-card">
          {users.data?.map((user) => <UserRow key={user.id} user={user} self={user.id === selfId} />)}
        </ul>
        <CreateUser />
      </section>
      <section className="flex flex-col gap-3">
        <h2 className="text-xl font-semibold">Clés de chiffrement</h2>
        <Rotation />
      </section>
    </div>
  )
}

export function SettingsPage() {
  const client = useClient()
  const me = useQuery({ queryKey: ['me'], queryFn: client.me })
  if (!me.data) return null
  if (me.data.role !== 'admin') {
    return <p className="text-sm text-muted-foreground">Réservé aux administrateurs.</p>
  }
  return <Admin selfId={me.data.id} />
}
