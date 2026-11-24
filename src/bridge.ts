import { invoke, isTauri } from '@tauri-apps/api/core'
import type { AudioCatalog, BrowserPairing, BrowserTab, EngineStatus, Project, Session, VirtualBus, VirtualBuses } from './model'

export const native = isTauri()

const key = 'patchbay.session.v1'
const demoCatalog: AudioCatalog = {
  inputs: [{ id: 'demo-mic', name: 'Microphone studio', channels: 1, sampleRate: 48_000, isDefault: true, isVirtual: false }],
  outputs: [
    { id: 'demo-virtual', name: 'BlackHole 2ch', channels: 2, sampleRate: 48_000, isDefault: false, isVirtual: true },
    { id: 'demo-monitor', name: 'Casque studio', channels: 2, sampleRate: 48_000, isDefault: true, isVirtual: false },
  ],
  applications: [{ bundleId: 'demo.application', name: 'Lecteur audio (exemple)', active: false }],
  systemAudio: true,
}

export async function audioCatalog(): Promise<AudioCatalog> {
  return native ? invoke('audio_catalog') : demoCatalog
}

export async function loadSession(): Promise<Session | null> {
  if (native) return invoke('load_session')
  const document = localStorage.getItem(key)
  return document ? JSON.parse(document) : null
}

export async function saveSession(session: Session): Promise<void> {
  if (native) return invoke('save_session', { session })
  localStorage.setItem(key, JSON.stringify(session))
}

export function startRouting(project: Project): Promise<unknown> { return invoke('start_routing', { project }) }
export function updateRouting(project: Project): Promise<unknown> { return invoke('update_routing', { project }) }
export function stopRouting(): Promise<unknown> { return invoke('stop_routing') }
export function routingStatus(): Promise<EngineStatus> { return invoke('routing_status') }
export function listVirtualBuses(): Promise<VirtualBuses> { return invoke('virtual_buses') }
export function restartCoreAudio(): Promise<VirtualBuses | null> { return invoke('restart_core_audio') }
export function createVirtualBus(name: string, inputChannels: number, outputChannels: number): Promise<VirtualBus> {
  return invoke('create_virtual_bus', { name, inputChannels, outputChannels })
}
export function deleteVirtualBus(id: string): Promise<void> { return invoke('delete_virtual_bus', { id }) }
export function virtualDriverPath(): Promise<string> { return invoke('virtual_driver_path') }
export function listBrowserTabs(): Promise<BrowserTab[]> { return invoke('browser_capture_tabs') }
export function pairBrowserExtension(): Promise<BrowserPairing> { return invoke('browser_capture_pairing') }
export function browserExtensionPath(): Promise<string> { return invoke('browser_capture_extension_path') }
