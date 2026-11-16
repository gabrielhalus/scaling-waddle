import dagre from '@dagrejs/dagre'
import type { Step, WorkflowDefinition } from './workflow-definition'

export const TRIGGER_NODE_ID = '__trigger__'

const STEP_ID = /^[a-z0-9_-]{1,64}$/
const NODE_WIDTH = 220
const NODE_HEIGHT = 72

export type GraphNode = {
  id: string
  position: { x: number; y: number }
  data: { kind: 'trigger' | 'step'; label: string; stepType?: string }
}

export type GraphEdge = { id: string; source: string; target: string; label?: string }

export type Graph = { nodes: GraphNode[]; edges: GraphEdge[] }

export function toGraph(def: WorkflowDefinition): Graph {
  const nodes: GraphNode[] = [
    {
      id: TRIGGER_NODE_ID,
      position: { x: 0, y: 0 },
      data: { kind: 'trigger', label: def.spec.trigger.type },
    },
    ...def.spec.steps.map((s) => ({
      id: s.id,
      position: { x: 0, y: 0 },
      data: { kind: 'step' as const, label: s.id, stepType: s.type },
    })),
  ]
  const edges = def.spec.steps.flatMap((s) => {
    const sources = s.needs?.length ? s.needs : [TRIGGER_NODE_ID]
    return sources.map((source) => ({
      id: `${source}->${s.id}`,
      source,
      target: s.id,
      ...(s.if ? { label: s.if } : {}),
    }))
  })
  return { nodes, edges }
}

export function layout(graph: Graph): Graph {
  const g = new dagre.graphlib.Graph()
  g.setGraph({ rankdir: 'LR', nodesep: 32, ranksep: 80 })
  g.setDefaultEdgeLabel(() => ({}))
  for (const n of graph.nodes) g.setNode(n.id, { width: NODE_WIDTH, height: NODE_HEIGHT })
  for (const e of graph.edges) g.setEdge(e.source, e.target)
  dagre.layout(g)
  return {
    edges: graph.edges,
    nodes: graph.nodes.map((n) => {
      const p = g.node(n.id)
      return { ...n, position: { x: p.x - NODE_WIDTH / 2, y: p.y - NODE_HEIGHT / 2 } }
    }),
  }
}

function mapSteps(def: WorkflowDefinition, fn: (steps: Step[]) => Step[]): WorkflowDefinition {
  return { ...def, spec: { ...def.spec, steps: fn(def.spec.steps) } }
}

function withNeeds(step: Step, needs: string[]): Step {
  const { needs: _old, ...rest } = step
  return needs.length ? { ...rest, needs } : rest
}

function reaches(steps: Step[], from: string, target: string): boolean {
  const seen = new Set<string>()
  const stack = [from]
  while (stack.length) {
    const id = stack.pop() as string
    if (id === target) return true
    if (seen.has(id)) continue
    seen.add(id)
    stack.push(...(steps.find((s) => s.id === id)?.needs ?? []))
  }
  return false
}

export function addStep(def: WorkflowDefinition, type: string): WorkflowDefinition {
  const prefix = type.replace(/[^a-z0-9]+/g, '-')
  const taken = new Set(def.spec.steps.map((s) => s.id))
  let n = 1
  while (taken.has(`${prefix}-${n}`)) n++
  return mapSteps(def, (steps) => [...steps, { id: `${prefix}-${n}`, type, with: {} }])
}

export function connect(def: WorkflowDefinition, source: string, target: string): WorkflowDefinition {
  if (source === target) throw new Error('Un step ne peut pas dépendre de lui-même')
  if (source === TRIGGER_NODE_ID) {
    return mapSteps(def, (steps) => steps.map((s) => (s.id === target ? withNeeds(s, []) : s)))
  }
  const current = def.spec.steps.find((s) => s.id === target)?.needs ?? []
  if (current.includes(source)) return def
  if (reaches(def.spec.steps, source, target)) throw new Error('Cette liaison crée un cycle')
  return mapSteps(def, (steps) =>
    steps.map((s) => (s.id === target ? withNeeds(s, [...current, source]) : s)),
  )
}

export function disconnect(def: WorkflowDefinition, source: string, target: string): WorkflowDefinition {
  return mapSteps(def, (steps) =>
    steps.map((s) =>
      s.id === target ? withNeeds(s, (s.needs ?? []).filter((id) => id !== source)) : s,
    ),
  )
}

export function removeStep(def: WorkflowDefinition, id: string): WorkflowDefinition {
  return mapSteps(def, (steps) =>
    steps
      .filter((s) => s.id !== id)
      .map((s) => (s.needs?.includes(id) ? withNeeds(s, s.needs.filter((n) => n !== id)) : s)),
  )
}

export function updateStep(
  def: WorkflowDefinition,
  id: string,
  patch: Partial<Step>,
): WorkflowDefinition {
  const next = patch.id
  if (next !== undefined && next !== id) {
    if (!STEP_ID.test(next)) throw new Error(`Identifiant invalide : ${next}`)
    if (def.spec.steps.some((s) => s.id === next)) throw new Error(`Identifiant déjà utilisé : ${next}`)
  }
  return mapSteps(def, (steps) =>
    steps.map((s) => {
      const renamed = next && s.needs?.includes(id) ? withNeeds(s, s.needs.map((n) => (n === id ? next : n))) : s
      return s.id === id ? { ...renamed, ...patch } : renamed
    }),
  )
}
