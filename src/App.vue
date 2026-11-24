<script setup lang="ts">
import {
  Activity, AudioLines, Cable, Check, ChevronRight, CircleAlert, Copy, Ellipsis, Headphones, Laptop,
  Mic, PanelTop, Pencil, Play, Plus, Radio, RefreshCw, Search, SlidersHorizontal, Square, Trash2, Volume2, VolumeX, Workflow, X,
} from 'lucide-vue-next'
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import BrowserTabs from './BrowserTabs.vue'
import GraphCanvas from './GraphCanvas.vue'
import OutputMixer from './OutputMixer.vue'
import {
  audioCatalog, authorizeAudioInput, createVirtualBus, deleteVirtualBus, listBrowserTabs, listVirtualBuses, loadSession, native, restartCoreAudio, routingStatus,
  saveSession, startRouting, stopRouting, updateRouting, virtualDriverPath,
} from './bridge'
import { MeterStore } from './meters'
import {
  browserTabFor, canConnectNodes, canInsertOutputChannels, canSource, canTarget, channelCount, channelLabel, channelLevels, dbText, deviceAvailable, deviceFor,
  emptyCatalog, hasVirtualFeedback, idleStatus, insertOutputChannels, isLegacyVirtualDevice, isMixStage,
  isNativeVirtualDevice, isVirtualDevice, labelFor, minimumMixStageChannels, newId, newProject, newRoute,
  projectNeedsMicrophone, rateText, resizeMixStage as resizeProjectMixStage,
  sourceChannels, starterSession, targetChannels, validConnection, variantFor,
  type AudioCatalog, type BrowserTab, type ChannelLevel, type Endpoints, type GraphNode, type InternalBusKind, type NodeChoice, type NodeKind, type OutputChannelKind, type Project,
  type Route, type SavedGraph, type Selection, type Session, type StereoPair, type VirtualBus, type VirtualBuses,
} from './model'

const icons = { input: Mic, app: Volume2, browserTab: PanelTop, system: Laptop, tone: AudioLines, output: Headphones, virtual: Radio, internalBus: Workflow, outputChannels: SlidersHorizontal }
const meters = new MeterStore()
const catalog = shallowRef<AudioCatalog>(emptyCatalog)
const session = shallowRef<Session | null>(null)
const status = shallowRef(idleStatus)
const selection = shallowRef<Selection>(null)
const busy = ref(false)
const loading = ref(true)
const refreshing = ref(false)
const saveState = ref<'idle' | 'saving' | 'saved' | 'error'>('idle')
const error = ref('')
const dismissStatusError = ref('')
const loadingError = ref(false)
const menu = ref(false)
const gainDraft = ref<number | null>(null)
const frequencyDraft = ref<number | null>(null)
const dialog = ref<HTMLDialogElement | null>(null)
const firstField = ref<HTMLInputElement | HTMLSelectElement | null>(null)
const search = ref('')
const graphName = ref('')
const routeSource = ref('')
const routeTarget = ref('')
const sourceChannel = ref(0)
const targetChannel = ref(0)
const routeDialogError = ref('')
const modal = ref<'source' | 'browserTabs' | 'destination' | 'internalBus' | 'outputChannels' | 'route' | 'new' | 'rename' | 'delete' | 'buses' | null>(null)
const browserTabs = shallowRef<BrowserTab[]>([])
const browserTabsLoading = ref(false)
const browserTabsError = ref('')
const internalBusName = ref('')
const internalBusCount = ref(2)
const internalBusError = ref('')
const outputChannelsName = ref('')
const outputChannelsCount = ref(2)
const outputChannelsTarget = ref('')
const outputChannelsError = ref('')
const busCatalog = shallowRef<VirtualBuses>({ driverAvailable: false, driverInstalled: false, buses: [] })
const busName = ref('')
const busInputChannels = ref(2)
const busOutputChannels = ref(2)
const busBusy = ref(false)
const busError = ref('')
const busToRemove = shallowRef<VirtualBus | null>(null)
const driverPath = ref('')
const driverPathField = ref<HTMLInputElement | null>(null)
const driverPathCopied = ref(false)
const confirmDriverRestart = ref(false)
const restartingCoreAudio = ref(false)
let saveTimer: ReturnType<typeof setTimeout> | undefined
let polling: ReturnType<typeof setInterval> | undefined
let catalogPolling: ReturnType<typeof setInterval> | undefined
let browserTabPolling: ReturnType<typeof setInterval> | undefined
let saveQueue = Promise.resolve()
let savedDocument: Session | null = null
let statusReading = false
let disposed = false
let pendingMix: Project | null = null
let mixUpdate: Promise<void> | null = null

const blankProject = newProject()
const active = computed(() => session.value?.graphs.find((graph) => graph.id === session.value?.activeId))
const project = computed(() => active.value?.project ?? blankProject)
const nodes = computed(() => project.value.nodes)
const graphCatalog = computed(() => ({ ...catalog.value, browserTabs: browserTabs.value }))
const assignedTabs = computed(() => new Set(nodes.value.flatMap(({ kind }) => kind.type === 'browserTab' ? [kind.tabId] : [])))
const routes = computed(() => project.value.routes)
const sources = computed(() => nodes.value.filter((node) => canSource(node) && !isMixStage(node.kind)))
const mixStages = computed(() => nodes.value.filter((node) => isMixStage(node.kind)))
const channelGroups = computed(() => mixStages.value.filter((node) => node.kind.type === 'outputChannels'))
const internalBuses = computed(() => mixStages.value.filter((node) => node.kind.type === 'internalBus'))
const destinations = computed(() => nodes.value.filter((node) => node.kind.type === 'deviceOutput'))
const channelGroupTargets = computed(() => destinations.value.filter((node) => canInsertOutputChannels(project.value, node.id)))
const topologyLocked = computed(() => status.value.running || busy.value || loading.value || restartingCoreAudio.value)
const visibleError = computed(() => error.value || (status.value.error !== dismissStatusError.value ? status.value.error : ''))
const selectedNode = computed(() => selection.value?.kind === 'node' ? nodeById(selection.value.id) : undefined)
const selectedOutput = computed(() => {
  const node = selectedNode.value
  return node?.kind.type === 'outputChannels' ? { id: node.id, label: node.label, kind: node.kind } : undefined
})
const selectedRoute = computed(() => selection.value?.kind === 'route' ? routes.value.find((route) => route.id === selection.value?.id) : undefined)
const chosenRouteSource = computed(() => nodeById(routeSource.value))
const chosenRouteTarget = computed(() => nodeById(routeTarget.value))
const routeSources = computed(() => nodes.value.filter((source) => canSource(source)
  && nodes.value.some((target) => canTarget(target) && canConnectNodes(source, target))))
const routeTargets = computed(() => {
  const source = chosenRouteSource.value
  return source ? [...mixStages.value, ...destinations.value].filter((target) => canConnectNodes(source, target)) : []
})
const chosenRouteSourceChannels = computed(() => chosenRouteSource.value ? sourceChannels(chosenRouteSource.value, catalog.value, project.value) : [])
const chosenRouteTargetChannels = computed(() => chosenRouteTarget.value ? targetChannels(chosenRouteTarget.value, catalog.value, project.value) : [])
const routeSourceNode = computed(() => selectedRoute.value ? nodeById(selectedRoute.value.source) : undefined)
const routeTargetNode = computed(() => selectedRoute.value ? nodeById(selectedRoute.value.target) : undefined)
const selectedNodeRoutes = computed(() => selectedNode.value
  ? routes.value.filter((route) => route.source === selectedNode.value?.id || route.target === selectedNode.value?.id) : [])
const gainValue = computed(() => gainDraft.value ?? selectedRoute.value?.gain ?? 1)
const gainSlider = computed(() => gainValue.value > 0 ? Math.max(-60, 20 * Math.log10(gainValue.value)) : -60)
const selectedNodeVariant = computed(() => selectedNode.value ? variantFor(selectedNode.value.kind, catalog.value) : 'input')
const selectedDevice = computed(() => selectedNode.value ? deviceFor(selectedNode.value.kind, catalog.value) : undefined)
const selectedBrowserTab = computed(() => selectedNode.value && browserTabFor(selectedNode.value.kind, graphCatalog.value))
const browserTabChoices = computed(() => browserTabs.value.filter((tab) => tab.id === selectedBrowserTab.value?.id || !assignedTabs.value.has(tab.id)))
const minimumSelectedChannels = computed(() => selectedNode.value ? minimumMixStageChannels(project.value, selectedNode.value.id) : 1)
const usedDeviceIds = computed(() => new Set(session.value?.graphs.flatMap((graph) =>
  graph.project.nodes.flatMap(({ kind }) => kind.type === 'deviceInput' || kind.type === 'deviceOutput' ? [kind.deviceId] : []))))
const validBusShape = computed(() => [busInputChannels.value, busOutputChannels.value].every((channels) =>
  Number.isInteger(channels) && channels >= 1 && channels <= 64))
const selectedLegacyDevice = computed(() => {
  const kind = selectedNode.value?.kind
  return (kind?.type === 'deviceInput' || kind?.type === 'deviceOutput') && isLegacyVirtualDevice(kind.deviceId) ? kind.deviceId : ''
})
const legacyReplacementDevices = computed(() => {
  const legacyId = selectedLegacyDevice.value
  if (!legacyId) return []
  const endpoints = nodes.value.flatMap(({ id, kind }) =>
    (kind.type === 'deviceInput' || kind.type === 'deviceOutput') && kind.deviceId === legacyId ? [{ id, type: kind.type }] : [])
  return catalog.value.inputs.flatMap((input): VirtualBus[] => {
    const { id, name } = input
    if (!isNativeVirtualDevice(id)) return []
    const output = catalog.value.outputs.find((device) => device.id === id)
    if (!output) return []
    const compatible = endpoints.every((endpoint) => {
      if (nodes.value.some(({ kind }) => kind.type === endpoint.type && kind.deviceId === id)) return false
      return routes.value.every((route) => (route.source !== endpoint.id || route.sourceChannel < input.channels)
        && (route.target !== endpoint.id || route.targetChannel < output.channels))
    })
    return compatible ? [{ id, name, inputChannels: input.channels, outputChannels: output.channels }] : []
  })
})
const dialogTitle = computed(() => ({
  source: 'Ajouter une source', destination: 'Ajouter une destination', internalBus: 'Ajouter un bus interne', outputChannels: 'Ajouter des canaux de sortie',
  browserTabs: 'Onglets du navigateur',
  route: 'Connecter deux canaux',
  new: 'Nouvelle configuration', rename: 'Renommer la configuration', delete: 'Supprimer la configuration',
  buses: 'Interfaces virtuelles',
})[modal.value ?? 'source'])
const saveLabel = computed(() => ({ idle: '', saving: 'Enregistrement…', saved: 'Sauvegardé', error: 'Sauvegarde en échec' })[saveState.value])

function sameKind(a: NodeKind, b: NodeKind) {
  if (a.type === 'application' && b.type === 'application') return a.bundleId === b.bundleId
  if (a.type === 'browserTab' && b.type === 'browserTab') return a.tabId === b.tabId
  return JSON.stringify(a) === JSON.stringify(b)
}
function nodeById(id: string) { return nodes.value.find((node) => node.id === id) }
function nodeChannels(node: GraphNode) { return channelCount(node, catalog.value, project.value) }
function channelName(node: GraphNode | undefined, channel: number) {
  return channelLabel(node, channel, catalog.value, project.value)
}
function showError(cause: unknown) { error.value = cause instanceof Error ? cause.message : String(cause) }
function clearError() { error.value = ''; dismissStatusError.value = status.value.error ?? '' }

async function refreshCatalog() {
  if (refreshing.value) return
  refreshing.value = true
  try { await fetchCatalog() } catch (cause) { showError(cause) }
  finally { refreshing.value = false }
}

async function fetchCatalog() {
  const next = await audioCatalog()
  if (JSON.stringify(next) !== JSON.stringify(catalog.value)) catalog.value = next
}

async function refreshBrowserTabs() {
  if (!native || browserTabsLoading.value) return
  browserTabsLoading.value = true
  try {
    const next = await listBrowserTabs()
    if (!disposed && JSON.stringify(next) !== JSON.stringify(browserTabs.value)) browserTabs.value = next
    browserTabsError.value = ''
  } catch (cause) {
    if (!disposed) {
      browserTabs.value = []
      browserTabsError.value = cause instanceof Error ? cause.message : String(cause)
    }
  } finally {
    browserTabsLoading.value = false
  }
}

const pickerGroups = computed(() => {
  const inventory = catalog.value
  const groups: { label: string; items: NodeChoice[] }[] = []
  if (modal.value === 'source') {
    const extra: NodeChoice[] = []
    if (inventory.systemAudio) extra.push({ label: 'Audio du système', kind: { type: 'systemAudio' }, variant: 'system', details: 'Mélange stéréo du Mac' })
    extra.push({ label: 'Tonalité de test', kind: { type: 'tone', frequencyHz: 440 }, variant: 'tone', details: 'Sinusoïde · 440 Hz' })
    groups.push({ label: 'Système', items: extra })
    groups.push({ label: 'Entrées audio', items: inventory.inputs.map((device) => ({
      label: device.name, kind: { type: 'deviceInput', deviceId: device.id }, variant: 'input',
      details: `${device.isVirtual ? 'Virtuelle · ' : ''}${device.channels} ch · ${rateText(device.sampleRate)}`,
    })) })
    groups.push({ label: 'Applications', items: inventory.applications.map((app) => ({
      label: app.name, kind: { type: 'application', bundleId: app.bundleId }, variant: 'app',
      details: app.active ? 'Disponible' : 'En attente d’audio',
    })) })
  }
  if (modal.value === 'destination') {
    for (const isVirtual of [true, false]) {
      groups.push({ label: isVirtual ? 'Périphériques virtuels' : 'Écoutes physiques', items: inventory.outputs
        .filter((device) => device.isVirtual === isVirtual).map((device) => ({
          label: device.name, kind: { type: 'deviceOutput', deviceId: device.id }, variant: isVirtual ? 'virtual' : 'output',
          details: `${device.channels} ch · ${rateText(device.sampleRate)}${device.isDefault ? ' · Par défaut' : ''}`,
        })) })
    }
  }
  const term = search.value.trim().toLocaleLowerCase('fr')
  return groups.map(({ label, items }) => ({
    label, items: items.filter((item) => item.label.toLocaleLowerCase('fr').includes(term)).map((item) => ({
      ...item, added: item.kind.type !== 'tone' && nodes.value.some((node) => sameKind(item.kind, node.kind)),
    })),
  })).filter(({ items }) => items.length)
})

async function persist() {
  clearTimeout(saveTimer)
  const document = session.value
  if (!document || loadingError.value || document === savedDocument) return
  saveState.value = 'saving'
  const operation = saveQueue.then(async () => {
    if (document === savedDocument) return
    await saveSession(document)
    savedDocument = document
  })
  saveQueue = operation.catch(() => {})
  try {
    await operation
    if (session.value === document) saveState.value = 'saved'
  } catch (cause) {
    saveState.value = 'error'
    showError(cause)
    throw cause
  }
}

watch(session, (document) => {
  if (!document || document === savedDocument || loadingError.value) return
  clearTimeout(saveTimer)
  saveState.value = 'saving'
  saveTimer = setTimeout(() => { void persist().catch(() => {}) }, 280)
})

function setProject(next: Project) {
  if (!session.value || !active.value) return
  const id = active.value.id
  session.value = { ...session.value, graphs: session.value.graphs.map((graph) => graph.id === id ? { ...graph, project: next } : graph) }
}

async function bootstrap() {
  loading.value = true
  try {
    const results = await Promise.allSettled([fetchCatalog(), loadSession()])
    const catalogResult = results[0]
    const sessionResult = results[1]
    if (catalogResult.status === 'rejected') showError(catalogResult.reason)
    if (sessionResult.status === 'rejected') {
      loadingError.value = true
      showError(sessionResult.reason)
      return
    }
    loadingError.value = false
    savedDocument = sessionResult.value
    const original = sessionResult.value
    session.value = original ?? starterSession(catalog.value, !native)
    if (original) saveState.value = 'saved'
    if (!native && routes.value.length) selection.value = { kind: 'route', id: routes.value[0].id }
  } finally {
    loading.value = false
  }
}

async function readStatus() {
  if (!native || statusReading || disposed || restartingCoreAudio.value) return
  statusReading = true
  try {
    const next = await routingStatus()
    meters.set(next.running ? next.meters : {})
    if (next.error !== status.value.error) dismissStatusError.value = ''
    // Avoid re-rendering the Vue shell for audio meters that change every poll.
    if (next.running !== status.value.running || next.realtime !== status.value.realtime || next.error !== status.value.error
      || next.offline.length !== status.value.offline.length || next.offline.some((id, index) => id !== status.value.offline[index])
      || next.underruns !== status.value.underruns || next.droppedFrames !== status.value.droppedFrames) status.value = next
  } catch (cause) {
    if (!disposed) showError(cause)
  } finally {
    statusReading = false
  }
}

async function toggleRouting() {
  if (!native || busy.value || restartingCoreAudio.value || (!status.value.running && !routes.value.length)) return
  busy.value = true
  error.value = ''
  dismissStatusError.value = ''
  try {
    pendingMix = null
    await mixUpdate
    if (status.value.running) await stopRouting()
    else {
      await persist()
      const currentProject = project.value
      if (projectNeedsMicrophone(currentProject)) await authorizeAudioInput()
      await startRouting(currentProject)
    }
  } catch (cause) { showError(cause) }
  finally { await readStatus(); busy.value = false }
}

function select(value: Selection) {
  if (selection.value?.kind === value?.kind && selection.value?.id === value?.id) return
  selection.value = value
  gainDraft.value = null
  frequencyDraft.value = null
}

function createConnection(endpoints: Endpoints) {
  if (topologyLocked.value) return false
  if (!validConnection(project.value, catalog.value, endpoints)) return false
  const route = newRoute(endpoints)
  setProject({ ...project.value, routes: [...routes.value, route] })
  select({ kind: 'route', id: route.id })
  return true
}

function connectEndpoints() {
  const ok = createConnection({ source: routeSource.value, sourceChannel: sourceChannel.value, target: routeTarget.value, targetChannel: targetChannel.value })
  if (ok) closeDialog()
  else routeDialogError.value = 'Cette connexion existe déjà ou créerait une boucle audio.'
}

function connectFromCanvas(endpoints: Endpoints) {
  if (!createConnection(endpoints)) showError('Cette connexion existe déjà ou créerait une boucle audio.')
}

function addNode(choice: NodeChoice, keepPickerOpen = false) {
  if (topologyLocked.value || !active.value) return
  if (nodes.value.length >= 64) { showError('Le graphe contient déjà 64 blocs.'); closeDialog(); return }
  if (choice.kind.type !== 'tone' && nodes.value.some((node) => sameKind(node.kind, choice.kind))) return
  const output = choice.kind.type === 'deviceOutput'
  const column = output ? destinations.value : sources.value
  const x = column[0]?.x ?? (output ? (mixStages.value[0]?.x ?? 112) + 328 : 22)
  const y = column.length ? Math.max(...column.map((item) => item.y)) + 222 : 22
  const node: GraphNode = { id: newId(), label: choice.label, x, y, kind: choice.kind }
  setProject({ ...project.value, nodes: [...nodes.value, node] })
  select({ kind: 'node', id: node.id })
  if (!keepPickerOpen) closeDialog()
}

function addBrowserTab(id: string) {
  const tab = browserTabs.value.find((item) => item.id === id)
  if (tab) addNode({ label: tab.title, kind: { type: 'browserTab', tabId: id }, variant: 'browserTab', details: tab.browser }, true)
}

function reassignBrowserTab(node: GraphNode, event: Event) {
  if (topologyLocked.value || node.kind.type !== 'browserTab') return
  const selectTab = event.currentTarget as HTMLSelectElement
  const tabId = selectTab.value
  if (tabId === node.kind.tabId) return
  if (!browserTabs.value.some((tab) => tab.id === tabId) || assignedTabs.value.has(tabId)) {
    selectTab.value = selectedBrowserTab.value?.id ?? ''
    return
  }
  const kind: NodeKind = { type: 'browserTab', tabId }
  setProject({ ...project.value, nodes: nodes.value.map((item) => item.id === node.id ? { ...item, kind } : item) })
}

function reassignLegacyDevice(event: Event) {
  const field = event.currentTarget as HTMLSelectElement
  const deviceId = field.value
  field.value = ''
  if (topologyLocked.value || !legacyReplacementDevices.value.some(({ id }) => id === deviceId)) return
  const legacyId = selectedLegacyDevice.value
  const replacement = nodes.value.map((node) => {
    const { kind } = node
    if ((kind.type === 'deviceInput' || kind.type === 'deviceOutput') && kind.deviceId === legacyId) {
      return { ...node, kind: { ...kind, deviceId } }
    }
    return node
  })
  const next = { ...project.value, nodes: replacement }
  if (hasVirtualFeedback(next, catalog.value)) {
    showError('Ce remplacement créerait une boucle audio.')
    return
  }
  setProject(next)
}

function inferOutputChannelsCount() {
  const target = nodeById(outputChannelsTarget.value)
  outputChannelsCount.value = target ? nodeChannels(target) : 2
  outputChannelsError.value = ''
}

function newMixStage(label: string, kind: InternalBusKind | OutputChannelKind, target?: GraphNode): GraphNode {
  const x = mixStages.value[0]?.x ?? Math.max(350, ...sources.value.map((source) => source.x + 304))
  const y = mixStages.value.length
    ? Math.max(...mixStages.value.map((stage) => stage.y + Math.max(222, 115 + 25 * Math.min(8, nodeChannels(stage)))))
    : target?.y ?? 22
  return { id: newId(), label, x, y, kind }
}

function showNewMixStage(next: Project, stage: GraphNode, target?: GraphNode) {
  // Make space for a new stage in graphs saved with two columns.
  const adjusted = next.nodes.map((item) => {
    const overlapsNewColumn = item.x >= stage.x - 30 && item.x < stage.x + 304
    return item.kind.type === 'deviceOutput' && (overlapsNewColumn || item.id === target?.id && item.x < stage.x + 304)
      ? { ...item, x: stage.x + 328 }
      : item
  })
  setProject({ ...next, nodes: adjusted })
  select({ kind: 'node', id: stage.id })
  closeDialog()
}

function addInternalBus() {
  if (topologyLocked.value || !active.value) return
  const name = internalBusName.value.trim()
  const count = internalBusCount.value
  if (!name || !Number.isInteger(count) || count < 1 || count > 64) {
    internalBusError.value = 'Choisis un nom et un nombre de canaux compris entre 1 et 64.'
    return
  }
  if (nodes.value.length >= 64) {
    internalBusError.value = 'Le graphe contient déjà 64 blocs.'
    return
  }
  const bus = newMixStage(name, { type: 'internalBus', channels: count })
  showNewMixStage({ ...project.value, nodes: [...nodes.value, bus] }, bus)
}

function addOutputChannels() {
  if (topologyLocked.value || !active.value) return
  const name = outputChannelsName.value.trim()
  const count = outputChannelsCount.value
  if (!name || !Number.isInteger(count) || count < 1 || count > 64) {
    outputChannelsError.value = 'Choisis un nom et un nombre de canaux compris entre 1 et 64.'
    return
  }
  if (nodes.value.length >= 64) {
    outputChannelsError.value = 'Le graphe contient déjà 64 blocs.'
    return
  }
  const target = nodeById(outputChannelsTarget.value)
  const group = newMixStage(name, { type: 'outputChannels', channels: count }, target)
  try {
    const next = target
      ? insertOutputChannels(project.value, catalog.value, group, target.id)
      : { ...project.value, nodes: [...nodes.value, group] }
    showNewMixStage(next, group, target)
  } catch (cause) {
    outputChannelsError.value = cause instanceof Error ? cause.message : String(cause)
  }
}

function resizeMixStage(node: GraphNode, event: Event) {
  if (topologyLocked.value || !isMixStage(node.kind)) return
  const input = event.currentTarget as HTMLInputElement
  try {
    const next = resizeProjectMixStage(project.value, catalog.value, node.id, input.valueAsNumber)
    if (next !== project.value) setProject(next)
  } catch (cause) {
    input.value = String(node.kind.channels)
    showError(cause)
  }
}

function moveNode(id: string, x: number, y: number) {
  if (busy.value) return
  setProject({ ...project.value, nodes: nodes.value.map((node) => node.id === id ? { ...node, x: Math.round(x), y: Math.round(y) } : node) })
}

function applyNodeLabel(node: GraphNode, event: Event) {
  const input = event.currentTarget as HTMLInputElement
  const label = input.value.trim()
  input.value = label || node.label
  if (!label || label === node.label || busy.value) return
  setProject({ ...project.value, nodes: nodes.value.map((item) => item.id === node.id ? { ...item, label } : item) })
}

function toggleMuteOriginal(node: GraphNode) {
  if (topologyLocked.value || node.kind.type !== 'application') return
  const kind = { ...node.kind, muteOriginal: node.kind.muteOriginal === false }
  setProject({ ...project.value, nodes: nodes.value.map((item) => item.id === node.id ? { ...item, kind } : item) })
}

function removeSelected() {
  if (topologyLocked.value || !selection.value) return
  const { id, kind } = selection.value
  if (kind === 'route') setProject({ ...project.value, routes: routes.value.filter((route) => route.id !== id) })
  else setProject({ ...project.value, nodes: nodes.value.filter((node) => node.id !== id),
    routes: routes.value.filter((route) => route.source !== id && route.target !== id) })
  select(null)
}

function commitFrequency(node: GraphNode | undefined) {
  if (topologyLocked.value || node?.kind.type !== 'tone' || frequencyDraft.value === null) return
  const frequencyHz = Math.min(20_000, Math.max(20, frequencyDraft.value))
  if (Number.isFinite(frequencyHz)) {
    setProject({ ...project.value, nodes: nodes.value.map((item) => item.id === node.id ? { ...item, kind: { type: 'tone', frequencyHz } } : item) })
  }
  frequencyDraft.value = null
}

function previewGain(event: Event) {
  const db = (event.currentTarget as HTMLInputElement).valueAsNumber
  gainDraft.value = db <= -60 ? 0 : Math.min(4, Math.pow(10, db / 20))
}

async function flushMix() {
  while (pendingMix && status.value.running && !busy.value && !disposed) {
    await new Promise<void>((resolve) => setTimeout(resolve, 32))
    const next = pendingMix
    pendingMix = null
    if (!next || !status.value.running || busy.value || disposed) return
    try { await updateRouting(next) }
    catch (cause) { if (!disposed && !busy.value) showError(cause) }
  }
}

function applyMix(next: Project) {
  setProject(next)
  if (!status.value.running) return
  pendingMix = next
  mixUpdate ??= flushMix().finally(() => { mixUpdate = null })
}

function updateMix(route: Route, values: Partial<Pick<Route, 'gain' | 'muted'>>) {
  if (busy.value) return
  applyMix({ ...project.value, routes: routes.value.map((item) => item.id === route.id ? { ...item, ...values } : item) })
}

function updateMixerLevel(id: string, channels: readonly number[], values: Partial<ChannelLevel>) {
  if (busy.value) return
  const node = nodeById(id)
  if (node?.kind.type !== 'outputChannels') return
  const levels = channelLevels(node.kind)
  if (channels.some((channel) => !levels[channel]) || !channels.some((channel) =>
    (values.gain !== undefined && values.gain !== levels[channel].gain)
    || (values.muted !== undefined && values.muted !== levels[channel].muted))) return
  for (const channel of channels) levels[channel] = { ...levels[channel], ...values }
  const kind = { ...node.kind, levels }
  applyMix({ ...project.value, nodes: nodes.value.map((item) => item.id === id ? { ...item, kind } : item) })
}

function updateStereoPair(id: string, pair: StereoPair, linked: boolean) {
  if (busy.value) return
  const output = nodeById(id)?.kind
  if (output?.type !== 'outputChannels') return
  const [left, right] = pair
  if (left === right || pair.some((channel) => !Number.isInteger(channel) || channel < 0 || channel >= output.channels)) return
  const previous = output.stereoPairs ?? []
  if (linked && previous.some(([a, b]) => pair.includes(a) || pair.includes(b))) return
  const pairs = linked ? [...previous, pair] : previous.filter(([a, b]) => a !== left || b !== right)
  const kind = { ...output, stereoPairs: pairs.length ? pairs : undefined }
  const next = { ...project.value, nodes: nodes.value.map((item) => item.id === id ? { ...item, kind } : item) }
  if (hasVirtualFeedback(next, catalog.value)) {
    showError('Ce changement de paire fermerait une boucle audio par le Master.')
    return
  }
  applyMix(next)
}

function updateMasterLevel(id: string, values: Partial<ChannelLevel>) {
  if (busy.value) return
  const output = nodeById(id)?.kind
  if (output?.type !== 'outputChannels') return
  const previous = output.master ?? { gain: 1, muted: false }
  const master = { ...previous, ...values }
  if (master.gain === previous.gain && master.muted === previous.muted) return
  const kind = { ...output, master }
  applyMix({ ...project.value, nodes: nodes.value.map((item) => item.id === id ? { ...item, kind } : item) })
}

function commitGain(route: Route) {
  if (gainDraft.value === null) return
  updateMix(route, { gain: gainDraft.value })
  gainDraft.value = null
}

function chooseGraph(graph: SavedGraph) {
  if (topologyLocked.value || !session.value || graph.id === active.value?.id) return
  session.value = { ...session.value, activeId: graph.id }
  select(null)
}

function copyGraph() {
  if (topologyLocked.value || !session.value || !active.value) return
  const { project: copied, name } = active.value
  const graph = { id: newId(), name: `${name.slice(0, 65)} (copie)`, project: copied }
  session.value = { ...session.value, activeId: graph.id, graphs: [...session.value.graphs, graph] }
  menu.value = false
  select(null)
}

function submitGraph() {
  const name = graphName.value.trim()
  if (!session.value || !name) return
  if (modal.value === 'new') {
    if (topologyLocked.value) return
    const graph = { id: newId(), name, project: newProject() }
    session.value = { ...session.value, activeId: graph.id, graphs: [...session.value.graphs, graph] }
    select(null)
  }
  if (modal.value === 'rename' && active.value) {
    const id = active.value.id
    session.value = { ...session.value, graphs: session.value.graphs.map((graph) => graph.id === id ? { ...graph, name } : graph) }
  }
  closeDialog()
}

function deleteGraph() {
  if (!session.value || !active.value || topologyLocked.value || session.value.graphs.length < 2) return
  const graphs = session.value.graphs.filter((graph) => graph.id !== active.value?.id)
  session.value = { ...session.value, activeId: graphs[0].id, graphs }
  select(null)
  closeDialog()
}

function resetRouteTarget(preferred?: GraphNode) {
  routeTarget.value = routeTargets.value.find((node) => node.id === preferred?.id)?.id ?? routeTargets.value[0]?.id ?? ''
  sourceChannel.value = chosenRouteSourceChannels.value[0] ?? 0
  targetChannel.value = chosenRouteTargetChannels.value[0] ?? 0
  routeDialogError.value = ''
}

async function openDialog(value: NonNullable<typeof modal.value>) {
  menu.value = false
  modal.value = value
  search.value = ''
  graphName.value = value === 'rename' ? active.value?.name ?? '' : ''
  routeDialogError.value = ''
  if (value === 'browserTabs') void refreshBrowserTabs()
  if (value === 'buses') {
    busName.value = ''
    busInputChannels.value = 2
    busOutputChannels.value = 2
    busError.value = ''
    busToRemove.value = null
    driverPathCopied.value = false
    confirmDriverRestart.value = false
    void refreshBuses()
  }
  if (value === 'internalBus') {
    const labels = new Set(internalBuses.value.map(({ label }) => label))
    let number = 1
    while (labels.has(`Bus ${number}`)) number++
    internalBusName.value = `Bus ${number}`
    internalBusCount.value = 2
    internalBusError.value = ''
  }
  if (value === 'outputChannels') {
    const number = channelGroups.value.length + 1
    outputChannelsName.value = number === 1 ? 'Mix principal' : `Mix ${number}`
    outputChannelsTarget.value = channelGroupTargets.value.find((node) => node.id === selectedNode.value?.id)?.id ?? ''
    inferOutputChannelsCount()
  }
  if (value === 'route') {
    const selected = selectedNode.value
    routeSource.value = routeSources.value.find((node) => node.id === selected?.id)?.id ?? routeSources.value[0]?.id ?? ''
    resetRouteTarget(selected)
  }
  await nextTick()
  if (!dialog.value?.open) dialog.value?.showModal()
  firstField.value?.focus()
  if (value === 'rename' && firstField.value instanceof HTMLInputElement) firstField.value.select()
}

function closeDialog() {
  dialog.value?.close()
  modal.value = null
}

async function readBuses() {
  busCatalog.value = await listVirtualBuses()
  if (!busCatalog.value.driverAvailable && !busCatalog.value.driverInstalled && !driverPath.value) {
    driverPath.value = await virtualDriverPath()
  }
}

async function refreshBuses() {
  if (busBusy.value) return
  busBusy.value = true
  busError.value = ''
  try { await readBuses() }
  catch (cause) { busError.value = cause instanceof Error ? cause.message : String(cause) }
  finally { busBusy.value = false }
}

async function activateVirtualDriver() {
  if (!confirmDriverRestart.value || topologyLocked.value || busBusy.value
    || !busCatalog.value.driverInstalled || busCatalog.value.driverAvailable) return
  confirmDriverRestart.value = false
  restartingCoreAudio.value = true
  busBusy.value = true
  busError.value = ''
  try {
    const next = await restartCoreAudio()
    if (next) {
      busCatalog.value = next
      await fetchCatalog()
    }
  } catch (cause) {
    busError.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    restartingCoreAudio.value = false
    busBusy.value = false
  }
}

async function modifyBus() {
  if (busBusy.value || topologyLocked.value) return
  if (!busToRemove.value && (!busName.value.trim() || !validBusShape.value)) return
  busBusy.value = true
  busError.value = ''
  try {
    if (busToRemove.value) {
      if (usedDeviceIds.value.has(busToRemove.value.id)) throw new Error('Retire cette interface des configurations qui l’utilisent.')
      await persist()
      await deleteVirtualBus(busToRemove.value.id)
      busToRemove.value = null
    } else {
      await createVirtualBus(busName.value.trim(), busInputChannels.value, busOutputChannels.value)
      busName.value = ''
    }
    await Promise.all([readBuses(), fetchCatalog()])
  } catch (cause) { busError.value = cause instanceof Error ? cause.message : String(cause) }
  finally { busBusy.value = false }
}

function virtualEndpointSummary({ inputChannels, outputChannels }: VirtualBus) {
  return `${inputChannels} ${inputChannels === 1 ? 'entrée' : 'entrées'} · ${outputChannels} ${outputChannels === 1 ? 'sortie' : 'sorties'}`
}

function canAddVirtualEndpoint(bus: VirtualBus, type: 'deviceInput' | 'deviceOutput') {
  const kind = { type, deviceId: bus.id }
  return Boolean(active.value && deviceFor(kind, catalog.value) && !nodes.value.some((node) => sameKind(node.kind, kind)))
}

function addVirtualEndpoint(bus: VirtualBus, type: 'deviceInput' | 'deviceOutput') {
  if (!canAddVirtualEndpoint(bus, type)) return
  const kind = { type, deviceId: bus.id }
  addNode({ kind, label: bus.name, variant: variantFor(kind, catalog.value), details: '' }, true)
}

async function copyDriverPath() {
  if (!driverPath.value) return
  try {
    await navigator.clipboard.writeText(driverPath.value)
    driverPathCopied.value = true
  } catch {
    driverPathField.value?.focus()
    driverPathField.value?.select()
    driverPathCopied.value = document.execCommand('copy')
  }
}

function onNavigateAway() {
  if (!native) void persist().catch(() => {})
}

onMounted(async () => {
  window.addEventListener('pagehide', onNavigateAway)
  await bootstrap()
  if (native && !disposed) {
    await readStatus()
    polling = setInterval(() => { void readStatus() }, 110)
    catalogPolling = setInterval(() => { void fetchCatalog().catch(() => {}) }, 4500)
    browserTabPolling = setInterval(() => {
      if (modal.value === 'browserTabs' || assignedTabs.value.size) void refreshBrowserTabs()
    }, 1200)
    if (assignedTabs.value.size) void refreshBrowserTabs()
  }
})

onBeforeUnmount(() => {
  disposed = true
  pendingMix = null
  clearInterval(polling)
  clearInterval(catalogPolling)
  clearInterval(browserTabPolling)
  clearTimeout(saveTimer)
  window.removeEventListener('pagehide', onNavigateAway)
})
</script>

<template>
  <div class="desktop" @click.self="menu = false">
    <aside class="sidebar" aria-label="Configurations et périphériques">
      <div class="sidebar-brand">
        <span class="brand-mark"><AudioLines :size="24" :stroke-width="2.2" aria-hidden="true" /></span>
        <div><strong>Patchbay</strong><span>Routage audio</span></div>
      </div>
      <div class="sidebar-section configurations">
        <div class="section-label"><h2>Configurations</h2><button type="button" class="icon-button subtle" title="Nouvelle configuration" aria-label="Nouvelle configuration"
          :disabled="topologyLocked || !session" @click="openDialog('new')"><Plus :size="16" /></button></div>
        <div class="config-list">
          <button v-for="graph in session?.graphs ?? []" :key="graph.id" type="button" class="configuration"
            :class="{ 'is-active': graph.id === active?.id }" :aria-current="graph.id === active?.id ? 'page' : undefined"
            :disabled="topologyLocked && graph.id !== active?.id" @click="chooseGraph(graph)">
            <span class="config-icon"><Cable :size="16" aria-hidden="true" /></span>
            <span class="config-name" :title="graph.name">{{ graph.name }}</span>
            <span v-if="graph.id === active?.id" class="config-indicator" />
          </button>
        </div>
      </div>
      <div class="sidebar-section devices">
        <div class="section-label"><h2>Équipement audio</h2><button type="button" class="icon-button subtle" title="Actualiser les périphériques" aria-label="Actualiser les périphériques"
          :disabled="refreshing || loading" @click="refreshCatalog"><RefreshCw :size="14" :class="{ spin: refreshing }" /></button></div>
        <button type="button" class="nav-action" :disabled="topologyLocked || !active" @click="openDialog('source')"><Mic :size="16" /><span>Ajouter une source</span><Plus :size="14" class="nav-trailing" /></button>
        <button v-if="native" type="button" class="nav-action" :disabled="loading || !active" @click="openDialog('browserTabs')"><PanelTop :size="16" /><span>Ajouter un onglet</span><Plus :size="14" class="nav-trailing" /></button>
        <button type="button" class="nav-action" :disabled="topologyLocked || !active" @click="openDialog('destination')"><Headphones :size="16" /><span>Ajouter une sortie</span><Plus :size="14" class="nav-trailing" /></button>
        <button type="button" class="nav-action" :disabled="topologyLocked || !active" @click="openDialog('internalBus')"><Workflow :size="16" /><span>Ajouter un bus</span><Plus :size="14" class="nav-trailing" /></button>
        <button v-if="native" type="button" class="nav-action" :disabled="loading" @click="openDialog('buses')"><Radio :size="16" /><span>Interfaces virtuelles</span><ChevronRight :size="14" class="nav-trailing" /></button>
      </div>
      <div class="sidebar-bottom">
        <span class="sidebar-status-icon"><Activity :size="16" /></span>
        <span><strong>{{ native ? 'Core Audio' : 'Aperçu navigateur' }}</strong><small>{{ native ? 'macOS' : 'Audio disponible dans l’app' }}</small></span>
      </div>
    </aside>

    <main class="workspace" @click.self="menu = false">
      <header class="workspace-header">
        <div class="header-name">
          <h1 :title="active?.name">{{ active?.name ?? 'Routage audio' }}</h1>
          <p><span class="status-dot" :data-running="status.running" />{{ loading ? 'Chargement' : status.running ? 'Routage actif' : 'Routage arrêté' }}<span class="header-divider" />{{ rateText(project.sampleRate) }}</p>
        </div>
        <div class="workspace-actions">
          <div class="menu-anchor">
            <button type="button" class="icon-button menu-trigger" title="Options de la configuration" aria-label="Options de la configuration"
              :aria-expanded="menu" :disabled="!active" @click="menu = !menu" @keydown.esc="menu = false"><Ellipsis :size="20" /></button>
            <div v-if="menu" class="pop-menu" aria-label="Options de la configuration" @keydown.esc="menu = false">
              <button type="button" @click="openDialog('rename')"><Pencil :size="15" /> Renommer</button>
              <button type="button" :disabled="topologyLocked" @click="copyGraph"><Copy :size="15" /> Dupliquer</button>
              <button type="button" :disabled="topologyLocked || (session?.graphs.length ?? 0) < 2" class="danger-text" @click="openDialog('delete')"><Trash2 :size="15" /> Supprimer</button>
            </div>
          </div>
          <button type="button" class="primary-button start-button" :class="{ 'stop-button': status.running }"
            :disabled="!native || busy || loading || restartingCoreAudio || (!status.running && !routes.length)"
            :title="!native ? 'Disponible dans l’application macOS' : !routes.length ? 'Ajoute au moins une connexion' : undefined"
            @click="toggleRouting">
            <RefreshCw v-if="busy" :size="15" class="spin" /><Square v-else-if="status.running" :size="13" fill="currentColor" /><Play v-else :size="15" fill="currentColor" />
            <span>{{ status.running ? 'Arrêter' : 'Démarrer' }}</span>
          </button>
        </div>
      </header>

      <div v-if="visibleError" class="alert-bar" role="alert">
        <CircleAlert :size="16" /><span>{{ visibleError }}</span>
        <button v-if="loadingError" type="button" @click="bootstrap">Réessayer</button>
        <button type="button" class="icon-button subtle" aria-label="Masquer le message" @click="clearError"><X :size="15" /></button>
      </div>

      <section class="studio" :class="{ 'has-mixer': selectedOutput }" aria-label="Éditeur audio">
        <div class="canvas-area">
          <div class="canvas-top">
            <h2><Cable :size="17" /> Graphe audio</h2>
            <div class="canvas-actions">
              <button type="button" class="tool-button output-channels-button" :disabled="topologyLocked || !active" @click="openDialog('outputChannels')"><SlidersHorizontal :size="15" /><span>Canaux de sortie</span></button>
              <button type="button" class="tool-button" :disabled="topologyLocked || !routeSources.length" @click="openDialog('route')"><Plus :size="15" /><span>Connexion</span></button>
            </div>
          </div>
          <div class="canvas-body" @click="menu = false">
            <GraphCanvas v-if="active" :key="active.id" :project="project" :catalog="graphCatalog" :selection="selection"
              :topology-locked="topologyLocked" :offline="status.offline" :meters="meters"
              @select="select" @connect="connectFromCanvas" @move="moveNode" @delete="removeSelected" />
            <div v-if="!loading && active && !nodes.length" class="canvas-empty">
              <div class="empty-icon"><Cable :size="25" /></div>
              <strong>Ton graphe est vide</strong><p>Ajoute une source et relie-la aux canaux de ton mix.</p>
              <button type="button" class="secondary-button" @click="openDialog('source')"><Plus :size="15" /> Ajouter une source</button>
            </div>
          </div>
          <div class="canvas-footer">
            <span>{{ sources.length }} {{ sources.length === 1 ? 'source' : 'sources' }}<template v-if="internalBuses.length"><b>·</b>{{ internalBuses.length }} bus</template><template v-if="channelGroups.length"><b>·</b>{{ channelGroups.length }} mix</template><b>·</b>{{ destinations.length }} {{ destinations.length === 1 ? 'sortie' : 'sorties' }}<b>·</b>{{ routes.length }} {{ routes.length === 1 ? 'connexion' : 'connexions' }}</span>
            <span v-if="saveLabel" class="save-label" :data-error="saveState === 'error'"><Check v-if="saveState === 'saved'" :size="13" />{{ saveLabel }}</span>
          </div>
        </div>

        <aside class="inspector" aria-label="Inspecteur">
          <div class="inspector-header"><h2><SlidersHorizontal :size="17" /> Inspecteur</h2>
            <button v-if="selection" type="button" class="icon-button subtle" title="Effacer la sélection" aria-label="Effacer la sélection" @click="select(null)"><X :size="16" /></button>
          </div>
          <div class="inspector-body" :key="selection?.id ?? 'overview'">
            <template v-if="selectedRoute">
              <div class="inspector-title"><div class="inspector-symbol" data-variant="virtual"><Cable :size="19" /></div>
                <div><h3>Connexion</h3><p>Canal vers canal</p></div></div>
              <div class="endpoint-pair">
                <div class="endpoint"><span class="endpoint-color" :data-variant="routeSourceNode ? variantFor(routeSourceNode.kind, catalog) : 'system'" />
                  <span><small>Source</small><strong>{{ routeSourceNode?.label }}</strong></span><code>{{ channelName(routeSourceNode, selectedRoute.sourceChannel) }}</code></div>
                <div class="endpoint"><span class="endpoint-color" :data-variant="routeTargetNode ? variantFor(routeTargetNode.kind, catalog) : 'output'" />
                  <span><small>Destination</small><strong>{{ routeTargetNode?.label }}</strong></span><code>{{ channelName(routeTargetNode, selectedRoute.targetChannel) }}</code></div>
              </div>
              <label class="gain-field"><span>Gain<output>{{ dbText(gainValue) }}</output></span>
                <input type="range" aria-label="Gain de la connexion" :aria-valuetext="dbText(gainValue)" min="-60" max="12.1" step="0.1"
                  :value="gainSlider" :disabled="busy" @input="previewGain" @change="commitGain(selectedRoute)" />
                <small><span>−60</span><span>+12 dB</span></small></label>
              <button type="button" class="switch-row" role="switch" :aria-checked="selectedRoute.muted" :disabled="busy"
                @click="updateMix(selectedRoute, { muted: !selectedRoute.muted })"><VolumeX :size="16" /><span>Couper le son</span><i /></button>
              <button type="button" class="delete-button" :disabled="topologyLocked" @click="removeSelected"><Trash2 :size="15" /> Supprimer la connexion</button>
            </template>

            <template v-else-if="selectedNode">
              <div class="inspector-title"><div class="inspector-symbol" :data-variant="selectedNodeVariant"><component :is="icons[selectedNodeVariant]" :size="19" /></div>
                <div><h3 :title="selectedNode.label">{{ selectedNode.label }}</h3><p>{{ selectedNode.kind.type === 'deviceInput' && isVirtualDevice(selectedNode.kind.deviceId, catalog) ? 'Entrée virtuelle' : labelFor(selectedNodeVariant) }}</p></div></div>
              <label class="field"><span>Nom dans le graphe</span><input :value="selectedNode.label" maxlength="100" @change="applyNodeLabel(selectedNode, $event)" @keydown.enter="($event.currentTarget as HTMLInputElement).blur()" /></label>
              <label v-if="isMixStage(selectedNode.kind)" class="field"><span>Nombre de canaux</span>
                <input type="number" :min="minimumSelectedChannels" max="64" step="1" :disabled="topologyLocked"
                  :value="selectedNode.kind.channels" @change="resizeMixStage(selectedNode, $event)" />
              </label>
              <div v-else class="info-row"><span>Canaux</span><strong>{{ nodeChannels(selectedNode) }}</strong></div>
              <div v-if="selectedDevice" class="info-row"><span>Fréquence du périphérique</span><strong>{{ rateText(selectedDevice.sampleRate) }}</strong></div>
              <div v-if="status.offline.includes(selectedNode.id) || !deviceAvailable(selectedNode, graphCatalog)" class="availability-note"><CircleAlert :size="15" />{{ selectedNode.kind.type === 'application' ? 'Application en attente' : selectedNode.kind.type === 'browserTab' ? 'Onglet en attente de capture' : 'Périphérique indisponible' }}</div>
              <template v-if="selectedLegacyDevice">
                <label class="field"><span>Réaffecter à</span>
                  <select :disabled="topologyLocked || !legacyReplacementDevices.length" @change="reassignLegacyDevice">
                    <option value="" selected disabled>Choisir une interface Patchbay</option>
                    <option v-for="device in legacyReplacementDevices" :key="device.id" :value="device.id">{{ device.name }} · {{ virtualEndpointSummary(device) }}</option>
                  </select>
                </label>
                <p v-if="legacyReplacementDevices.length" class="inspector-note">Réaffecte les blocs d'entrée et de sortie de cette ancienne interface en gardant leurs câbles.</p>
                <p v-else class="inspector-note">Crée une interface Patchbay avec assez de canaux pour reprendre ce câblage.</p>
              </template>
              <template v-if="selectedNode.kind.type === 'browserTab'">
                <label class="field"><span>Onglet associé</span>
                  <select :value="selectedBrowserTab?.id ?? ''" :disabled="topologyLocked || !browserTabChoices.length" @change="reassignBrowserTab(selectedNode, $event)">
                    <option v-if="!selectedBrowserTab" value="" disabled>Choisir un onglet autorisé</option>
                    <option v-for="tab in browserTabChoices" :key="tab.id" :value="tab.id">{{ tab.title }} · {{ tab.browser }}</option>
                  </select>
                </label>
                <p v-if="!selectedBrowserTab" class="inspector-note">Active la capture dans l'onglet voulu pour l'associer ici.</p>
              </template>
              <template v-if="selectedNode.kind.type === 'application'">
                <button type="button" class="switch-row" role="switch" :aria-checked="selectedNode.kind.muteOriginal !== false"
                  aria-describedby="application-playback-note" :disabled="topologyLocked" @click="toggleMuteOriginal(selectedNode)">
                  <VolumeX :size="16" /><span>Couper la sortie directe</span><i />
                </button>
                <p id="application-playback-note" class="inspector-note">{{ selectedNode.kind.muteOriginal !== false
                  ? 'Pendant la capture, le son passe uniquement par le graphe.'
                  : 'La sortie de l’application joue en parallèle du graphe.' }}</p>
              </template>
              <label v-if="selectedNode.kind.type === 'tone'" class="field"><span>Fréquence de la tonalité (Hz)</span><input type="number" min="20" max="20000" :disabled="topologyLocked"
                :value="frequencyDraft ?? selectedNode.kind.frequencyHz" @input="frequencyDraft = ($event.currentTarget as HTMLInputElement).valueAsNumber" @change="commitFrequency(selectedNode)" /></label>
              <div v-if="selectedNodeRoutes.length" class="inspector-routes"><h4>Connexions</h4>
                <button v-for="route in selectedNodeRoutes" :key="route.id" type="button" class="route-row" @click="select({ kind: 'route', id: route.id })">
                  <Cable :size="14" /><span>{{ route.source === selectedNode.id ? nodeById(route.target)?.label : nodeById(route.source)?.label }}
                    <small>{{ channelName(nodeById(route.source), route.sourceChannel) }} → {{ channelName(nodeById(route.target), route.targetChannel) }}</small></span><ChevronRight :size="14" />
                </button>
              </div>
              <button type="button" class="delete-button" :disabled="topologyLocked" @click="removeSelected"><Trash2 :size="15" /> Retirer du graphe</button>
            </template>

            <template v-else>
              <div class="inspector-title"><div class="inspector-symbol overview-icon"><AudioLines :size="20" /></div>
                <div><h3>Configuration</h3><p>Paramètres du graphe</p></div></div>
              <label class="field"><span>Fréquence du moteur</span>
                <select :value="project.sampleRate" :disabled="topologyLocked || !active"
                  @change="setProject({ ...project, sampleRate: Number(($event.currentTarget as HTMLSelectElement).value) })">
                  <option :value="44100">44,1 kHz</option><option :value="48000">48 kHz</option><option :value="96000">96 kHz</option>
                </select></label>
              <div class="inspector-routes output-summary"><h4>Sorties du graphe</h4>
                <button v-for="node in [...channelGroups, ...destinations]" :key="node.id" type="button" class="route-row" @click="select({ kind: 'node', id: node.id })">
                  <component :is="icons[variantFor(node.kind, catalog)]" :size="16" /><span>{{ node.label }}
                    <small>{{ node.kind.type === 'outputChannels' ? `${node.kind.channels} canaux de sortie` : routes.some((route) => route.target === node.id) ? labelFor(variantFor(node.kind, catalog)) : 'Non connectée' }}</small></span><ChevronRight :size="14" />
                </button>
                <p v-if="!destinations.length" class="inspector-note">Ajoute un périphérique virtuel ou une sortie d’écoute.</p>
              </div>
              <p class="inspector-note">Sélectionne un bloc ou un câble pour le régler.</p>
              <div v-if="status.running && (!status.realtime || status.underruns || status.droppedFrames)" class="engine-counters">
                <Activity :size="15" />
                <span>
                  <template v-if="!status.realtime">Priorité temps réel indisponible</template>
                  <template v-if="status.underruns || status.droppedFrames">
                    <br v-if="!status.realtime" />{{ status.underruns }} sous-alimentations<br />{{ status.droppedFrames }} trames perdues
                  </template>
                </span>
              </div>
            </template>
          </div>
        </aside>
        <OutputMixer v-if="selectedOutput" :key="selectedOutput.id" v-bind="selectedOutput"
          :meters="meters" :running="status.running" :disabled="busy" @level="updateMixerLevel" @pair="updateStereoPair" @master="updateMasterLevel" />
      </section>
    </main>

    <dialog ref="dialog" class="dialog" aria-labelledby="dialog-title" @cancel="closeDialog" @click.self="closeDialog">
      <div class="dialog-content">
        <header class="dialog-header"><h2 id="dialog-title">{{ dialogTitle }}</h2><button type="button" class="icon-button subtle" aria-label="Fermer" @click="closeDialog"><X :size="18" /></button></header>
        <template v-if="modal === 'source' || modal === 'destination'">
          <div class="dialog-filter"><Search :size="16" /><input ref="firstField" v-model="search" aria-label="Filtrer les périphériques" placeholder="Rechercher" />
            <button class="icon-button subtle" type="button" title="Actualiser" aria-label="Actualiser" :disabled="refreshing" @click="refreshCatalog"><RefreshCw :size="15" :class="{ spin: refreshing }" /></button></div>
          <div class="picker-list">
            <button v-if="native && modal === 'source'" type="button" class="picker-option browser-picker-link" @click="openDialog('browserTabs')">
              <span class="picker-icon" data-variant="browserTab"><PanelTop :size="17" /></span>
              <span><strong>Onglets du navigateur</strong><small>Brave, Chrome ou Edge</small></span>
              <ChevronRight :size="16" />
            </button>
            <section v-for="group in pickerGroups" :key="group.label" class="picker-group"><h3>{{ group.label }}</h3>
              <button v-for="item in group.items" :key="`${item.label}-${JSON.stringify(item.kind)}`" type="button" class="picker-option" :disabled="item.added" @click="addNode(item)">
                <span class="picker-icon" :data-variant="item.variant"><component :is="icons[item.variant]" :size="17" /></span>
                <span><strong>{{ item.label }}</strong><small>{{ item.details }}</small></span>
                <Check v-if="item.added" :size="16" aria-label="Dans le graphe" /><Plus v-else :size="16" />
              </button>
            </section>
            <p v-if="!pickerGroups.length" class="dialog-empty">{{ search ? 'Aucun résultat.' : 'Aucun périphérique disponible.' }}</p>
          </div>
        </template>
        <BrowserTabs v-else-if="modal === 'browserTabs'" :tabs="browserTabs" :added="assignedTabs"
          :loading="browserTabsLoading" :error="browserTabsError" :locked="topologyLocked" :full="nodes.length >= 64"
          @add="addBrowserTab" @refresh="refreshBrowserTabs" />
        <form v-else-if="modal === 'internalBus'" class="dialog-form" @submit.prevent="addInternalBus">
          <p>Le bus mélange et redistribue les canaux dans le graphe.</p>
          <label class="field"><span>Nom du bus</span><input ref="firstField" v-model="internalBusName" required maxlength="64" @input="internalBusError = ''" /></label>
          <label class="field"><span>Nombre de canaux</span><input type="number" v-model.number="internalBusCount"
            min="1" max="64" step="1" required @input="internalBusError = ''" /></label>
          <p v-if="internalBusError" class="danger-text" role="alert">{{ internalBusError }}</p>
          <div class="dialog-buttons"><button type="button" class="secondary-button" @click="closeDialog">Annuler</button>
            <button type="submit" class="primary-button" :disabled="!internalBusName.trim() || topologyLocked"><Plus :size="15" /> Ajouter le bus</button>
          </div>
        </form>
        <form v-else-if="modal === 'outputChannels'" class="dialog-form" @submit.prevent="addOutputChannels">
          <p>Chaque canal mélange ses sources. Envoie ensuite le mix vers un bus ou une sortie.</p>
          <label class="field"><span>Nom du groupe</span><input ref="firstField" v-model="outputChannelsName" required maxlength="64" /></label>
          <label v-if="channelGroupTargets.length" class="field"><span>Insérer avant une sortie</span>
            <select v-model="outputChannelsTarget" @change="inferOutputChannelsCount">
              <option value="">Aucune · groupe indépendant</option>
              <option v-for="node in channelGroupTargets" :key="node.id" :value="node.id">{{ node.label }}</option>
            </select>
          </label>
          <label class="field"><span>Nombre de canaux</span><input type="number" v-model.number="outputChannelsCount"
            min="1" max="64" step="1" required @input="outputChannelsError = ''" /></label>
          <p v-if="outputChannelsTarget" class="inspector-note">Les connexions et leurs gains seront repris dans le groupe.</p>
          <p v-if="outputChannelsError" class="danger-text" role="alert">{{ outputChannelsError }}</p>
          <div class="dialog-buttons"><button type="button" class="secondary-button" @click="closeDialog">Annuler</button>
            <button type="submit" class="primary-button" :disabled="!outputChannelsName.trim() || topologyLocked"><Plus :size="15" /> Ajouter les canaux</button>
          </div>
        </form>
        <form v-else-if="modal === 'route'" class="dialog-form" @submit.prevent="connectEndpoints">
          <div class="endpoint-fields"><label class="field"><span>Source</span><select ref="firstField" v-model="routeSource" @change="resetRouteTarget()">
            <option v-for="node in routeSources" :key="node.id" :value="node.id">{{ node.label }}</option></select></label>
            <label class="field"><span>Canal</span><select v-model.number="sourceChannel" @change="routeDialogError = ''">
              <option v-for="channel in chosenRouteSourceChannels" :key="channel" :value="channel">{{ channelName(chosenRouteSource, channel) }}</option></select></label></div>
          <div class="endpoint-fields"><label class="field"><span>Destination</span><select v-model="routeTarget" @change="targetChannel = chosenRouteTargetChannels[0] ?? 0; routeDialogError = ''">
            <option v-for="node in routeTargets" :key="node.id" :value="node.id">{{ node.label }}</option></select></label>
            <label class="field"><span>Canal</span><select v-model.number="targetChannel" @change="routeDialogError = ''">
              <option v-for="channel in chosenRouteTargetChannels" :key="channel" :value="channel">{{ channelName(chosenRouteTarget, channel) }}</option></select></label></div>
          <p v-if="routeDialogError" class="danger-text" role="alert">{{ routeDialogError }}</p>
          <div class="dialog-buttons"><button type="button" class="secondary-button" @click="closeDialog">Annuler</button><button type="submit" class="primary-button"><Cable :size="16" /> Connecter</button></div>
        </form>
        <form v-else-if="modal === 'new' || modal === 'rename'" class="dialog-form" @submit.prevent="submitGraph">
          <label class="field"><span>Nom</span><input ref="firstField" v-model="graphName" required maxlength="80" placeholder="Studio, streaming, podcast…" /></label>
          <div class="dialog-buttons"><button type="button" class="secondary-button" @click="closeDialog">Annuler</button><button type="submit" class="primary-button" :disabled="!graphName.trim()">{{ modal === 'new' ? 'Créer' : 'Enregistrer' }}</button></div>
        </form>
        <div v-else-if="modal === 'delete'" class="dialog-form">
          <p>Supprimer « {{ active?.name }} » et ses connexions ?</p>
          <div class="dialog-buttons"><button type="button" class="secondary-button" @click="closeDialog">Annuler</button><button type="button" class="danger-button" @click="deleteGraph">Supprimer</button></div>
        </div>
        <div v-else-if="modal === 'buses'" class="dialog-form bus-dialog">
          <p>Chaque interface Patchbay fournit sa propre entrée et sa propre sortie dans macOS.</p>
          <p v-if="busError" class="danger-text" role="alert">{{ busError }}</p>
          <p v-if="status.running" class="inspector-note">Arrête le routage pour modifier les interfaces.</p>
          <p v-if="restartingCoreAudio" role="status">Autorisation et chargement du pilote Core Audio en cours…</p>
          <section v-else-if="!busCatalog.driverAvailable && !busBusy" class="virtual-driver-setup" aria-label="État du pilote">
            <p v-if="busCatalog.driverInstalled"><strong>Pilote installé.</strong> Relance Core Audio pour charger le pilote et créer des interfaces.</p>
            <template v-else>
              <p>Installe <strong>Patchbay.driver</strong> dans <code>/Library/Audio/Plug-Ins/HAL</code> avec les droits administrateur. Après le chargement par Core Audio, vérifie le pilote ici.</p>
              <label v-if="driverPath" class="field"><span>Dossier du pilote fourni</span>
                <span class="virtual-driver-path">
                  <input ref="driverPathField" readonly :value="driverPath" @focus="driverPathField?.select()" />
                  <button type="button" class="icon-button subtle" title="Copier le chemin du pilote" aria-label="Copier le chemin du pilote" @click="copyDriverPath">
                    <Check v-if="driverPathCopied" :size="15" /><Copy v-else :size="15" />
                  </button>
                </span>
              </label>
            </template>
            <template v-if="confirmDriverRestart && busCatalog.driverInstalled">
              <p>Le son de toutes les apps sera brièvement coupé. Mets les lectures et le monitoring en pause pour limiter le risque de craquement. macOS peut demander une autorisation administrateur.</p>
              <div class="dialog-buttons">
                <button type="button" class="secondary-button" @click="confirmDriverRestart = false">Annuler</button>
                <button type="button" class="primary-button" :disabled="topologyLocked" @click="activateVirtualDriver">Confirmer la relance</button>
              </div>
            </template>
            <div v-else class="virtual-driver-actions">
              <button v-if="busCatalog.driverInstalled" type="button" class="primary-button" :disabled="topologyLocked"
                @click="confirmDriverRestart = true"><RefreshCw :size="14" /> Relancer Core Audio</button>
              <button type="button" class="secondary-button" @click="refreshBuses">Vérifier le pilote</button>
            </div>
          </section>
          <section v-if="busCatalog.buses.length" class="bus-list" aria-label="Interfaces existantes">
            <div v-for="bus in busCatalog.buses" :key="bus.id" class="bus-row">
              <span class="picker-icon" data-variant="virtual"><Radio :size="17" /></span>
              <span><strong>{{ bus.name }}</strong><small>{{ virtualEndpointSummary(bus) }}</small></span>
              <button type="button" class="secondary-button virtual-endpoint-button" :aria-label="`Ajouter ${bus.name} comme source`"
                :disabled="topologyLocked || busBusy || !canAddVirtualEndpoint(bus, 'deviceInput')" @click="addVirtualEndpoint(bus, 'deviceInput')"><Plus :size="13" /> Source</button>
              <button type="button" class="secondary-button virtual-endpoint-button" :aria-label="`Ajouter ${bus.name} comme destination`"
                :disabled="topologyLocked || busBusy || !canAddVirtualEndpoint(bus, 'deviceOutput')" @click="addVirtualEndpoint(bus, 'deviceOutput')"><Plus :size="13" /> Destination</button>
              <button type="button" class="icon-button subtle danger-text" :title="`Supprimer ${bus.name}`" :aria-label="`Supprimer ${bus.name}`"
                :disabled="busBusy || topologyLocked || usedDeviceIds.has(bus.id)" @click="busToRemove = bus"><Trash2 :size="15" /></button>
            </div>
          </section>
          <section v-if="busToRemove" class="bus-confirm" aria-label="Confirmer la suppression">
            <p>Supprimer « {{ busToRemove.name }} » des périphériques audio de macOS ?</p>
            <div class="dialog-buttons">
              <button type="button" class="secondary-button" :disabled="busBusy" @click="busToRemove = null">Annuler</button>
              <button type="button" class="danger-button" :disabled="busBusy || topologyLocked || usedDeviceIds.has(busToRemove.id)" @click="modifyBus">Supprimer l’interface</button>
            </div>
          </section>
          <form v-else-if="busCatalog.driverAvailable" class="bus-create" @submit.prevent="modifyBus">
            <label class="field"><span>Nom de l’interface</span><input ref="firstField" v-model="busName" required maxlength="64" placeholder="Mix streaming"
              :disabled="busBusy || topologyLocked" /></label>
            <div class="bus-direction-fields">
              <label class="field"><span>Nombre d'entrées</span><input v-model.number="busInputChannels" type="number" min="1" max="64" required
                :disabled="busBusy || topologyLocked" /></label>
              <label class="field"><span>Nombre de sorties</span><input v-model.number="busOutputChannels" type="number" min="1" max="64" required
                :disabled="busBusy || topologyLocked" /></label>
            </div>
            <div class="dialog-buttons"><button type="submit" class="primary-button" :disabled="!busName.trim() || !validBusShape || busBusy || topologyLocked"><Plus :size="15" /> Créer l’interface</button></div>
          </form>
        </div>
      </div>
    </dialog>
  </div>
</template>
