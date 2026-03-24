# Project: Local MCA Underwriter Workspace
# Core Directive: Performance at Scale. Component-by-component development.
# Target OS: Linux Primary (Cross-platform via Tauri)

## 1. Project Vision
Build a blazing-fast, local-first underwriting workspace focused on deep-work and speed for MCA finance professionals.
- Environment: 100% local, offline-first. No cloud leakage.
- Tech Stack: Rust (Backend) + Tauri (App Framework) + Vue.js (Frontend) + Tailwind CSS (Styling).
- Vibe: Dark mode, minimalist, Zed/Cursor-aesthetic. Corporate hybrid app (professional Fintech meets developer-speed). Use a modern AI Chat Panel.
- Core UX: Dynamic workspace. 60/40 split for setup, shifting to 30/70 split for deep data review.

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
5. ✅ Image paths returned to frontend
6. ✅ Automatic temp file cleanup

## 5. Phase 4: PDF Viewer ✅ COMPLETE → Rust-Native Image Pipeline

**Completed:**
1. ✅ **Rust-Native Image Pipeline** - Serve JPEG directly from Rust backend
2. ✅ **Lightweight Preview** - Simple <img> tag, no heavy JS PDF renderer
3. ✅ **Page 1 Preview** - Display first page as image preview
4. ✅ **Zero OOM Crashes** - Eliminated vue-pdf-embed and ArrayBuffer issues
5. ✅ **Page Count Display** - Sync with backend page count

**Architecture Shift:**
- **Removed:** vue-pdf-embed (heavy JavaScript PDF renderer)
- **Reason:** Out-Of-Memory crashes, ArrayBuffer detachments on large PDFs
- **Replaced with:** Rust-native JPEG serving via Tauri's convertFileSrc
- **Result:** 100% more stable, zero client-side PDF parsing

## 6. Phase 5: Workspace Features ✅ COMPLETE

**Completed:**
1. ✅ Master Underwriting Prompt editor
2. ✅ Tab navigation (Underwrite | Prompt | Settings)
3. ✅ Zed-style AI Chat Assistant view (Dashboard + Conversational UI)
4. ✅ Dashboard parsing (extract JSON into UI cards)
5. ✅ Export analysis to JSON/CSV & Print-friendly view
6. ✅ Auto-switch to Underwrite tab after analysis

## 7. Phase 11: The Underwriter Pivot ✅ COMPLETE

**Completed:**
1. ✅ **Dynamic UI Resizing** - Smooth 60/40 → 30/70 split animation on COMPLETE state using Tailwind `transition-all duration-500 ease-in-out`
2. ✅ **Positions Card** - Premium dark-mode table showing Lender, Payment, Frequency, Funded amounts
3. ✅ **Bank Metrics Cards** - 2x2 grid: True Revenue (excludes loans), Negative Days (red if >0), Avg Daily Balance, NSF Count
4. ✅ **Debt & Leverage Card** - Total Debt Service, Safe New Payment, Leverage Ratio
5. ✅ **MCA Prompt Rewrite** - Enforces position detection, true revenue calculation, negative days counting, leverage analysis
6. ✅ **Updated Export/Print** - CSV and print reports include new MCA data structure

## 7b. Phase 12: Chat & Prompt Architecture ✅ COMPLETE

**Completed:**
1. ✅ **Text-Only Follow-up Chat** - New `chat_with_ollama` command (instant responses, no PDF re-processing)
2. ✅ **Auto-Greeting** - Assistant welcomes user when analysis completes
3. ✅ **Cursor-Style Prompt Split** - Hardcoded SYSTEM_PROMPT + editable user instructions
4. ✅ **Anti-Hallucination Rules** - CRITICAL FALLBACK RULES prevent fake lender data on non-bank docs
5. ✅ **Generic Backend Aggregator** - Works with any document type, defers to original prompt for formatting

## 7c. Phase 13: Batch Processing ✅ COMPLETE

**Completed:**
1. ✅ **Multiple File Upload** - Select 3-6 months of bank statements at once
2. ✅ **File Queue UI** - Preview, remove individual files, clear all
3. ✅ **Sequential Batch Analysis** - Process each PDF file one at a time
4. ✅ **Combined Master Report** - All files aggregated into ONE cohesive merchant profile (MCA standard)
5. ✅ **Batch Progress Tracking** - Shows "File X of Y" during analysis

## 7d. Phase 14: Event-Driven Architecture ✅ COMPLETE

**Completed:**
1. ✅ **Disk-Based Image Storage** - JPEGs saved to temp files instead of base64 in memory
2. ✅ **Live Progress Events** - `analysis-progress` events: start, page_start, page_complete, aggregating
3. ✅ **Real-Time UI Updates** - Frontend listens and updates progress bar live
4. ✅ **Temp File Cleanup** - Images deleted immediately after each page to free disk space
5. ✅ **Scales to 50+ Pages** - No more IPC memory limits or silent timeouts

## 7e. Phase 15: Stability & JSON Enforcement ✅ COMPLETE

**Completed:**
1. ✅ **Strict JSON Output** - Added `format: "json"` to Ollama requests to prevent array truncation
2. ✅ **Temp Folder Permissions** - Added fs:allow-temp-read for convertFileSrc to work
3. ✅ **Arc-Based TempDir Management** - Temp files persist until cleanup_temp_images() called
4. ✅ **Preview Image Loading** - Fixed race condition where files were deleted before frontend could load
5. ✅ **Bundle Size Reduction** - Removed vue-pdf-embed (2.6MB → 111KB, 95% smaller)

## 7f. Phase 16: Data URI Preview + Garbage Collector ✅ COMPLETE

**Completed:**
1. ✅ **Data URI Preview** - Rust generates `data:image/jpeg;base64,...` URI from page-1.jpg
2. ✅ **Asset Protocol Bypass** - No CSP configuration needed, no symlink issues
3. ✅ **Garbage Collector** - `cleanup_temp_files()` command deletes temp directories
4. ✅ **Auto-Cleanup on Close** - Temp files deleted when app unmounts
5. ✅ **Security Hardening** - Sensitive financial PDFs no longer linger in /tmp

## 7g. Phase 17: JSON Merger & Prompt Cleanup ✅ COMPLETE

**Completed:**
1. ✅ **No More Echo** - Aggregator stops passing original prompt to prevent AI from echoing instructions back
2. ✅ **Strict JSON Merger** - New aggregation prompt with 6 critical rules for pure JSON combining
3. ✅ **Merchant Definition** - Explicit rule: merchant is NEVER the bank name (Chase, BoA, LendingClub)
4. ✅ **Removed Notes Field** - Eliminated `notes` field to prevent AI hallucination
5. ✅ **Placeholder Format** - Changed prompt to use `REPLACE_WITH_...` style for clearer extraction

## 7h. Phase 18: Custom Prompt Templates ✅ COMPLETE

**Completed:**
1. ✅ **Rust Backend Commands** - `get_templates`, `save_template`, `delete_template` using Tauri IPC
2. ✅ **OS App Data Storage** - Templates saved to `templates.json` in native OS app data directory
3. ✅ **Template Manager UI** - Dropdown selector, save/delete buttons in Prompt tab
4. ✅ **Enterprise Architecture** - All file I/O handled by Rust (no browser LocalStorage)
5. ✅ **Auto-Load on Mount** - Templates load automatically when app starts

## 7i. Phase 19: Analysis History ✅ COMPLETE

**Completed:**
1. ✅ **HistoryEntry Struct** - id, timestamp, file_name, merchant_name, risk_score, parsed_data
2. ✅ **Rust Backend Commands** - `get_history`, `save_history_entry`, `delete_history_entry`, `clear_all_history`
3. ✅ **OS App Data Storage** - History saved to `analysis_history.json` in native OS app data directory
4. ✅ **History Tab UI** - List view with date, file, merchant, risk score for each entry
5. ✅ **Instant Reload** - Load button restores dashboard without re-running AI analysis
6. ✅ **Auto-Save** - Every analysis automatically saved on completion
7. ✅ **Added chrono** - RFC3339 timestamp support for cross-platform compatibility

## 7j. Phase 20: UI Refactor - Edge-to-Edge IDE Layout ✅ COMPLETE

**Completed:**
1. ✅ **Full-Screen Shell** - Root div uses h-screen w-screen flex flex-row overflow-hidden
2. ✅ **Removed Card Look** - Eliminated max-w-7xl, h-[80vh], rounded-xl, gap-4, p-8
3. ✅ **Fluid Right Sidebar** - flex-1 h-full min-w-0 border-l (expands to fill remaining space)
4. ✅ **Dynamic Left Pane** - PDF viewer still animates 60% → 30% on COMPLETE state
5. ✅ **Clean Dividers** - border-r between panes, border-l on right sidebar
6. ✅ **IDLE State Centered** - Drop zone uses m-auto to center in edge-to-edge layout
7. ✅ **Functional Logic Preserved** - All v-if, v-model, @click bindings unchanged

## 7k. Phase 21: Full Streaming for Thinking Models ✅ COMPLETE

**Completed:**
1. ✅ **Streaming Architecture** - Added OllamaStreamChunk and OllamaStreamMessage structs
2. ✅ **Live Thought Streaming** - analyze_single_page() emits stream-thought events in real-time
3. ✅ **Live Chat Streaming** - chat_with_ollama() emits stream-thought and stream-token events
4. ✅ **Extended Timeouts** - 60 minutes for thinking models (qwen3, deepseek, o1, r1)
5. ✅ **Regular Timeout** - 15 minutes for non-thinking vision models
6. ✅ **Frontend Listeners** - stream-thought listener accumulates thinking live in UI
7. ✅ **No More Timeout Anxiety** - Users see thinking progress in real-time, know system is working

**Architecture:**
- **Before:** Non-streaming, timeout after 10 minutes, no feedback during thinking
- **After:** Full streaming, 60-minute timeout, live thought display shows AI is working
- **Events:** stream-thought (live thinking), stream-token (live response tokens)
- **Works For:** PDF analysis (single + multi-page) and follow-up chat

## 8. Current Status

**ALL PHASES COMPLETE (1-21)** - App features full MCA Underwriter Pivot, multi-page analysis, text-only chat, batch processing, event-driven architecture, strict JSON enforcement, Data URI preview, automatic garbage collection, clean JSON merger without echo, custom prompt templates, analysis history with Rust persistence, edge-to-edge IDE-style layout, and **live streaming thoughts for thinking models with 60-minute timeout**.

### User Flow:
1. Upload PDFs → View in full PDF viewer (left pane, 60% width initially)
2. Select Ollama vision model in right sidebar (fluid width, fills remaining space)
3. Click "Underwrite File" → PDF converted to grayscale JPEG
4. Results displayed → Left pane shrinks to 30% width, right sidebar expands to 70%
5. All tabs (Underwrite, Prompt, Settings, History) accessible in fluid right sidebar

## 9. Key Technical Decisions

### Why Grayscale JPEG?
- Bank statements are B&W - no color info lost
- 55-60% smaller than color PNG
- Faster transmission to Ollama

### Why reqwest (not Tauri HTTP)?
- More robust timeout handling and error reporting

## 10. Current Development Hurdles (WIP - TO BE FIXED)

### Multi-Page Handling
- **Status:** ✅ RESOLVED - Sequential page analysis implemented
- **Implementation:** Each page analyzed individually, then aggregated into final JSON
- **Timeout:** 5 minutes per page + aggregation pass

## 11. Core MCA Underwriting Logic (The AI Prompt Rules)
The application relies on highly specific prompting to extract true underwriting metrics, not just generic OCR data.

- **Position Detection:** The AI must scan debits for known MCA lenders (OnDeck, Kabbage, Fundbox, etc.). If no name is present, the AI must flag **recurring identical ACH withdrawals** (daily or weekly) as assumed positions.
- **Funding Detection:** Scan deposits for matches to debited lenders to extract "Funded Amount" and "Funded Date".
- **True Revenue Calculation:** Total monthly deposits MUST explicitly exclude incoming loan/MCA deposits to determine actual business revenue.
- **Negative Days:** Do not just count NSF fees. The AI must count the exact number of days the "Daily Ending Balance" fell below $0.00.
- **Leverage:** Calculate the total daily/weekly debt service of all existing positions to determine how much new daily payment the merchant can afford.

## 12. Future Roadmap

### High Priority (The Underwriter Pivot) ✅ ALL COMPLETE
- [x] **Dynamic UI Resizing:** Animate layout from 60/40 (pre-analysis) to 30/70 (post-analysis) so the dashboard becomes the primary focus.
- [x] **Advanced JSON Parsing:** Update the UI cards to display the new "Positions" array, True Revenue, and exact Negative Days count.
- [x] **Prompt Rewrite:** Overwrite the default prompt to enforce the rules in Section 11.
- [x] **Multi-page full analysis (sequential processing)** - Each page analyzed individually, results aggregated
- [x] **JSON Merger (Phase 17)** - Aggregator uses strict JSON merge rules, no echo of original prompt

### Medium Priority ✅ ALL COMPLETE
- [x] **Batch processing (multiple PDFs)** - Upload and analyze multiple bank statements in one session
- [x] **Custom prompt templates (save/load)** - Save custom underwriting templates for different deal types
- [x] **Analysis history (local storage)** - Store past analyses locally for quick reference

### Low Priority
- [ ] Streaming responses (show tokens as generated)

## 13. Strict Guidelines (CRITICAL)

- Keep the UI extremely clean, minimal, and dark-themed (think Zed or Cursor aesthetics).
- Build feature-by-feature, test thoroughly before moving on.
- 100% local/offline - no cloud dependencies.
- **Dynamic Layout Constraint:** The PDF viewer and right-hand dashboard must remain visible during all states. Use CSS transitions (e.g., `transition-all duration-300`) to resize the panels smoothly when changing states, rather than unmounting the DOM.

## 14. Testing Checklist

Before any release:
- [x] Layout persists during ANALYZING state ✅
- [x] Dashboard cards display parsed data ✅
- [x] Export JSON/CSV & Print works ✅
- [x] UI smoothly transitions to 30/70 split upon completion ✅
- [x] Dashboard successfully renders multiple existing positions ✅

## 15. Performance Benchmarks

### Image Processing
- 3-page PDF @ 72 DPI: 3-4 seconds
- Grayscale conversion: <1 second per page
- JPEG compression: <1 second per page
- **Disk-based storage:** No memory limits on large PDFs

### Ollama Processing
- llama3.2-vision 
- llava
- qwen2.5-vl
- qwen3-vl

### Thinking Model Support (Qwen3-VL, DeepSeek-R1, O1, R1)

**Phase 21: Full Streaming Implementation:**
- **Streaming API:** Uses Ollama streaming to capture thinking in real-time
- **Thinking Field Extraction:** Captures `thinking` field from streaming chunks
- **Live Thoughts Display:** Users see AI reasoning IN REAL-TIME during analysis
- **Purple "AI Thinking Process" Panel:** Shows thoughts streaming live
- **Test Result Display:** Test button shows both thoughts and response in UI
- **Auto-Enable Toggle:** Thoughts toggle auto-enables for thinking models
- **User Control:** Toggle to show/hide thoughts panel, persisted to localStorage
- **Applied To:** `analyze_single_page`, `chat_with_ollama`, `test_ollama_model`
- **Timeout:** 3600s (60 minutes) for thinking models - no more premature timeouts!
- **Regular Models:** 900s (15 minutes) timeout for non-thinking vision models
- **Events:** stream-thought (live thinking), stream-token (live response)

### Recent Features (Latest - Phase 21)
- **Live Thought Streaming:** Thoughts display in real-time as AI thinks
- **Extended Timeouts:** 60 minutes for thinking models (qwen3, deepseek, o1, r1)
- **Chat Streaming:** Follow-up chat also streams thoughts and tokens live
- **No Timeout Anxiety:** Users see progress, know the system is still working

### Memory Usage
- App baseline: ~150MB
- PDF viewer: +50MB per large PDF
- Ollama processing: Model-dependent (2-30GB) 'or higher untesed'
- **Event-driven architecture:** Scales to 50+ pages safely