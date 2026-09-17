<template>
  <div class="flex-1 flex items-start justify-center overflow-auto">
    <div class="w-full max-w-3xl p-10 text-sm text-gray-300">
      <h1 class="text-xl font-medium text-gray-100">Set up the local AI engine</h1>
      <p class="text-gray-500 mt-1">
        One download, then the app works fully offline. Statements never leave this machine.
      </p>

      <!-- Machine summary -->
      <div v-if="status" class="mt-6 grid grid-cols-4 gap-x-6 gap-y-1 font-mono text-xs">
        <span class="text-gray-500">System</span>
        <span class="col-span-3">{{ status.hardware.os }} {{ status.hardware.arch }}, {{ status.hardware.cpu_threads }} threads</span>
        <span class="text-gray-500">Memory</span>
        <span class="col-span-3">{{ status.hardware.total_ram_gb.toFixed(0) }} GB RAM</span>
        <span class="text-gray-500">Disk</span>
        <span class="col-span-3" :class="status.hardware.free_disk_gb < requiredGb + 1 ? 'text-red-400' : ''">
          {{ status.hardware.free_disk_gb.toFixed(1) }} GB free, {{ requiredGb.toFixed(1) }} GB needed
        </span>
        <span class="text-gray-500">Compute</span>
        <span class="col-span-3">{{ backendLabel }}</span>
        <span class="text-gray-500">Memory now</span>
        <span class="col-span-3" :class="status.memory.fits ? '' : 'text-red-400'">{{ status.memory.message }}</span>
        <span class="text-gray-500">Folder</span>
        <span class="col-span-3 truncate" :title="status.engine_dir">{{ status.engine_dir }}</span>
      </div>

      <p v-if="status && !status.platform_supported" class="mt-6 text-red-400">
        No llama.cpp build is available for this platform ({{ status.hardware.os }} {{ status.hardware.arch }}).
      </p>

      <!-- Components -->
      <div v-if="status" class="mt-6 border-t border-border">
        <div v-for="c in components" :key="c.id" class="border-b border-border py-3">
          <div class="flex items-center justify-between gap-4">
            <div class="min-w-0">
              <span class="text-gray-200">{{ c.title }}</span>
              <span class="text-gray-500 ml-2">{{ c.subtitle }}</span>
            </div>
            <div class="font-mono text-xs whitespace-nowrap">
              <span v-if="c.installed" class="text-green-400">installed</span>
              <span v-else-if="progressFor(c)" class="text-primary">
                {{ fmtBytes(progressFor(c).downloaded) }} / {{ fmtBytes(progressFor(c).total) }}
                <span v-if="progressFor(c).bytes_per_sec" class="text-gray-500"> {{ fmtBytes(progressFor(c).bytes_per_sec) }}/s</span>
              </span>
              <span v-else class="text-gray-500">{{ fmtBytes(c.size) }}</span>
            </div>
          </div>
          <div v-if="!c.installed && progressFor(c)" class="mt-2 h-1 bg-border">
            <div class="h-full bg-primary" :style="{ width: pct(progressFor(c)) + '%' }"></div>
          </div>
        </div>
      </div>

      <!-- Reasoning model choice -->
      <div v-if="status && !installing" class="mt-5">
        <label class="block text-xs text-gray-500 mb-1">Reasoning model</label>
        <select
          v-model="chosenUnderwriter"
          @change="saveConfig"
          class="bg-background border border-border px-3 py-2 text-sm text-gray-300 focus:outline-none focus:border-primary"
        >
          <option v-for="m in status.underwriters" :key="m.id" :value="m.id" :disabled="m.min_ram_gb > status.hardware.total_ram_gb + 0.5">
            {{ m.display_name }}, {{ fmtBytes(m.total_size) }}, needs {{ m.min_ram_gb }} GB RAM{{ m.installed ? ', installed' : '' }}{{ m.min_ram_gb > status.hardware.total_ram_gb + 0.5 ? ', too large for this machine' : '' }}
          </option>
        </select>
        <p class="text-xs text-gray-600 mt-1">
          The OCR model reads the pages. The reasoning model writes the report. The larger model is more accurate and slower.
          Recommended for this machine: {{ recommended?.display_name }}.
        </p>
      </div>

      <!-- Actions -->
      <div class="mt-6 flex items-center gap-3">
        <button
          v-if="status && !status.ready"
          @click="install"
          :disabled="installing || !status.platform_supported"
          class="px-5 py-2 bg-primary hover:bg-blue-600 text-white disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {{ installing ? 'Downloading' : (hasPartial ? 'Resume download' : 'Download ' + fmtBytes(missingBytes)) }}
        </button>
        <button
          v-if="status && status.ready"
          @click="start"
          :disabled="starting || !status.memory.fits"
          :title="status.memory.fits ? '' : status.memory.message"
          class="px-5 py-2 bg-primary hover:bg-blue-600 text-white disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {{ starting ? 'Starting engine' : (status.memory.fits ? 'Start engine' : 'Not enough free memory') }}
        </button>
        <button v-if="status && !status.memory.fits" @click="refresh" class="px-3 py-2 bg-surface border border-border text-gray-300 hover:border-gray-500">
          Check again
        </button>
      </div>

      <p v-if="status?.stopped_reason" class="mt-4 text-red-400 font-mono text-xs">{{ status.stopped_reason }}</p>
      <div v-if="error" class="mt-4 text-red-400 whitespace-pre-wrap font-mono text-xs">
        {{ error }}
        <div v-if="canFallbackToCpu" class="mt-2">
          <button @click="useCpu" class="px-3 py-1.5 bg-surface border border-border text-gray-300 hover:border-gray-500">
            Switch to the CPU build and retry
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
// First-launch setup for the built-in engine. Owns the download and start flow and
// emits `ready` when llama-server is up. Also reused from Settings to add models.
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

const emit = defineEmits(['ready'])

const status = ref(null)
const chosenUnderwriter = ref('')
const progress = ref({}) // asset_id -> latest DownloadProgress
const installing = ref(false)
const starting = ref(false)
const error = ref('')
const canFallbackToCpu = ref(false)
let unlisten = null

const refresh = async () => {
  status.value = await invoke('engine_status')
  chosenUnderwriter.value = status.value.config.underwriter_model
}

const chosenModel = computed(() => status.value?.underwriters.find(m => m.id === chosenUnderwriter.value))

// Rows shown in the component table: runtime, OCR model files, chosen reasoning model.
const components = computed(() => {
  if (!status.value) return []
  const rows = [{
    id: 'runtime', assetIds: ['runtime'],
    title: 'llama.cpp runtime', subtitle: backendLabel.value,
    size: status.value.runtime_size, installed: status.value.runtime_installed
  }]
  const ocr = status.value.ocr
  rows.push({
    id: ocr.id, assetIds: ocr.files.map(f => f.asset_id),
    title: ocr.display_name, subtitle: 'OCR, ' + ocr.hf_repo,
    size: ocr.total_size, installed: ocr.installed
  })
  const uw = chosenModel.value
  if (uw) rows.push({
    id: uw.id, assetIds: uw.files.map(f => f.asset_id),
    title: uw.display_name, subtitle: 'reasoning, ' + uw.hf_repo,
    size: uw.total_size, installed: uw.installed
  })
  return rows
})

// Largest reasoning model whose RAM requirement this machine meets.
const recommended = computed(() => {
  if (!status.value) return null
  const ram = status.value.hardware.total_ram_gb
  return [...status.value.underwriters].reverse().find(m => m.min_ram_gb <= ram) || status.value.underwriters[0]
})

const missingBytes = computed(() => components.value.filter(c => !c.installed).reduce((s, c) => s + c.size, 0))
const requiredGb = computed(() => missingBytes.value / 1e9)
const hasPartial = computed(() => Object.values(progress.value).some(p => !p.done && p.downloaded > 0))

const backendLabel = computed(() => {
  if (!status.value) return ''
  const os = status.value.hardware.os
  if (os === 'macos') return 'Metal (Apple GPU)'
  return status.value.config.backend === 'cpu' ? 'CPU only' : 'Vulkan GPU, falls back to CPU'
})

// Progress for a row is the sum over its files, so multi-file models show one bar.
const progressFor = (c) => {
  const parts = c.assetIds.map(id => progress.value[id]).filter(Boolean)
  if (parts.length === 0) return null
  return {
    downloaded: parts.reduce((s, p) => s + p.downloaded, 0),
    total: c.size,
    bytes_per_sec: parts.find(p => !p.done)?.bytes_per_sec || 0
  }
}
const pct = (p) => p.total ? Math.min(100, Math.round(p.downloaded / p.total * 100)) : 0
const fmtBytes = (n) => {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + ' GB'
  if (n >= 1e6) return (n / 1e6).toFixed(0) + ' MB'
  if (n >= 1e3) return (n / 1e3).toFixed(0) + ' KB'
  return n + ' B'
}

const saveConfig = async () => {
  const config = { ...status.value.config, underwriter_model: chosenUnderwriter.value }
  await invoke('engine_save_config', { config })
  await refresh()
}

const install = async () => {
  error.value = ''
  installing.value = true
  try {
    await invoke('engine_install')
    await refresh()
  } catch (e) {
    error.value = String(e)
  } finally {
    installing.value = false
  }
}

const start = async () => {
  error.value = ''
  canFallbackToCpu.value = false
  starting.value = true
  try {
    await invoke('engine_start')
    emit('ready')
  } catch (e) {
    error.value = String(e)
    canFallbackToCpu.value = status.value?.config.backend !== 'cpu' && status.value?.hardware.os !== 'macos'
  } finally {
    starting.value = false
  }
}

const useCpu = async () => {
  error.value = ''
  starting.value = true
  installing.value = true
  try {
    await invoke('engine_use_cpu_backend')
    await refresh()
    emit('ready')
  } catch (e) {
    error.value = String(e)
  } finally {
    starting.value = false
    installing.value = false
  }
}

onMounted(async () => {
  unlisten = await listen('engine-download-progress', (ev) => {
    progress.value = { ...progress.value, [ev.payload.asset_id]: ev.payload }
  })
  await refresh()
})
onUnmounted(() => { if (unlisten) unlisten() })
</script>
