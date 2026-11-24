import { describe, expect, it } from 'vitest'
import {
  MASTER_CHANNELS, canInsertOutputChannels, channelLevels, emptyCatalog, hasVirtualFeedback,
  insertOutputChannels, isMasterChannel, minimumMixStageChannels, projectNeedsMicrophone, resizeMixStage, validConnection,
  type AudioCatalog, type AudioDevice, type GraphNode, type NodeKind, type OutputChannelKind, type Project, type Route,
} from './model'

function graphNode(id: string, kind: NodeKind): GraphNode {
  return { id, kind, label: id, x: 0, y: 0 }
}

function projectOf(nodes: GraphNode[], routes: Route[] = []): Project {
  return { nodes, routes, sampleRate: 48_000 }
}

function route(id: string, source: string, target: string, sourceChannel = 0, targetChannel = 0): Route {
  return { id, source, target, sourceChannel, targetChannel, gain: 1, muted: false }
}

function device(id: string, channels: number, isVirtual = false): AudioDevice {
  return { id, name: id, channels, isVirtual, isDefault: false, sampleRate: 48_000 }
}

const monitor = graphNode('monitor', { type: 'deviceOutput', deviceId: 'monitor-device' })
const output = device('monitor-device', 4)
const catalog: AudioCatalog = { ...emptyCatalog, outputs: [output] }
const application = graphNode('app', { type: 'application', bundleId: 'example.app' })
const tone = graphNode('tone', { type: 'tone', frequencyHz: 1000 })
const mix = graphNode('mix', { type: 'outputChannels', channels: 4 })
const [masterLeft, masterRight] = MASTER_CHANNELS

describe('insertOutputChannels', () => {
  it('préserve les câbles existants et crée un départ unitaire par canal utilisé', () => {
    const stereo = { ...route('stereo', application.id, monitor.id, 1, 2), gain: 0.35, muted: true }
    const sameChannel = { ...route('same-channel', tone.id, monitor.id, 0, 2), gain: 0.75 }
    const otherChannel = route('other-channel', application.id, monitor.id)
    const otherDestination = graphNode('other', { type: 'internalBus', channels: 2 })
    const unrelated = route('unrelated', tone.id, otherDestination.id)
    const project = projectOf([application, tone, monitor, otherDestination], [stereo, sameChannel, otherChannel, unrelated])
    const next = insertOutputChannels(project, catalog, mix, monitor.id)

    expect(next.nodes).toEqual([...project.nodes, mix])
    expect(next.routes).toEqual([
      { ...stereo, target: mix.id }, { ...sameChannel, target: mix.id }, { ...otherChannel, target: mix.id }, unrelated,
      { ...route('send-3', mix.id, monitor.id, 2, 2), id: expect.any(String) },
      { ...route('send-1', mix.id, monitor.id), id: expect.any(String) },
    ])
    expect(project.routes[0]).toBe(stereo)
  })

  it.each([2, 6])('câble le minimum des capacités sans câble entrant (%i canaux de mix)', (channels) => {
    const stage = graphNode('mix', { type: 'outputChannels', channels })
    const next = insertOutputChannels(projectOf([monitor]), catalog, stage, monitor.id)
    const count = Math.min(channels, output.channels)
    expect(next.routes).toEqual(Array.from({ length: count }, (_, channel) =>
      { return { ...route('send', stage.id, monitor.id, channel, channel), id: expect.any(String) } },
    ))
  })

  it('refuse un canal utilisé au-delà de la capacité du groupe', () => {
    const stage = graphNode('mix', { type: 'outputChannels', channels: 2 })
    const project = projectOf([tone, monitor], [route('high', tone.id, monitor.id, 0, 2)])
    expect(() => insertOutputChannels(project, catalog, stage, monitor.id)).toThrowError('Le câblage existant utilise le canal 3.')
  })

  it('refuse une sortie qui reçoit déjà un groupe', () => {
    const stage = graphNode('added', { type: 'outputChannels', channels: 2 })
    const project = projectOf([mix, monitor], [route('existing', mix.id, monitor.id)])
    expect(() => insertOutputChannels(project, catalog, stage, monitor.id)).toThrowError(
      'Choisis une sortie qui ne reçoit pas déjà un bloc de canaux.',
    )
  })

  it('refuse de dépasser 64 blocs', () => {
    const nodes = Array.from({ length: 63 }, (_, index) => graphNode(`tone-${index}`, tone.kind))
    expect(() => insertOutputChannels(projectOf([monitor, ...nodes]), catalog, mix, monitor.id)).toThrowError(
      'Impossible d’ajouter le bloc de canaux au graphe.',
    )
  })

  it('refuse de dépasser 1024 liaisons', () => {
    const sources = Array.from({ length: 16 }, (_, index) => graphNode(`tone-${index}`, tone.kind))
    const fullBus = graphNode('full', { type: 'internalBus', channels: 64 })
    const routes = sources.flatMap((source) => Array.from({ length: 64 }, (_, channel) =>
      route(`${source.id}-${channel}`, source.id, fullBus.id, 0, channel)))
    const project = projectOf([monitor, fullBus, ...sources], routes)
    expect(() => insertOutputChannels(project, catalog, mix, monitor.id)).toThrowError('Le graphe dépasserait 1024 connexions.')
  })
})

describe('canInsertOutputChannels', () => {
  it('accepte une sortie simple et exclut les autres nœuds et une sortie déjà alimentée par un groupe', () => {
    const project = projectOf([tone, mix, monitor])
    expect(canInsertOutputChannels(project, monitor.id)).toBe(true)
    expect(canInsertOutputChannels(project, tone.id)).toBe(false)
    expect(canInsertOutputChannels({ ...project, routes: [route('existing', mix.id, monitor.id)] }, monitor.id)).toBe(false)
  })
})

describe('validConnection', () => {
  it('refuse un doublon, même avec un autre identifiant de câble', () => {
    const existing = route('existing', tone.id, monitor.id)
    const project = projectOf([tone, monitor], [existing])
    expect(validConnection(project, catalog, route('copy', tone.id, monitor.id))).toBe(false)
  })

  it('refuse une boucle entre étages, même sur des canaux distincts', () => {
    const busA = graphNode('a', { type: 'internalBus', channels: 2 })
    const busB = graphNode('b', { type: 'internalBus', channels: 2 })
    const project = projectOf([busA, busB], [route('forward', busA.id, busB.id)])
    expect(validConnection(project, catalog, route('loop', busB.id, busA.id, 1, 1))).toBe(false)
  })

  it('détecte la boucle entre un périphérique virtuel et son alias et accepte les canaux indépendants', () => {
    const input = graphNode('input', { type: 'deviceInput', deviceId: 'BlackHole2ch_UID' })
    const aliasId = 'fr.patchbay.audio.bus.BlackHole2ch_UID'
    const alias = graphNode('alias', { type: 'deviceOutput', deviceId: aliasId })
    const virtual = device('BlackHole2ch_UID', 2, true)
    const virtualCatalog = { ...emptyCatalog, inputs: [virtual], outputs: [device(aliasId, 2, true)] }
    const project = projectOf([input, alias])
    expect(validConnection(project, virtualCatalog, route('loop', input.id, alias.id))).toBe(false)
    expect(validConnection(project, virtualCatalog, route('independent', input.id, alias.id, 0, 1))).toBe(true)
  })

  it.each(MASTER_CHANNELS)('réserve le canal Master %i aux départs d’un groupe de sortie', (channel) => {
    const internalBus = graphNode('internal', { type: 'internalBus', channels: 64 })
    const project = projectOf([monitor, mix, internalBus])
    expect(validConnection(project, catalog, route('master', mix.id, monitor.id, channel))).toBe(true)
    expect(validConnection(project, catalog, route('internal', internalBus.id, monitor.id, channel))).toBe(false)
    expect(validConnection(project, catalog, route('target', internalBus.id, mix.id, 0, channel))).toBe(false)
  })
})

describe('resizeMixStage', () => {
  it('initialise les nouveaux canaux à 0 dB, sans mute, et conserve les niveaux existants', () => {
    const level = { gain: 0.5, muted: true }
    const stage = graphNode('stage', { type: 'outputChannels', channels: 1, levels: [level] })
    const next = resizeMixStage(projectOf([stage]), catalog, stage.id, 3)
    expect(next.nodes[0].kind).toMatchObject({ channels: 3, levels: [level, { gain: 1, muted: false }, { gain: 1, muted: false }] })
    expect(stage.kind).toMatchObject({ channels: 1, levels: [level] })
  })

  it('conserve les canaux câblés, les départs Master et les paires stéréo encore dans les limites', () => {
    const stage = graphNode('stage', { type: 'outputChannels', channels: 4, stereoPairs: [[0, 1], [2, 3]] })
    const routes = [route('numbered', stage.id, monitor.id, 1, 1),
      route('master-g', stage.id, monitor.id, masterLeft, 2), route('master-d', stage.id, monitor.id, masterRight, 3)]
    const project = projectOf([stage, monitor], routes)
    expect(minimumMixStageChannels(project, stage.id)).toBe(2)
    const next = resizeMixStage(project, catalog, stage.id, 2)
    expect(next.nodes[0].kind).toMatchObject({ channels: 2, stereoPairs: [[0, 1]] })
    expect(next.routes).toEqual(routes)
  })

  it('refuse une réduction sous le dernier canal relié en entrée', () => {
    const project = projectOf([mix, tone], [route('source', tone.id, mix.id, 0, 2)])
    expect(() => resizeMixStage(project, catalog, mix.id, 2)).toThrowError(
      'Le canal 3 est encore relié. Déconnecte les canaux à retirer avant de réduire le groupe.',
    )
  })

  it.each([0, 65, NaN])('refuse une capacité invalide (%s)', (count) => {
    expect(() => resizeMixStage(projectOf([mix]), catalog, mix.id, count)).toThrowError('Choisis entre 1 et 64 canaux.')
  })

  it('détecte une boucle via le Master après suppression d’une paire stéréo', () => {
    const virtualId = 'fr.patchbay.audio.native.virtual'
    const stage = graphNode('stage', { type: 'outputChannels', channels: 2, stereoPairs: [[1, 0]] })
    const input = graphNode('input', { type: 'deviceInput', deviceId: virtualId })
    const destination = graphNode('output', { type: 'deviceOutput', deviceId: virtualId })
    const project = projectOf([input, stage, destination], [route('read', input.id, stage.id),
      route('write', stage.id, destination.id, masterLeft)])
    expect(hasVirtualFeedback(project, emptyCatalog)).toBe(false)
    expect(() => resizeMixStage(project, emptyCatalog, stage.id, 1)).toThrowError(
      'Ce redimensionnement fermerait une boucle audio par le Master.',
    )
  })
})

describe('helpers de canaux et de capture', () => {
  it('normalise les niveaux absents et respecte la longueur demandée', () => {
    const level = { gain: 0.25, muted: true }
    const kind: OutputChannelKind = { type: 'outputChannels', channels: 2, levels: [level] }
    expect(channelLevels(kind)).toEqual([level, { gain: 1, muted: false }])
    expect(channelLevels(kind, 1)).toEqual([level])
  })

  it.each([[63, false], [64, true], [65, true], [66, false]] as const)('reconnaît uniquement les prises Master (%i)', (channel, expected) => {
    expect(isMasterChannel(channel)).toBe(expected)
  })

  it('demande l’autorisation micro uniquement pour une entrée de périphérique utilisée comme source', () => {
    const microphone = graphNode('mic', { type: 'deviceInput', deviceId: 'microphone' })
    const system = graphNode('system', { type: 'systemAudio' })
    const nodes = [microphone, system, application, monitor]
    expect(projectNeedsMicrophone(projectOf(nodes, [route('sys', system.id, monitor.id), route('app', application.id, monitor.id)]))).toBe(false)
    expect(projectNeedsMicrophone(projectOf(nodes, [route('target-only', application.id, microphone.id)]))).toBe(false)
    expect(projectNeedsMicrophone(projectOf(nodes, [route('capture', microphone.id, monitor.id)]))).toBe(true)
  })
})
