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

## 8. Current Status

**PHASE 11 COMPLETE** - App now features the full MCA Underwriter Pivot with dynamic resizing and specialized dashboard cards.

### User Flow:
1. Upload PDFs → View in full PDF viewer (60% width)
2. Select Ollama vision model
3. Click "Underwrite File" → PDF converted to grayscale JPEG
4. Results displayed → Dashboard expands (70% width), showing advanced metric cards.

## 9. Key Technical Decisions

### Why Grayscale JPEG?
- Bank statements are B&W - no color info lost
- 55-60% smaller than color PNG
- Faster transmission to Ollama

### Why reqwest (not Tauri HTTP)?
- More robust timeout handling and error reporting

## 10. Current Development Hurdles (WIP - TO BE FIXED)

### Multi-Page Handling
- **Current Limitation:** Sends only FIRST page to vision model
- **Goal:** Implement sequential page analysis or multipart upload to handle full statements.

## 11. Core MCA Underwriting Logic (The AI Prompt Rules)
The application relies on highly specific prompting to extract true underwriting metrics, not just generic OCR data.

- **Position Detection:** The AI must scan debits for known MCA lenders (OnDeck, Kabbage, Fundbox, etc.). If no name is present, the AI must flag **recurring identical ACH withdrawals** (daily or weekly) as assumed positions.
- **Funding Detection:** Scan deposits for matches to debited lenders to extract "Funded Amount" and "Funded Date".
- **True Revenue Calculation:** Total monthly deposits MUST explicitly exclude incoming loan/MCA deposits to determine actual business revenue.
- **Negative Days:** Do not just count NSF fees. The AI must count the exact number of days the "Daily Ending Balance" fell below $0.00.
- **Leverage:** Calculate the total daily/weekly debt service of all existing positions to determine how much new daily payment the merchant can afford.

## 12. Future Roadmap

### High Priority (The Underwriter Pivot)
- [x] **Dynamic UI Resizing:** Animate layout from 60/40 (pre-analysis) to 30/70 (post-analysis) so the dashboard becomes the primary focus.
- [x] **Advanced JSON Parsing:** Update the UI cards to display the new "Positions" array, True Revenue, and exact Negative Days count.
- [x] **Prompt Rewrite:** Overwrite the default prompt to enforce the rules in Section 11.
- [ ] Multi-page full analysis (sequential processing)

### Medium Priority
- [ ] Batch processing (multiple PDFs)
- [ ] Custom prompt templates (save/load)
- [ ] Analysis history (local storage)

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

### Ollama Processing
- llama3.2-vision: 5-10 minutes per page (hardware dependent)
- llava: 5-10 minutes per page (hardware dependent)
- qwen2.5-vl: Best small model to use (Highly Recommended)

### Memory Usage
- App baseline: ~150MB
- PDF viewer: +50MB per large PDF
- Ollama processing: Model-dependent (2-8GB)