import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import './index.css'
import { browserStorage, createClient } from './lib/api'
import { App } from './router'

const client = createClient({ storage: browserStorage() })

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App client={client} />
  </StrictMode>,
)
