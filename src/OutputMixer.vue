<script setup lang="ts">
import { ChevronLeft, ChevronRight, Link2, SlidersHorizontal, Unlink2, VolumeX, X } from 'lucide-vue-next'
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import type { MeterStore } from './meters'
import { channelLevels, dbText, type ChannelLevel, type Meter, type OutputChannelKind, type StereoPair } from './model'

const props = defineProps<{
  id: string
  label: string
  kind: OutputChannelKind
  meters: MeterStore
  running: boolean
  disabled: boolean
}>()

const emit = defineEmits<{
  level: [id: string, channels: readonly number[], values: Partial<ChannelLevel>]
  pair: [id: string, pair: StereoPair, linked: boolean]
  master: [id: string, values: Partial<ChannelLevel>]
}>()
const minGainDb = -60
const maxGainDb = 12.1
const peakFormat = new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 0 })
const meter = shallowRef<Meter>()
const bank = ref<HTMLOListElement | null>(null)
const canScrollLeft = ref(false)
const canScrollRight = ref(false)
const linkFrom = ref<number | null>(null)
let observer: ResizeObserver | undefined

watch(() => props.id, (id, _, cleanup) => {
  const read = () => { meter.value = props.meters.get(id) }
  read()
  cleanup(props.meters.subscribe(id, read))
}, { immediate: true })

const levels = computed(() => channelLevels(props.kind))
const master = computed(() => props.kind.master ?? { gain: 1, muted: false })
const groups = computed(() => {
  const pairedChannels = new Map<number, StereoPair>()
  for (const pair of props.kind.stereoPairs ?? []) {
    pairedChannels.set(pair[0], pair)
    pairedChannels.set(pair[1], pair)
  }
  const cells: number[][] = []
  for (let channel = 0; channel < props.kind.channels; channel++) {
    const pair = pairedChannels.get(channel)
    if (!pair) cells.push([channel])
    else if (channel === Math.min(...pair)) cells.push(pair)
  }
  return cells
})
const availableMono = computed(() => groups.value.filter((group) => group.length === 1).length)

function readPeak(sample: number, clipping: boolean) {
  const peakDb = sample > 0 ? 20 * Math.log10(sample) : -Infinity
  const meterDb = Math.max(-60, Math.min(0, peakDb))
  return {
    peak: sample > 0 ? peakFormat.format(peakDb) : '−∞',
    meterDb,
    fill: (meterDb + 60) / 60 * 100,
    clipping,
  }
}

function channelMeter(index: number) {
  const sample = props.running ? meter.value?.channels[index] ?? 0 : 0
  const clipping = props.running && (meter.value?.clippedChannels?.[index] ?? Boolean(meter.value?.clipping && sample >= 1))
  return { index, number: index + 1, ...readPeak(sample, clipping) }
}

const masterMeters = computed(() => {
  const stereo = props.running ? meter.value?.master : undefined
  return (['gauche', 'droit'] as const).map((side, index) => ({
    side,
    letter: index === 0 ? 'G' : 'D',
    ...readPeak(stereo?.channels[index] ?? 0, stereo?.clippedChannels[index] ?? false),
  }))
})

function gainDb(gain: number) {
  return gain > 0 ? Math.max(minGainDb, 20 * Math.log10(gain)) : minGainDb
}

const strips = computed(() => groups.value.map((channels) => {
  const [left, right] = channels
  const first = levels.value[left]
  const second = levels.value[right]
  const mixedGain = second && first.gain !== second.gain
  const gain = dbText(first.gain)
  const otherGain = second ? dbText(second.gain) : gain
  return {
    channels,
    key: channels.join('-'),
    stereo: channels.length === 2,
    numbers: channels.map((channel) => channel + 1).join(' et '),
    label: second ? `des canaux ${left + 1} et ${right + 1}` : `du canal ${left + 1}`,
    gain: mixedGain ? `G ${gain.replace(' dB', '')} · D ${otherGain}` : gain,
    gainAria: mixedGain ? `Gauche ${gain}, droite ${otherGain}` : gain,
    mixedGain: Boolean(mixedGain),
    gainDb: gainDb(first.gain),
    muted: second && first.muted !== second.muted ? 'mixed' as const : first.muted,
    meters: channels.map(channelMeter),
  }
}))

function gainFromSlider(event: Event) {
  const db = (event.currentTarget as HTMLInputElement).valueAsNumber
  if (!Number.isFinite(db)) return undefined
  return db <= minGainDb ? 0 : Math.min(4, 10 ** (db / 20))
}

function updateGain(channels: readonly number[], event: Event) {
  const gain = gainFromSlider(event)
  if (gain !== undefined) emit('level', props.id, channels, { gain })
}

function updateMasterGain(event: Event) {
  const gain = gainFromSlider(event)
  if (gain !== undefined) emit('master', props.id, { gain })
}

async function chooseLink([channel, partner]: number[]) {
  if (partner !== undefined) {
    emit('pair', props.id, [channel, partner], false)
    await focusLink(String(channel))
  } else if (linkFrom.value === channel) {
    linkFrom.value = null
  } else if (linkFrom.value === null) {
    linkFrom.value = channel
  } else {
    const pair: StereoPair = [Math.min(linkFrom.value, channel), Math.max(linkFrom.value, channel)]
    linkFrom.value = null
    emit('pair', props.id, pair, true)
    await focusLink(pair.join('-'))
  }
}

async function focusLink(key: string) {
  await nextTick()
  bank.value?.querySelector<HTMLButtonElement>(`[data-link-key="${key}"]`)?.focus()
}

function updateScroll() {
  const element = bank.value
  if (!element) return
  canScrollLeft.value = element.scrollLeft > 1
  canScrollRight.value = element.scrollLeft + element.clientWidth < element.scrollWidth - 1
}

function moveBank(direction: number) {
  const element = bank.value
  if (element) element.scrollBy({ left: direction * element.clientWidth })
}

watch([() => props.kind.channels, () => props.kind.stereoPairs], () => {
  if (linkFrom.value !== null && !groups.value.some(([channel, partner]) => channel === linkFrom.value && partner === undefined)) linkFrom.value = null
  void nextTick(updateScroll)
})
onMounted(() => {
  if (!bank.value) return
  observer = new ResizeObserver(updateScroll)
  observer.observe(bank.value)
  updateScroll()
})
onBeforeUnmount(() => observer?.disconnect())
</script>

<template>
  <section class="output-mixer" aria-labelledby="output-mixer-heading" @keydown.esc="linkFrom = null">
    <header class="mixer-header">
      <div class="mixer-header-title">
        <SlidersHorizontal :size="16" aria-hidden="true" />
        <h2 id="output-mixer-heading">Mixer</h2>
        <span :title="label">{{ label }}</span>
      </div>
      <div class="mixer-header-actions">
        <div v-if="linkFrom !== null" class="mixer-link-prompt">
          <span role="status">Canal {{ linkFrom + 1 }} sélectionné. Choisis le second.</span>
          <button type="button" class="icon-button subtle" title="Annuler la liaison" aria-label="Annuler la liaison"
            @click="linkFrom = null"><X :size="14" /></button>
        </div>
        <span v-else>{{ kind.channels }} {{ kind.channels === 1 ? 'canal' : 'canaux' }}</span>
        <div v-if="canScrollLeft || canScrollRight" class="mixer-bank-controls">
          <button type="button" class="icon-button subtle" aria-label="Canaux précédents" :disabled="!canScrollLeft" @click="moveBank(-1)"><ChevronLeft :size="15" /></button>
          <button type="button" class="icon-button subtle" aria-label="Canaux suivants" :disabled="!canScrollRight" @click="moveBank(1)"><ChevronRight :size="15" /></button>
        </div>
      </div>
    </header>

    <div class="mixer-body">
      <ol ref="bank" class="mixer-bank" @scroll.passive="updateScroll">
        <li v-for="strip in strips" :key="strip.key" class="mixer-strip" :data-stereo="strip.stereo"
          :data-link-source="!strip.stereo && linkFrom === strip.channels[0]">
          <div class="mixer-strip-header">
            <span>{{ strip.stereo ? 'Canaux' : 'Canal' }} <strong>{{ strip.numbers }}</strong></span>
            <div class="mixer-strip-actions">
              <span v-if="!strip.stereo" class="mixer-clip" :data-active="Boolean(strip.meters[0].clipping)"
                :title="strip.meters[0].clipping ? 'Écrêtage' : undefined" aria-hidden="true" />
              <button type="button" class="mixer-link" :data-link-key="strip.key"
                :disabled="disabled || (strip.stereo ? linkFrom !== null : availableMono < 2)"
                :aria-label="strip.stereo ? `Délier les canaux ${strip.numbers}` : `Lier le canal ${strip.numbers}`"
                :title="strip.stereo ? 'Délier la paire stéréo' : 'Créer une paire stéréo'"
                :aria-pressed="!strip.stereo ? linkFrom === strip.channels[0] : undefined" @click="chooseLink(strip.channels)">
                <Unlink2 v-if="strip.stereo" :size="13" aria-hidden="true" /><Link2 v-else :size="13" aria-hidden="true" />
              </button>
            </div>
          </div>
          <div class="mixer-strip-controls">
            <div class="mixer-fader">
              <input type="range" :aria-label="`Gain ${strip.label}`" aria-orientation="vertical"
                :aria-valuetext="strip.gainAria" :min="minGainDb" :max="maxGainDb" step="0.1" :value="strip.gainDb"
                :disabled="disabled" @input="updateGain(strip.channels, $event)" />
            </div>
            <div v-for="(channel, index) in strip.meters" :key="channel.index" class="mixer-meter-column">
              <span v-if="strip.stereo" class="mixer-meter-label" aria-hidden="true">
                <span class="mixer-clip" :data-active="Boolean(channel.clipping)" />{{ index === 0 ? 'G' : 'D' }} {{ channel.number }}
              </span>
              <div class="mixer-meter" role="meter" :aria-label="`Niveau du canal ${channel.number}`" aria-valuemin="-60"
                aria-valuemax="0" :aria-valuenow="channel.meterDb" :aria-valuetext="`${channel.peak} dBFS${channel.clipping ? ', écrêtage' : ''}`"
                :data-clipped="Boolean(channel.clipping)" :style="{ '--peak': `${channel.fill}%` }" />
            </div>
          </div>
          <div class="mixer-values">
            <output :data-mixed="strip.mixedGain" :title="strip.mixedGain ? 'Gains différents jusqu’au premier réglage du fader' : undefined">{{ strip.gain }}</output>
            <span><template v-for="(channel, index) in strip.meters" :key="channel.index"><span v-if="index"> / </span>{{ channel.peak }}</template> <small>dBFS</small></span>
          </div>
          <button type="button" class="mixer-mute" :disabled="disabled" :aria-label="`Couper le son ${strip.label}`"
            :aria-pressed="strip.muted" @click="emit('level', id, strip.channels, { muted: strip.muted !== true })">
            <VolumeX :size="12" aria-hidden="true" /> {{ strip.muted === 'mixed' ? 'Mute partiel' : 'Mute' }}
          </button>
        </li>
      </ol>

      <div class="mixer-master-dock">
        <section class="mixer-strip mixer-master" data-stereo="true" aria-label="Master">
          <div class="mixer-strip-header">
            <strong>Master</strong><span>Stéréo</span>
          </div>
          <div class="mixer-strip-controls">
            <div class="mixer-fader">
              <input type="range" aria-label="Gain du master" aria-orientation="vertical" :aria-valuetext="dbText(master.gain)"
                :min="minGainDb" :max="maxGainDb" step="0.1" :value="gainDb(master.gain)" :disabled="disabled" @input="updateMasterGain" />
            </div>
            <div v-for="channel in masterMeters" :key="channel.side" class="mixer-meter-column">
              <span class="mixer-meter-label" aria-hidden="true">
                <span class="mixer-clip" :data-active="channel.clipping" />{{ channel.letter }}
              </span>
              <div class="mixer-meter" role="meter" :aria-label="`Niveau ${channel.side} du master`" aria-valuemin="-60"
                aria-valuemax="0" :aria-valuenow="channel.meterDb" :aria-valuetext="`${channel.peak} dBFS${channel.clipping ? ', écrêtage' : ''}`"
                :data-clipped="channel.clipping" :style="{ '--peak': `${channel.fill}%` }" />
            </div>
          </div>
          <div class="mixer-values">
            <output>{{ dbText(master.gain) }}</output>
            <span>{{ masterMeters[0].peak }} / {{ masterMeters[1].peak }} <small>dBFS</small></span>
          </div>
          <button type="button" class="mixer-mute" aria-label="Couper le son du master" :aria-pressed="master.muted" :disabled="disabled"
            @click="emit('master', id, { muted: !master.muted })"><VolumeX :size="12" aria-hidden="true" /> Mute</button>
        </section>
      </div>
    </div>
  </section>
</template>
