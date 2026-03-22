# Project: Local MCA Underwriter Workspace
# Core Directive: Performance at Scale. Component-by-component development.
# Target OS: Linux Primary (Cross-platform via Tauri)

## 1. Project Vision
Build a blazing-fast, local-first underwriting workspace focused on deep-work and speed for finance professionals.
- Environment: 100% local, offline-first. No cloud leakage.
- Tech Stack: Rust (Backend) + Tauri (App Framework) + Vue.js (Frontend) + Tailwind CSS (Styling).
- Vibe: Dark mode, minimalist, Zed/Cursor-aesthetic. Corporate hybrid app (professional Fintech meets developer-speed). Use a modern AI Chat Panel instead of a raw terminal. No raw JSON dumps.

## 2. Phase 1: "Command Center" UI Scaffolding ✅ COMPLETE

**Completed:**
1. ✅ Initialized Rust + Tauri + Vue project
2. ✅ Built Empty State UI - centered drop zone with drag & drop
3. ✅ Built Active State UI - 60/40 split layout
4. ✅ Implemented native file picker (Tauri dialog API)
5. ✅ Drag-and-drop file handling

## 3. Phase 2: Ollama Integration ✅ COMPLETE

**Completed:**
1. ✅ Ollama API integration (Rust backend with reqwest)
2. ✅ Connection status indicator with Test button
3. ✅ Auto-populate model selector from Ollama
4. ✅ Send PDF + prompt to vision models
5. ✅ Model configuration (temperature, tokens, context)
6. ✅ Comprehensive error handling and timeout management

## 4. Phase 3: PDF Processing ✅ COMPLETE

**Completed:**
1. ✅ PDF-to-image conversion (poppler-utils/pdftocairo)
2. ✅ Multi-page PDF support
3. ✅ Page count display
4. ✅ Grayscale JPEG compression (55-60% size reduction)
5. ✅ Base64 encoding for Ollama API
6. ✅ Automatic temp file cleanup

## 5. Phase 4: PDF Viewer ✅ COMPLETE

**Completed:**
1. ✅ Full PDF.js viewer integration (vue-pdf-embed)
2. ✅ Page navigation (prev/next buttons)
3. ✅ Zoom controls (50%-200%)
4. ✅ Thumbnail strip for page jumping
5. ✅ Page counter display
6. ✅ Sync with backend page count

## 6. Phase 5: Workspace Features ✅ COMPLETE

**Completed:**
1. ✅ Master Underwriting Prompt editor
2. ✅ Tab navigation (Underwrite | Prompt | Settings)
3. ✅ Zed-style AI Chat Assistant view (Dashboard + Conversational UI)
4. ✅ Prompt reset to default
5. ✅ Loading states with progress bar
6. ✅ Auto-switch to Underwrite tab after analysis

## 7. Phase 6: UI Pivot - Blank Screen Fix ✅ COMPLETE

**Problem:** The `v-if/v-else` state logic was destroying (unmounting) the PDF viewer and right panel during the `ANALYZING` state, causing a silent crash when trying to rebuild the DOM.

**Solution:** 
- Replaced `v-if/v-else` with `v-show` for the main dashboard layout
- Left Panel (PDF Viewer) and Right Panel (Chat/Dashboard) now ALWAYS remain mounted once a file is loaded
- Removed the raw terminal output box entirely
- Added targeted loading states inside the Right Panel during analysis
- States (`ANALYZING`, `ERROR`, `COMPLETE`, `READY`) now render content within the persistent Right Panel

**Result:** No more blank screen crashes. The layout remains stable throughout all state transitions.

## 8. Phase 7: Analysis Dashboard ✅ COMPLETE

**Problem:** AI response was displayed as raw text without structured visualization.

**Solution:**
- Added JSON parsing utility to extract structured data from AI responses
- Built premium dashboard cards with proper formatting
- Implemented color-coded indicators for recommendations and risk scores
- Added Copy Results button with clipboard feedback

**Result:** Professional dashboard displaying business info, financial metrics, risk assessment, and recommendations in clean UI cards.

## 9. Phase 8: Follow-up Chat ✅ COMPLETE

**Problem:** Users couldn't ask follow-up questions about the analysis without re-running the full analysis.

**Solution:**
- Added conversational chat interface below dashboard cards
- Context-aware prompts include full previous analysis
- Message thread with user/AI messages, loading states
- Same PDF sent for vision model reference

**Result:** Users can now ask unlimited follow-up questions about the analyzed document with full context retention.

## 10. Phase 9: Export & Print ✅ COMPLETE

**Problem:** Users needed to save and share analysis results externally.

**Solution:**
- **JSON Export:** Save full parsed analysis data
- **CSV Export:** Structured spreadsheet-ready format
- **Print Report:** Professional light-themed report with print dialog
- All exports use native file save dialogs

**Result:** Users can export results in multiple formats for record-keeping, sharing, or further analysis in spreadsheets.

## 11. Current Status

**ALL PHASES COMPLETE** - App is fully functional:

### User Flow:
1. Upload PDFs → View in full PDF viewer with navigation
2. Select Ollama vision model → Auto-populated from local Ollama
3. Click "Underwrite File" → PDF converted to grayscale JPEG
4. Wait 5-10 minutes → Vision model analyzes document
5. Results displayed → Dashboard cards + AI Chat Feed (auto-switched)
6. Ask follow-up questions → Get instant answers with full context
7. Export results → Save as JSON, CSV, or print professional report

### Technical Highlights:
- **Grayscale JPEG compression:** 55-60% size reduction
- **Timeout handling:** 10-minute timeout for vision models
- **Error reporting:** Specific error messages (timeout/connect/request)
- **Security:** Temp files auto-deleted after processing
- **UX:** Progress bar, loading messages, auto-tab switching
- **Stability:** Persistent layout prevents blank screen crashes
- **Dashboard:** Parsed JSON into premium UI cards
- **Chat:** Context-aware follow-up questions
- **Export:** JSON, CSV, and print report functionality

## 11. Key Technical Decisions

### Why Grayscale JPEG?
- Bank statements are B&W - no color info lost
- 55-60% smaller than color PNG
- Faster transmission to Ollama
- Vision models still read text/numbers clearly

### Why Base64 (not file paths)?
- Ollama API requires Base64 in JSON payload
- File paths not supported by Ollama REST API
- Compression makes payload manageable (~85KB per page)

### Why 72 DPI?
- Screen quality sufficient for text recognition
- Higher DPI = exponentially larger files
- Vision models don't need print-quality images

### Why reqwest (not Tauri HTTP)?
- More robust timeout handling
- Better error reporting
- Industry-standard Rust HTTP client

### Why Persistent Layout (v-show vs v-if)?
- Heavy PDF viewer components crash when unmounted/rebuilt rapidly
- State transitions should not destroy DOM structure
- Targeted loaders (inside panels) are smoother than full-page overlays

### Why Context-Aware Chat?
- Re-sending PDF with full analysis context ensures accurate answers
- Avoids building complex conversation state management
- Each question is self-contained with full history

## 12. Current Development Hurdles (WIP - TO BE FIXED)

### Multi-Page Handling
- **Current Limitation:** Sends only FIRST page to vision model
- **Reason:** Multi-image support unstable in Ollama
- **Goal:** Implement sequential page analysis or multipart upload to handle full statements.

### Processing Time
- **Current Limitation:** 5-10 minutes for vision model analysis (single page)
- **Bottleneck:** Image tokenization in vision encoder, model size, CPU/GPU hardware
- **Goal:** Streaming responses, model optimization, hardware acceleration

### Model Compatibility
- **Tested:** llama3.2-vision ✅
- **Issues:** qwen3-vl (API incompatibility)
- **Recommendation:** Use llama3.2-vision or llava

## 13. State Machine Implementation ✅

**Problem Solved:** Boolean spaghetti (`isLoading`, `fileSelected`, `activeTab`) caused race conditions and blank screens after analysis completed.

**Solution:** Explicit state machine with clear transitions:

```javascript
const appState = ref('IDLE')
// States: 'IDLE' | 'LOADING_PDF' | 'READY' | 'ANALYZING' | 'COMPLETE' | 'ERROR'
```

**State Transitions:**
```
IDLE ──[upload PDF]──→ LOADING_PDF ──[PDF processed]──→ READY
                                                              │
                                                              ↓
ERROR ←──[Ollama fails]── ANALYZING ←──[click Underwrite]───┘
  │                         │
  └──[user retries]────────→┘
                             ↓
                        COMPLETE (display results in Chat/Dashboard)
```

**Benefits:**
- No more blank screens after loading
- Clear UI for each state
- Explicit error handling with retry button
- Predictable state transitions
- Debuggable state flow

## 15. Future Roadmap

### High Priority
- [x] Dashboard parsing (extract JSON into UI cards) ✅
- [x] Conversational Follow-up Chat UI ✅
- [x] Export analysis to JSON/CSV ✅
- [ ] Analysis history (local storage)
- [ ] Multi-page full analysis (all pages, not just first)

### Medium Priority
- [ ] Batch processing (multiple PDFs)
- [ ] PDF text layer for search
- [ ] Custom prompt templates (save/load)
- [ ] Side-by-side PDF comparison

### Low Priority
- [ ] Model download manager (built-in ollama pull)
- [ ] Progress indication during Ollama processing
- [ ] Risk scoring visualization

## 15. Strict Guidelines (CRITICAL)
1. Keep the UI extremely clean, minimal, and dark-themed (think Zed or Cursor aesthetics).
2. Build feature-by-feature, test thoroughly before moving on.
3. 100% local/offline - no cloud dependencies.
4. Performance at scale - optimize for large documents.
5. Security first - auto-cleanup of temp files.
6. **DO NOT unmount the layout:** The PDF viewer and right-hand dashboard must remain visible during all states (including ANALYZING and COMPLETE) to prevent rendering crashes and blank screens. Use targeted loaders instead of replacing the whole DOM.

## 17. Testing Checklist
Before any release:

- [ ] PDF upload (single page)
- [ ] PDF upload (multi-page)
- [ ] Drag and drop
- [ ] Model selection
- [ ] Connection test
- [ ] Underwrite flow (full)
- [ ] Timeout handling
- [ ] Error states
- [ ] Prompt editing
- [ ] Settings adjustment
- [ ] Chat/Dashboard UI renders properly
- [ ] Zoom controls
- [ ] Page navigation
- [ ] Layout persists during ANALYZING state ✅
- [ ] Dashboard cards display parsed data ✅
- [ ] Follow-up chat sends questions ✅
- [ ] Export JSON works ✅
- [ ] Export CSV works ✅
- [ ] Print report works ✅

## 17. Performance Benchmarks

### Image Processing
- 3-page PDF @ 72 DPI: 3-4 seconds
- Grayscale conversion: <1 second per page
- JPEG compression: <1 second per page

### Ollama Processing
- llama3.2-vision: 5-10 minutes per page (hardware dependent)
- llava: 5-10 minutes per page (hardware dependent)
- qwen3-vl: Unstable (avoid)

### Memory Usage
- App baseline: ~150MB
- PDF viewer: +50MB per large PDF
- Ollama processing: Model-dependent (2-8GB)
