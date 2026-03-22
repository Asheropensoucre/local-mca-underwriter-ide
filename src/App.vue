<template>
  <div class="min-h-screen bg-background text-gray-300 flex items-center justify-center p-8">
    <!-- Debug State -->
    <div class="fixed top-2 left-2 text-xs font-mono bg-surface p-2 border border-border z-50">
      State: {{ appState }} | Tab: {{ activeTab }}
    </div>

    <!-- IDLE State - Drop Zone -->
    <div
      v-show="appState === 'IDLE'"
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

    <!-- Main Dashboard Layout - ALWAYS MOUNTED once file is loaded -->
    <div v-show="appState !== 'IDLE'" class="w-full max-w-7xl h-[80vh] flex gap-4">
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
        <div v-if="pdfSource" class="flex-1 overflow-hidden">
          <PdfViewer :source="pdfSource" :page-count="pdfPageCount" />
        </div>
        <div v-else class="flex-1 flex items-center justify-center bg-background/50 m-4 rounded-lg border border-border border-dashed">
          <p class="text-gray-500 text-sm">PDF loaded (viewer pending)</p>
        </div>
      </div>

      <!-- Right Sidebar (40%) - AI Chat Assistant -->
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
          <div v-if="activeTab === 'underwrite'" class="space-y-4 h-full flex flex-col">
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
                <button
                  v-if="ollamaConnected"
                  @click="testConnection"
                  class="text-xs text-primary hover:text-blue-400 ml-2"
                >
                  Test
                </button>
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
                Start Ollama and run: <code class="bg-surface px-2 py-1 rounded">ollama pull llama3.2-vision</code>
              </p>
            </div>

            <!-- Underwrite Button -->
            <button
              class="w-full bg-primary hover:bg-blue-600 text-white font-medium py-3 rounded-lg transition-all hover:shadow-lg hover:shadow-primary/25 active:scale-[0.98]"
              @click="handleUnderwrite"
            >
              Underwrite File
            </button>

            <!-- ANALYZING State - Loading Spinner in Chat Area -->
            <div v-if="appState === 'ANALYZING'" class="flex-1 flex flex-col items-center justify-center bg-background border border-border rounded-lg p-8">
              <div class="relative mb-4">
                <div class="animate-spin rounded-full h-12 w-12 border-4 border-primary border-t-transparent"></div>
              </div>
              <p class="text-gray-300 font-medium mb-2">{{ loadingMessage }}</p>
              <p class="text-xs text-gray-500 text-center">This may take 5-10 minutes for AI analysis (hardware dependent)</p>
            </div>

            <!-- ERROR State - Error Display in Chat Area -->
            <div v-else-if="appState === 'ERROR'" class="flex-1 flex flex-col items-center justify-center bg-background border border-border rounded-lg p-8">
              <div class="text-red-400 text-4xl mb-4">❌</div>
              <p class="text-lg font-medium text-red-300 mb-2">Analysis Failed</p>
              <p class="text-sm text-gray-400 text-center mb-4">{{ errorMessage }}</p>
              <button
                @click="handleUnderwrite"
                class="px-4 py-2 bg-primary hover:bg-blue-600 rounded-lg text-white text-sm font-medium transition-colors"
              >
                Try Again
              </button>
            </div>

            <!-- COMPLETE State - Dashboard Cards + Analysis -->
            <div v-else-if="appState === 'COMPLETE'" class="flex-1 flex flex-col space-y-4">
              <!-- Business Info Header -->
              <div v-if="parsedData?.business" class="bg-background border border-border rounded-lg p-4">
                <h4 class="text-sm font-medium text-gray-300 mb-2">Business Information</h4>
                <div class="grid grid-cols-2 gap-4 text-sm">
                  <div>
                    <span class="text-gray-500">Business:</span>
                    <span class="text-gray-200 ml-2">{{ parsedData.business.name || 'N/A' }}</span>
                  </div>
                  <div>
                    <span class="text-gray-500">Account:</span>
                    <span class="text-gray-200 ml-2">{{ parsedData.business.account || 'N/A' }}</span>
                  </div>
                  <div class="col-span-2">
                    <span class="text-gray-500">Period:</span>
                    <span class="text-gray-200 ml-2">{{ parsedData.business.period || 'N/A' }}</span>
                  </div>
                </div>
              </div>

              <!-- Dashboard Metrics Cards -->
              <div class="grid grid-cols-2 gap-3">
                <div class="bg-background border border-border rounded-lg p-3">
                  <p class="text-xs text-gray-500 uppercase">Avg Daily Balance</p>
                  <p class="text-lg font-semibold text-gray-200">{{ formatCurrency(parsedData?.metrics?.avg_daily_balance) }}</p>
                </div>
                <div class="bg-background border border-border rounded-lg p-3">
                  <p class="text-xs text-gray-500 uppercase">Total Deposits</p>
                  <p class="text-lg font-semibold text-green-400">{{ formatCurrency(parsedData?.metrics?.total_deposits) }}</p>
                </div>
                <div class="bg-background border border-border rounded-lg p-3">
                  <p class="text-xs text-gray-500 uppercase">Total Withdrawals</p>
                  <p class="text-lg font-semibold text-red-400">{{ formatCurrency(parsedData?.metrics?.total_withdrawals) }}</p>
                </div>
                <div class="bg-background border border-border rounded-lg p-3">
                  <p class="text-xs text-gray-500 uppercase">NSF Count</p>
                  <p class="text-lg font-semibold" :class="parsedData?.metrics?.nsf_count > 0 ? 'text-red-400' : 'text-green-400'">{{ parsedData?.metrics?.nsf_count ?? 'N/A' }}</p>
                </div>
              </div>

              <!-- Risk & Recommendation Row -->
              <div class="grid grid-cols-2 gap-3">
                <div class="bg-background border border-border rounded-lg p-3">
                  <p class="text-xs text-gray-500 uppercase">Risk Score</p>
                  <p class="text-lg font-semibold" :class="getRiskScoreColor(parsedData?.risk?.score)">{{ parsedData?.risk?.score ?? 'N/A' }}/10</p>
                </div>
                <div class="bg-background border border-border rounded-lg p-3 flex items-center justify-between">
                  <div>
                    <p class="text-xs text-gray-500 uppercase">Recommendation</p>
                    <p class="text-lg font-semibold text-gray-200">{{ parsedData?.recommendation || 'N/A' }}</p>
                  </div>
                  <div class="w-3 h-3 rounded-full" :class="getRecommendationColor(parsedData?.recommendation)"></div>
                </div>
              </div>

              <!-- Analysis Notes -->
              <div class="bg-background border border-border rounded-lg p-4 flex-1">
                <div class="flex items-center justify-between mb-2">
                  <h4 class="text-sm font-medium text-gray-300">Analysis Notes</h4>
                  <button
                    @click="copyResults"
                    class="text-xs px-3 py-1 bg-surface hover:bg-border border border-border rounded transition-colors"
                  >
                    {{ copyButtonText }}
                  </button>
                </div>
                <p class="text-sm text-gray-400 whitespace-pre-wrap">{{ parsedData?.notes || rawResponse }}</p>
              </div>

              <!-- Follow-up Chat Section -->
              <div class="border-t border-border pt-4">
                <h4 class="text-sm font-medium text-gray-300 mb-3">Follow-up Questions</h4>
                
                <!-- Chat Messages -->
                <div class="space-y-3 mb-3 max-h-64 overflow-auto">
                  <div v-if="chatMessages.length === 0" class="text-center py-6 text-gray-500 text-sm">
                    Ask follow-up questions about this analysis
                  </div>
                  <div
                    v-for="(msg, idx) in chatMessages"
                    :key="idx"
                    class="rounded-lg p-3 text-sm"
                    :class="msg.role === 'user' ? 'bg-primary/20 border border-primary/30' : 'bg-surface border border-border'"
                  >
                    <div class="flex items-center gap-2 mb-1">
                      <span class="text-xs font-medium" :class="msg.role === 'user' ? 'text-primary' : 'text-gray-400'">
                        {{ msg.role === 'user' ? 'You' : 'Assistant' }}
                      </span>
                    </div>
                    <p class="text-gray-300 whitespace-pre-wrap">{{ msg.content }}</p>
                  </div>
                  <div v-if="isChatLoading" class="bg-surface border border-border rounded-lg p-3">
                    <div class="flex items-center gap-2">
                      <div class="animate-spin rounded-full h-4 w-4 border-2 border-primary border-t-transparent"></div>
                      <span class="text-xs text-gray-500">Thinking...</span>
                    </div>
                  </div>
                </div>

                <!-- Chat Input -->
                <div class="flex gap-2">
                  <input
                    v-model="chatInput"
                    @keyup.enter="sendChatMessage"
                    type="text"
                    placeholder="Ask a follow-up question..."
                    class="flex-1 bg-background border border-border rounded-lg px-3 py-2 text-sm text-gray-300 placeholder-gray-600 focus:outline-none focus:border-primary focus:ring-1 focus:ring-primary"
                    :disabled="isChatLoading || !ollamaConnected"
                  />
                  <button
                    @click="sendChatMessage"
                    :disabled="isChatLoading || !chatInput.trim() || !ollamaConnected"
                    class="px-4 py-2 bg-primary hover:bg-blue-600 disabled:bg-gray-600 disabled:cursor-not-allowed rounded-lg text-white text-sm font-medium transition-colors"
                  >
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
                    </svg>
                  </button>
                </div>
                <p v-if="!ollamaConnected" class="text-xs text-red-400 mt-2">Connect to Ollama to send questions</p>
              </div>
            </div>

            <!-- READY State - Waiting for Analysis -->
            <div v-else class="flex-1 flex flex-col items-center justify-center bg-background border border-border rounded-lg p-8">
              <div class="text-center space-y-3">
                <svg class="w-12 h-12 text-gray-600 mx-auto" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z" />
                </svg>
                <p class="text-gray-500 text-sm">Click "Underwrite File" to analyze</p>
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

// ═══════════════════════════════════════════════════════════════════════════
// STATE MACHINE - Explicit app states (no boolean spaghetti)
// ═══════════════════════════════════════════════════════════════════════════
const appState = ref('IDLE') // 'IDLE' | 'LOADING_PDF' | 'READY' | 'ANALYZING' | 'COMPLETE' | 'ERROR'
const errorMessage = ref('')

// File data
const filePath = ref('')
const pdfPageCount = ref(0)
const pdfSource = ref(null)

// Analysis data
const analysisResult = ref(null) // Parsed JSON from AI
const rawResponse = ref('') // Raw text from AI
const parsedData = ref(null) // Structured parsed data for dashboard

// Chat data
const chatMessages = ref([]) // Array of { role: 'user' | 'assistant', content: string }
const chatInput = ref('')
const isChatLoading = ref(false)

// UI state
const activeTab = ref('underwrite')
const loadingProgress = ref(0)
const loadingMessage = ref('')

// Ollama state
const ollamaConnected = ref(false)
const ollamaModels = ref([])
const isCheckingConnection = ref(true)
const selectedModel = ref('llama-3-vision')

// Drag-drop state
const isDragging = ref(false)
const dropError = ref('')

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
      filePath.value = selected

      console.log('[State] Loading PDF:', selected)

      // Read PDF file as ArrayBuffer for viewer
      try {
        const { readFile } = await import('@tauri-apps/plugin-fs')
        const pdfData = await readFile(selected)
        pdfSource.value = new Uint8Array(pdfData)
      } catch (err) {
        console.error('Error reading PDF:', err)
      }

      // Get PDF page count
      try {
        const result = await invoke('convert_pdf_to_images', {
          pdfPath: selected,
          dpi: 72
        })
        pdfPageCount.value = result.pages.length
        loadingProgress.value = 50
        loadingMessage.value = `Converting ${result.pages.length} page(s)...`
      } catch (err) {
        console.error('Error getting page count:', err)
        pdfPageCount.value = 1
      }

      // Transition: IDLE → LOADING_PDF → READY
      appState.value = 'LOADING_PDF'
      loadingProgress.value = 0
      loadingMessage.value = 'Loading PDF...'
      
      setTimeout(() => {
        loadingProgress.value = 100
        appState.value = 'READY'
        console.log('[State] PDF loaded, ready for analysis')
      }, 500)
    }
  } catch (error) {
    console.error('Error opening file:', error)
    dropError.value = 'Failed to open file picker'
    appState.value = 'IDLE'
  }
}

const handleDrop = async (event) => {
  isDragging.value = false
  dropError.value = ''

  const files = event.dataTransfer?.files
  if (files && files.length > 0) {
    const file = files[0]
    if (file.type === 'application/pdf' || file.name.endsWith('.pdf')) {
      filePath.value = file.name
      console.log('[Rust Backend] Dropped file:', file.name)
      appState.value = 'READY'
    } else {
      dropError.value = 'Please drop a PDF file only'
    }
  }
}

const handleUnderwrite = async () => {
  if (!ollamaConnected.value) {
    errorMessage.value = 'Ollama is not running. Please start Ollama and ensure vision models are installed.'
    appState.value = 'ERROR'
    return
  }

  // Transition: READY → ANALYZING
  appState.value = 'ANALYZING'
  loadingProgress.value = 0
  loadingMessage.value = 'Converting PDF to grayscale JPEG...'

  console.log('[State] Starting analysis...')

  try {
    loadingMessage.value = `Sending to ${selectedModel.value}...`

    const result = await invoke('send_pdf_to_ollama', {
      model: selectedModel.value,
      prompt: masterPrompt.value,
      pdfPath: filePath.value,
      temperature: modelConfig.value.temperature,
      maxTokens: modelConfig.value.maxTokens
    })

    console.log('[Underwrite] RAW RESPONSE FROM OLLAMA:', result)
    console.log('[Underwrite] Response length:', result?.length || 'N/A')

    loadingProgress.value = 100

    // Store results
    rawResponse.value = result
    analysisResult.value = result
    
    // Parse JSON from response
    parsedData.value = parseJsonFromResponse(result)
    console.log('[Underwrite] Parsed dashboard data:', parsedData.value)

    // Transition: ANALYZING → COMPLETE
    appState.value = 'COMPLETE'
    activeTab.value = 'underwrite'

    console.log('[State] Analysis complete')
    console.log('[State] UI State:', { appState: appState.value, activeTab: activeTab.value })

  } catch (error) {
    console.error('Underwrite error:', error)

    // Transition: ANALYZING → ERROR
    appState.value = 'ERROR'
    errorMessage.value = error
  }
}

const testConnection = async () => {
  try {
    const result = await invoke('test_ollama_model', { model: selectedModel.value })
    console.log('Test successful:', result)
  } catch (error) {
    console.error('Test failed:', error)
    errorMessage.value = `Connection test failed: ${error}`
    appState.value = 'ERROR'
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// JSON PARSING - Extract structured data from AI response
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Parse JSON from AI response text
 * Handles cases where JSON is wrapped in markdown code blocks or mixed with prose
 */
const parseJsonFromResponse = (text) => {
  if (!text) return null
  
  try {
    // Try parsing the entire text first
    return JSON.parse(text)
  } catch {
    // Look for JSON in markdown code blocks
    const codeBlockMatch = text.match(/```(?:json)?\s*([\s\S]*?)```/)
    if (codeBlockMatch) {
      try {
        return JSON.parse(codeBlockMatch[1].trim())
      } catch {
        // Fall through to regex extraction
      }
    }
    
    // Look for JSON object pattern
    const jsonMatch = text.match(/\{[\s\S]*\}/)
    if (jsonMatch) {
      try {
        return JSON.parse(jsonMatch[0])
      } catch {
        console.warn('Found JSON-like text but parsing failed')
      }
    }
  }
  
  return null
}

/**
 * Format currency value
 */
const formatCurrency = (value) => {
  if (value === null || value === undefined) return 'N/A'
  const num = typeof value === 'string' ? parseFloat(value.replace(/[^0-9.-]/g, '')) : value
  if (isNaN(num)) return 'N/A'
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: 0,
    maximumFractionDigits: 0
  }).format(num)
}

/**
 * Get recommendation badge color
 */
const getRecommendationColor = (recommendation) => {
  if (!recommendation) return 'bg-gray-500'
  const rec = recommendation.toUpperCase()
  if (rec === 'APPROVE') return 'bg-green-500'
  if (rec === 'DENY') return 'bg-red-500'
  if (rec === 'REVIEW') return 'bg-yellow-500'
  return 'bg-gray-500'
}

/**
 * Get risk score color
 */
const getRiskScoreColor = (score) => {
  if (score === null || score === undefined) return 'text-gray-400'
  if (score >= 8) return 'text-green-400'
  if (score >= 5) return 'text-yellow-400'
  return 'text-red-400'
}

/**
 * Copy results to clipboard
 */
const copyResults = async () => {
  const dataToCopy = parsedData.value || analysisResult.value
  if (!dataToCopy) return
  
  try {
    await navigator.clipboard.writeText(JSON.stringify(dataToCopy, null, 2))
    // Show brief success feedback
    const originalText = copyButtonText.value
    copyButtonText.value = 'Copied!'
    setTimeout(() => {
      copyButtonText.value = originalText
    }, 2000)
  } catch (err) {
    console.error('Failed to copy:', err)
  }
}

const copyButtonText = ref('Copy Results')

// ═══════════════════════════════════════════════════════════════════════════
// FOLLOW-UP CHAT - Send questions to Ollama with context
// ═══════════════════════════════════════════════════════════════════════════

const sendChatMessage = async () => {
  const question = chatInput.value.trim()
  if (!question || !ollamaConnected.value || isChatLoading.value) return

  // Add user message to chat
  chatMessages.value.push({ role: 'user', content: question })
  chatInput.value = ''
  isChatLoading.value = true

  try {
    // Build context-aware prompt
    const contextPrompt = `
Previous analysis of this bank statement:
${rawResponse.value}

User follow-up question: ${question}

Provide a concise, helpful answer based on the bank statement analysis above.`

    const response = await invoke('send_pdf_to_ollama', {
      model: selectedModel.value,
      prompt: contextPrompt,
      pdfPath: filePath.value,
      temperature: modelConfig.value.temperature,
      maxTokens: modelConfig.value.maxTokens
    })

    // Add assistant response to chat
    chatMessages.value.push({ role: 'assistant', content: response })
  } catch (error) {
    console.error('Chat error:', error)
    chatMessages.value.push({ 
      role: 'assistant', 
      content: `Error: ${error}\n\nPlease try again or check your Ollama connection.` 
    })
  } finally {
    isChatLoading.value = false
  }
}
</script>
