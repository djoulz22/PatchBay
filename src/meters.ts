import type { Meter } from './model'

export class MeterStore {
  private meters: Record<string, Meter> = {}
  private listeners = new Map<string, Set<() => void>>()

  get(id: string) { return this.meters[id] }

  subscribe(id: string, listener: () => void): () => void {
    const group = this.listeners.get(id) ?? new Set<() => void>()
    group.add(listener)
    this.listeners.set(id, group)
    return () => {
      group.delete(listener)
      if (!group.size) this.listeners.delete(id)
    }
  }

  set(meters: Record<string, Meter>) {
    const previous = this.meters
    this.meters = meters
    for (const [id, group] of this.listeners) {
      const before = previous[id]
      const after = meters[id]
      if (!before && !after) continue
      const previousMaster = before?.master
      const nextMaster = after?.master
      const unchangedMaster = (!previousMaster && !nextMaster) || (previousMaster && nextMaster
        && previousMaster.channels.every((peak, index) => peak === nextMaster.channels[index])
        && previousMaster.clippedChannels.every((clipped, index) => clipped === nextMaster.clippedChannels[index]))
      if (before && after && before.clipping === after.clipping && before.channels.length === after.channels.length
        && before.clippedChannels?.length === after.clippedChannels?.length
        && (before.clippedChannels ?? []).every((clipped, index) => clipped === after.clippedChannels?.[index])
        && before.channels.every((value, index) => value === after.channels[index]) && unchangedMaster) continue
      for (const listener of group) listener()
    }
  }
}
