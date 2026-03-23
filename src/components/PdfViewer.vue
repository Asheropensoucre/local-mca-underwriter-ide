<template>
  <div class="h-full flex flex-col">
    <!-- Toolbar -->
    <div class="flex items-center justify-between p-3 border-b border-border bg-surface">
      <div class="flex items-center gap-2">
        <svg class="w-5 h-5 text-gray-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" />
        </svg>
        <span class="text-sm text-gray-400">
          Page <span class="text-gray-200">1</span> of <span class="text-gray-200">{{ totalPages }}</span>
        </span>
      </div>
      <div class="text-xs text-gray-500">
        Preview Mode
      </div>
    </div>

    <!-- Image Viewer -->
    <div class="flex-1 overflow-auto bg-background/50 p-4 flex items-center justify-center relative" ref="viewerContainer">
      <!-- Loading State -->
      <div v-if="!imageSrc" class="text-center">
        <div class="animate-spin rounded-full h-12 w-12 border-4 border-primary border-t-transparent mx-auto mb-3"></div>
        <p class="text-sm text-gray-400">Loading Preview...</p>
        <p class="text-xs text-gray-600 mt-2">Path: {{ source ? source.substring(0, 50) + '...' : 'none' }}</p>
      </div>

      <!-- Image -->
      <img
        v-if="imageSrc"
        :src="imageSrc"
        alt="PDF Page 1 Preview"
        class="max-w-full max-h-full object-contain shadow-2xl"
        @load="handleImageLoad"
        @error="handleImageError"
      />
      
      <!-- Error State -->
      <div v-if="imageError" class="text-center text-red-400">
        <p class="text-sm mb-2">Failed to load preview</p>
        <p class="text-xs text-gray-600 break-all max-w-md">{{ imageError }}</p>
        <p class="text-xs text-gray-500 mt-2">Source path: {{ source }}</p>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, watch, onMounted, onUnmounted } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'

const props = defineProps({
  source: {
    type: String, // Now expects a file path string
    required: true
  },
  pageCount: {
    type: Number,
    default: 0
  }
})

const emit = defineEmits(['error', 'load'])

const viewerContainer = ref(null)
const imageSrc = ref(null)
const imageError = ref(null)
const totalPages = ref(1)

// Setup image from file path
const setupImage = async () => {
  imageError.value = null
  
  if (!props.source) {
    console.warn('[PdfViewer] No source provided')
    imageError.value = 'No file path provided'
    return
  }
  
  console.log('[PdfViewer] Setting up image from path:', props.source)
  console.log('[PdfViewer] Path length:', props.source.length)
  console.log('[PdfViewer] Path starts with /:', props.source.startsWith('/'))
  
  try {
    // Convert file path to Tauri URL
    const url = convertFileSrc(props.source)
    imageSrc.value = url
    console.log('[PdfViewer] Image URL created:', url)
    console.log('[PdfViewer] URL protocol:', url.split(':')[0])
  } catch (err) {
    console.error('[PdfViewer] Failed to convert file path:', err)
    imageError.value = err.message || 'Failed to load image'
    emit('error', err)
  }
}

const handleImageLoad = (event) => {
  console.log('[PdfViewer] Image loaded successfully:', event.target.naturalWidth, 'x', event.target.naturalHeight)
  imageError.value = null
  emit('load', {
    width: event.target.naturalWidth,
    height: event.target.naturalHeight
  })
}

const handleImageError = (event) => {
  const errorMsg = `Failed to load: ${props.source}`
  console.error('[PdfViewer] Image load error:', errorMsg, event)
  imageError.value = errorMsg
  emit('error', new Error(errorMsg))
}

// Sync page count from parent
if (props.pageCount > 0) {
  totalPages.value = props.pageCount
}

onMounted(() => {
  console.log('[PdfViewer] Component mounted, source:', props.source)
  setupImage()
})

// Watch for source changes
watch(() => props.source, (newSource, oldSource) => {
  console.log('[PdfViewer] Source changed:', oldSource, '->', newSource)
  if (newSource && newSource !== oldSource) {
    imageSrc.value = null // Reset to show loading state
    setTimeout(() => setupImage(), 10) // Small delay to ensure reactivity
  }
}, { immediate: false })

onUnmounted(() => {
  console.log('[PdfViewer] Component unmounted')
  imageSrc.value = null
  imageError.value = null
})
</script>
