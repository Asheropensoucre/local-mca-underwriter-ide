<template>
  <div class="min-h-screen bg-background text-gray-300 flex items-center justify-center p-8">
    <!-- Debug State -->
    <div class="fixed top-2 left-2 text-xs font-mono bg-surface p-2 border border-border z-50">
      State: {{ appState }} | Tab: {{ activeTab }}
    </div>

    <!-- IDLE State - Drop Zone -->
    <div
      v-if="appState === 'IDLE'"
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

    <!-- LOADING_PDF State -->
    <div v-else-if="appState === 'LOADING_PDF'" class="text-center space-y-6">
      <div class="relative">
        <div class="inline-block animate-spin rounded-full h-16 w-16 border-4 border-primary border-t-transparent"></div>
        <div v-if="loadingProgress > 0" class="absolute inset-0 flex items-center justify-center">
          <span class="text-xs font-mono">{{ loadingProgress }}%</span>
        </div>
      </div>
      <div class="space-y-2">
        <p class="text-gray-300 font-medium">{{ loadingMessage }}</p>
        <div class="w-64 h-2 bg-surface rounded-full overflow-hidden mx-auto">
          <div
            class="h-full bg-primary transition-all duration-300"
            :style="{ width: loadingProgress + '%' }"
          ></div>
        </div>
        <p class="text-xs text-gray-500">Loading PDF...</p>
      </div>
    </div>

    <!-- ANALYZING State -->
    <div v-else-if="appState === 'ANALYZING'" class="text-center space-y-6">
      <div class="relative">
        <div class="inline-block animate-spin rounded-full h-16 w-16 border-4 border-primary border-t-transparent"></div>
        <div v-if="loadingProgress > 0" class="absolute inset-0 flex items-center justify-center">
          <span class="text-xs font-mono">{{ loadingProgress }}%</span>
        </div>
      </div>
      <div class="space-y-2">
        <p class="text-gray-300 font-medium">{{ loadingMessage }}</p>
        <div class="w-64 h-2 bg-surface rounded-full overflow-hidden mx-auto">
          <div
            class="h-full bg-primary transition-all duration-300"
            :style="{ width: loadingProgress + '%' }"
          ></div>
        </div>
        <p class="text-xs text-gray-500">This may take 30-90 seconds for AI analysis</p>
      </div>
    </div>

    <!-- ERROR State -->
    <div v-else-if="appState === 'ERROR'" class="text-center space-y-6">
      <div class="text-red-400 text-6xl mb-4">❌</div>
      <p class="text-xl font-medium text-red-300">Analysis Failed</p>
      <p class="text-gray-400 max-w-md">{{ errorMessage }}</p>
      <button
        @click="appState = 'READY'"
        class="px-6 py-3 bg-primary hover:bg-blue-600 rounded-lg text-white font-medium transition-colors"
      >
        Try Again
      </button>
    </div>

    <!-- READY/COMPLETE State - Dashboard -->
    <div v-else class="w-full max-w-7xl h-[80vh] flex gap-4">
      <!-- DEBUG: Dashboard is visible -->
      <div class="fixed bottom-2 right-2 text-xs font-mono bg-primary text-white px-3 py-1 rounded z-50">
        DASHBOARD VISIBLE | State: {{ appState }}
      </div>
    
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
            <div class="flex-1 bg-background border border-border rounded-lg overflow-hidden flex flex-col min-h-[300px]">
              <div class="flex items-center justify-between px-3 py-2 bg-surface border-b border-border">
                <span class="text-xs font-mono text-gray-500">Output</span>
                <div class="flex items-center gap-2">
                  <span v-if="terminalOutput.length > 0" class="text-xs text-green-400">● {{ terminalOutput.length }} chars</span>
                  <button @click="clearTerminal" class="text-xs text-gray-600 hover:text-gray-400">Clear</button>
                </div>
              </div>
              <div class="flex-1 p-4 overflow-auto">
                <pre class="text-xs font-mono text-green-400 whitespace-pre-wrap">{{ terminalOutput || '// Waiting for analysis...' }}</pre>
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
const terminalOutput = ref('// Extracted data will appear here...\n')

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
      // Transition: IDLE → LOADING_PDF
      appState.value = 'LOADING_PDF'
      loadingProgress.value = 0
      loadingMessage.value = 'Loading PDF...'
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

      // Transition: LOADING_PDF → READY
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
//   ollama pull llama3.2-vision
//   ollama pull llava
//
// Then restart the app
`
    return
  }

  // Transition: READY → ANALYZING
  appState.value = 'ANALYZING'
  loadingProgress.value = 0
  loadingMessage.value = 'Converting PDF to grayscale JPEG...'
  terminalOutput.value = '// Starting analysis...\n'

  console.log('[State] Starting analysis...')

  // Progress simulation
  const progressInterval = setInterval(() => {
    loadingProgress.value = Math.min(loadingProgress.value + 3, 85)
  }, 500)

  try {
    loadingMessage.value = `Sending to ${selectedModel.value}...`
    terminalOutput.value += `// Sending to Ollama (this may take 30-60 seconds)...\n\n`

    const result = await invoke('send_pdf_to_ollama', {
      model: selectedModel.value,
      prompt: masterPrompt.value,
      pdfPath: filePath.value,
      temperature: modelConfig.value.temperature,
      maxTokens: modelConfig.value.maxTokens
    })

    console.log('[Underwrite] RAW RESPONSE FROM OLLAMA:', result)
    console.log('[Underwrite] Response length:', result?.length || 'N/A')

    clearInterval(progressInterval)
    loadingProgress.value = 100

    // Store results
    rawResponse.value = result
    analysisResult.value = result // Will be parsed later

    // Build visible output
    terminalOutput.value = `
// ═══════════════════════════════════════════════════
// ✅ ANALYSIS COMPLETE
// ═══════════════════════════════════════════════════
// Model: ${selectedModel.value}
// Time: ${new Date().toLocaleTimeString()}
// Response Length: ${result?.length || 0} characters
// ═══════════════════════════════════════════════════

${result}
`

    // Transition: ANALYZING → COMPLETE
    appState.value = 'COMPLETE'
    activeTab.value = 'underwrite' // Ensure user sees results

    console.log('[State] Analysis complete')
    console.log('[State] UI State:', { appState: appState.value, activeTab: activeTab.value })

  } catch (error) {
    clearInterval(progressInterval)
    console.error('Underwrite error:', error)

    // Transition: ANALYZING → ERROR
    appState.value = 'ERROR'
    errorMessage.value = error

    terminalOutput.value = `// ❌ Error: ${error}

// Debug info:
// 1. Check terminal for detailed logs
// 2. Try a different model: llama3.2-vision or llava
// 3. Make sure Ollama is running: ollama serve
`
  }
}

const clearTerminal = () => {
  terminalOutput.value = '// Terminal cleared\n'
}

const testConnection = async () => {
  terminalOutput.value = `// Testing connection to Ollama...\n`
  try {
    const result = await invoke('test_ollama_model', { model: selectedModel.value })
    terminalOutput.value = `// ✅ Test successful!\n// Model responded: ${result}\n`
  } catch (error) {
    terminalOutput.value = `// ❌ Test failed: ${error}\n\n// Try:\n// 1. Check Ollama is running: ollama serve\n// 2. Verify model exists: ollama list\n// 3. Test with: ollama run ${selectedModel.value} "hello"\n`
  }
}
</script>
