import { describe, expect, it } from 'vitest'
import {
  addStep,
  connect,
  disconnect,
  layout,
  removeStep,
  toGraph,
  TRIGGER_NODE_ID,
  updateStep,
} from './workflow-graph'
import type { WorkflowDefinition } from './workflow-definition'

function def(steps: WorkflowDefinition['spec']['steps']): WorkflowDefinition {
  return {
    apiVersion: 'scaling-waddle.dev/v1',
    kind: 'Workflow',
    metadata: { name: 'demo', project: 'p' },
    spec: { trigger: { type: 'manual' }, steps },
  }
}

const base = def([
  { id: 'a', type: 'transform', with: { value: '1' } },
  { id: 'b', type: 'condition', needs: ['a'], with: { expression: 'true' } },
  { id: 'c', type: 'delay', needs: ['b'], if: 'steps.b.result == true', with: { seconds: 5 } },
])

describe('toGraph', () => {
  it('ajoute un nœud trigger et relie les steps sans needs au trigger', () => {
    const g = toGraph(base)
    expect(g.nodes.map((n) => n.id)).toEqual([TRIGGER_NODE_ID, 'a', 'b', 'c'])
    expect(g.edges).toContainEqual(expect.objectContaining({ source: TRIGGER_NODE_ID, target: 'a' }))
  })

  it('crée une arête par needs et porte le if en label', () => {
    const g = toGraph(base)
    expect(g.edges).toContainEqual(expect.objectContaining({ source: 'a', target: 'b' }))
    expect(g.edges).toContainEqual(
      expect.objectContaining({ source: 'b', target: 'c', label: 'steps.b.result == true' }),
    )
  })
})

describe('layout', () => {
  it('place les dépendances à gauche de leurs dépendants', () => {
    const g = layout(toGraph(base))
    const x = (id: string) => g.nodes.find((n) => n.id === id)!.position.x
    expect(x(TRIGGER_NODE_ID)).toBeLessThan(x('a'))
    expect(x('a')).toBeLessThan(x('b'))
    expect(x('b')).toBeLessThan(x('c'))
  })
})

describe('addStep', () => {
  it('génère un id unique à partir du type', () => {
    const d1 = addStep(base, 'delay')
    const d2 = addStep(d1, 'delay')
    const ids = d2.spec.steps.map((s) => s.id)
    expect(new Set(ids).size).toBe(ids.length)
    expect(ids.at(-1)).toMatch(/^[a-z0-9_-]{1,64}$/)
  })

  it('ne mute pas la définition source', () => {
    addStep(base, 'delay')
    expect(base.spec.steps).toHaveLength(3)
  })
})

describe('connect / disconnect', () => {
  it('ajoute et retire une dépendance', () => {
    const d = connect(base, 'a', 'c')
    expect(d.spec.steps[2]?.needs).toEqual(['b', 'a'])
    expect(disconnect(d, 'a', 'c').spec.steps[2]?.needs).toEqual(['b'])
  })

  it('refuse un cycle', () => {
    expect(() => connect(base, 'c', 'a')).toThrow(/cycle/i)
  })

  it('refuse une auto-dépendance et un doublon', () => {
    expect(() => connect(base, 'a', 'a')).toThrow()
    expect(connect(base, 'a', 'b')).toEqual(base)
  })

  it('relier le trigger à un step retire ses needs', () => {
    const d = connect(base, TRIGGER_NODE_ID, 'c')
    expect(d.spec.steps[2]?.needs).toBeUndefined()
  })

  it('retirer la dernière dépendance supprime needs', () => {
    expect(disconnect(base, 'a', 'b').spec.steps[1]?.needs).toBeUndefined()
  })
})

describe('removeStep', () => {
  it('retire le step et nettoie les needs qui le référencent', () => {
    const d = removeStep(base, 'b')
    expect(d.spec.steps.map((s) => s.id)).toEqual(['a', 'c'])
    expect(d.spec.steps[1]?.needs).toBeUndefined()
  })
})

describe('updateStep', () => {
  it('remplace les champs et renomme les références', () => {
    const d = updateStep(base, 'b', { id: 'check' })
    expect(d.spec.steps[1]?.id).toBe('check')
    expect(d.spec.steps[2]?.needs).toEqual(['check'])
  })

  it('refuse un id invalide ou déjà pris', () => {
    expect(() => updateStep(base, 'b', { id: 'A b' })).toThrow()
    expect(() => updateStep(base, 'b', { id: 'a' })).toThrow()
  })
})
