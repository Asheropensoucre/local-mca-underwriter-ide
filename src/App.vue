<template>
  <div class="min-h-screen bg-background text-gray-300 flex items-center justify-center p-8">
    <!-- Empty State - Drop Zone -->
    <div 
      v-if="!fileSelected"
      class="border-2 border-dashed rounded-xl p-16 text-center cursor-pointer max-w-2xl w-full transition-all duration-200"
      :class="[
        isDragging 
          ? 'border-primary bg-primary/10 scale-105 shadow-lg shadow-primary/20' 
          : 'border-border hover:border-gray-500 hover:bg-surface/30'
      ]"
      @click="openFileDialog"
      @dragover.prevent="isDragging = true"
      @dragleave="isDragging = false"
      @drop.prevent="handleDrop"
    >
      <div class="space-y-6">
        <div class="flex justify-center">
          <svg class="w-20 h-20 text-gray-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" />
          </svg>
        </div>
        <div>
          <p class="text-xl font-medium text-gray-200">Drop Bank Statements Here</p>
          <p class="text-sm text-gray-500 mt-2">or click to browse your files</p>
        </div>
        <div class="flex items-center justify-center gap-4">
          <span class="px-3 py-1 bg-surface border border-border rounded text-xs text-gray-500">PDF</span>
        </div>
        <p v-if="dropError" class="text-sm text-red-400">{{ dropError }}</p>
      </div>
    </div>

    <!-- Loading State -->
    <div v-else-if="isLoading" class="text-center space-y-4">
      <div class="inline-block animate-spin rounded-full h-12 w-12 border-4 border-primary border-t-transparent"></div>
      <p class="text-gray-400">{{ loadingMessage || 'Loading ' + fileName + '...' }}</p>
    </div>

    <!-- Active State - Processing UI -->
    <div v-else class="w-full max-w-7xl h-[80vh] flex gap-4">
      <!-- Left Pane - PDF Viewer (60%) -->
      <div class="w-[60%] bg-surface rounded-xl border border-border flex flex-col overflow-hidden">
        <!-- File Info Header -->
        <div class="flex items-center justify-between p-4 border-b border-border">
          <div class="flex items-center gap-3 flex-1 min-w-0">
            <svg class="w-5 h-5 text-gray-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
            </svg>
            <div class="flex-1 min-w-0">
              <p class="text-sm font-medium text-gray-300 truncate">{{ fileName }}</p>
              <p class="text-xs text-gray-500">{{ filePath }}</p>
            </div>
          </div>
          <div v-if="pdfPageCount > 0" class="flex items-center gap-2 px-3 py-1.5 bg-background border border-border rounded-lg">
            <svg class="w-4 h-4 text-gray-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
            </svg>
            <span class="text-sm text-gray-400">{{ pdfPageCount }} page{{ pdfPageCount > 1 ? 's' : '' }}</span>
          </div>
        </div>
        <!-- PDF Viewer Component -->
        <PdfViewer v-if="pdfSource" :source="pdfSource" :page-count="pdfPageCount" />
      </div>

      <!-- Right Sidebar (40%) -->
      <div class="w-[40%] bg-surface rounded-xl border border-border flex flex-col overflow-hidden">
        <!-- Tab Navigation -->
        <div class="flex border-b border-border">
          <button
            v-for="tab in tabs"
            :key="tab.id"
            @click="activeTab = tab.id"
            class="flex-1 px-4 py-3 text-sm font-medium transition-colors border-b-2"
            :class="activeTab === tab.id ? 'border-primary text-primary' : 'border-transparent text-gray-500 hover:text-gray-300'"
          >
            {{ tab.label }}
          </button>
        </div>

        <!-- Tab Content -->
        <div class="flex-1 p-5 overflow-auto">
          <!-- Underwrite Tab -->
          <div v-if="activeTab === 'underwrite'" class="space-y-4">
            <!-- Ollama Connection Status -->
            <div class="flex items-center justify-between">
              <span class="text-xs text-gray-500">Ollama Status:</span>
              <div class="flex items-center gap-2">
                <div v-if="isCheckingConnection" class="w-2 h-2 rounded-full bg-yellow-500 animate-pulse"></div>
                <div v-else-if="ollamaConnected" class="w-2 h-2 rounded-full bg-green-500"></div>
                <div v-else class="w-2 h-2 rounded-full bg-red-500"></div>
                <span class="text-xs" :class="ollamaConnected ? 'text-green-400' : 'text-red-400'">
                  {{ isCheckingConnection ? 'Checking...' : ollamaConnected ? 'Connected' : 'Disconnected' }}
                </span>
              </div>
            </div>

            <!-- Model Selector -->
            <div>
              <label class="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">Select Model</label>
              <select
                v-model="selectedModel"
                class="w-full bg-background border border-border rounded-lg px-4 py-3 text-sm text-gray-300 focus:outline-none focus:border-primary focus:ring-1 focus:ring-primary transition-colors"
              >
                <option v-for="model in ollamaModels" :key="model.name" :value="model.name">
                  {{ model.name }}
                </option>
                <option v-if="ollamaModels.length === 0" value="llama-3-vision" disabled>
                  No models found (is Ollama running?)
                </option>
              </select>
              <p v-if="ollamaModels.length === 0" class="text-xs text-gray-600 mt-2">
                Start Ollama and run: <code class="bg-surface px-2 py-1 rounded">ollama pull llava</code>
              </p>
            </div>

            <!-- Underwrite Button -->
            <button
              class="w-full bg-primary hover:bg-blue-600 text-white font-medium py-3 rounded-lg transition-all hover:shadow-lg hover:shadow-primary/25 active:scale-[0.98]"
              @click="handleUnderwrite"
            >
              Underwrite File
            </button>

            <!-- Terminal-style JSON Viewer -->
            <div class="flex-1 bg-background border border-border rounded-lg overflow-hidden flex flex-col min-h-[200px]">
              <div class="flex items-center justify-between px-3 py-2 bg-surface border-b border-border">
                <span class="text-xs font-mono text-gray-500">Output</span>
                <button @click="clearTerminal" class="text-xs text-gray-600 hover:text-gray-400">Clear</button>
              </div>
              <div class="flex-1 p-4 overflow-auto">
                <pre class="text-xs font-mono text-green-400 whitespace-pre-wrap">{{ terminalOutput }}</pre>
              </div>
            </div>
          </div>

          <!-- Prompt Tab -->
          <div v-if="activeTab === 'prompt'" class="space-y-4">
            <div>
              <label class="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">Master Underwriting Prompt</label>
              <textarea
                v-model="masterPrompt"
                class="w-full h-[400px] bg-background border border-border rounded-lg px-4 py-3 text-sm font-mono text-gray-300 focus:outline-none focus:border-primary focus:ring-1 focus:ring-primary resize-none"
                spellcheck="false"
              ></textarea>
            </div>
            <button
              class="w-full bg-surface hover:bg-border text-gray-300 font-medium py-2 rounded-lg transition-colors"
              @click="resetPrompt"
            >
              Reset to Default
            </button>
          </div>

          <!-- Settings Tab -->
          <div v-if="activeTab === 'settings'" class="space-y-5">
            <div>
              <label class="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">Temperature: {{ modelConfig.temperature }}</label>
              <input
                type="range"
                v-model="modelConfig.temperature"
                min="0"
                max="1"
                step="0.1"
                class="w-full accent-primary"
              />
              <p class="text-xs text-gray-600 mt-1">Lower = more deterministic, Higher = more creative</p>
            </div>

            <div>
              <label class="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">Max Tokens: {{ modelConfig.maxTokens }}</label>
              <input
                type="range"
                v-model="modelConfig.maxTokens"
                min="512"
                max="8192"
                step="512"
                class="w-full accent-primary"
              />
            </div>

            <div>
              <label class="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">Context Window: {{ modelConfig.contextWindow }}</label>
              <select
                v-model="modelConfig.contextWindow"
                class="w-full bg-background border border-border rounded-lg px-3 py-2 text-sm"
              >
                <option :value="4096">4K tokens</option>
                <option :value="8192">8K tokens</option>
                <option :value="16384">16K tokens</option>
                <option :value="32768">32K tokens</option>
              </select>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import PdfViewer from './components/PdfViewer.vue'

const fileSelected = ref(false)
const isLoading = ref(false)
const loadingMessage = ref('')
const filePath = ref('')
const pdfPageCount = ref(0)
const pdfSource = ref(null)
const selectedModel = ref('llama-3-vision')
const isDragging = ref(false)
const dropError = ref('')
const terminalOutput = ref('// Extracted data will appear here...\n')
const activeTab = ref('underwrite')

// Ollama state
const ollamaConnected = ref(false)
const ollamaModels = ref([])
const isCheckingConnection = ref(true)

// Model configuration
const modelConfig = ref({
  temperature: 0.3,
  maxTokens: 4096,
  contextWindow: 8192
})

// Master Underwriting Prompt
const masterPrompt = ref(`# MCA Underwriting Analysis Prompt

## Role
You are an expert MCA (Merchant Cash Advance) underwriter with 15+ years of experience analyzing small business financials.

## Task
Analyze the uploaded bank statement PDF and extract the following:

### 1. Business Information
- Business name
- Account number (last 4 digits only)
- Statement period

### 2. Financial Metrics
- Average daily balance
- Total monthly deposits
- Total monthly withdrawals
- Number of NSF/returned transactions
- Largest single deposit
- Largest single withdrawal

### 3. Risk Indicators
- Overdraft frequency
- Irregular large transactions
- Gambling/transaction patterns
- Cash deposit ratio

### 4. Recommendation
- Approve / Deny / Review
- Suggested funding amount
- Risk score (1-10)

## Output Format
Return results as valid JSON matching this schema:
{
  "business": { "name": "", "account": "", "period": "" },
  "metrics": { "avg_daily_balance": 0, "deposits": 0, "withdrawals": 0 },
  "risk": { "nsf_count": 0, "overdrafts": 0, "score": 0 },
  "recommendation": "APPROVE|DENY|REVIEW",
  "notes": ""
}`)

const fileName = computed(() => {
  if (!filePath.value) return ''
  return filePath.value.split('/').pop() || filePath.value.split('\\').pop() || ''
})

const tabs = [
  { id: 'underwrite', label: 'Underwrite' },
  { id: 'prompt', label: 'Prompt' },
  { id: 'settings', label: 'Settings' }
]

const defaultPrompt = `# MCA Underwriting Analysis Prompt

## Role
You are an expert MCA (Merchant Cash Advance) underwriter with 15+ years of experience analyzing small business financials.

## Task
Analyze the uploaded bank statement PDF and extract the following:

### 1. Business Information
- Business name
- Account number (last 4 digits only)
- Statement period

### 2. Financial Metrics
- Average daily balance
- Total monthly deposits
- Total monthly withdrawals
- Number of NSF/returned transactions
- Largest single deposit
- Largest single withdrawal

### 3. Risk Indicators
- Overdraft frequency
- Irregular large transactions
- Gambling/transaction patterns
- Cash deposit ratio

### 4. Recommendation
- Approve / Deny / Review
- Suggested funding amount
- Risk score (1-10)

## Output Format
Return results as valid JSON matching this schema:
{
  "business": { "name": "", "account": "", "period": "" },
  "metrics": { "avg_daily_balance": 0, "deposits": 0, "withdrawals": 0 },
  "risk": { "nsf_count": 0, "overdrafts": 0, "score": 0 },
  "recommendation": "APPROVE|DENY|REVIEW",
  "notes": ""
}`

const resetPrompt = () => {
  masterPrompt.value = defaultPrompt
}

// Check Ollama connection on mount
onMounted(async () => {
  await checkOllamaConnection()
})

const checkOllamaConnection = async () => {
  isCheckingConnection.value = true
  try {
    ollamaConnected.value = await invoke('check_ollama_connection')
    if (ollamaConnected.value) {
      ollamaModels.value = await invoke('get_ollama_models')
      // Update model selector with available models
      if (ollamaModels.value.length > 0) {
        const visionModels = ollamaModels.value.filter(m => 
          m.name.toLowerCase().includes('vision') || 
          m.name.toLowerCase().includes('llava') ||
          m.name.toLowerCase().includes('qwen')
        )
        if (visionModels.length > 0) {
          selectedModel.value = visionModels[0].name
        }
      }
    }
  } catch (error) {
    console.error('Ollama connection failed:', error)
    ollamaConnected.value = false
  }
  isCheckingConnection.value = false
}

const openFileDialog = async () => {
  dropError.value = ''
  try {
    const selected = await open({
      multiple: false,
      filters: [{
        name: 'PDF',
        extensions: ['pdf']
      }]
    })
    if (selected) {
      isLoading.value = true
      loadingMessage.value = 'Loading PDF...'
      filePath.value = selected
      
      console.log('[Rust Backend] Selected file:', selected)
      
      // Read PDF file as ArrayBuffer for viewer
      try {
        const { readFile } = await import('@tauri-apps/plugin-fs')
        const pdfData = await readFile(selected)
        pdfSource.value = new Uint8Array(pdfData)
      } catch (err) {
        console.error('Error reading PDF:', err)
      }
      
      // Get PDF page count and convert for vision model
      try {
        const result = await invoke('convert_pdf_to_images', {
          pdfPath: selected,
          dpi: 72 // Low DPI just for page count
        })
        pdfPageCount.value = result.pages.length
        loadingMessage.value = `Converting ${result.pages.length} page(s)...`
      } catch (err) {
        console.error('Error getting page count:', err)
        pdfPageCount.value = 1
      }
      
      // Simulate loading delay
      setTimeout(() => {
        isLoading.value = false
        fileSelected.value = true
      }, 800)
    }
  } catch (error) {
    console.error('Error opening file:', error)
    dropError.value = 'Failed to open file picker'
  }
}

const handleDrop = async (event) => {
  isDragging.value = false
  dropError.value = ''
  
  // Note: In Tauri, drag-drop from desktop requires special handling
  // For now, guide user to click instead
  const files = event.dataTransfer?.files
  if (files && files.length > 0) {
    const file = files[0]
    if (file.type === 'application/pdf' || file.name.endsWith('.pdf')) {
      isLoading.value = true
      filePath.value = file.name
      console.log('[Rust Backend] Dropped file:', file.name)
      
      setTimeout(() => {
        isLoading.value = false
        fileSelected.value = true
      }, 800)
    } else {
      dropError.value = 'Please drop a PDF file only'
    }
  }
}

const handleUnderwrite = async () => {
  if (!ollamaConnected.value) {
    terminalOutput.value = `// Error: Ollama is not running
// Please start Ollama and make sure vision models are installed
// 
// To install a vision model:
//   ollama pull llava
//   ollama pull llama3-vision
//
// Then restart the app
`
    return
  }

  const timestamp = new Date().toISOString()
  const pageCountText = pdfPageCount.value > 0 ? `(${pdfPageCount.value} page${pdfPageCount.value > 1 ? 's' : ''})` : ''
  terminalOutput.value = `// Converting PDF to images...
// Model: ${selectedModel.value}
// File: ${fileName.value} ${pageCountText}
// Temperature: ${modelConfig.value.temperature}
// Max Tokens: ${modelConfig.value.maxTokens}

`

  isLoading.value = true
  loadingMessage.value = pdfPageCount.value > 1 
    ? `Sending ${pdfPageCount.value} pages to ${selectedModel.value}...` 
    : `Sending to ${selectedModel.value}...`
  
  try {
    const result = await invoke('send_pdf_to_ollama', {
      model: selectedModel.value,
      prompt: masterPrompt.value,
      pdfPath: filePath.value,
      temperature: modelConfig.value.temperature,
      maxTokens: modelConfig.value.maxTokens
    })
    
    terminalOutput.value = `// Response from ${selectedModel.value}:

${result}
`
  } catch (error) {
    terminalOutput.value = `// Error: ${error}
`
  }
  
  isLoading.value = false
  loadingMessage.value = ''
}

const clearTerminal = () => {
  terminalOutput.value = '// Terminal cleared\n'
}
</script>
