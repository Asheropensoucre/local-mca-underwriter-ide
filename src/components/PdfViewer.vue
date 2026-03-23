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
    
    <!-- PDF Viewer -->
    <div class="flex-1 overflow-auto bg-background/50 p-4" ref="viewerContainer">
      <vue-pdf-embed
        ref="pdfRef"
        :source="pdfSource"
        :page="currentPage"
        :rotation="rotation"
        class="mx-auto shadow-2xl"
        @rendering="handleRendered"
        @error="handleError"
      />
      <div v-if="isLoading" class="flex items-center justify-center h-full">
        <div class="animate-spin rounded-full h-12 w-12 border-4 border-primary border-t-transparent"></div>
      </div>
    </div>
    
    <!-- Thumbnail Strip - Only show for PDFs with 2-20 pages -->
    <div v-if="totalPages > 1 && totalPages <= 20" class="border-t border-border bg-surface p-3">
      <div class="flex gap-2 overflow-x-auto pb-2">
        <button
          v-for="page in totalPages"
          :key="page"
          @click="goToPage(page)"
          class="flex-shrink-0 border-2 rounded overflow-hidden transition-colors"
          :class="currentPage === page ? 'border-primary' : 'border-border hover:border-gray-600'"
        >
          <div class="w-16 h-20 bg-background flex items-center justify-center text-xs text-gray-600">
            {{ page }}
          </div>
        </button>
      </div>
      <p v-if="totalPages > 20" class="text-xs text-gray-500 mt-2">
        {{ totalPages }} pages - use page navigation buttons
      </p>
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

const goToPage = (page) => {
  console.log('[PdfViewer] Going to page:', page)
  currentPage.value = page
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
