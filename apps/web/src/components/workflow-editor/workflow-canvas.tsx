import '@xyflow/react/dist/style.css'

import { Background, Controls, Handle, Position, ReactFlow, type Node, type NodeProps } from '@xyflow/react'
import { useMemo } from 'react'

import { cn } from '../../lib/utils'
import { stepTypeInfo, type WorkflowDefinition } from '../../lib/workflow-definition'
import { layout, toGraph, type GraphNode } from '../../lib/workflow-graph'
import { StatusBadge } from '../status-badge'

type NodeData = GraphNode['data'] & { status?: string }

function WorkflowNode({ data, selected }: NodeProps<Node<NodeData>>) {
  const unimplemented = data.stepType !== undefined && stepTypeInfo(data.stepType)?.implemented === false
  return (
    <div
      className={cn(
        'flex w-[220px] flex-col gap-1 rounded-lg border bg-card px-3 py-2 text-sm',
        data.kind === 'trigger' && 'border-primary',
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
  onSelectStep,
  className,
}: {
  definition: WorkflowDefinition
  statuses?: Record<string, string>
  onSelectStep?: (id: string) => void
  className?: string
}) {
  const graph = useMemo(() => layout(toGraph(definition)), [definition])
  const nodes = useMemo<Node<NodeData>[]>(
    () =>
      graph.nodes.map((n) => ({
        ...n,
        type: 'workflow',
        data: { ...n.data, status: statuses?.[n.id] },
      })),
    [graph, statuses],
  )

  return (
    <div className={cn('h-[420px] rounded-lg border bg-background', className)}>
      <ReactFlow
        nodes={nodes}
        edges={graph.edges}
        nodeTypes={nodeTypes}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={onSelectStep !== undefined}
        onNodeClick={(_, node) => onSelectStep?.(node.id)}
        fitView
        proOptions={{ hideAttribution: true }}
      >
        <Background />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  )
}
