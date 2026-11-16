import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import type { WorkflowDefinition } from '../../lib/workflow-definition'
import { WorkflowCanvas } from './workflow-canvas'

const definition: WorkflowDefinition = {
  apiVersion: 'scaling-waddle.dev/v1',
  kind: 'Workflow',
  metadata: { name: 'demo', project: 'p' },
  spec: {
    trigger: { type: 'webhook' },
    steps: [
      { id: 'check', type: 'condition', with: { expression: 'true' } },
      { id: 'notify', type: 'notification', needs: ['check'], with: {} },
    ],
  },
}

describe('WorkflowCanvas', () => {
  it('draws the trigger and one node per step with its type', () => {
    render(<WorkflowCanvas definition={definition} />)
    expect(screen.getByText('webhook')).toBeInTheDocument()
    expect(screen.getByText('check')).toBeInTheDocument()
    expect(screen.getByText('condition')).toBeInTheDocument()
    expect(screen.getByText('notify')).toBeInTheDocument()
    expect(screen.getByText('notification')).toBeInTheDocument()
  })

  it('overlays the status of each step when provided', () => {
    render(<WorkflowCanvas definition={definition} statuses={{ check: 'succeeded', notify: 'failed' }} />)
    expect(screen.getByText('succeeded')).toBeInTheDocument()
    expect(screen.getByText('failed')).toBeInTheDocument()
  })

  it('flags steps whose type is not implemented', () => {
    const scripted: WorkflowDefinition = {
      ...definition,
      spec: { ...definition.spec, steps: [{ id: 's', type: 'script.run', with: {} }] },
    }
    render(<WorkflowCanvas definition={scripted} />)
    expect(screen.getByText('non implémenté')).toBeInTheDocument()
  })
})
