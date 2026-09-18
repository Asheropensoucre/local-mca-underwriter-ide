//! Developer mode: run the whole pipeline from the command line, no window.
//!
//! ```text
//! local-mca-underwriter-ide --headless-analyze a.pdf [b.pdf ...] [--instructions "text"]
//! local-mca-underwriter-ide --headless-ledger a.pdf [b.pdf ...] [--ocr]
//! local-mca-underwriter-ide --headless-plan
//! ```
//! `--headless-plan` prints the memory plan the engine would start with right now.
//! `--headless-ledger` runs only the deterministic parser on the text layers and prints
//! the ledger. With `--ocr` it starts the engine and reads scanned pages with the OCR
//! model (cached, see `pipeline::ocr_cache_dir`), so the parser sees what the app sees.
//! Installs anything missing, starts the engine, analyzes, prints the report JSON to
//! stdout (progress and timings go to stderr) and exits with 0, or 1 on failure.

use tauri::{Listener, Manager};

pub struct HeadlessArgs {
    pub pdfs: Vec<String>,
    pub instructions: String,
    /// `--headless-ledger`: only run the deterministic parser on the text layer and print it.
    pub ledger_only: bool,
    /// `--ocr` with `--headless-ledger`: OCR scanned pages through the engine first.
    pub ocr: bool,
    /// `--headless-plan`: print the memory plan and exit.
    pub plan_only: bool,
}

/// Parse `--headless-analyze` from argv. None when the app should start normally.
pub fn parse_args() -> Option<HeadlessArgs> {
    let args: Vec<String> = std::env::args().collect();
    let pos = args.iter().position(|a| a == "--headless-analyze" || a == "--headless-ledger" || a == "--headless-plan")?;
    let ledger_only = args[pos] == "--headless-ledger";
    let plan_only = args[pos] == "--headless-plan";
    let mut pdfs = Vec::new();
    let mut instructions = String::new();
    let mut ocr = false;
    let mut i = pos + 1;
    while i < args.len() {
        if args[i] == "--instructions" {
            instructions = args.get(i + 1).cloned().unwrap_or_default();
            i += 2;
        } else if args[i] == "--ocr" {
            ocr = true;
            i += 1;
        } else {
            pdfs.push(args[i].clone());
            i += 1;
        }
    }
    Some(HeadlessArgs { pdfs, instructions, ledger_only, ocr, plan_only })
}

static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static LEDGER_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True while a headless job owns the process (set before the window's scripts run).
pub fn active() -> bool {
    ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// True for `--headless-ledger`: the engine only ever reads pages, so the memory plan
/// needs room for the OCR model alone.
pub fn ledger_only() -> bool {
    LEDGER_ONLY.load(std::sync::atomic::Ordering::Relaxed)
}

/// Called from the Tauri setup hook. Hides the window, runs the job, exits the process.
pub fn run(app: &tauri::AppHandle, args: HeadlessArgs) {
    ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    LEDGER_ONLY.store(args.ledger_only, std::sync::atomic::Ordering::Relaxed);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    // Mirror the UI events to stderr so a terminal run shows the same progress.
    app.listen_any("analysis-progress", |e| eprintln!("[progress] {}", e.payload()));
    app.listen_any("engine-download-progress", |e| {
        let v: serde_json::Value = serde_json::from_str(e.payload()).unwrap_or_default();
        if v["done"].as_bool() == Some(true) {
            eprintln!("[download] {} done", v["asset_id"]);
        }
    });

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let code = match run_job(&app, args).await {
            Ok(json) => {
                println!("{json}");
                0
            }
            Err(e) => {
                eprintln!("[headless] FAILED: {e}");
                1
            }
        };
        app.state::<super::runtime::EngineProcess>().stop();
        std::process::exit(code);
    });
}

async fn run_job(app: &tauri::AppHandle, args: HeadlessArgs) -> Result<String, String> {
    if args.plan_only {
        let cfg = super::runtime::load_config(app);
        let uw = super::registry::underwriter_model(&cfg.underwriter_model).ok_or("unknown reasoning model")?;
        let plan = super::memory::plan(&super::registry::ocr_model(), Some(&uw));
        return serde_json::to_string_pretty(&plan).map_err(|e| e.to_string());
    }
    if args.pdfs.is_empty() {
        return Err("no PDF paths given".into());
    }
    if args.ledger_only {
        // .txt page dumps are parser inputs, not PDFs: nothing to repair.
        let pdfs: Vec<String> = if args.pdfs.iter().all(|p| p.ends_with(".txt")) { args.pdfs.clone() } else { super::pipeline::prepare_inputs(app, &args.pdfs)? };
        let pages = if args.ocr { ocr_pages(app, &pdfs).await? } else { text_pages(&pdfs)? };
        return ledger_dump(pages);
    }
    let t = std::time::Instant::now();
    super::engine_install(app.clone()).await?;
    eprintln!("[headless] install ok ({:.1}s)", t.elapsed().as_secs_f32());
    let url = start_engine(app).await?;
    eprintln!("[headless] engine at {url} ({:.1}s)", t.elapsed().as_secs_f32());
    let json = super::engine_analyze(app.clone(), args.pdfs, args.instructions, 0.2, 4096).await?;
    eprintln!("[headless] analyze done ({:.1}s total)", t.elapsed().as_secs_f32());
    Ok(json)
}

/// The job's own engine start (the `engine_start` command refuses while headless).
async fn start_engine(app: &tauri::AppHandle) -> Result<String, String> {
    let cfg = super::runtime::load_config(app);
    Ok(super::runtime::start(app, &cfg).await?.base_url)
}

/// Page texts from the PDF text layers only. Pages with no text but a full-page image are
/// marked "scan" so the caller knows the parse is incomplete without `--ocr`.
fn text_pages(pdfs: &[String]) -> Result<Vec<super::pipeline::PageText>, String> {
    let mut pages = Vec::new();
    for pdf in pdfs {
        // .txt inputs are page dumps (see MCA_DUMP_PAGES), one page per file.
        if pdf.ends_with(".txt") {
            let text = std::fs::read_to_string(pdf).map_err(|e| e.to_string())?;
            pages.push(super::pipeline::PageText { file_name: pdf.clone(), page: pages.len() + 1, method: "text", seconds: 0.0, text });
            continue;
        }
        let n = super::pipeline::page_count(pdf)?;
        for page in 1..=n {
            let text = super::pipeline::text_layer(pdf, page)?;
            // Same decision as the app makes; "ocr" here means the page was skipped.
            let method = match super::pipeline::page_method(pdf, page, &text, false) { "ocr" => "scan", m => m };
            pages.push(super::pipeline::PageText { file_name: pdf.clone(), page, method, seconds: 0.0, text });
        }
    }
    Ok(pages)
}

/// Page texts the way the app reads them: text layer or OCR. The engine is started only
/// when a scanned page is not in the OCR cache yet.
async fn ocr_pages(app: &tauri::AppHandle, pdfs: &[String]) -> Result<Vec<super::pipeline::PageText>, String> {
    let total: usize = pdfs.iter().map(|p| super::pipeline::page_count(p).unwrap_or(0)).sum();
    match super::pipeline::read_pages(app, None, pdfs, total).await {
        Err(e) if e == super::pipeline::NEEDS_ENGINE => {
            super::engine_install(app.clone()).await?;
            let url = start_engine(app).await?;
            eprintln!("[headless] engine at {url}");
            let ep = app.state::<super::runtime::EngineProcess>().endpoint().ok_or("engine not running")?;
            super::pipeline::read_pages(app, Some(&ep), pdfs, total).await.map_err(|e| {
                app.state::<super::runtime::EngineProcess>().stopped_reason.lock().ok().and_then(|g| g.clone()).unwrap_or(e)
            })
        }
        r => r,
    }
}

/// Deterministic pass over page texts, no reasoning model involved.
fn ledger_dump(pages: Vec<super::pipeline::PageText>) -> Result<String, String> {
    let texts: Vec<(usize, String)> = pages.iter().enumerate().map(|(i, p)| (i + 1, p.text.clone())).collect();
    let mut methods: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for p in &pages {
        *methods.entry(p.method).or_default() += 1;
    }
    let pages: Vec<(usize, &str)> = texts.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let ledger = super::ledger::parse(&pages);
    let metrics = super::ledger::compute_metrics(&ledger, &ledger.funding_candidates, &[]);
    let credits = ledger.transactions.iter().filter(|t| t.kind == super::ledger::Kind::Credit).count();
    let out = serde_json::json!({
        "pages": methods,
        "summary": ledger.summary,
        "statements": ledger.statements,
        "parsed": { "credit_lines": credits, "debit_lines": ledger.transactions.len() - credits,
                    "credit_total": ledger.parsed_credit_total, "debit_total": ledger.parsed_debit_total },
        "daily_balances": ledger.daily_balances.len(),
        "recurring_debits": ledger.recurring_debits,
        "nsf_items": ledger.nsf_items.iter().map(|i| &ledger.transactions[*i]).collect::<Vec<_>>(),
        "funding_candidates": ledger.funding_candidates.iter().map(|i| &ledger.transactions[*i]).collect::<Vec<_>>(),
        "large_unlabeled_credits": ledger.large_unlabeled_credits.iter().map(|i| &ledger.transactions[*i]).collect::<Vec<_>>(),
        "payees": ledger.payees.iter().take(25).collect::<Vec<_>>(),
        "metrics_if_all_candidates_are_funding": metrics,
        "transactions": ledger.transactions,
    });
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}
