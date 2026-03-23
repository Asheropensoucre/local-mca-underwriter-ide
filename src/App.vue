<template>
  <div class="h-screen w-screen flex flex-row overflow-hidden bg-background text-gray-300">
    <!-- IDLE State - Drop Zone -->
    <div
      v-show="appState === 'IDLE'"
      class="border-2 border-dashed rounded-xl p-16 text-center cursor-pointer max-w-2xl w-full transition-all duration-200 m-auto"
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
          <p class="text-sm text-gray-500 mt-2">or click to browse (select multiple months)</p>
        </div>
        <div class="flex items-center justify-center gap-4">
          <span class="px-3 py-1 bg-surface border border-border rounded text-xs text-gray-500">PDF</span>
          <span class="px-3 py-1 bg-surface border border-border rounded text-xs text-gray-500">Multiple Files</span>
        </div>
        <p v-if="dropError" class="text-sm text-red-400">{{ dropError }}</p>

        <!-- File Queue Preview -->
        <div v-if="fileQueue.length > 0" class="text-left">
          <p class="text-sm font-medium text-gray-400 mb-2">Selected Files ({{ fileQueue.length }}):</p>
          <div class="max-h-48 overflow-auto space-y-1">
            <div
              v-for="(file, idx) in fileQueue"
              :key="idx"
              class="flex items-center justify-between bg-surface border border-border rounded px-3 py-2 text-sm"
            >
              <span class="text-gray-300 truncate flex-1">{{ file.name }}</span>
              <button
                @click.stop="removeFile(idx)"
                class="ml-2 text-red-400 hover:text-red-300 text-xs"
              >
                ✕ Remove
              </button>
            </div>
          </div>
          <button
            @click.stop="clearFileQueue"
            class="mt-2 text-xs text-gray-500 hover:text-gray-400"
          >
            Clear All
          </button>
        </div>
      </div>
    </div>

    <!-- Main Dashboard Layout - ALWAYS MOUNTED once file is loaded -->
    <div v-show="appState !== 'IDLE'" class="flex-1 flex flex-row h-full overflow-hidden">
      <!-- Left Pane - PDF Viewer (60% → 30% on COMPLETE) -->
      <div
        class="bg-surface border-r border-border flex flex-col overflow-hidden transition-all duration-500 ease-in-out"
        :class="appState === 'COMPLETE' ? 'w-[30%]' : 'w-[60%]'"
      >
        <!-- File Info Header -->
        <div class="flex items-center justify-between p-4 border-b border-border">
          <div class="flex items-center gap-3 flex-1 min-w-0">
            <svg class="w-5 h-5 text-gray-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
            </svg>
            <div class="flex-1 min-w-0">
              <!-- Batch mode: show current file index -->
              <p v-if="fileQueue.length > 1" class="text-sm font-medium text-gray-300">
                File {{ currentFileIndex + 1 }} of {{ fileQueue.length }}: {{ fileName }}
              </p>
              <p v-else class="text-sm font-medium text-gray-300 truncate">{{ fileName }}</p>
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
        <!-- PDF Viewer Component - Only render when not processing to free RAM -->
        <div v-if="appState !== 'ANALYZING' && appState !== 'PROCESSING' && pdfSource" class="flex-1 overflow-hidden">
          <PdfViewer :preview-image="pdfSource" :page-count="pdfPageCount" />
        </div>
        <div v-else-if="!pdfSource" class="flex-1 flex items-center justify-center bg-background/50 m-4 rounded-lg border border-border border-dashed">
          <p class="text-gray-500 text-sm">PDF loaded (viewer pending)</p>
        </div>
        <div v-else class="flex-1 flex items-center justify-center bg-background/50 m-4 rounded-lg border border-border border-dashed">
          <p class="text-gray-500 text-sm">PDF viewer paused during analysis to free memory...</p>
        </div>
      </div>

      <!-- Right Sidebar - AI Chat Assistant -->
      <div
        class="w-[400px] h-full flex-shrink-0 border-l border-border bg-surface flex flex-col"
      >
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

            <!-- ANALYZING State - Loading Spinner with Multi-page Progress -->
            <div v-if="appState === 'ANALYZING'" class="flex-1 flex flex-col items-center justify-center bg-background border border-border rounded-lg p-8">
              <div class="relative mb-4">
                <div class="animate-spin rounded-full h-12 w-12 border-4 border-primary border-t-transparent"></div>
              </div>
              <p class="text-gray-300 font-medium mb-2">{{ loadingMessage }}</p>
              <!-- Multi-page Progress -->
              <div v-if="totalPages > 1" class="w-full max-w-xs mt-4">
                <div class="flex justify-between text-xs text-gray-500 mb-1">
                  <span>Page {{ currentPage }} of {{ totalPages }}</span>
                  <span>{{ Math.round((currentPage / totalPages) * 100) }}%</span>
                </div>
                <div class="w-full h-2 bg-slate-700 rounded-full overflow-hidden">
                  <div 
                    class="h-full bg-primary transition-all duration-500"
                    :style="{ width: (currentPage / totalPages * 100) + '%' }"
                  ></div>
                </div>
                <p class="text-xs text-gray-600 mt-2 text-center">
                  <span v-if="currentPage < totalPages">Analyzing page {{ currentPage }}...</span>
                  <span v-else>Aggregating results...</span>
                </p>
              </div>
              <p v-else class="text-xs text-gray-500 text-center">This may take 5-10 minutes for AI analysis (hardware dependent)</p>
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

            <!-- COMPLETE State - NEW MCA Dashboard Cards -->
            <div v-else-if="appState === 'COMPLETE'" class="flex-1 flex flex-col space-y-4">
              
              <!-- === SECTION 1: POSITIONS TABLE === -->
              <div v-if="parsedData?.positions && parsedData.positions.length > 0" class="bg-gradient-to-br from-slate-900 to-slate-800 border border-slate-700 rounded-xl p-4 shadow-lg">
                <div class="flex items-center gap-2 mb-3">
                  <svg class="w-5 h-5 text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                  </svg>
                  <h4 class="text-sm font-semibold text-gray-200 uppercase tracking-wide">Existing Positions</h4>
                </div>
                <div class="overflow-x-auto">
                  <table class="w-full text-sm">
                    <thead>
                      <tr class="border-b border-slate-700">
                        <th class="text-left py-2 px-3 text-xs font-medium text-gray-500 uppercase">Lender</th>
                        <th class="text-left py-2 px-3 text-xs font-medium text-gray-500 uppercase">Payment</th>
                        <th class="text-left py-2 px-3 text-xs font-medium text-gray-500 uppercase">Frequency</th>
                        <th class="text-right py-2 px-3 text-xs font-medium text-gray-500 uppercase">Funded</th>
                      </tr>
                    </thead>
                    <tbody>
                      <tr v-for="(pos, idx) in parsedData.positions" :key="idx" class="border-b border-slate-700/50 last:border-0 hover:bg-slate-700/30 transition-colors">
                        <td class="py-2 px-3 text-gray-200 font-medium">{{ pos.lender || 'Unknown' }}</td>
                        <td class="py-2 px-3 text-red-400 font-mono">{{ formatCurrency(pos.payment) }}</td>
                        <td class="py-2 px-3 text-gray-400 text-xs">{{ pos.frequency || 'N/A' }}</td>
                        <td class="py-2 px-3 text-right text-green-400 font-mono">{{ formatCurrency(pos.funded) }}</td>
                      </tr>
                    </tbody>
                  </table>
                </div>
              </div>

              <!-- === SECTION 2: BANK METRICS === -->
              <div class="grid grid-cols-2 gap-3">
                <!-- True Revenue Card -->
                <div class="bg-gradient-to-br from-emerald-900/40 to-emerald-800/20 border border-emerald-700/50 rounded-xl p-4">
                  <div class="flex items-center gap-2 mb-2">
                    <svg class="w-4 h-4 text-emerald-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8c-1.657 0-3 .895-3 2s1.343 2 3 2 3 .895 3 2-1.343 2-3 2m0-8c1.11 0 2.08.402 2.599 1M12 8V7m0 1v8m0 0v1m0-1c-1.11 0-2.08-.402-2.599-1M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                    <p class="text-xs text-emerald-300 uppercase font-semibold">True Revenue</p>
                  </div>
                  <p class="text-2xl font-bold text-emerald-400">{{ formatCurrency(parsedData?.bank_metrics?.true_revenue) }}</p>
                  <p class="text-xs text-emerald-600 mt-1">Excludes loan deposits</p>
                </div>

                <!-- Negative Days Card -->
                <div class="bg-gradient-to-br from-red-900/40 to-red-800/20 border border-red-700/50 rounded-xl p-4">
                  <div class="flex items-center gap-2 mb-2">
                    <svg class="w-4 h-4 text-red-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 17h8m0 0V9m0 8l-8-8-4 4-6-6" />
                    </svg>
                    <p class="text-xs text-red-300 uppercase font-semibold">Negative Days</p>
                  </div>
                  <p class="text-2xl font-bold" :class="(parsedData?.bank_metrics?.negative_days || 0) > 0 ? 'text-red-400' : 'text-emerald-400'">
                    {{ parsedData?.bank_metrics?.negative_days ?? 0 }}
                  </p>
                  <p class="text-xs text-red-600 mt-1">Days balance &lt; $0</p>
                </div>

                <!-- Avg Daily Balance Card -->
                <div class="bg-gradient-to-br from-blue-900/40 to-blue-800/20 border border-blue-700/50 rounded-xl p-4">
                  <div class="flex items-center gap-2 mb-2">
                    <svg class="w-4 h-4 text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
                    </svg>
                    <p class="text-xs text-blue-300 uppercase font-semibold">Avg Daily Balance</p>
                  </div>
                  <p class="text-xl font-bold text-blue-400">{{ formatCurrency(parsedData?.bank_metrics?.avg_daily_balance) }}</p>
                </div>

                <!-- NSF Count Card -->
                <div class="bg-gradient-to-br from-amber-900/40 to-amber-800/20 border border-amber-700/50 rounded-xl p-4">
                  <div class="flex items-center gap-2 mb-2">
                    <svg class="w-4 h-4 text-amber-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                    </svg>
                    <p class="text-xs text-amber-300 uppercase font-semibold">NSF Count</p>
                  </div>
                  <p class="text-xl font-bold" :class="(parsedData?.bank_metrics?.nsf_count || 0) > 0 ? 'text-amber-400' : 'text-emerald-400'">
                    {{ parsedData?.bank_metrics?.nsf_count ?? 0 }}
                  </p>
                </div>
              </div>

              <!-- === SECTION 3: DEBT & LEVERAGE === -->
              <div class="bg-gradient-to-br from-violet-900/40 to-violet-800/20 border border-violet-700/50 rounded-xl p-4 shadow-lg">
                <div class="flex items-center gap-2 mb-3">
                  <svg class="w-5 h-5 text-violet-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 6l3 1m0 0l-3 9a5.002 5.002 0 006.001 0M6 7l3 9M6 7l6-2m6 2l3-1m-3 1l-3 9a5.002 5.002 0 006.001 0M18 7l3 9m-3-9l-6-2m0-2v2m0 16V5m0 16H9m3 0h3" />
                  </svg>
                  <h4 class="text-sm font-semibold text-violet-200 uppercase tracking-wide">Debt & Leverage</h4>
                </div>
                <div class="grid grid-cols-3 gap-4">
                  <div>
                    <p class="text-xs text-violet-400 mb-1">Total Debt Service</p>
                    <p class="text-lg font-bold text-red-400 font-mono">{{ formatCurrency(parsedData?.debt_leverage?.total_debt_service) }}</p>
                    <p class="text-xs text-violet-600">Daily/Weekly</p>
                  </div>
                  <div>
                    <p class="text-xs text-violet-400 mb-1">Safe New Payment</p>
                    <p class="text-lg font-bold text-emerald-400 font-mono">{{ formatCurrency(parsedData?.debt_leverage?.safe_new_payment) }}</p>
                    <p class="text-xs text-violet-600">Max affordable</p>
                  </div>
                  <div>
                    <p class="text-xs text-violet-400 mb-1">Leverage Ratio</p>
                    <p class="text-lg font-bold text-violet-300 font-mono">{{ parsedData?.debt_leverage?.leverage_ratio || 'N/A' }}</p>
                    <p class="text-xs text-violet-600">Debt-to-revenue</p>
                  </div>
                </div>
              </div>

              <!-- === SECTION 4: BUSINESS INFO & RISK === -->
              <div class="grid grid-cols-2 gap-3">
                <!-- Business Info -->
                <div v-if="parsedData?.business" class="bg-background border border-border rounded-lg p-4">
                  <h4 class="text-xs font-medium text-gray-500 uppercase mb-3">Business Information</h4>
                  <div class="space-y-2 text-sm">
                    <div>
                      <span class="text-gray-600">Business:</span>
                      <span class="text-gray-200 ml-2">{{ parsedData.business.name || 'N/A' }}</span>
                    </div>
                    <div>
                      <span class="text-gray-600">Account:</span>
                      <span class="text-gray-200 ml-2">{{ parsedData.business.account || 'N/A' }}</span>
                    </div>
                    <div>
                      <span class="text-gray-600">Period:</span>
                      <span class="text-gray-200 ml-2">{{ parsedData.business.period || 'N/A' }}</span>
                    </div>
                  </div>
                </div>

                <!-- Risk Score -->
                <div class="bg-background border border-border rounded-lg p-4 flex flex-col justify-center">
                  <p class="text-xs text-gray-500 uppercase mb-2">Risk Score</p>
                  <p class="text-3xl font-bold" :class="getRiskScoreColor(parsedData?.risk?.score)">{{ parsedData?.risk?.score ?? 'N/A' }}/10</p>
                  <div class="flex items-center gap-2 mt-2">
                    <div class="flex-1 h-2 bg-slate-700 rounded-full overflow-hidden">
                      <div 
                        class="h-full rounded-full transition-all duration-500"
                        :class="getRiskScoreBarColor(parsedData?.risk?.score)"
                        :style="{ width: ((parsedData?.risk?.score || 0) / 10 * 100) + '%' }"
                      ></div>
                    </div>
                  </div>
                </div>
              </div>

              <!-- === SECTION 5: RECOMMENDATION & ACTIONS === -->
              <div class="bg-background border border-border rounded-lg p-4">
                <div class="flex items-center justify-between mb-3">
                  <div class="flex items-center gap-3">
                    <div class="w-4 h-4 rounded-full" :class="getRecommendationColor(parsedData?.recommendation)"></div>
                    <div>
                      <p class="text-xs text-gray-500 uppercase">Recommendation</p>
                      <p class="text-lg font-semibold text-gray-200">{{ parsedData?.recommendation || 'N/A' }}</p>
                    </div>
                  </div>
                  <div class="flex gap-2">
                    <button
                      @click="printReport"
                      :disabled="!rawResponse"
                      class="text-xs px-3 py-1.5 bg-surface hover:bg-border border border-border rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                      title="Print Report"
                    >
                      🖨️ Print
                    </button>
                    <button
                      @click="exportToJSON"
                      :disabled="isExporting || !parsedData"
                      class="text-xs px-3 py-1.5 bg-surface hover:bg-border border border-border rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                      title="Export as JSON"
                    >
                      📄 JSON
                    </button>
                    <button
                      @click="exportToCSV"
                      :disabled="isExporting || !parsedData"
                      class="text-xs px-3 py-1.5 bg-surface hover:bg-border border border-border rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                      title="Export as CSV"
                    >
                      📊 CSV
                    </button>
                    <button
                      @click="copyResults"
                      :disabled="isExporting || !rawResponse"
                      class="text-xs px-3 py-1.5 bg-surface hover:bg-border border border-border rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                      {{ copyButtonText }}
                    </button>
                  </div>
                </div>
                <div>
                  <h4 class="text-xs font-medium text-gray-500 uppercase mb-2">Analysis Notes</h4>
                  <p class="text-sm text-gray-400 whitespace-pre-wrap max-h-32 overflow-auto">{{ parsedData?.notes || rawResponse }}</p>
                </div>
              </div>

              <!-- === SECTION 6: FOLLOW-UP CHAT === -->
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
            <!-- Template Management -->
            <div class="bg-surface border border-border rounded-lg p-4">
              <h4 class="text-sm font-medium text-gray-300 mb-3">Saved Templates</h4>
              <div class="flex items-center gap-3 mb-3">
                <select
                  v-model="selectedTemplateName"
                  @change="loadSelectedTemplate"
                  class="flex-1 bg-background border border-border rounded-lg px-4 py-2 text-sm text-gray-300 focus:outline-none focus:border-primary focus:ring-1 focus:ring-primary"
                >
                  <option value="">Select a template...</option>
                  <option v-for="template in savedTemplates" :key="template.name" :value="template.name">
                    {{ template.name }}
                  </option>
                </select>
                <button
                  @click="deleteSelectedTemplate"
                  :disabled="!selectedTemplateName"
                  class="px-4 py-2 bg-red-900/50 hover:bg-red-800/50 border border-red-700 rounded-lg text-sm text-red-300 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  🗑️ Delete
                </button>
              </div>
              <div class="flex items-center gap-3">
                <input
                  v-model="newTemplateName"
                  type="text"
                  placeholder="Template name..."
                  class="flex-1 bg-background border border-border rounded-lg px-4 py-2 text-sm text-gray-300 focus:outline-none focus:border-primary focus:ring-1 focus:ring-primary"
                />
                <button
                  @click="saveNewTemplate"
                  :disabled="!newTemplateName.trim()"
                  class="px-4 py-2 bg-primary hover:bg-blue-600 rounded-lg text-sm text-white font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  💾 Save Template
                </button>
              </div>
            </div>

            <!-- Custom Instructions Editor -->
            <div>
              <label class="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">Custom Underwriting Instructions</label>
              <p class="text-xs text-gray-600 mb-2">Add specific focus areas. Core MCA rules are automatically applied.</p>
              <textarea
                v-model="userCustomInstructions"
                class="w-full h-[400px] bg-background border border-border rounded-lg px-4 py-3 text-sm font-mono text-gray-300 focus:outline-none focus:border-primary focus:ring-1 focus:ring-primary resize-none"
                spellcheck="false"
              ></textarea>
            </div>
            <button
              class="w-full bg-surface hover:bg-border text-gray-300 font-medium py-2 rounded-lg transition-colors"
              @click="resetCustomInstructions"
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

          <!-- History Tab -->
          <div v-if="activeTab === 'history'" class="space-y-4 h-full flex flex-col">
            <!-- Header with Clear All Button -->
            <div class="flex items-center justify-between">
              <h4 class="text-sm font-medium text-gray-300">Analysis History</h4>
              <button
                @click="clearAllHistory"
                :disabled="analysisHistory.length === 0"
                class="px-3 py-1.5 bg-red-900/50 hover:bg-red-800/50 border border-red-700 rounded-lg text-xs text-red-300 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
              >
                🗑️ Clear All
              </button>
            </div>

            <!-- History List -->
            <div class="flex-1 overflow-auto space-y-2">
              <div v-if="analysisHistory.length === 0" class="text-center py-12 text-gray-500 text-sm">
                No analysis history yet.<br/>Run an analysis to see it here.
              </div>
              <div
                v-for="entry in analysisHistory"
                :key="entry.id"
                class="bg-surface border border-border rounded-lg p-3 hover:border-primary/50 transition-colors"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="flex-1 min-w-0">
                    <p class="text-sm font-medium text-gray-200 truncate">{{ entry.file_name }}</p>
                    <p class="text-xs text-gray-500 mt-0.5">{{ formatHistoryDate(entry.timestamp) }}</p>
                    <div class="flex items-center gap-3 mt-2">
                      <span class="text-xs text-gray-400">Merchant: <span class="text-gray-300">{{ entry.merchant_name || 'N/A' }}</span></span>
                      <span class="text-xs text-gray-400">Risk: <span :class="getRiskScoreColor(entry.risk_score)" class="font-medium">{{ entry.risk_score || 'N/A' }}</span></span>
                    </div>
                  </div>
                  <div class="flex items-center gap-2">
                    <button
                      @click="loadHistoryEntry(entry)"
                      class="px-3 py-1.5 bg-primary hover:bg-blue-600 rounded-lg text-xs text-white font-medium transition-colors"
                    >
                      📂 Load
                    </button>
                    <button
                      @click="deleteHistoryEntry(entry.id)"
                      class="px-3 py-1.5 bg-surface hover:bg-border border border-border rounded-lg text-xs text-gray-300 transition-colors"
                    >
                      🗑️
                    </button>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import PdfViewer from './components/PdfViewer.vue'

// ═══════════════════════════════════════════════════════════════════════════
// STATE MACHINE - Explicit app states (no boolean spaghetti)
// ═══════════════════════════════════════════════════════════════════════════
const appState = ref('IDLE') // 'IDLE' | 'LOADING_PDF' | 'READY' | 'ANALYZING' | 'COMPLETE' | 'ERROR'
const errorMessage = ref('')

// File data - BATCH PROCESSING SUPPORT
// File queue for multiple PDFs (3-6 months of statements)
const fileQueue = ref([]) // Array of { path: string, name: string, pageCount: number }
const currentFileIndex = ref(0) // Current file being processed
const pdfPageCount = ref(0)
const pdfSource = ref(null) // Blob URL for PDF (not Uint8Array)
const pdfBlobUrl = ref(null) // Track blob URL for cleanup

// Batch processing state
const isBatchProcessing = ref(false)
const batchResults = ref([]) // Array of raw results from each PDF
const combinedResult = ref(null) // Final aggregated result

// Analysis data
const analysisResult = ref(null)
const rawResponse = ref('')
const parsedData = ref(null)

// Chat data
const chatMessages = ref([])
const chatInput = ref('')
const isChatLoading = ref(false)

// Export state
const isExporting = ref(false)

// UI state
const activeTab = ref('underwrite')
const loadingProgress = ref(0)
const loadingMessage = ref('')
const totalPages = ref(0)
const currentPage = ref(0)

// Event-driven analysis state
const analysisUnlisten = ref(null) // Function to unsubscribe from events
const pageResults = ref([]) // Accumulated results from each page event

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

// Template management state
const savedTemplates = ref([]) // Array of { name, instructions }
const selectedTemplateName = ref('')
const newTemplateName = ref('')

// Analysis history state
const analysisHistory = ref([]) // Array of HistoryEntry

// ═══════════════════════════════════════════════════════════════════════════
// CURSOR-STYLE PROMPT ARCHITECTURE
// System prompt is hardcoded to ensure consistent JSON output
// User can only add custom instructions on top of the system prompt
// ═══════════════════════════════════════════════════════════════════════════

const SYSTEM_PROMPT = `You are a strict Data Extraction AI for Merchant Cash Advance (MCA) Underwriting.

CRITICAL DEFINITIONS:
1. THE MERCHANT: The business that owns the bank account. CRITICAL: The merchant is NEVER the bank that issued the statement (e.g., ignore "Chase", "Bank of America", "LendingClub").
2. THE LENDER: A third-party MCA company debiting daily/weekly ACH payments from the account.

STRICT RULES:
- If you do not see recurring daily/weekly MCA debits, you MUST leave the positions array completely empty: "positions": []
- DO NOT invent or guess lenders.
- DO NOT do any math or calculate averages. Only extract exact numbers you see.
- You MUST replace all "REPLACE_WITH_..." placeholder strings with the actual data you find. If data is missing, replace it with null.

OUTPUT FORMAT:
Return ONLY valid JSON. Replace the placeholder values with your extracted data.
{
  "merchant_info": { 
    "name": "REPLACE_WITH_MERCHANT_NAME", 
    "account_number": "REPLACE_WITH_ACCOUNT_NUMBER",
    "statement_period": "REPLACE_WITH_STATEMENT_PERIOD" 
  },
  "positions": [ 
    { 
      "lender": "REPLACE_WITH_LENDER_NAME", 
      "payment": 0.0, 
      "frequency": "REPLACE_WITH_DAILY_OR_WEEKLY", 
      "funded_amount": 0.0, 
      "funded_date": "REPLACE_WITH_DATE" 
    } 
  ],
  "bank_metrics": { 
    "total_deposits_count": 0,
    "negative_days_count": 0,
    "nsf_count": 0
  }
}`

// User-customizable instructions (editable in UI)
const userCustomInstructions = ref('Add custom underwriting focus areas here...')

// Build the full prompt by merging system prompt + user instructions
const buildFullPrompt = () => {
  return SYSTEM_PROMPT + '\n\nUSER CUSTOM INSTRUCTIONS:\n' + userCustomInstructions.value
}

// File name computed from current file in queue
const fileName = computed(() => {
  if (fileQueue.value.length > 0 && currentFileIndex.value < fileQueue.value.length) {
    return fileQueue.value[currentFileIndex.value].name
  }
  return ''
})

// File path computed from current file in queue
const filePath = computed(() => {
  if (fileQueue.value.length > 0 && currentFileIndex.value < fileQueue.value.length) {
    return fileQueue.value[currentFileIndex.value].path
  }
  return ''
})

const tabs = [
  { id: 'underwrite', label: 'Underwrite' },
  { id: 'prompt', label: 'Prompt' },
  { id: 'settings', label: 'Settings' },
  { id: 'history', label: 'History' }
]

const resetCustomInstructions = () => {
  userCustomInstructions.value = 'Add custom underwriting focus areas here...'
}

// ═══════════════════════════════════════════════════════════════════════════
// PROMPT TEMPLATE MANAGEMENT
// ═══════════════════════════════════════════════════════════════════════════

// Load all templates from Rust backend
const loadTemplates = async () => {
  try {
    savedTemplates.value = await invoke('get_templates')
  } catch (error) {
    console.error('Failed to load templates:', error)
  }
}

// Load selected template into the editor
const loadSelectedTemplate = () => {
  const template = savedTemplates.value.find(t => t.name === selectedTemplateName.value)
  if (template) {
    userCustomInstructions.value = template.instructions
  }
}

// Save current instructions as a new template
const saveNewTemplate = async () => {
  const name = newTemplateName.value.trim()
  if (!name) return
  
  try {
    await invoke('save_template', { 
      name, 
      instructions: userCustomInstructions.value 
    })
    await loadTemplates() // Refresh the list
    selectedTemplateName.value = name // Select the newly saved template
    newTemplateName.value = '' // Clear the input
  } catch (error) {
    console.error('Failed to save template:', error)
  }
}

// Delete the selected template
const deleteSelectedTemplate = async () => {
  if (!selectedTemplateName.value) return

  try {
    await invoke('delete_template', { name: selectedTemplateName.value })
    await loadTemplates() // Refresh the list
    selectedTemplateName.value = '' // Clear selection
    resetCustomInstructions() // Reset to default
  } catch (error) {
    console.error('Failed to delete template:', error)
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// ANALYSIS HISTORY MANAGEMENT
// ═══════════════════════════════════════════════════════════════════════════

// Load all history from Rust backend
const loadHistory = async () => {
  try {
    analysisHistory.value = await invoke('get_history')
  } catch (error) {
    console.error('Failed to load history:', error)
  }
}

// Format history entry timestamp to readable date
const formatHistoryDate = (timestamp) => {
  const date = new Date(timestamp)
  return date.toLocaleString('en-US', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit'
  })
}

// Load a history entry and display it
const loadHistoryEntry = (entry) => {
  // Set the parsed data from history
  parsedData.value = entry.parsed_data
  rawResponse.value = JSON.stringify(entry.parsed_data, null, 2)
  
  // Set file name from history entry
  // Note: We can't restore the actual file path, but we show the filename
  fileName.value = entry.file_name
  
  // Switch to COMPLETE state to show the dashboard
  appState.value = 'COMPLETE'
  activeTab.value = 'underwrite' // Switch to Underwrite tab to show the data
  
  console.log('[History] Loaded entry:', entry.id)
}

// Delete a single history entry
const deleteHistoryEntry = async (id) => {
  try {
    await invoke('delete_history_entry', { id })
    await loadHistory() // Refresh the list
  } catch (error) {
    console.error('Failed to delete history entry:', error)
  }
}

// Clear all history
const clearAllHistory = async () => {
  try {
    await invoke('clear_all_history')
    await loadHistory() // Refresh the list
  } catch (error) {
    console.error('Failed to clear history:', error)
  }
}

// Save current analysis to history
const saveCurrentAnalysisToHistory = async () => {
  if (!parsedData.value || !fileName.value) return
  
  try {
    const merchantName = parsedData.value.merchant_info?.name || 
                         parsedData.value.business?.name || 
                         null
    const riskScore = parsedData.value.risk?.score || null
    
    await invoke('save_history_entry', {
      fileName: fileName.value,
      merchantName,
      riskScore,
      parsedData: parsedData.value
    })
    await loadHistory() // Refresh the list
    console.log('[History] Saved current analysis')
  } catch (error) {
    console.error('Failed to save to history:', error)
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// BATCH FILE QUEUE MANAGEMENT
// ═══════════════════════════════════════════════════════════════════════════

const removeFile = (index) => {
  fileQueue.value.splice(index, 1)
  // Adjust current index if needed
  if (currentFileIndex.value >= fileQueue.value.length) {
    currentFileIndex.value = Math.max(0, fileQueue.value.length - 1)
  }
  // If queue is empty, go back to IDLE
  if (fileQueue.value.length === 0) {
    appState.value = 'IDLE'
    pdfSource.value = null
    pdfPageCount.value = 0
  }
}

const clearFileQueue = () => {
  fileQueue.value = []
  currentFileIndex.value = 0
  appState.value = 'IDLE'
  pdfSource.value = null
  pdfPageCount.value = 0
}

// Check Ollama connection on mount and set up event listeners
onMounted(async () => {
  await checkOllamaConnection()
  await loadTemplates() // Load saved prompt templates
  await loadHistory() // Load analysis history
  // Set up event listeners (non-blocking, don't await)
  setupAnalysisEventListeners()
})

// Clean up event listeners and blob URLs on unmount
onUnmounted(async () => {
  if (analysisUnlisten.value) {
    analysisUnlisten.value()
  }
  // Clean up blob URL to prevent memory leaks
  if (pdfBlobUrl.value) {
    URL.revokeObjectURL(pdfBlobUrl.value)
    pdfBlobUrl.value = null
  }
  // Call Rust garbage collector to delete temp files
  try {
    const count = await invoke('cleanup_temp_files')
    console.log('[GarbageCollector] Deleted', count, 'temp directories on app close')
  } catch (err) {
    console.error('[GarbageCollector] Cleanup failed:', err)
  }
})

// Set up Tauri event listeners for analysis progress
const setupAnalysisEventListeners = () => {
  // Listen for analysis-progress events
  listen('analysis-progress', (event) => {
    const payload = event.payload
    
    console.log('[Event] Received:', payload.type, payload)
    
    if (payload.type === 'start') {
      totalPages.value = payload.total_pages
      currentPage.value = 0
      pageResults.value = []
      loadingMessage.value = payload.message
      console.log('[Event] Analysis started:', payload)
    }
    
    if (payload.type === 'page_start') {
      currentPage.value = payload.current_page
      loadingMessage.value = payload.message
      console.log('[Event] Page start:', payload)
    }
    
    if (payload.type === 'page_complete') {
      currentPage.value = payload.current_page
      pageResults.value.push(payload.page_result)
      loadingMessage.value = payload.message
      console.log('[Event] Page complete:', payload)
    }
    
    if (payload.type === 'aggregating') {
      loadingMessage.value = payload.message
      currentPage.value = payload.total_pages
      console.log('[Event] Aggregating:', payload)
    }
  }).then(() => {
    console.log('[Event] Analysis progress listener registered successfully')
  }).catch((error) => {
    console.error('[Event] Failed to set up analysis progress listener:', error)
    // Don't crash the app - just log the error
  })
}

const checkOllamaConnection = async () => {
  isCheckingConnection.value = true
  try {
    ollamaConnected.value = await invoke('check_ollama_connection')
    if (ollamaConnected.value) {
      ollamaModels.value = await invoke('get_ollama_models')
      if (ollamaModels.value.length > 0) {
        const visionModels = ollamaModels.value.filter(
          m => m.name.toLowerCase().includes('vision') ||
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
      multiple: true, // Allow multiple file selection
      filters: [{
        name: 'PDF',
        extensions: ['pdf']
      }]
    })
    if (selected && Array.isArray(selected)) {
      // Add all selected files to queue
      for (const file of selected) {
        fileQueue.value.push({
          path: file,
          name: file.split('/').pop() || file.split('\\').pop() || file,
          pageCount: 0
        })
      }
      
      console.log('[Batch] Added', selected.length, 'files to queue')
      
      // Load the first file for preview
      if (fileQueue.value.length > 0) {
        await loadFileForPreview(0)
      }
    }
  } catch (error) {
    console.error('Error opening file:', error)
    dropError.value = 'Failed to open file picker'
  }
}

// Load a file from queue for PDF preview
const loadFileForPreview = async (index) => {
  if (index >= fileQueue.value.length) return

  const file = fileQueue.value[index]
  currentFileIndex.value = index

  console.log('[State] Loading PDF:', file.path)

  // Clean up previous blob URL
  if (pdfBlobUrl.value) {
    URL.revokeObjectURL(pdfBlobUrl.value)
    pdfBlobUrl.value = null
  }

  try {
    const result = await invoke('convert_pdf_to_images', {
      pdfPath: file.path,
      dpi: 72
    })

    fileQueue.value[index].pageCount = result.pages.length
    pdfPageCount.value = result.pages.length

    // Use the Data URI from Rust (no CSP issues!)
    if (result.preview_image_data_uri) {
      pdfSource.value = result.preview_image_data_uri
      console.log('[State] Preview Data URI loaded:', result.preview_image_data_uri.length, 'chars')
    } else if (result.preview_path) {
      // Fallback to path if Data URI failed
      pdfSource.value = result.preview_path
      console.log('[State] Fallback to preview path:', result.preview_path)
    } else {
      console.warn('[State] No preview data available')
    }

    loadingProgress.value = 50
    loadingMessage.value = `Converting ${result.pages.length} page(s)...`
  } catch (err) {
    console.error('Error converting PDF:', err)
    pdfPageCount.value = 1
    fileQueue.value[index].pageCount = 1
  }

  appState.value = 'LOADING_PDF'
  loadingProgress.value = 0
  loadingMessage.value = 'Loading PDF...'

  setTimeout(() => {
    loadingProgress.value = 100
    appState.value = 'READY'
    console.log('[State] PDF loaded, ready for analysis')
  }, 500)
}

const handleDrop = async (event) => {
  isDragging.value = false
  dropError.value = ''
  
  const files = event.dataTransfer?.files
  if (files && files.length > 0) {
    // Process all dropped files
    for (const file of files) {
      if (file.type === 'application/pdf' || file.name.endsWith('.pdf')) {
        fileQueue.value.push({
          path: file.path || file.name,
          name: file.name,
          pageCount: 0
        })
      }
    }
    
    console.log('[Batch] Added', files.length, 'files from drop')
    
    // Load the first file for preview
    if (fileQueue.value.length > 0) {
      await loadFileForPreview(0)
    }
  }
}

const handleUnderwrite = async () => {
  if (!ollamaConnected.value) {
    errorMessage.value = 'Ollama is not running. Please start Ollama and ensure vision models are installed.'
    appState.value = 'ERROR'
    return
  }

  if (fileQueue.value.length === 0) {
    errorMessage.value = 'No files selected. Please upload at least one PDF.'
    appState.value = 'ERROR'
    return
  }

  appState.value = 'ANALYZING'
  loadingProgress.value = 0
  batchResults.value = [] // Reset batch results
  pageResults.value = [] // Reset page results for events

  console.log('[Batch] Starting batch analysis of', fileQueue.value.length, 'files...')

  // Set up one-time listener for analysis-complete
  const completeUnlisten = await listen('analysis-complete', (event) => {
    const payload = event.payload
    console.log('[Event] Analysis complete:', payload)

    try {
      // Store the final result
      rawResponse.value = payload.result
      analysisResult.value = payload.result
      parsedData.value = parseJsonFromResponse(payload.result)

      // Check if parsing succeeded
      if (!parsedData.value) {
        throw new Error('Failed to parse AI response. The document may be too large or the AI output was invalid.')
      }

      console.log('[Batch] Parsed master dashboard data:', parsedData.value)

      appState.value = 'COMPLETE'
      activeTab.value = 'underwrite'
      
      // Auto-save to history
      saveCurrentAnalysisToHistory()

      // Add automatic greeting to chat
      chatMessages.value.push({
        role: 'assistant',
        content: `✅ Combined analysis of ${fileQueue.value.length} file(s) complete. The dashboard shows aggregated metrics across all statements. What specific questions do you have about this merchant?`
      })

      console.log('[State] Batch analysis complete')
      console.log('[State] UI State:', { appState: appState.value, activeTab: activeTab.value })
    } catch (parseError) {
      console.error('Parse error:', parseError)
      appState.value = 'ERROR'
      errorMessage.value = parseError.message || 'Failed to parse AI response. The document may be too large or the AI output was invalid.'
    }
  })

  try {
    // Process each file in the queue sequentially
    for (let i = 0; i < fileQueue.value.length; i++) {
      currentFileIndex.value = i
      const file = fileQueue.value[i]

      loadingMessage.value = `Analyzing file ${i + 1} of ${fileQueue.value.length}: ${file.name}...`
      totalPages.value = fileQueue.value.length
      currentPage.value = i

      console.log('[Batch] Processing file', i + 1, '/', fileQueue.value.length, '-', file.name)

      try {
        // Analyze this file - events will update UI in real-time
        const result = await invoke('send_pdf_to_ollama', {
          model: selectedModel.value,
          prompt: buildFullPrompt(),
          pdfPath: file.path,
          temperature: modelConfig.value.temperature,
          maxTokens: modelConfig.value.maxTokens
        })

        batchResults.value.push(result)
        console.log('[Batch] File', i + 1, 'complete -', result.length, 'chars')
      } catch (fileError) {
        console.error(`[Batch] File ${i + 1} error:`, fileError)
        throw new Error(`Failed to analyze file ${i + 1} (${file.name}): ${fileError}`)
      }
    }

    // All files processed - now aggregate into one master result
    loadingMessage.value = `Combining ${fileQueue.value.length} files into master report...`
    currentPage.value = fileQueue.value.length

    console.log('[Batch] All files analyzed, aggregating results...')

    try {
      // Use the Rust aggregator to combine all results
      const combinedResult = await invoke('aggregate_batch_results', {
        model: selectedModel.value,
        originalPrompt: buildFullPrompt(),
        pageResults: batchResults.value,
        temperature: modelConfig.value.temperature,
        maxTokens: modelConfig.value.maxTokens
      })

      console.log('[Batch] Aggregation complete -', combinedResult.length, 'chars')

      loadingProgress.value = 100

      // Clean up the complete listener
      completeUnlisten()
    } catch (aggError) {
      console.error('[Batch] Aggregation error:', aggError)
      throw new Error(`Failed to combine results: ${aggError}. The document may be too large or the AI output was invalid.`)
    }

  } catch (error) {
    console.error('Underwrite error:', error)

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
// JSON PARSING
// ═══════════════════════════════════════════════════════════════════════════

const parseJsonFromResponse = (text) => {
  if (!text) return null

  try {
    return JSON.parse(text)
  } catch {
    const codeBlockMatch = text.match(/```(?:json)?\s*([\s\S]*?)```/)
    if (codeBlockMatch) {
      try {
        return JSON.parse(codeBlockMatch[1].trim())
      } catch {
        // Fall through
      }
    }

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

const getRecommendationColor = (recommendation) => {
  if (!recommendation) return 'bg-gray-500'
  const rec = recommendation.toUpperCase()
  if (rec === 'APPROVE') return 'bg-green-500'
  if (rec === 'DENY') return 'bg-red-500'
  if (rec === 'REVIEW') return 'bg-yellow-500'
  return 'bg-gray-500'
}

const getRiskScoreColor = (score) => {
  if (score === null || score === undefined) return 'text-gray-400'
  if (score >= 8) return 'text-emerald-400'
  if (score >= 5) return 'text-amber-400'
  return 'text-red-400'
}

const getRiskScoreBarColor = (score) => {
  if (score === null || score === undefined) return 'bg-gray-500'
  if (score >= 8) return 'bg-emerald-500'
  if (score >= 5) return 'bg-amber-500'
  return 'bg-red-500'
}

const copyResults = async () => {
  const dataToCopy = parsedData.value || rawResponse.value
  if (!dataToCopy) return

  try {
    const textToCopy = typeof dataToCopy === 'object'
      ? JSON.stringify(dataToCopy, null, 2)
      : dataToCopy

    await navigator.clipboard.writeText(textToCopy)
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
// EXPORT FUNCTIONS
// ═══════════════════════════════════════════════════════════════════════════

const exportToJSON = async () => {
  const dataToExport = parsedData.value || analysisResult.value
  if (!dataToExport) return

  isExporting.value = true
  try {
    const filePath = await invoke('export_json', {
      data: dataToExport,
      defaultPath: `analysis-${fileName.value.replace('.pdf', '')}.json`
    })

    if (filePath) {
      console.log('[Export] JSON saved to:', filePath)
    }
  } catch (error) {
    console.error('[Export] JSON export failed:', error)
  } finally {
    isExporting.value = false
  }
}

const exportToCSV = async () => {
  const data = parsedData.value
  if (!data) return

  isExporting.value = true
  try {
    const csvRows = [
      ['Business Information', '', ''],
      ['Business Name', data.business?.name || '', ''],
      ['Account', data.business?.account || '', ''],
      ['Period', data.business?.period || '', ''],
      ['', '', ''],
      ['Positions', '', ''],
      ...data.positions?.map(p => [p.lender || 'Unknown', p.payment || '', p.frequency || '', p.funded || '']) || [],
      ['', '', ''],
      ['Bank Metrics', '', ''],
      ['True Revenue', data.bank_metrics?.true_revenue || '', 'USD'],
      ['Negative Days', data.bank_metrics?.negative_days || '', ''],
      ['Avg Daily Balance', data.bank_metrics?.avg_daily_balance || '', 'USD'],
      ['NSF Count', data.bank_metrics?.nsf_count || '', ''],
      ['', '', ''],
      ['Debt & Leverage', '', ''],
      ['Total Debt Service', data.debt_leverage?.total_debt_service || '', 'USD'],
      ['Safe New Payment', data.debt_leverage?.safe_new_payment || '', 'USD'],
      ['Leverage Ratio', data.debt_leverage?.leverage_ratio || '', ''],
      ['', '', ''],
      ['Risk', '', ''],
      ['Risk Score', data.risk?.score || '', '/10'],
      ['', '', ''],
      ['Recommendation', data.recommendation || '', ''],
      ['', '', ''],
      ['Notes', data.notes || '', '']
    ]

    const csvContent = csvRows.map(row => row.map(cell => {
      const str = String(cell ?? '')
      return str.includes(',') || str.includes('"') ? `"${str.replace(/"/g, '""')}"` : str
    }).join(',')).join('\n')

    const filePath = await invoke('export_csv', {
      content: csvContent,
      defaultPath: `analysis-${fileName.value.replace('.pdf', '')}.csv`
    })

    if (filePath) {
      console.log('[Export] CSV saved to:', filePath)
    }
  } catch (error) {
    console.error('[Export] CSV export failed:', error)
  } finally {
    isExporting.value = false
  }
}

const printReport = () => {
  const data = parsedData.value
  const hasParsedData = !!data

  const notesContent = hasParsedData ? (data.notes || '') : rawResponse.value
  if (!notesContent && !hasParsedData) return

  const printContent = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>MCA Analysis Report - ${fileName.value}</title>
      <style>
        body { font-family: Arial, sans-serif; padding: 40px; color: #333; background: #fff; }
        h1 { color: #1a1a1a; border-bottom: 3px solid #3b82f6; padding-bottom: 15px; }
        h2 { color: #444; margin-top: 30px; font-size: 18px; }
        .section { margin: 20px 0; padding: 20px; background: #f5f5f5; border-radius: 8px; }
        .grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 15px; margin: 15px 0; }
        .grid-3 { display: grid; grid-template-columns: repeat(3, 1fr); gap: 15px; }
        .card { padding: 15px; background: white; border-radius: 6px; border: 1px solid #e0e0e0; }
        .label { font-size: 11px; color: #666; text-transform: uppercase; font-weight: 600; }
        .value { font-size: 20px; font-weight: bold; color: #1a1a1a; margin-top: 5px; }
        .recommendation { display: inline-block; padding: 8px 20px; border-radius: 4px; color: white; font-weight: bold; }
        .approve { background: #22c55e; }
        .deny { background: #ef4444; }
        .review { background: #eab308; }
        .notes { white-space: pre-wrap; line-height: 1.6; }
        .footer { margin-top: 40px; padding-top: 20px; border-top: 2px solid #e0e0e0; font-size: 12px; color: #666; }
        table { width: 100%; border-collapse: collapse; margin: 15px 0; }
        th, td { padding: 10px; text-align: left; border-bottom: 1px solid #ddd; }
        th { background: #f0f0f0; font-weight: 600; font-size: 12px; text-transform: uppercase; }
        .positive { color: #22c55e; }
        .negative { color: #ef4444; }
        @media print { body { padding: 20px; } }
      </style>
    </head>
    <body>
      <h1>MCA Bank Statement Analysis Report</h1>
      <p><strong>File:</strong> ${fileName.value}</p>
      <p><strong>Generated:</strong> ${new Date().toLocaleString()}</p>

      ${hasParsedData ? `
      <div class="section">
        <h2>Business Information</h2>
        <div class="grid">
          <div class="card">
            <div class="label">Business Name</div>
            <div class="value">${data.business?.name || 'N/A'}</div>
          </div>
          <div class="card">
            <div class="label">Account</div>
            <div class="value">${data.business?.account || 'N/A'}</div>
          </div>
          <div class="card" style="grid-column: span 2;">
            <div class="label">Statement Period</div>
            <div class="value">${data.business?.period || 'N/A'}</div>
          </div>
        </div>
      </div>

      ${data.positions && data.positions.length > 0 ? `
      <div class="section">
        <h2>Existing Positions</h2>
        <table>
          <thead>
            <tr>
              <th>Lender</th>
              <th>Payment</th>
              <th>Frequency</th>
              <th>Funded</th>
            </tr>
          </thead>
          <tbody>
            ${data.positions.map(p => `
            <tr>
              <td>${p.lender || 'Unknown'}</td>
              <td class="negative">$${(p.payment || 0).toLocaleString()}</td>
              <td>${p.frequency || 'N/A'}</td>
              <td class="positive">$${(p.funded || 0).toLocaleString()}</td>
            </tr>
            `).join('')}
          </tbody>
        </table>
      </div>
      ` : ''}

      <div class="section">
        <h2>Bank Metrics</h2>
        <div class="grid">
          <div class="card">
            <div class="label">True Revenue</div>
            <div class="value positive">$${(data.bank_metrics?.true_revenue || 0).toLocaleString()}</div>
            <div style="font-size: 11px; color: #666;">Excludes loan deposits</div>
          </div>
          <div class="card">
            <div class="label">Negative Days</div>
            <div class="value ${(data.bank_metrics?.negative_days || 0) > 0 ? 'negative' : 'positive'}">${data.bank_metrics?.negative_days || 0}</div>
            <div style="font-size: 11px; color: #666;">Days balance &lt; $0</div>
          </div>
          <div class="card">
            <div class="label">Avg Daily Balance</div>
            <div class="value">$${(data.bank_metrics?.avg_daily_balance || 0).toLocaleString()}</div>
          </div>
          <div class="card">
            <div class="label">NSF Count</div>
            <div class="value ${(data.bank_metrics?.nsf_count || 0) > 0 ? 'negative' : 'positive'}">${data.bank_metrics?.nsf_count || 0}</div>
          </div>
        </div>
      </div>

      <div class="section">
        <h2>Debt & Leverage</h2>
        <div class="grid-3">
          <div class="card">
            <div class="label">Total Debt Service</div>
            <div class="value negative">$${(data.debt_leverage?.total_debt_service || 0).toLocaleString()}</div>
          </div>
          <div class="card">
            <div class="label">Safe New Payment</div>
            <div class="value positive">$${(data.debt_leverage?.safe_new_payment || 0).toLocaleString()}</div>
          </div>
          <div class="card">
            <div class="label">Leverage Ratio</div>
            <div class="value">${data.debt_leverage?.leverage_ratio || 'N/A'}</div>
          </div>
        </div>
      </div>

      <div class="section">
        <h2>Risk Assessment</h2>
        <div class="grid">
          <div class="card">
            <div class="label">Risk Score</div>
            <div class="value">${data.risk?.score ?? 'N/A'}/10</div>
          </div>
          <div class="card">
            <div class="label">Recommendation</div>
            <div class="value">
              <span class="recommendation ${data.recommendation?.toLowerCase() || 'review'}">
                ${data.recommendation || 'N/A'}
              </span>
            </div>
          </div>
        </div>
      </div>
      ` : `
      <div class="section">
        <h2>Analysis Result</h2>
        <p style="color: #666; font-style: italic;">No structured data was parsed from this analysis.</p>
      </div>
      `}

      <div class="section">
        <h2>Analysis Notes</h2>
        <div class="notes">${notesContent}</div>
      </div>

      <div class="footer">
        <p>Generated by Local MCA Underwriter Workspace | 100% Offline Analysis</p>
      </div>
    </body>
    </html>
  `

  const printWindow = window.open('', '_blank')
  printWindow.document.write(printContent)
  printWindow.document.close()
  printWindow.print()
}

// ═══════════════════════════════════════════════════════════════════════════
// FOLLOW-UP CHAT - Text-only, no PDF re-processing
// ═══════════════════════════════════════════════════════════════════════════

const sendChatMessage = async () => {
  const question = chatInput.value.trim()
  if (!question || !ollamaConnected.value || isChatLoading.value) return

  chatMessages.value.push({ role: 'user', content: question })
  chatInput.value = ''
  isChatLoading.value = true

  try {
    const contextPrompt = `
Previous analysis of this bank statement:
${rawResponse.value}

User follow-up question: ${question}

Provide a concise, helpful answer based on the bank statement analysis above.`

    // Use text-only chat command - NO PDF re-processing!
    const response = await invoke('chat_with_ollama', {
      model: selectedModel.value,
      prompt: contextPrompt,
      temperature: modelConfig.value.temperature,
      maxTokens: modelConfig.value.maxTokens
      // NO pdfPath - pure text-to-text!
    })

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
