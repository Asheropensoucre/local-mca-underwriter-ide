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
    <div class="flex-1 overflow-auto bg-background/50 p-4 flex items-center justify-center" ref="viewerContainer">
      <!-- Loading State -->
      <div v-if="!imageSrc" class="text-center">
        <div class="animate-spin rounded-full h-12 w-12 border-4 border-primary border-t-transparent mx-auto mb-3"></div>
        <p class="text-sm text-gray-400">Loading Preview...</p>
      </div>
      
      <!-- Image -->
      <img
        v-else
        :src="imageSrc"
        alt="PDF Page 1 Preview"
        class="max-w-full max-h-full object-contain shadow-2xl"
        @load="handleImageLoad"
        @error="handleImageError"
      />
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'

const props = defineProps({
  source: {
    type: String, // Now expects a file path string, not ArrayBuffer
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
const totalPages = ref(1)

// Convert file path to Tauri URL
const imageSrcComputed = computed(() => {
  if (!props.source) return null
  console.log('[PdfViewer] Converting file path to URL:', props.source)
  return convertFileSrc(props.source)
})

// Watch for source changes
const setupImage = () => {
  if (props.source) {
    imageSrc.value = imageSrcComputed.value
    console.log('[PdfViewer] Image source set:', imageSrc.value)
  }
}

const handleImageLoad = (event) => {
  console.log('[PdfViewer] Image loaded successfully:', event.target.naturalWidth, 'x', event.target.naturalHeight)
  emit('load', {
    width: event.target.naturalWidth,
    height: event.target.naturalHeight
  })
}

const handleImageError = (error) => {
  console.error('[PdfViewer] Image load error:', error)
  emit('error', new Error('Failed to load PDF preview image'))
}

// Sync page count from parent
if (props.pageCount > 0) {
  totalPages.value = props.pageCount
}

onMounted(() => {
  console.log('[PdfViewer] Component mounted')
  setupImage()
})

// Watch for source changes
import { watch } from 'vue'
watch(() => props.source, (newSource) => {
  console.log('[PdfViewer] Source changed:', newSource)
  if (newSource) {
    setupImage()
  }
})

onUnmounted(() => {
  console.log('[PdfViewer] Component unmounted')
  imageSrc.value = null
})
</script>
