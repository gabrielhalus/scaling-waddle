import '@xyflow/react/dist/style.css'

import {
  Background,
  Controls,
  Handle,
  Position,
  ReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from '@xyflow/react'
import { useMemo } from 'react'

import { cn } from '../../lib/utils'
import { stepTypeInfo, type WorkflowDefinition } from '../../lib/workflow-definition'
import { layout, toGraph, TRIGGER_NODE_ID, type GraphNode } from '../../lib/workflow-graph'
import { StatusBadge } from '../status-badge'

type NodeData = GraphNode['data'] & { status?: string; invalid?: boolean }

function WorkflowNode({ data, selected }: NodeProps<Node<NodeData>>) {
  const unimplemented = data.stepType !== undefined && stepTypeInfo(data.stepType)?.implemented === false
  return (
    <div
      className={cn(
        'flex w-[220px] flex-col gap-1 rounded-lg border bg-card px-3 py-2 text-sm',
        data.kind === 'trigger' && 'border-primary',
        data.invalid && 'border-destructive',
        selected && 'ring-2 ring-ring',
      )}
    >
      <Handle type="target" position={Position.Left} />
      <span className="truncate font-medium">{data.label}</span>
      <span className="flex items-center justify-between gap-2 text-xs whitespace-nowrap text-muted-foreground">
        {data.stepType ?? 'déclencheur'}
        {data.status && <StatusBadge status={data.status} />}
      </span>
      {unimplemented && <span className="text-xs text-amber-700">non implémenté</span>}
      <Handle type="source" position={Position.Right} />
    </div>
  )
}

const nodeTypes = { workflow: WorkflowNode }

export function WorkflowCanvas({
  definition,
  statuses,
  selectedId,
  errorIds,
  onSelectStep,
  onConnect,
  onDisconnect,
  onRemoveStep,
  className,
}: {
  definition: WorkflowDefinition
  statuses?: Record<string, string>
  selectedId?: string | null
  errorIds?: string[]
  onSelectStep?: (id: string) => void
  onConnect?: (source: string, target: string) => void
  onDisconnect?: (source: string, target: string) => void
  onRemoveStep?: (id: string) => void
  className?: string
}) {
  const graph = useMemo(() => layout(toGraph(definition)), [definition])
  const editable = onConnect !== undefined
  const nodes = useMemo<Node<NodeData>[]>(
    () =>
      graph.nodes.map((n) => ({
        ...n,
        type: 'workflow',
        selected: n.id === selectedId,
        deletable: n.id !== TRIGGER_NODE_ID,
        data: { ...n.data, status: statuses?.[n.id], invalid: errorIds?.includes(n.id) },
      })),
    [graph, statuses, selectedId, errorIds],
  )
  const edges = useMemo<Edge[]>(
    () => graph.edges.map((e) => ({ ...e, deletable: e.source !== TRIGGER_NODE_ID })),
    [graph],
  )

  return (
    <div className={cn('h-[420px] rounded-lg border bg-background', className)}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        nodesDraggable={false}
        nodesConnectable={editable}
        elementsSelectable={onSelectStep !== undefined}
        onNodeClick={(_, node) => onSelectStep?.(node.id)}
        onConnect={(c) => onConnect?.(c.source, c.target)}
        onEdgesDelete={(deleted) => deleted.forEach((e) => onDisconnect?.(e.source, e.target))}
        onNodesDelete={(deleted) => deleted.forEach((n) => onRemoveStep?.(n.id))}
        fitView
        proOptions={{ hideAttribution: true }}
      >
        <Background />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  )
}
