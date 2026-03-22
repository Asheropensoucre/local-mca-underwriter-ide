# Local MCA Underwriter Workspace

A blazing-fast, local-first underwriting workspace for analyzing bank statements and financial documents using local vision models — 100% offline, no cloud.

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

### Workspace Features
- ✏️ **Master Prompt Editor** - Edit the underwriting prompt
- 💾 **Prompt Persistence** - Reset to default anytime
- 📝 **AI Chat Assistant** - Conversational interface for follow-up questions and parsed data cards
- 🎨 **Premium Dark Mode UI** - Minimalist, Zed/Cursor-inspired corporate hybrid design
- ⏳ **Loading States** - Progress bar with targeted status messages (No full-page unmounting)

## How It Works

Upload PDF → PDF Viewer (Vue.js + PDF.js)Convert to Images → pdftocairo (poppler-utils)Compress → Grayscale JPEG (55-60% smaller)Send to Ollama → Base64 encoded imagesVision Model Analyzes → 30-90 secondsResponse Displayed → Dashboard / AI Chat Feed (auto-switched)
## State Machine

The app uses explicit state management for reliable UX:

IDLE ──[upload]──→ LOADING_PDF ──[done]──→ READY│↓ERROR ←──[fail]── ANALYZING ←──[underwrite]──┘│                    │└──[retry]──────────→┘↓COMPLETE (show results in Chat/Dashboard)
| State | Description |
|-------|-------------|
| `IDLE` | No PDF loaded, showing drop zone |
| `LOADING_PDF` | Processing uploaded PDF |
| `READY` | PDF loaded, ready for analysis |
| `ANALYZING` | Waiting for Ollama response (Layout remains visible) |
| `COMPLETE` | Analysis done, showing results in dashboard |
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
Arch Linux:Bashsudo pacman -S webkit2gtk gtk3 libappindicator-gtk3 librsvg poppler
Fedora:Bashsudo dnf install webkit2gtk3 gtk3 libappindicator-gtk3 librsvg2 poppler-utils
Development ToolsNode.js 18+npm or pnpmRust 1.70+ (install)Ollama SetupInstall Ollama: https://ollama.aiStart Ollama: ollama serveInstall a vision model:Bashollama pull llama3.2-vision   # Recommended
ollama pull llava             # Alternative
ollama pull qwen2.5-vl        # Advanced
InstallationBash# Clone the repository
git clone <repository-url>
cd "Open-Source Local Underwriter IDE"

# Install dependencies
npm install

# Run in development mode
npm run dev
UsageStart the app: npm run devUpload a PDF: Click the drop zone or drag & dropSelect a model: Choose from available Ollama vision modelsClick "Underwrite File": Wait 30-90 seconds for analysisView Results: Analysis appears in the AI Chat / Dashboard panel (Underwrite tab)Project Structure├── src/                          # Vue.js frontend
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
Architecture┌─────────────────────────────────────────────────────────────┐
│                 Local MCA Underwriter Workspace             │
├──────────────────────────┬──────────────────────────────────┤
│   PDF Viewer (60%)       │   Right Sidebar (40%)            │
│   ┌────────────────────┐ │  ┌────────────────────────────┐  │
│   │ [<] Page 1/5 [>]   │ │  │ [Underwrite][Prompt][⚙️]  │  │
│   │ [-] 100% [+] [Fit] │ │  ├────────────────────────────┤  │
│   ├────────────────────┤ │  │ ● Ollama Connected [Test]  │  │
│   │                    │ │  │ Model: [llama3.2-vision ▼] │  │
│   │   [PDF Rendered]   │ │  │ [Underwrite File]          │  │
│   │                    │ │  │ ┌────────────────────────┐ │  │
│   ├────────────────────┤ │  │ │ 📊 Dashboard Cards     │ │  │
│   │ [1][2][3][4][5]    │ │  │ │ 💬 Follow-up Chat      │ │  │
│   └────────────────────┘ │  │ └────────────────────────┘ │  │
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
CommandsCommandDescriptionnpm run devStart development modenpm run buildBuild for productionnpm run tauri devRun Tauri devnpm run tauri buildBuild production appConfigurationMaster Underwriting PromptLocated in the Prompt tab. Default prompt extracts:Business information (name, account, period)Financial metrics (deposits, withdrawals, balances)Risk indicators (NSF, overdrafts)Funding recommendation (APPROVE/DENY/REVIEW)Model SettingsLocated in the Settings tab:Temperature (0-1): Lower = deterministic, Higher = creativeMax Tokens: Response length (512-8192)Context Window: Model context size (4K-32K)Troubleshooting"pdftocairo not found"Bashsudo apt install poppler-utils
"Ollama is not running"Bashollama serve
Request timeoutVision models need 30-90 seconds to process imagesWait at least 2 minutes before assuming failureCheck Ollama terminal for model loading status"No models found"Bashollama list  # Check installed models
ollama pull llama3.2-vision  # Install a vision model
Blank results screenEnsure you're on the Underwrite tabCheck the chat feed for error messagesTry the Test button firstPerformance NotesImage CompressionOriginal PNG: ~145KB per pageGrayscale JPEG: ~64KB per page (55% reduction)Base64 encoded: ~85KB per pageTotal payload (1 page): ~85KB (well within HTTP limits)Processing TimePDF Conversion: 1-2 secondsImage Compression: 1-2 secondsOllama Analysis: 30-90 seconds (model dependent)Total: 35-95 seconds for 3-page PDFRoadmap[ ] Dashboard parsing (extract JSON into UI cards)[ ] Conversational Follow-up Chat UI[ ] Streaming responses (show tokens as generated)[ ] Export analysis to JSON/CSV[ ] Batch processing (multiple PDFs)[ ] PDF text layer for search[ ] Side-by-side PDF comparison[ ] Custom prompt templates[ ] Analysis history[ ] Multi-page full analysis (currently sends first page only)LicenseOpen SourceContributingContributions welcome! This is an open-source project built for the MCA underwriting community.AcknowledgmentsOllama - Local AI runtimeTauri - Desktop app frameworkPDF.js - PDF renderingpoppler-utils - PDF conversion