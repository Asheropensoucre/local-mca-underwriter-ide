# Local MCA Underwriter Workspace

A local-first underwriting workspace for the Merchant Cash Advance (MCA) industry. Reads bank statements on your own machine, computes the numbers an underwriter needs, and asks a small local model only for the judgment calls. 100% offline after the first-run download. No cloud, no account.

![License](https://img.shields.io/badge/license-MIT-blue)
![Tauri](https://img.shields.io/badge/Tauri-v2.0-blue)
![Rust](https://img.shields.io/badge/Rust-1.70+-orange)
![Vue](https://img.shields.io/badge/Vue-3.x-green)

## How it works

```
PDF statement(s)
  per page ─ digital page? ── yes ─▶ PDF text layer (exact, instant)
            └─ scanned page ── no ──▶ render at 150 DPI ─▶ GLM-OCR ─▶ page text
  ledger.rs (deterministic) ─▶ every transaction line, statement summary figures,
                               daily balances, NSF items, recurring debits, funding candidates
  reasoning model (one call, JSON-schema constrained) ─▶ which recurring debits are positions,
                               which deposits are borrowed money, merchant identity, risk, notes
  report assembled in Rust ─▶ true revenue, negative days, NSF count, debt service, leverage,
                               plus a verification block (stated vs parsed totals, sources)
```

The model never produces a dollar figure. Every number in the dashboard is computed from parsed lines and checked against the totals the bank printed on the statement.

## The engine

The app manages its own inference runtime. On first launch it shows a setup screen that downloads, with resumable and checksummed downloads:

| Component | Source | Size |
|---|---|---|
| llama.cpp `llama-server` (pinned release, Vulkan on Windows/Linux, Metal on macOS) | github.com/ggml-org/llama.cpp | 11 to 32 MB |
| GLM-OCR (0.9B, Q8_0 + mmproj), reads scanned pages | huggingface.co/ggml-org/GLM-OCR-GGUF | 1.4 GB |
| Qwen3.5 4B (Q4_K_M), classification and notes | huggingface.co/unsloth/Qwen3.5-4B-GGUF | 2.7 GB |
| Qwen3.5 9B (Q4_K_M), optional, needs 16 GB RAM | huggingface.co/unsloth/Qwen3.5-9B-GGUF | 5.7 GB |

Files land in the OS app-data directory under `engine/`. `llama-server` runs in router mode on a random loopback port with a per-launch API key, is started with the app and stopped with it (pid file, signal handlers, stale-process reaping at start). Integrated GPUs work through Vulkan; if the GPU build fails to start, the setup screen offers the CPU build.

Registry of pinned files: `src-tauri/src/engine/registry.rs`.

## Features

- Batch: several months of statements are analyzed as one job and one report.
- Positions: recurring debits detected by payee and amount, cadence (daily, weekly, biweekly, monthly) derived from dates; the model names the lender and rejects vendors, payroll, taxes, floor plan and transfers.
- True revenue: statement total credits minus deposits the model confirms as borrowed money. Only credits whose description carries loan/funding wording can be excluded; large unlabeled deposits are listed for verification instead.
- Negative days from the daily balance table when the statement has one, otherwise from the printed minimum balance, with the source shown.
- Verification block on every report: stated vs parsed credit and debit totals, transaction count, page read method and timing.
- Follow-up chat about the finished analysis, prompt templates, analysis history, JSON/CSV export, print view.

## Prerequisites

- Poppler (`pdftocairo`, `pdftotext`, `pdfinfo`, `pdfimages`) on PATH. Ubuntu/Debian: `sudo apt install poppler-utils`. Arch: `sudo pacman -S poppler`. macOS: `brew install poppler`. Windows: a Poppler build such as https://github.com/oschwartz10612/poppler-windows, folder added to PATH.
- For development: Node 18+, Rust stable, and the Tauri v2 system packages (`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev` on Ubuntu).

## Run

```bash
npm install
npm run dev
```

## Headless testing

The full pipeline runs from a terminal, no window, and prints the report JSON:

```bash
./src-tauri/target/debug/local-mca-underwriter-ide --headless-analyze statement.pdf [more.pdf ...]
./src-tauri/target/debug/local-mca-underwriter-ide --headless-ledger statement.pdf   # parser only
```

## Measured on a laptop with an AMD Radeon 680M (integrated) and 14 GB RAM

- Digital page via text layer: 0.1 s.
- Scanned page via GLM-OCR at 150 DPI: 30 to 50 s, no digit errors observed; 100 DPI misread digits.
- Classification call (Qwen3.5 4B, thinking off): 40 to 90 s depending on output length.
- 9-page statement, 1 scanned page: about 2.5 minutes end to end.

## Layout

- `src-tauri/src/engine/registry.rs` pinned runtime and model files
- `src-tauri/src/engine/download.rs` resumable, checksummed downloads with progress events
- `src-tauri/src/engine/runtime.rs` install, spawn, health, shutdown of `llama-server`
- `src-tauri/src/engine/llama.rs` OpenAI-compatible client with SSE decoding
- `src-tauri/src/engine/ledger.rs` deterministic statement parser and metrics
- `src-tauri/src/engine/pipeline.rs` page reading, facts block, classification, report assembly
- `src/components/EngineSetup.vue` first-launch download screen

## License

MIT
