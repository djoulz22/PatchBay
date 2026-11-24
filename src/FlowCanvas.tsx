import { AudioLines, CircleDot, Headphones, Laptop, Maximize, Mic, Minus, PanelTop, Plus, Radio, SlidersHorizontal, Volume2, Workflow } from 'lucide-react'
import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useState, useSyncExternalStore } from 'react'
import {
  applyNodeChanges, Background, BaseEdge, getBezierPath, Handle, Panel, Position, ReactFlow,
  useNodesInitialized, useReactFlow, useStore, useUpdateNodeInternals,
  type Connection, type Edge, type EdgeProps, type Node, type NodeProps, type OnEdgesChange, type OnNodesChange,
} from '@xyflow/react'
import type { MeterStore } from './meters'
import {
  canSource, canTarget, channelCount, channelLabel, deviceAvailable, isMasterChannel, isMixStage, isVirtualDevice, labelFor,
  MASTER_CHANNELS, validConnection, variantFor,
  type AudioCatalog, type Endpoints, type NodeVariant, type Project, type Route, type Selection,
} from './model'

type Card = {
  id: string
  label: string
  variant: NodeVariant
  hasInput: boolean
  hasOutput: boolean
  available: boolean
  isInterface: boolean
  channels: number
  visibleChannels: number[]
  channelLabels: Record<number, string>
  expanded: boolean
  connectedInputs: number[]
  connectedOutputs: number[]
  virtualInput: boolean
  setExpanded: (id: string) => void
}
type CardNode = Node<Card, 'device'>
type Cable = Edge<{ route: Route; variant: NodeVariant }, 'cable'>
type KeyPort = { nodeId: string; channel: number; inbound: boolean }

export type FlowProps = {
  project: Project
  catalog: AudioCatalog
  selection: Selection
  topologyLocked: boolean
  offline: readonly string[]
  meters: MeterStore
  onSelect: (selection: Selection) => void
  onConnect: (endpoints: Endpoints) => void
  onMove: (id: string, x: number, y: number) => void
  onDelete: () => void
}

const Meters = createContext<MeterStore | null>(null)
const KeyboardCable = createContext<{ port: KeyPort | null; choose: (port: KeyPort | null) => void } | null>(null)
const icons = { input: Mic, system: Laptop, app: Volume2, browserTab: PanelTop, tone: AudioLines, output: Headphones, virtual: Radio, internalBus: Workflow, outputChannels: SlidersHorizontal }
const fitOptions = { minZoom: 0.24, maxZoom: 1.03, padding: 0.12 }

function handleId(channel: number, inbound: boolean) {
  return `${inbound ? 'in' : 'out'}-${channel}`
}

function Port({ data, channel, name, inbound, connectable }: {
  data: Card
  channel: number
  name: string
  inbound: boolean
  connectable: boolean
}) {
  const cable = useContext(KeyboardCable)
  const keyActive = cable?.port?.nodeId === data.id && cable.port.channel === channel && cable.port.inbound === inbound
  return (
    <Handle id={handleId(channel, inbound)} type={inbound ? 'target' : 'source'}
      position={inbound ? Position.Left : Position.Right}
      isConnectable={connectable} data-connected={(inbound ? data.connectedInputs : data.connectedOutputs).includes(channel)}
      className="node-port nodrag nopan" role="button" aria-label={`${inbound ? 'Entrée' : 'Sortie'} ${name} · ${data.label}`} aria-pressed={keyActive}
      aria-disabled={!connectable}
      tabIndex={connectable ? 0 : -1}
      onKeyDown={(event) => {
        if (!connectable || !['Enter', ' ', 'Escape'].includes(event.key)) return
        event.preventDefault()
        event.stopPropagation()
        cable?.choose(event.key === 'Escape' ? null : { nodeId: data.id, channel, inbound })
      }}
    />
  )
}

function ChannelRows({ data, connectable }: { data: Card; connectable: boolean }) {
  const store = useContext(Meters)
  const subscribe = useCallback((callback: () => void) => store?.subscribe(data.id, callback) ?? (() => {}), [store, data.id])
  const read = useCallback(() => store?.get(data.id), [store, data.id])
  const meter = useSyncExternalStore(subscribe, read, read)
  return (
    <div className="node-channels">
      {data.visibleChannels.map((channel) => {
        const name = data.channelLabels[channel]
        const master = data.variant === 'outputChannels' && isMasterChannel(channel)
        const masterIndex = channel - MASTER_CHANNELS[0]
        const archived = !master && channel >= data.channels
        const amplitude = (master ? meter?.master?.channels[masterIndex] : meter?.channels[channel]) ?? 0
        const db = amplitude > 0 ? Math.max(-60, Math.min(0, 20 * Math.log10(amplitude))) : -60
        const fill = (db + 60) / 60 * 100
        const clipped = (master ? meter?.master?.clippedChannels[masterIndex] : meter?.clippedChannels?.[channel]) ?? amplitude >= 1
        return (
          <div className="node-channel" key={channel} data-master={master} data-archived={archived}
            title={archived ? 'Canal enregistré hors de la capacité actuelle du périphérique' : undefined}>
            <span className="channel-letter" aria-hidden="true">{name}</span>
            <div className="mini-meter" aria-label={`Niveau ${name} de ${data.label}`} role="meter" aria-valuemin={-60}
              aria-valuemax={0} aria-valuenow={Math.round(db)} aria-valuetext={db === -60 ? 'Silence' : `${Math.round(db)} dBFS`}>
              <span style={{ width: `${fill}%` }} data-clipped={clipped} />
            </div>
            <span className="channel-db" aria-hidden="true">{db === -60 ? '−∞' : Math.round(db)}</span>
            {data.hasInput && !master && <Port data={data} channel={channel} name={name} inbound connectable={connectable && !archived} />}
            {data.hasOutput && <Port data={data} channel={channel} name={name} inbound={false} connectable={connectable && !archived} />}
          </div>
        )
      })}
    </div>
  )
}

function DeviceNode({ data, isConnectable, selected }: NodeProps<CardNode>) {
  const updateInternals = useUpdateNodeInternals()
  const portKey = data.visibleChannels.join(',')
  useLayoutEffect(() => { updateInternals(data.id) }, [data.id, portKey, updateInternals])
  const Icon = icons[data.variant]
  const title = data.virtualInput ? 'Entrée virtuelle' : labelFor(data.variant)
  const visible = data.visibleChannels.filter((channel) => channel < data.channels).length
  const expandable = data.expanded || visible < data.channels
  const unit = data.isInterface ? data.hasInput ? 'sortie' : 'entrée' : 'canal'
  const units = data.channels === 1 ? unit : unit === 'canal' ? 'canaux' : `${unit}s`
  const count = `${visible < data.channels ? `${visible} / ` : ''}${data.channels} ${units}`
  const showPortSummary = data.isInterface || expandable
  return (
    <article className="patch-node" data-variant={data.variant} data-selected={selected}>
      <header className="node-header">
        <span className="node-icon"><Icon size={17} strokeWidth={1.8} aria-hidden="true" /></span>
        <div className="node-name">
          <span className="node-kind">{title}</span>
          <strong title={data.label}>{data.label}</strong>
        </div>
        {data.variant !== 'tone' && data.variant !== 'internalBus' && data.variant !== 'outputChannels' && <span role="img" className="node-availability" data-available={data.available}
          title={data.available ? 'Disponible' : data.variant === 'app' ? 'Application en attente' : data.variant === 'browserTab' ? 'Onglet en attente' : 'Indisponible'}
          aria-label={data.available ? 'Disponible' : 'En attente'} />}
      </header>
      {showPortSummary && <div className="node-port-summary">
        <span>{count}{data.isInterface && !data.available ? ' hors ligne' : ''}</span>
        {expandable && <button type="button" className="node-expand nodrag nopan" onClick={() => data.setExpanded(data.id)}
          aria-expanded={data.expanded}>{data.expanded ? 'Réduire' : 'Tous les canaux'}</button>}
      </div>}
      <ChannelRows data={data} connectable={isConnectable} />
      {!showPortSummary && <div className="node-footer">
        <span>{data.variant === 'outputChannels' || data.variant === 'internalBus' ? `${data.channels} ${data.channels === 1 ? 'canal' : 'canaux'}`
          : data.channels === 1 ? 'Mono' : data.channels === 2 ? 'Stéréo' : `${data.channels} canaux`}</span>
        <span className="node-unit">dBFS</span>
      </div>}
    </article>
  )
}

function CableEdge({ id, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, selected, data }: EdgeProps<Cable>) {
  const [path] = getBezierPath({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, curvature: 0.32 })
  return (
    <g className="patch-cable" data-variant={data?.variant} data-selected={selected} data-muted={data?.route.muted}>
      <path className="patch-cable-halo" d={path} fill="none" />
      <BaseEdge id={id} path={path} interactionWidth={22} />
    </g>
  )
}

const nodeTypes = { device: DeviceNode }
const edgeTypes = { cable: CableEdge }

function ViewportTools() {
  const { zoomIn, zoomOut, fitView } = useReactFlow<CardNode, Cable>()
  return (
    <Panel position="bottom-left" className="canvas-controls">
      <button type="button" title="Réduire" aria-label="Réduire le zoom" onClick={() => void zoomOut({ duration: 110 })}><Minus size={15} /></button>
      <button type="button" title="Agrandir" aria-label="Agrandir le zoom" onClick={() => void zoomIn({ duration: 110 })}><Plus size={15} /></button>
      <span />
      <button type="button" title="Recentrer (F)" aria-label="Recentrer le graphe" onClick={() => void fitView({ ...fitOptions, duration: 180 })}><Maximize size={14} /></button>
    </Panel>
  )
}

function FitCanvas({ nodeIds }: { nodeIds: string }) {
  const { fitView } = useReactFlow<CardNode, Cable>()
  const ready = useNodesInitialized()
  const width = useStore((state) => state.width)
  const height = useStore((state) => state.height)
  useEffect(() => {
    if (!ready || width <= 0 || height <= 0) return
    const operation = window.setTimeout(() => { void fitView(fitOptions) }, 90)
    return () => window.clearTimeout(operation)
  }, [fitView, ready, nodeIds, width, height])
  return null
}

function CanvasKeys({ onDelete, editable }: { onDelete: () => void; editable: boolean }) {
  const { fitView } = useReactFlow<CardNode, Cable>()
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return
      if (event.target instanceof HTMLElement && event.target.closest('input, textarea, select, [contenteditable], dialog, [role="dialog"]')) return
      if (event.key.toLowerCase() === 'f') {
        event.preventDefault()
        void fitView({ ...fitOptions, duration: 180 })
      }
      if (editable && ['Delete', 'Backspace'].includes(event.key)) {
        event.preventDefault()
        onDelete()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [editable, fitView, onDelete])
  return null
}

function toEndpoints(connection: Connection): Endpoints | null {
  if (!connection.sourceHandle?.startsWith('out-') || !connection.targetHandle?.startsWith('in-')) return null
  return {
    source: connection.source, target: connection.target,
    sourceChannel: Number(connection.sourceHandle.slice(4)), targetChannel: Number(connection.targetHandle.slice(3)),
  }
}

export function FlowCanvas({ project, catalog, selection, topologyLocked, offline, meters, onSelect, onConnect, onMove, onDelete }: FlowProps) {
  const [expanded, setExpanded] = useState<string[]>([])
  const [port, setPort] = useState<KeyPort | null>(null)
  const toggle = useCallback((id: string) => {
    setExpanded((current) => current.includes(id) ? current.filter((expandedId) => expandedId !== id) : [...current, id])
  }, [])
  const displayed = useMemo(() => project.nodes.map((node): CardNode => {
    const stage = isMixStage(node.kind)
    const channels = channelCount(node, catalog, project)
    const connectedInputs = project.routes.filter((route) => route.target === node.id).map((route) => route.targetChannel)
    const connectedOutputs = project.routes.filter((route) => route.source === node.id).map((route) => route.sourceChannel)
    const connected = new Set([...connectedInputs, ...connectedOutputs])
    const unfolded = expanded.includes(node.id)
    const visibleChannels = Array.from({ length: channels }, (_, index) => index)
      .filter((index) => unfolded || index < (stage ? 8 : 2) || connected.has(index))
    // Keep the handle of an existing cable visible when its hardware reports fewer channels.
    const archived = [...connected].filter((channel) => channel >= channels && channel < MASTER_CHANNELS[0])
    visibleChannels.push(...archived.sort((left, right) => left - right))
    if (node.kind.type === 'outputChannels') visibleChannels.push(...MASTER_CHANNELS)
    return {
      id: node.id, type: 'device', position: { x: node.x, y: node.y },
      selected: selection?.kind === 'node' && selection.id === node.id,
      data: {
        id: node.id, label: node.label, variant: variantFor(node.kind, catalog), hasInput: canTarget(node), hasOutput: canSource(node),
        available: !offline.includes(node.id) && deviceAvailable(node, catalog), channels, visibleChannels, expanded: unfolded,
        isInterface: node.kind.type === 'deviceInput' || node.kind.type === 'deviceOutput',
        channelLabels: Object.fromEntries(visibleChannels.map((channel) => [channel, channelLabel(node, channel, catalog, project)])),
        connectedInputs, connectedOutputs,
        virtualInput: node.kind.type === 'deviceInput' && isVirtualDevice(node.kind.deviceId, catalog),
        setExpanded: toggle,
      },
    }
  }), [project, catalog, selection, offline, expanded, toggle])
  const [nodes, setNodes] = useState(displayed)
  useEffect(() => {
    setNodes((current) => displayed.map((node) => {
      const previous = current.find((item) => item.id === node.id)
      return previous ? { ...previous, ...node, position: previous.dragging ? previous.position : node.position } : node
    }))
  }, [displayed])
  const edges = useMemo(() => project.routes.map((route): Cable => {
    const target = project.nodes.find((node) => node.id === route.target)
    return {
      id: route.id, source: route.source, target: route.target,
      sourceHandle: handleId(route.sourceChannel, false), targetHandle: handleId(route.targetChannel, true),
      type: 'cable', selected: selection?.kind === 'route' && selection.id === route.id,
      data: { route, variant: target ? variantFor(target.kind, catalog) : 'output' },
    }
  }), [project, catalog, selection])
  const updateNodes: OnNodesChange<CardNode> = useCallback((changes) => {
    const positions = changes.filter((change) => change.type === 'position' || change.type === 'dimensions')
    if (positions.length) setNodes((current) => applyNodeChanges(positions, current))
    const selected = changes.find((change) => change.type === 'select' && change.selected)
    if (selected?.type === 'select') onSelect({ kind: 'node', id: selected.id })
  }, [onSelect])
  const updateEdges: OnEdgesChange<Cable> = useCallback((changes) => {
    const selected = changes.find((change) => change.type === 'select' && change.selected)
    if (selected?.type === 'select') onSelect({ kind: 'route', id: selected.id })
  }, [onSelect])

  function choose(next: KeyPort | null) {
    if (topologyLocked || !next) { setPort(null); return }
    if (!port || port.inbound === next.inbound) { setPort(next); return }
    const [source, target] = port.inbound ? [next, port] : [port, next]
    onConnect({ source: source.nodeId, sourceChannel: source.channel, target: target.nodeId, targetChannel: target.channel })
    setPort(null)
  }

  return (
    <Meters.Provider value={meters}>
      <KeyboardCable.Provider value={{ port, choose }}>
        <ReactFlow<CardNode, Cable> aria-label="Graphe des connexions audio" nodes={nodes} edges={edges}
          nodeTypes={nodeTypes} edgeTypes={edgeTypes} onNodesChange={updateNodes} onEdgesChange={updateEdges}
          fitView fitViewOptions={fitOptions} minZoom={0.24} maxZoom={2.25}
          deleteKeyCode={null} multiSelectionKeyCode={null} nodesConnectable={!topologyLocked}
          colorMode="dark"
          onNodeClick={(_, node) => onSelect({ kind: 'node', id: node.id })}
          onNodeDragStop={(_, node) => onMove(node.id, node.position.x, node.position.y)}
          onEdgeClick={(_, edge) => onSelect({ kind: 'route', id: edge.id })}
          onPaneClick={() => { setPort(null); onSelect(null) }}
          isValidConnection={(candidate) => {
            if (topologyLocked) return false
            const endpoints = toEndpoints({ ...candidate, sourceHandle: candidate.sourceHandle ?? null, targetHandle: candidate.targetHandle ?? null })
            return endpoints !== null && validConnection(project, catalog, endpoints)
          }}
          onConnect={(connection) => {
            const endpoints = toEndpoints(connection)
            if (endpoints && !topologyLocked) onConnect(endpoints)
          }}
        >
          <Background gap={20} size={1.15} color="#344251" />
          <FitCanvas nodeIds={project.nodes.map(({ id }) => id).join(',')} />
          <ViewportTools />
          <CanvasKeys onDelete={onDelete} editable={!topologyLocked} />
          {port && !topologyLocked && <Panel position="top-center" className="keyboard-cable-note" role="status"><CircleDot size={14} /> Sélectionne le canal à connecter</Panel>}
        </ReactFlow>
      </KeyboardCable.Provider>
    </Meters.Provider>
  )
}
