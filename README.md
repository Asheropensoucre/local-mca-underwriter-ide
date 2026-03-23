# Local MCA Underwriter Workspace

A blazing-fast, local-first underwriting workspace built specifically for the Merchant Cash Advance (MCA) industry. Analyzes bank statements using local vision models to extract true revenue, detect competing positions, and calculate leverage — 100% offline, no cloud.

![License](https://img.shields.io/badge/license-Open%20Source-blue)
![Tauri](https://img.shields.io/badge/Tauri-v2.0-blue)
![Rust](https://img.shields.io/badge/Rust-1.70+-orange)
![Vue](https://img.shields.io/badge/Vue-3.x-green)

## Features

### Advanced MCA Underwriting Logic
- 🏦 **Position Detection** - Identifies known MCA lenders (OnDeck, Kabbage, etc.) and flags recurring daily/weekly ACH withdrawals as assumed positions.
- 💰 **True Revenue Calculation** - Excludes incoming loan and advance deposits to determine true business revenue.
- 📉 **Negative Days Analysis** - Accurately counts days where the "Daily Ending Balance" fell below $0.00 (not just NSF fee occurrences).
- ⚖️ **Leverage & Debt Service** - Calculates total daily/weekly debt service to determine safe new payment thresholds.

### PDF Analysis & Workspace
- 📄 **Rust-Native Image Pipeline** - Serve JPEG previews directly from Rust backend
- 🖼️ **Lightweight Preview** - Simple <img> tag, zero JavaScript PDF rendering
- 📊 **Page Count Display** - Sync with backend page count
- 🖼️ **Grayscale JPEG Conversion** - 55-60% compression for faster local processing
- 🎨 **Dynamic UI Resizing** - Starts at a 60/40 split for PDF reading, dynamically animating to a 30/70 split when analysis completes to give the Dashboard maximum space.

**Architecture Note:** We removed vue-pdf-embed (heavy JavaScript PDF renderer) to eliminate Out-Of-Memory crashes and ArrayBuffer detachments. The Rust backend already generates JPEGs for Ollama - we now serve Page 1 directly to the frontend via Tauri's convertFileSrc. Result: 100% more stable, zero client-side PDF parsing.

### AI Integration
- 🤖 **Ollama Integration** - Connect to local vision models (100% offline)
- 📡 **Connection Status** - Real-time indicator with test button
- 🧠 **Vision Model Support** - Optimized for **Qwen 2.5-VL** (Highly Recommended), llama3.2-vision, llava
- 📝 **AI Chat Assistant** - Conversational interface for follow-up questions and parsed data cards

## How It Works

```
1. Upload PDF(s) → Rust converts to JPEGs (60% Width)
   - Single file or batch (3-6 months of statements)
   - Page 1 JPEG served as preview via convertFileSrc
2. Convert to Images → pdftocairo (poppler-utils, 72 DPI)
3. Compress → Grayscale JPEG (saved to disk, not memory)
4. Read & Encode → Each JPEG read from disk, encoded to Base64
5. Send to Ollama → Base64 images with format: "json" enforced
6. Vision Model Analyzes → 5-10 minutes per page (hardware dependent)
7. Live Progress Events → UI updates in real-time ("Page 3 of 9...")
8. Multi-page Processing → Each page analyzed sequentially
9. Result Aggregation → All page findings combined into final JSON
10. Temp File Cleanup → Images deleted immediately after use
11. Dynamic UI Shift → Dashboard expands to 70% width
12. Results Displayed → MCA Data Cards + Follow-up Chat
```

## State Machine

The app uses explicit state management for reliable UX. The main layout never unmounts, preventing render crashes.

```
IDLE ──[upload]──→ LOADING_PDF ──[done]──→ READY
                                              │
                                              ↓
ERROR ←──[fail]── ANALYZING ←──[underwrite]──┘
  │                    │
  └──[retry]──────────→┘
                        ↓
                    COMPLETE (UI transitions to 30/70 split)
```

## Prerequisites

### System Dependencies

**Ubuntu/Debian:**
```bash
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  poppler-utils
```

### Development Tools
- **Node.js** 18+
- **npm** or **pnpm**
- **Rust** 1.70+ ([install](https://rustup.rs/))

### Ollama Setup
1. Install Ollama: https://ollama.ai
2. Start Ollama: `ollama serve`
3. Install a vision model:
   ```bash
   ollama pull qwen2.5-vl        # Best small model (Highly Recommended)
   ollama pull llama3.2-vision   # Alternative
   ```

## Installation

```bash
# Clone the repository
git clone https://github.com/Asheropensoucre/local-mca-underwriter-ide

cd "local-mca-underwriter-ide"

# Install dependencies
npm install

# Run in development mode
npm run dev
```

## Troubleshooting

### "pdftocairo not found"
```bash
sudo apt install poppler-utils
```

### Request timeout
- Vision models need 5-10 minutes to process images depending on your hardware.
- Wait at least 15 minutes before assuming failure. Check Ollama terminal for model loading status.

### Blank results screen / Missing Data
- Ensure you're on the Underwrite tab. Check the chat feed for error messages.
- Verify your prompt includes the strict MCA extraction rules.

## Performance Notes

### Image Compression
- Original PNG: ~145KB per page
- Grayscale JPEG: ~64KB per page (55% reduction)
- Base64 encoded: ~85KB per page

### Processing Time
- PDF Conversion: 1-2 seconds
- Ollama Analysis: 5-10 minutes per page (hardware dependent)
- Aggregation Pass: ~1 minute
- **Total:** ~5-10 minutes per page (e.g., 3-page PDF = 15-30 minutes)
- **Event-Driven:** Live progress updates, no silent timeouts

### Scalability
- **Disk-based images:** No memory limits on large PDFs
- **Temp file cleanup:** Immediate deletion after each page
- **Tested up to:** 50+ pages safely

### AI Prompt Improvements
- **No Echo/Duplication:** Aggregator no longer sends original prompt, preventing AI from echoing instructions
- **Pure JSON Merge:** Aggregator uses strict rules to combine page results into one flat JSON object
- **Merchant Definition:** Explicit rule that merchant is NEVER the bank name (e.g., ignore "Chase", "Bank of America")
- **No Notes Field:** Removed `notes` field to prevent AI from filling it with garbage text

### Recent Improvements (Phase 16)
- **Data URI Preview:** Rust generates `data:image/jpeg;base64,...` - no CSP issues
- **Garbage Collector:** Auto-deletes temp files on app close
- **Security:** Sensitive PDFs no longer linger in /tmp
- **Asset Protocol Bypass:** No symlink routing problems

### Recent Improvements (Phase 17)
- **No More Echo:** Aggregator stops passing original prompt to prevent AI from echoing instructions
- **Strict JSON Merger:** Aggregator now acts as a pure JSON combiner with strict rules
- **Cleaner Prompt:** Removed `notes` field, added explicit merchant vs bank distinction
- **Anti-Hallucination:** Explicit ban on guessing bank names (Chase, BoA) as merchants

### Recent Improvements (Phase 18)
- **Custom Prompt Templates:** Save and load custom underwriting instructions
- **Rust Persistence:** Templates stored in OS app data directory (not LocalStorage)
- **Template Manager UI:** Dropdown selector, save/delete buttons in Prompt tab
- **Enterprise Architecture:** All file I/O handled by Rust backend

### Recent Improvements (Phase 19)
- **Analysis History:** View and reload past analyses instantly
- **Rust Persistence:** History stored in OS app data directory (analysis_history.json)
- **History Tab UI:** List of past runs with date, file, merchant, risk score
- **Auto-Save:** Every analysis automatically saved to history on completion
- **Instant Reload:** Load button restores dashboard without re-running AI

### Recent Improvements (Phase 15)
- **Strict JSON Output:** `format: "json"` prevents AI from truncating arrays
- **Temp Permissions:** fs:allow-temp-read enables convertFileSrc to work
- **Arc-Based TempDir:** Files persist until cleanup_temp_images() called
- **Bundle Size:** 95% smaller (2.6MB → 111KB) after removing vue-pdf-embed

## Roadmap

### High Priority (The Underwriter Pivot) ✅ ALL COMPLETE
- [x] **Dynamic UI Resizing:** Animate layout from 60/40 (pre-analysis) to 30/70 (post-analysis).
- [x] **Advanced JSON Parsing:** UI cards for Positions, True Revenue, and Negative Days.
- [x] **Prompt Rewrite:** Overwrite default prompt for strict MCA logic extraction.
- [x] **Multi-page full analysis** - Sequential page processing with result aggregation

### Medium Priority
- [x] **Batch processing (multiple PDFs)** - Upload and analyze multiple bank statements in one session ✅ COMPLETE
- [x] **Custom prompt templates (save/load)** - Save custom underwriting templates for different deal types ✅ COMPLETE
- [x] **Analysis history (local storage)** - Store past analyses locally for quick reference ✅ COMPLETE

### Low Priority
- [ ] Streaming responses (show tokens as generated)

## License

Open Source

## Contributing

Contributions welcome! This is an open-source project built for the MCA underwriting community.

## Acknowledgments

- **Ollama** - Local AI runtime
- **Tauri** - Desktop app framework
- **PDF.js** - PDF rendering
- **poppler-utils** - PDF conversion
