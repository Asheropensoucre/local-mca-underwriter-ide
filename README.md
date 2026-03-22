# Local MCA Underwriter IDE

A blazing-fast, local-first underwriting IDE focused on deep-work and speed. Analyze bank statements using local vision models — 100% offline, no cloud.

![License](https://img.shields.io/badge/license-Open%20Source-blue)
![Tauri](https://img.shields.io/badge/Tauri-v2.0-blue)
![Rust](https://img.shields.io/badge/Rust-1.70+-orange)
![Vue](https://img.shields.io/badge/Vue-3.x-green)

## Features

### PDF Analysis
- 📄 **Full PDF Viewer** - Multi-page navigation, zoom controls, thumbnail strip
- 🔄 **Page Navigation** - Previous/Next buttons, page counter, click thumbnails
- 🔍 **Zoom Controls** - 50%-200% zoom, fit-to-width option
- 📊 **Auto Page Count** - Displays total pages in header

### AI Integration
- 🤖 **Ollama Integration** - Connect to local vision models (llava, llama3-vision, qwen-vl)
- 📡 **Connection Status** - Real-time indicator showing Ollama connection
- 📋 **Model Selector** - Auto-populates with installed Ollama models
- 🧠 **Multi-Page Analysis** - Sends all PDF pages to vision model at once
- ⚙️ **Model Configuration** - Temperature, max tokens, context window controls

### IDE Features
- ✏️ **Master Prompt Editor** - Edit the underwriting prompt that drives analysis
- 💾 **Prompt Persistence** - Reset to default prompt anytime
- 📝 **Terminal Output** - View model responses in styled terminal panel
- 🎨 **Dark Mode UI** - Minimalist, terminal-aesthetic design

## Tech Stack

| Layer | Technology |
|-------|------------|
| Backend | Rust |
| App Framework | Tauri v2 |
| Frontend | Vue.js 3 + Vite |
| Styling | Tailwind CSS |
| PDF Rendering | PDF.js + vue-pdf-embed |
| PDF Conversion | poppler-utils (pdftocairo) |
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
   ollama pull llava
   # or
   ollama pull llama3-vision
   # or  
   ollama pull qwen-vl
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
2. **Upload a PDF:** Click the drop zone or drag & drop a bank statement
3. **Select a model:** Choose from available Ollama vision models
4. **Configure (optional):** Adjust temperature, tokens in Settings tab
5. **Edit prompt (optional):** Customize the Master Underwriting Prompt
6. **Click "Underwrite File":** Analysis appears in terminal output

## Project Structure

```
├── src/                          # Vue.js frontend
│   ├── components/
│   │   └── PdfViewer.vue        # PDF viewer component
│   ├── App.vue                  # Main application component
│   ├── main.js                  # Vue entry point
│   └── style.css                # Global styles + Tailwind
├── src-tauri/                    # Rust backend
│   ├── capabilities/
│   │   └── main-capability.json # Tauri permissions
│   ├── src/
│   │   ├── main.rs              # Tauri app entry + Ollama integration
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
│   ├────────────────────┤ │  │ ● Ollama Connected          │  │
│   │                    │ │  │ Model: [llava ▼]            │  │
│   │   [PDF Rendered]   │ │  │ [Underwrite File]           │  │
│   │                    │ │  │ ┌────────────────────────┐  │  │
│   ├────────────────────┤ │  │ │ Output                 │  │  │
│   │ [1][2][3][4][5]    │ │  │ │ { JSON analysis... }   │  │  │
│   └────────────────────┘ │  │ └────────────────────────┘  │  │
└──────────────────────────┴──────────────────────────────────┘
                              │
                              ▼
                    ┌─────────────────────┐
                    │   Ollama (Local)    │
                    │   - llava           │
                    │   - llama3-vision   │
                    │   - qwen-vl         │
                    └─────────────────────┘
```

## Commands

| Command | Description |
|---------|-------------|
| `npm run dev` | Start development mode (Vite + Tauri) |
| `npm run build` | Build for production |
| `npm run tauri dev` | Run Tauri dev (same as npm run dev) |
| `npm run tauri build` | Build production desktop app |

## Configuration

### Master Underwriting Prompt

Located in the **Prompt** tab. Default prompt extracts:
- Business information
- Financial metrics (deposits, withdrawals, balances)
- Risk indicators (NSF, overdrafts)
- Funding recommendation

### Model Settings

Located in the **Settings** tab:
- **Temperature** (0-1): Lower = deterministic, Higher = creative
- **Max Tokens**: Response length limit
- **Context Window**: Model context size (4K-32K)

## Troubleshooting

### "pdftocairo not found"
Install poppler-utils for your system (see Prerequisites).

### "Ollama is not running"
1. Start Ollama: `ollama serve`
2. Install a vision model: `ollama pull llava`

### "No models found"
1. Ensure Ollama is running
2. Install a vision model: `ollama pull llava`
3. Restart the app

### PDF only shows first page
Check that the page count displays correctly in the header. If it shows "1 page" for a multi-page PDF, try re-uploading the file.

## Roadmap

- [ ] Streaming responses (show output as it generates)
- [ ] Export analysis to JSON/CSV
- [ ] Batch processing (multiple PDFs)
- [ ] PDF text layer for search
- [ ] Side-by-side PDF comparison
- [ ] Custom prompt templates
- [ ] Analysis history

## License

Open Source

## Contributing

Contributions welcome! This is an open-source project built for the MCA underwriting community.
