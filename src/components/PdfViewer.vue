<template>
  <div class="h-full flex flex-col">
    <!-- Toolbar -->
    <div class="flex items-center justify-between p-3 border-b border-border bg-surface">
      <div class="flex items-center gap-2">
        <button
          @click="prevPage"
          :disabled="currentPage <= 1"
          class="p-2 rounded hover:bg-border disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
          title="Previous Page"
        >
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
          </svg>
        </button>
        
        <span class="text-sm text-gray-400">
          Page <span class="text-gray-200">{{ currentPage }}</span> of <span class="text-gray-200">{{ totalPages }}</span>
        </span>
        
        <button
          @click="nextPage"
          :disabled="currentPage >= totalPages"
          class="p-2 rounded hover:bg-border disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
          title="Next Page"
        >
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
          </svg>
        </button>
      </div>
      
      <div class="flex items-center gap-2">
        <button
          @click="zoomOut"
          :disabled="zoom <= 0.5"
          class="px-3 py-1.5 text-sm rounded hover:bg-border disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
        >
          −
        </button>
        
        <span class="text-sm text-gray-400 w-16 text-center">{{ Math.round(zoom * 100) }}%</span>
        
        <button
          @click="zoomIn"
          :disabled="zoom >= 2"
          class="px-3 py-1.5 text-sm rounded hover:bg-border disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
        >
          +
        </button>
        
        <button
          @click="resetZoom"
          class="px-3 py-1.5 text-xs text-gray-500 hover:text-gray-300 transition-colors"
        >
          Fit
        </button>
      </div>
    </div>
    
    <!-- PDF Viewer - Single Page Mode -->
    <div class="flex-1 overflow-auto bg-background/50 p-4" ref="viewerContainer">
      <!-- Loading Overlay -->
      <div v-if="isLoading || !pdfSource" class="absolute inset-0 flex items-center justify-center bg-background/80 z-10">
        <div class="text-center">
          <div class="animate-spin rounded-full h-12 w-12 border-4 border-primary border-t-transparent mx-auto mb-3"></div>
          <p class="text-sm text-gray-400">Loading PDF...</p>
        </div>
      </div>
      
      <vue-pdf-embed
        ref="pdfRef"
        :source="pdfSource"
        :page="1"
        :rotation="rotation"
        class="mx-auto shadow-2xl"
        @rendering="handleRendered"
        @error="handleError"
      />
    </div>
  </div>
</template>

<script setup>
import { ref, computed, watch, onMounted } from 'vue'
import VuePdfEmbed from 'vue-pdf-embed'

const props = defineProps({
  source: {
    type: [String, ArrayBuffer],
    required: true
  },
  pageCount: {
    type: Number,
    default: 0
  }
})

const emit = defineEmits(['error'])

const pdfRef = ref(null)
const viewerContainer = ref(null)
const currentPage = ref(1)
const totalPages = ref(1)
const zoom = ref(1)
const rotation = ref(0)
const isLoading = ref(false)

const pdfSource = computed(() => props.source)

// Log when source changes
watch(() => props.source, (newSource) => {
  console.log('[PdfViewer] Source changed:', newSource ? 'loaded' : 'empty')
  if (newSource) {
    isLoading.value = true
  }
}, { immediate: true })

// Sync page count from parent if available
watch(() => props.pageCount, (newCount) => {
  if (newCount && newCount > 0) {
    totalPages.value = newCount
    console.log('[PdfViewer] Page count from parent:', newCount)
  }
}, { immediate: true })

const handleRendered = (pdfDocument) => {
  console.log('[PdfViewer] Rendered:', pdfDocument)
  isLoading.value = false
  
  if (pdfDocument && pdfDocument.numPages) {
    totalPages.value = pdfDocument.numPages
    console.log('[PdfViewer] Total pages from PDF:', pdfDocument.numPages)
  }
}

const handleError = (error) => {
  console.error('[PdfViewer] Render error:', error)
  isLoading.value = false
  emit('error', error)
}

const prevPage = () => {
  if (currentPage.value > 1) {
    currentPage.value--
    console.log('[PdfViewer] Previous page:', currentPage.value)
  }
}

const nextPage = () => {
  if (currentPage.value < totalPages.value) {
    currentPage.value++
    console.log('[PdfViewer] Next page:', currentPage.value)
  }
}

const zoomIn = () => {
  zoom.value = Math.min(zoom.value + 0.25, 2)
  console.log('[PdfViewer] Zoom in:', zoom.value)
}

const zoomOut = () => {
  zoom.value = Math.max(zoom.value - 0.25, 0.5)
  console.log('[PdfViewer] Zoom out:', zoom.value)
}

const resetZoom = () => {
  zoom.value = 1
  console.log('[PdfViewer] Zoom reset:', zoom.value)
}

onMounted(() => {
  console.log('[PdfViewer] Component mounted')
})
</script>
