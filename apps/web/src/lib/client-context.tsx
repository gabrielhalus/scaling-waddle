import { createContext, useContext } from 'react'

import type { ApiClient } from './api'

const ClientContext = createContext<ApiClient | null>(null)

export const ClientProvider = ClientContext.Provider

export function useClient(): ApiClient {
  const client = useContext(ClientContext)
  if (!client) throw new Error('ClientProvider manquant')
  return client
}
