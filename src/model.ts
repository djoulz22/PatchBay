export interface AudioDevice {
  id: string
  name: string
  channels: number
  sampleRate: number
  isDefault: boolean
  isVirtual: boolean
}

export interface BrowserTab { id: string; title: string; browser: string }
export interface BrowserPairing { port: number; token: string }

export interface AudioCatalog {
  inputs: AudioDevice[]
  outputs: AudioDevice[]
  applications: { bundleId: string; name: string; active: boolean }[]
  systemAudio: boolean
  browserTabs?: BrowserTab[]
}

export interface VirtualBus { id: string; name: string; inputChannels: number; outputChannels: number }
export interface VirtualBuses { driverAvailable: boolean; driverInstalled: boolean; buses: VirtualBus[] }

export interface ChannelLevel { gain: number; muted: boolean }
export type StereoPair = [number, number]
export const MASTER_CHANNELS = [64, 65] as const

export interface OutputChannelKind {
  type: 'outputChannels'
  channels: number
  levels?: ChannelLevel[]
  stereoPairs?: StereoPair[]
  master?: ChannelLevel
}

export interface InternalBusKind {
  type: 'internalBus'
  channels: number
}

export type NodeKind =
  | { type: 'tone'; frequencyHz: number }
  | { type: 'deviceInput'; deviceId: string }
  | { type: 'systemAudio' }
  | { type: 'application'; bundleId: string; muteOriginal?: boolean }
  | { type: 'browserTab'; tabId: string }
  | InternalBusKind
  | OutputChannelKind
  | { type: 'deviceOutput'; deviceId: string }

export interface GraphNode {
  id: string
  label: string
  x: number
  y: number
  kind: NodeKind
}

export interface Route {
  id: string
  source: string
  target: string
  sourceChannel: number
  targetChannel: number
  gain: number
  muted: boolean
}

export interface Project {
  sampleRate: number
  nodes: GraphNode[]
  routes: Route[]
}

export interface SavedGraph { id: string; name: string; project: Project }
export interface Session { version: 1; activeId: string; graphs: SavedGraph[] }

export interface Meter {
  channels: number[]
  clipping: boolean
  clippedChannels?: boolean[]
  master?: { channels: [number, number]; clippedChannels: [boolean, boolean] }
}
export interface EngineStatus {
  running: boolean
  realtime: boolean
  offline: string[]
  error: string | null
  meters: Record<string, Meter>
  underruns: number
  droppedFrames: number
}

export type Selection = { kind: 'node' | 'route'; id: string } | null
export type Endpoints = Pick<Route, 'source' | 'sourceChannel' | 'target' | 'targetChannel'>
export type NodeChoice = { label: string; kind: NodeKind; variant: NodeVariant; details: string }
export type NodeVariant = 'input' | 'system' | 'app' | 'browserTab' | 'tone' | 'internalBus' | 'outputChannels' | 'output' | 'virtual'

export const emptyCatalog: AudioCatalog = { inputs: [], outputs: [], applications: [], systemAudio: false }
export const idleStatus: EngineStatus = { running: false, realtime: false, offline: [], error: null, meters: {}, underruns: 0, droppedFrames: 0 }

export function newProject(): Project { return { sampleRate: 48_000, nodes: [], routes: [] } }
export function projectNeedsMicrophone(project: Project): boolean {
  const sources = new Set(project.routes.map(({ source }) => source))
  return project.nodes.some(({ id, kind }) => kind.type === 'deviceInput' && sources.has(id))
}

export function channelLevels(kind: OutputChannelKind, count = kind.channels): ChannelLevel[] {
  return Array.from({ length: count }, (_, index) => kind.levels?.[index] ?? { gain: 1, muted: false })
}

export function newId(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID()
  return Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) =>
    byte.toString(16).padStart(2, '0')).join('')
}
export function newRoute(endpoints: Endpoints): Route {
  return { id: newId(), ...endpoints, gain: 1, muted: false }
}

export function deviceFor(kind: NodeKind, catalog: AudioCatalog) {
  if (kind.type === 'deviceInput') return catalog.inputs.find((device) => device.id === kind.deviceId)
  if (kind.type === 'deviceOutput') return catalog.outputs.find((device) => device.id === kind.deviceId)
  return undefined
}

export function browserTabFor(kind: NodeKind, catalog: AudioCatalog) {
  if (kind.type === 'browserTab') return catalog.browserTabs?.find((tab) => tab.id === kind.tabId)
}

export function variantFor(kind: NodeKind, catalog: AudioCatalog): NodeVariant {
  switch (kind.type) {
    case 'deviceInput': return 'input'
    case 'internalBus': return 'internalBus'
    case 'outputChannels': return 'outputChannels'
    case 'deviceOutput': return isVirtualDevice(kind.deviceId, catalog) ? 'virtual' : 'output'
    case 'systemAudio': return 'system'
    case 'application': return 'app'
    case 'browserTab': return 'browserTab'
    case 'tone': return 'tone'
  }
}

export function labelFor(variant: NodeVariant) {
  return {
    input: 'Entrée audio', system: 'Capture système', app: 'Application', browserTab: 'Onglet du navigateur', tone: 'Générateur',
    internalBus: 'Bus interne', outputChannels: 'Canaux de sortie', output: 'Écoute physique', virtual: 'Sortie virtuelle',
  }[variant]
}

export function isMixStage(kind: NodeKind): kind is InternalBusKind | OutputChannelKind {
  return kind.type === 'internalBus' || kind.type === 'outputChannels'
}

export function canSource(node: GraphNode): boolean { return node.kind.type !== 'deviceOutput' }

export function canTarget(node: GraphNode): boolean {
  return node.kind.type === 'deviceOutput' || isMixStage(node.kind)
}

export function canConnectNodes(source: GraphNode, target: GraphNode): boolean {
  return source.id !== target.id && canSource(source) && canTarget(target)
    && (source.kind.type !== 'outputChannels' || target.kind.type !== 'outputChannels')
}

export function channelCount(node: GraphNode, catalog: AudioCatalog, project: Project): number {
  const { kind } = node
  if (kind.type === 'tone') return 1
  if (kind.type === 'systemAudio' || kind.type === 'application' || kind.type === 'browserTab') return 2
  if (isMixStage(kind)) return kind.channels
  const device = deviceFor(kind, catalog)
  if (device) return device.channels
  const used = project.routes.reduce((count, route) => {
    if (route.source === node.id) return Math.max(count, route.sourceChannel + 1)
    if (route.target === node.id) return Math.max(count, route.targetChannel + 1)
    return count
  }, 0)
  return Math.min(64, Math.max(2, used))
}

export function isMasterChannel(channel: number): boolean {
  return channel === MASTER_CHANNELS[0] || channel === MASTER_CHANNELS[1]
}

export function minimumMixStageChannels(project: Project, nodeId: string): number {
  return project.routes.reduce((minimum, route) => {
    const channel = route.source === nodeId ? route.sourceChannel : route.target === nodeId ? route.targetChannel : undefined
    if (channel === undefined || isMasterChannel(channel)) return minimum
    return Math.max(minimum, channel + 1)
  }, 1)
}

export function sourceChannels(node: GraphNode, catalog: AudioCatalog, project: Project): number[] {
  if (!canSource(node)) return []
  const channels = Array.from({ length: channelCount(node, catalog, project) }, (_, channel) => channel)
  if (node.kind.type === 'outputChannels') channels.push(...MASTER_CHANNELS)
  return channels
}

export function targetChannels(node: GraphNode, catalog: AudioCatalog, project: Project): number[] {
  if (!canTarget(node)) return []
  return Array.from({ length: channelCount(node, catalog, project) }, (_, channel) => channel)
}

export function deviceAvailable(node: GraphNode, catalog: AudioCatalog): boolean {
  const { kind } = node
  if (kind.type === 'deviceInput' || kind.type === 'deviceOutput') return Boolean(deviceFor(kind, catalog))
  if (kind.type === 'application') return Boolean(catalog.applications.find((app) => app.bundleId === kind.bundleId)?.active)
  if (kind.type === 'browserTab') return Boolean(browserTabFor(kind, catalog))
  return kind.type !== 'systemAudio' || catalog.systemAudio
}

export function shortChannel(channel: number, count: number): string {
  if (count === 1) return 'M'
  if (count === 2) return channel === 0 ? 'L' : 'R'
  return String(channel + 1).padStart(2, '0')
}

export function channelLabel(node: GraphNode | undefined, channel: number, catalog: AudioCatalog, project: Project): string {
  if (node?.kind.type === 'outputChannels') {
    if (channel === MASTER_CHANNELS[0]) return 'Master G'
    if (channel === MASTER_CHANNELS[1]) return 'Master D'
    return String(channel + 1)
  }
  if (!node || node.kind.type === 'internalBus') return String(channel + 1)
  const kind = node.kind
  if ((kind.type === 'deviceInput' || kind.type === 'deviceOutput') && !isVirtualDevice(kind.deviceId, catalog)) {
    return String(channel + 1).padStart(2, '0')
  }
  return shortChannel(channel, Math.max(channel + 1, channelCount(node, catalog, project)))
}

export function rateText(rate: number) {
  return new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 1 }).format(rate / 1000) + ' kHz'
}

export function dbText(gain: number) {
  if (gain <= 0) return '−∞ dB'
  const db = 20 * Math.log10(gain)
  return new Intl.NumberFormat('fr-FR', { signDisplay: 'exceptZero', maximumFractionDigits: 1 }).format(db) + ' dB'
}

function backingDevice(id: string) {
  const prefix = 'fr.patchbay.audio.bus.'
  const base = id.startsWith(prefix) ? id.slice(prefix.length) : ''
  return /^blackhole.*_uid$/i.test(base) ? base : id
}

export function isLegacyVirtualDevice(id: string): boolean {
  return backingDevice(id) !== id
}

export function isNativeVirtualDevice(id: string): boolean {
  return id.startsWith('fr.patchbay.audio.native.')
}

export function isVirtualDevice(id: string, catalog: AudioCatalog): boolean {
  return isLegacyVirtualDevice(id) || isNativeVirtualDevice(id)
    || catalog.inputs.some((device) => device.id === id && device.isVirtual)
    || catalog.outputs.some((device) => device.id === id && device.isVirtual)
}

function feedbackTopology(project: Project, catalog: AudioCatalog) {
  const index = new Map(project.nodes.map((node) => [node.id, node.kind]))
  function port(id: string, channel: number) {
    const kind = index.get(id)
    if (kind && isMixStage(kind)) return JSON.stringify(['channels', id, channel])
    if ((kind?.type === 'deviceInput' || kind?.type === 'deviceOutput') && isVirtualDevice(kind.deviceId, catalog)) {
      return JSON.stringify(['device', backingDevice(kind.deviceId), channel])
    }
  }
  const links = new Map<string, string[]>()
  function link(from: string | undefined, to: string | undefined) {
    if (!from || !to) return
    const targets = links.get(from) ?? []
    targets.push(to)
    links.set(from, targets)
  }
  for (const route of project.routes) {
    link(port(route.source, route.sourceChannel), port(route.target, route.targetChannel))
  }
  for (const node of project.nodes) {
    if (node.kind.type !== 'outputChannels') continue
    const sides = new Map<number, number>()
    for (const [left, right] of node.kind.stereoPairs ?? []) {
      sides.set(left, MASTER_CHANNELS[0])
      sides.set(right, MASTER_CHANNELS[1])
    }
    for (let channel = 0; channel < node.kind.channels; channel++) {
      const from = port(node.id, channel)
      const side = sides.get(channel)
      for (const master of side === undefined ? MASTER_CHANNELS : [side]) link(from, port(node.id, master))
    }
  }
  return { port, links }
}

export function hasVirtualFeedback(project: Project, catalog: AudioCatalog): boolean {
  const { links } = feedbackTopology(project, catalog)
  const inbound = new Map<string, number>()
  for (const [source, targets] of links) {
    if (!inbound.has(source)) inbound.set(source, 0)
    for (const target of targets) inbound.set(target, (inbound.get(target) ?? 0) + 1)
  }
  const remaining = [...inbound].filter(([, count]) => count === 0).map(([port]) => port)
  let visited = 0
  while (remaining.length) {
    const current = remaining.pop()!
    visited += 1
    for (const target of links.get(current) ?? []) {
      const count = inbound.get(target)! - 1
      inbound.set(target, count)
      if (count === 0) remaining.push(target)
    }
  }
  return visited < inbound.size
}

export function resizeMixStage(project: Project, catalog: AudioCatalog, nodeId: string, count: number): Project {
  const node = project.nodes.find(({ id }) => id === nodeId)
  if (!node || !isMixStage(node.kind)) return project
  const minimum = minimumMixStageChannels(project, nodeId)
  if (!Number.isInteger(count) || count < minimum || count > 64) {
    throw new Error(count >= 1 && count < minimum
      ? `Le canal ${minimum} est encore relié. Déconnecte les canaux à retirer avant de réduire le groupe.`
      : 'Choisis entre 1 et 64 canaux.')
  }
  if (count === node.kind.channels) return project

  const kind = { ...node.kind, channels: count }
  if (kind.type === 'outputChannels') {
    const stereoPairs = kind.stereoPairs?.filter(([left, right]) => left < count && right < count)
    kind.levels = kind.levels?.length ? channelLevels(kind) : undefined
    kind.stereoPairs = stereoPairs?.length ? stereoPairs : undefined
  }
  const next = { ...project, nodes: project.nodes.map((item) => item === node ? { ...item, kind } : item) }
  if (kind.type === 'outputChannels' && hasVirtualFeedback(next, catalog)) {
    throw new Error('Ce redimensionnement fermerait une boucle audio par le Master.')
  }
  return next
}

function reaches(links: Map<string, string[]>, from: string, to: string): boolean {
  const visited = new Set<string>()
  const queue = [from]
  while (queue.length) {
    const port = queue.pop()!
    if (port === to) return true
    if (visited.has(port)) continue
    visited.add(port)
    queue.push(...(links.get(port) ?? []))
  }
  return false
}

function feedback(project: Project, catalog: AudioCatalog, next: Endpoints): boolean {
  const { port, links } = feedbackTopology(project, catalog)
  const start = port(next.source, next.sourceChannel)
  const end = port(next.target, next.targetChannel)
  return !!start && !!end && reaches(links, end, start)
}

function stageCycle(project: Project, source: GraphNode, target: GraphNode): boolean {
  if (!isMixStage(source.kind) || !isMixStage(target.kind)) return false
  const stageIds = new Set(project.nodes.filter(({ kind }) => isMixStage(kind)).map(({ id }) => id))
  const links = new Map<string, string[]>()
  for (const route of project.routes) {
    if (!stageIds.has(route.source) || !stageIds.has(route.target)) continue
    const targets = links.get(route.source) ?? []
    targets.push(route.target)
    links.set(route.source, targets)
  }
  return reaches(links, target.id, source.id)
}

export function validConnection(project: Project, catalog: AudioCatalog, connection: Endpoints): boolean {
  if (project.routes.length >= 1024) return false
  const source = project.nodes.find((node) => node.id === connection.source)
  const target = project.nodes.find((node) => node.id === connection.target)
  if (!source || !target || !canConnectNodes(source, target)) return false
  if (!sourceChannels(source, catalog, project).includes(connection.sourceChannel)
    || !targetChannels(target, catalog, project).includes(connection.targetChannel)) return false
  const alreadyConnected = project.routes.some((route) => route.source === connection.source && route.target === connection.target
    && route.sourceChannel === connection.sourceChannel && route.targetChannel === connection.targetChannel)
  return !alreadyConnected && !stageCycle(project, source, target) && !feedback(project, catalog, connection)
}

export function canInsertOutputChannels(project: Project, destinationId: string): boolean {
  const destination = project.nodes.find((node) => node.id === destinationId)
  if (destination?.kind.type !== 'deviceOutput') return false
  const stageIds = new Set(project.nodes.filter((node) => node.kind.type === 'outputChannels').map((node) => node.id))
  return !project.routes.some((route) => route.target === destinationId && stageIds.has(route.source))
}

export function insertOutputChannels(project: Project, catalog: AudioCatalog, stage: GraphNode, destinationId: string): Project {
  const { kind } = stage
  if (kind.type !== 'outputChannels' || !Number.isInteger(kind.channels)
    || kind.channels < 1 || kind.channels > 64) throw new Error('Choisis entre 1 et 64 canaux.')
  if (project.nodes.length >= 64 || project.nodes.some(({ id }) => id === stage.id)) {
    throw new Error('Impossible d’ajouter le bloc de canaux au graphe.')
  }
  const destination = project.nodes.find((node) => node.id === destinationId)
  if (!destination || !canInsertOutputChannels(project, destinationId)) {
    throw new Error('Choisis une sortie qui ne reçoit pas déjà un bloc de canaux.')
  }
  const incoming = project.routes.filter((route) => route.target === destinationId)
  const usedChannels = [...new Set(incoming.map((route) => route.targetChannel))]
  if (usedChannels.some((channel) => channel >= kind.channels)) {
    throw new Error(`Le câblage existant utilise le canal ${Math.max(...usedChannels) + 1}.`)
  }
  const count = Math.min(kind.channels, channelCount(destination, catalog, project))
  const channels = usedChannels.length ? usedChannels : Array.from({ length: count }, (_, channel) => channel)
  if (project.routes.length + channels.length > 1024) throw new Error('Le graphe dépasserait 1024 connexions.')

  const routes = project.routes.map((route) => route.target === destinationId ? { ...route, target: stage.id } : route)
  for (const channel of channels) {
    routes.push(newRoute({ source: stage.id, sourceChannel: channel, target: destinationId, targetChannel: channel }))
  }
  return { ...project, nodes: [...project.nodes, stage], routes }
}

export function starterSession(catalog: AudioCatalog, demo: boolean): Session {
  const project = newProject()
  const output = catalog.outputs.find((device) => device.isVirtual && device.channels === 2 && /blackhole/i.test(device.name))
    ?? catalog.outputs.find((device) => device.isVirtual && device.channels === 2)
    ?? catalog.outputs.find((device) => device.isVirtual)
  const monitor = catalog.outputs.find((device) => !device.isVirtual && device.isDefault)
    ?? catalog.outputs.find((device) => !device.isVirtual)
  const microphone = demo ? catalog.inputs.find((device) => !device.isVirtual) : undefined
  const channels = Math.min(2, output?.channels ?? 2)
  if (catalog.systemAudio) project.nodes.push({ id: 'system', label: 'Audio du système', x: 22, y: 22, kind: { type: 'systemAudio' } })
  if (microphone) project.nodes.push({ id: 'mic', label: microphone.name, x: 22, y: 244, kind: { type: 'deviceInput', deviceId: microphone.id } })
  project.nodes.push({ id: 'channels', label: channels === 1 ? 'Canal 1' : 'Canaux 1 et 2', x: 350, y: 22, kind: { type: 'outputChannels', channels } })
  if (output) project.nodes.push({ id: 'virtual', label: output.name, x: 678, y: 22, kind: { type: 'deviceOutput', deviceId: output.id } })
  if (monitor) project.nodes.push({ id: 'monitor', label: monitor.name, x: 678, y: output ? 244 : 22, kind: { type: 'deviceOutput', deviceId: monitor.id } })
  for (let channel = 0; channel < channels; channel++) {
    if (catalog.systemAudio && (output || (demo && monitor))) {
      project.routes.push(newRoute({ source: 'system', sourceChannel: channel, target: 'channels', targetChannel: channel }))
    }
    if (microphone && (output || monitor)) {
      project.routes.push({ ...newRoute({ source: 'mic', sourceChannel: 0, target: 'channels', targetChannel: channel }), gain: 0.708 })
    }
    if (output) project.routes.push(newRoute({ source: 'channels', sourceChannel: channel, target: 'virtual', targetChannel: channel }))
    if (demo && monitor && channel < monitor.channels) {
      project.routes.push(newRoute({ source: 'channels', sourceChannel: channel, target: 'monitor', targetChannel: channel }))
    }
  }
  return { version: 1, activeId: 'studio', graphs: [{ id: 'studio', name: 'Studio principal', project }] }
}
