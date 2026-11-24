<script setup lang="ts">
import { createElement } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { FlowCanvas, type FlowProps } from './FlowCanvas'
import type { Endpoints, Selection } from './model'

const props = defineProps<Pick<FlowProps, 'project' | 'catalog' | 'selection' | 'topologyLocked' | 'meters'>>()
const emit = defineEmits<{
  select: [selection: Selection]
  connect: [endpoints: Endpoints]
  move: [id: string, x: number, y: number]
  delete: []
}>()
const host = ref<HTMLDivElement | null>(null)
let root: Root | undefined
const onSelect = (value: Selection) => emit('select', value)
const onConnect = (value: Endpoints) => emit('connect', value)
const onMove = (id: string, x: number, y: number) => emit('move', id, x, y)
const onDelete = () => emit('delete')

function renderFlow() {
  root?.render(createElement(FlowCanvas, {
    ...props, onSelect, onConnect, onMove, onDelete,
  }))
}

onMounted(() => {
  if (host.value) root = createRoot(host.value)
  renderFlow()
})
watch(() => [props.project, props.catalog, props.selection, props.topologyLocked], renderFlow)
onBeforeUnmount(() => root?.unmount())
</script>

<template>
  <div ref="host" class="graph-react-host" />
</template>
