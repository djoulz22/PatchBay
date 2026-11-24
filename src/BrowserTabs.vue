<script setup lang="ts">
import { Check, ChevronDown, Copy, PanelTop, Plus, RefreshCw } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { browserExtensionPath, pairBrowserExtension } from './bridge'
import type { BrowserTab } from './model'

defineProps<{
  tabs: BrowserTab[]
  added: ReadonlySet<string>
  loading: boolean
  error: string
  locked: boolean
  full: boolean
}>()

defineEmits<{ add: [id: string]; refresh: [] }>()

const configuring = ref(false)
const fetching = ref(false)
const setupError = ref('')
const feedback = ref('')
const extensionDirectory = ref('')
const pairingCode = ref('')
const directoryInput = ref<HTMLInputElement | null>(null)
const codeInput = ref<HTMLInputElement | null>(null)
const ready = computed(() => Boolean(extensionDirectory.value && pairingCode.value))

async function prepareSetup() {
  if (fetching.value || ready.value) return
  fetching.value = true
  setupError.value = ''
  try {
    const [directory, pairing] = await Promise.all([browserExtensionPath(), pairBrowserExtension()])
    extensionDirectory.value = directory
    pairingCode.value = `${pairing.port}:${pairing.token}`
  } catch (cause) {
    setupError.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    fetching.value = false
  }
}

function toggleSetup() {
  configuring.value = !configuring.value
  if (configuring.value) void prepareSetup()
}

async function copy(field: HTMLInputElement | null, success: string) {
  if (!field) return
  try {
    await navigator.clipboard.writeText(field.value)
    feedback.value = success
  } catch {
    field.focus()
    field.select()
    feedback.value = 'Utilise ⌘C pour copier le texte sélectionné.'
  }
}
</script>

<template>
  <div class="dialog-form browser-tabs-dialog">
    <div class="browser-tab-heading">
      <h3>Onglets autorisés</h3>
      <button type="button" class="icon-button subtle" aria-label="Actualiser les onglets" title="Actualiser les onglets" :disabled="loading" @click="$emit('refresh')">
        <RefreshCw :size="15" :class="{ spin: loading }" />
      </button>
    </div>
    <p v-if="error" class="danger-text" role="alert">{{ error }}</p>
    <p v-else-if="!tabs.length" class="browser-empty">Clique sur l'icône Patchbay dans chaque onglet de Brave, Chrome ou Edge que tu veux capter.</p>
    <p v-if="locked" class="browser-empty">Arrête le routage pour ajouter un onglet au graphe.</p>
    <p v-else-if="full" class="browser-empty">Le graphe contient déjà 64 blocs.</p>
    <div v-if="tabs.length" class="browser-tab-list">
      <button v-for="tab in tabs" :key="tab.id" type="button" class="picker-option"
        :disabled="locked || full || added.has(tab.id)" @click="$emit('add', tab.id)">
        <span class="picker-icon" data-variant="browserTab"><PanelTop :size="17" /></span>
        <span><strong>{{ tab.title }}</strong><small>{{ tab.browser }}</small></span>
        <Check v-if="added.has(tab.id)" :size="16" aria-label="Dans le graphe" /><Plus v-else :size="16" aria-hidden="true" />
      </button>
    </div>

    <button type="button" class="secondary-button" :aria-expanded="configuring" @click="toggleSetup">
      Configurer l'extension <ChevronDown :size="15" />
    </button>
    <div v-if="configuring" class="browser-setup">
      <p v-if="fetching" role="status">Préparation de l'association…</p>
      <template v-if="setupError">
        <p class="danger-text" role="alert">{{ setupError }}</p>
        <button type="button" class="secondary-button" @click="prepareSetup">Réessayer</button>
      </template>
      <ol v-if="ready">
        <li>Dans les extensions de Brave, Chrome ou Edge, active le mode développeur et choisis <strong>Charger l'extension non empaquetée</strong>. Sélectionne ce dossier avec ⌘⇧G.
          <div class="browser-copy-row">
            <input ref="directoryInput" aria-label="Dossier de l'extension" :value="extensionDirectory" readonly />
            <button type="button" class="icon-button" aria-label="Copier le dossier de l'extension" title="Copier le dossier" @click="copy(directoryInput, 'Dossier copié.')"><Copy :size="15" /></button>
          </div>
        </li>
        <li>Dans les options de l'extension Patchbay, colle ce code puis clique sur <strong>Associer</strong>.
          <div class="browser-copy-row">
            <input ref="codeInput" aria-label="Code d’association" :value="pairingCode" readonly />
            <button type="button" class="icon-button" aria-label="Copier le code d’association" title="Copier le code" @click="copy(codeInput, 'Code copié.')"><Copy :size="15" /></button>
          </div>
        </li>
        <li>Clique sur l'icône Patchbay dans chaque onglet voulu, puis ajoute-le ici. Refais l'association après un redémarrage de Patchbay.</li>
      </ol>
      <p v-if="feedback" role="status">{{ feedback }}</p>
    </div>
    <p class="browser-safari-note">Pour récupérer Safari entier, choisis Safari dans <strong>Ajouter une source → Applications</strong>.</p>
  </div>
</template>
