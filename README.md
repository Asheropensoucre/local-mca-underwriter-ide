# Local MCA Underwriter IDE

A blazing-fast, local-first underwriting IDE for analyzing bank statements and financial documents using local vision models — 100% offline, no cloud.

![License](https://img.shields.io/badge/license-Open%20Source-blue)
![Tauri](https://img.shields.io/badge/Tauri-v2.0-blue)
![Rust](https://img.shields.io/badge/Rust-1.70+-orange)
![Vue](https://img.shields.io/badge/Vue-3.x-green)

## Features

### PDF Analysis
- 📄 **Full PDF Viewer** - Multi-page navigation with PDF.js
- 🔄 **Page Navigation** - Previous/Next buttons, page counter, thumbnail strip
- 🔍 **Zoom Controls** - 50%-200% zoom, fit-to-width
- 🖼️ **Grayscale JPEG Conversion** - 55-60% compression for faster processing

### AI Integration
- 🤖 **Ollama Integration** - Connect to local vision models
- 📡 **Connection Status** - Real-time indicator with test button
- 📋 **Model Selector** - Auto-populates with installed Ollama models
- 🧠 **Vision Model Support** - llama3.2-vision, llava, qwen-vl
- ⚙️ **Model Configuration** - Temperature, max tokens, context window

### IDE Features
- ✏️ **Master Prompt Editor** - Edit the underwriting prompt
- 💾 **Prompt Persistence** - Reset to default anytime
- 📝 **Terminal Output** - View model responses with success/error states
- 🎨 **Dark Mode UI** - Minimalist, terminal-aesthetic design
- ⏳ **Loading States** - Progress bar with status messages

## How It Works

```
1. Upload PDF → PDF Viewer (Vue.js + PDF.js)
2. Convert to Images → pdftocairo (poppler-utils)
3. Compress → Grayscale JPEG (55-60% smaller)
4. Send to Ollama → Base64 encoded images
5. Vision Model Analyzes → 30-90 seconds
6. Response Displayed → Terminal panel (auto-switched)
```

## State Machine

The app uses explicit state management for reliable UX:

```
IDLE ──[upload]──→ LOADING_PDF ──[done]──→ READY
                                              │
                                              ↓
ERROR ←──[fail]── ANALYZING ←──[underwrite]──┘
  │                    │
  └──[retry]──────────→┘
                        ↓
                    COMPLETE (show results)
```

| State | Description |
|-------|-------------|
| `IDLE` | No PDF loaded, showing drop zone |
| `LOADING_PDF` | Processing uploaded PDF |
| `READY` | PDF loaded, ready for analysis |
| `ANALYZING` | Waiting for Ollama response |
| `COMPLETE` | Analysis done, showing results |
| `ERROR` | Analysis failed, can retry |

## Tech Stack

| Layer | Technology |
|-------|------------|
| Backend | Rust + tokio |
| App Framework | Tauri v2 |
| Frontend | Vue.js 3 + Vite |
| Styling | Tailwind CSS |
| PDF Rendering | PDF.js + vue-pdf-embed |
| PDF Conversion | poppler-utils (pdftocairo) |
| Image Processing | image crate (grayscale + JPEG) |
| HTTP Client | reqwest |
| AI Runtime | Ollama (local) |

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

**Arch Linux:**
```bash
sudo pacman -S webkit2gtk gtk3 libappindicator-gtk3 librsvg poppler
```

**Fedora:**
```bash
sudo dnf install webkit2gtk3 gtk3 libappindicator-gtk3 librsvg2 poppler-utils
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
   ollama pull llama3.2-vision   # Recommended
   ollama pull llava             # Alternative
   ollama pull qwen2.5-vl        # Advanced
   ```

## Installation

```bash
# Clone the repository
git clone <repository-url>
cd "Open-Source Local Underwriter IDE"

# Install dependencies
npm install

# Run in development mode
npm run dev
```

## Usage

1. **Start the app:** `npm run dev`
2. **Upload a PDF:** Click the drop zone or drag & drop
3. **Select a model:** Choose from available Ollama vision models
4. **Click "Underwrite File":** Wait 30-90 seconds for analysis
5. **View Results:** Analysis appears in terminal panel (Underwrite tab)

## Project Structure

```
├── src/                          # Vue.js frontend
│   ├── components/
│   │   └── PdfViewer.vue        # PDF viewer with navigation
│   ├── App.vue                  # Main application component
│   ├── main.js                  # Vue entry point
│   └── style.css                # Global styles + Tailwind
├── src-tauri/                    # Rust backend
│   ├── capabilities/
│   │   └── main-capability.json # Tauri permissions
│   ├── src/
│   │   ├── main.rs              # Tauri app + Ollama integration
│   │   └── ollama.rs            # Ollama API types
│   ├── Cargo.toml               # Rust dependencies
│   └── tauri.conf.json          # Tauri configuration
├── index.html                    # HTML entry point
├── vite.config.js                # Vite configuration
├── tailwind.config.js            # Tailwind configuration
├── postcss.config.js             # PostCSS configuration
└── package.json                  # Node.js dependencies
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Local MCA Underwriter IDE                 │
├──────────────────────────┬──────────────────────────────────┤
│   PDF Viewer (60%)       │   Right Sidebar (40%)            │
│   ┌────────────────────┐ │  ┌────────────────────────────┐  │
│   │ [<] Page 1/5 [>]   │ │  │ [Underwrite][Prompt][⚙️]  │  │
│   │ [-] 100% [+] [Fit] │ │  ├────────────────────────────┤  │
│   ├────────────────────┤ │  │ ● Ollama Connected [Test]  │  │
│   │                    │ │  │ Model: [llama3.2-vision ▼] │  │
│   │   [PDF Rendered]   │ │  │ [Underwrite File]           │  │
│   │                    │ │  │ ┌────────────────────────┐  │  │
│   ├────────────────────┤ │  │ │ Output                 │  │  │
│   │ [1][2][3][4][5]    │ │  │ │ ✅ Response...         │  │  │
│   └────────────────────┘ │  │ └────────────────────────┘  │  │
└──────────────────────────┴──────────────────────────────────┘
                              │
                              ▼
                    ┌─────────────────────┐
                    │   Ollama (Local)    │
                    │   - PDF → JPEG      │
                    │   - Grayscale       │
                    │   - Base64 encode   │
                    │   - Vision analysis │
                    └─────────────────────┘
```

## Commands

| Command | Description |
|---------|-------------|
| `npm run dev` | Start development mode |
| `npm run build` | Build for production |
| `npm run tauri dev` | Run Tauri dev |
| `npm run tauri build` | Build production app |

## Configuration

### Master Underwriting Prompt

Located in the **Prompt** tab. Default prompt extracts:
- Business information (name, account, period)
- Financial metrics (deposits, withdrawals, balances)
- Risk indicators (NSF, overdrafts)
- Funding recommendation (APPROVE/DENY/REVIEW)

### Model Settings

Located in the **Settings** tab:
- **Temperature** (0-1): Lower = deterministic, Higher = creative
- **Max Tokens**: Response length (512-8192)
- **Context Window**: Model context size (4K-32K)

## Troubleshooting

### "pdftocairo not found"
```bash
sudo apt install poppler-utils
```

### "Ollama is not running"
```bash
ollama serve
```

### Request timeout
- Vision models need 30-90 seconds to process images
- Wait at least 2 minutes before assuming failure
- Check Ollama terminal for model loading status

### "No models found"
```bash
ollama list  # Check installed models
ollama pull llama3.2-vision  # Install a vision model
```

### Blank results screen
- Ensure you're on the **Underwrite** tab
- Check terminal for error messages
- Try the **Test** button first

## Performance Notes

### Image Compression
- **Original PNG:** ~145KB per page
- **Grayscale JPEG:** ~64KB per page (55% reduction)
- **Base64 encoded:** ~85KB per page
- **Total payload (1 page):** ~85KB (well within HTTP limits)

### Processing Time
- **PDF Conversion:** 1-2 seconds
- **Image Compression:** 1-2 seconds
- **Ollama Analysis:** 30-90 seconds (model dependent)
- **Total:** 35-95 seconds for 3-page PDF

## Roadmap

- [ ] Streaming responses (show tokens as generated)
- [ ] Export analysis to JSON/CSV
- [ ] Batch processing (multiple PDFs)
- [ ] PDF text layer for search
- [ ] Side-by-side PDF comparison
- [ ] Custom prompt templates
- [ ] Analysis history
- [ ] Multi-page full analysis (currently sends first page only)

## License

Open Source

## Contributing

Contributions welcome! This is an open-source project built for the MCA underwriting community.

## Acknowledgments

- **Ollama** - Local AI runtime
- **Tauri** - Desktop app framework
- **PDF.js** - PDF rendering
- **poppler-utils** - PDF conversion
