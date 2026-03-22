# Project: Local MCA Underwriter IDE
# Core Directive: Performance at Scale. Component-by-component development.
# Target OS: Linux Primary (Cross-platform via Tauri)

## 1. Project Vision
Build a blazing-fast, local-first underwriting IDE focused on deep-work and speed.
- Environment: 100% local, offline-first. No cloud leakage.
- Tech Stack: Rust (Backend) + Tauri (App Framework) + Vue.js (Frontend) + Tailwind CSS (Styling).
- Vibe: Dark mode, minimalist, terminal-aesthetic.

## 2. Phase 1: "Command Center" UI Scaffolding ✅ COMPLETE

**Completed Tasks:**
1. ✅ Initialized Rust + Tauri + Vue project
2. ✅ Built Empty State UI - centered drop zone with drag & drop
3. ✅ Built Active State UI - 60/40 split layout
4. ✅ Implemented native file picker (Tauri dialog API)
5. ✅ Drag-and-drop file handling

## 3. Phase 2: Ollama Integration ✅ COMPLETE

**Completed Tasks:**
1. ✅ Ollama API integration (Rust backend)
2. ✅ Connection status indicator
3. ✅ Auto-populate model selector from Ollama
4. ✅ Send PDF + prompt to vision models
5. ✅ Model configuration (temperature, tokens, context)

## 4. Phase 3: PDF Processing ✅ COMPLETE

**Completed Tasks:**
1. ✅ PDF-to-image conversion (poppler-utils/pdftocairo)
2. ✅ Multi-page PDF support
3. ✅ Page count display
4. ✅ Send all pages to vision model

## 5. Phase 4: PDF Viewer ✅ COMPLETE

**Completed Tasks:**
1. ✅ Full PDF.js viewer integration
2. ✅ Page navigation (prev/next buttons)
3. ✅ Zoom controls (50%-200%, fit)
4. ✅ Thumbnail strip for page jumping
5. ✅ Page counter display

## 6. Phase 5: IDE Features ✅ COMPLETE

**Completed Tasks:**
1. ✅ Master Underwriting Prompt editor
2. ✅ Tab navigation (Underwrite | Prompt | Settings)
3. ✅ Terminal-style output viewer
4. ✅ Prompt reset to default

## 7. Current Status

**ALL PHASES COMPLETE** - App is fully functional:
- Upload PDFs → View in full PDF viewer
- Select Ollama vision model → Send all pages
- Edit prompts → Configure model settings
- Receive analysis → View in terminal

## 8. Future Roadmap

- [ ] Streaming responses (show output as it generates)
- [ ] Export analysis to JSON/CSV
- [ ] Batch processing (multiple PDFs)
- [ ] PDF text layer for search
- [ ] Side-by-side PDF comparison
- [ ] Custom prompt templates
- [ ] Analysis history

## 9. Strict Guidelines (Still Active)
- Keep the UI extremely clean, minimal, and dark-themed (think Zed or Cursor aesthetics).
- Build feature-by-feature, test thoroughly before moving on.
- 100% local/offline - no cloud dependencies.
