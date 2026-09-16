//! Developer mode: run the whole pipeline from the command line, no window.
//!
//! ```text
//! local-mca-underwriter-ide --headless-analyze a.pdf [b.pdf ...] [--instructions "text"]
//! ```
//! Installs anything missing, starts the engine, analyzes, prints the report JSON to
//! stdout (progress and timings go to stderr) and exits with 0, or 1 on failure.

use tauri::{Listener, Manager};

pub struct HeadlessArgs {
    pub pdfs: Vec<String>,
    pub instructions: String,
    /// `--headless-ledger`: only run the deterministic parser on the text layer and print it.
    pub ledger_only: bool,
}

/// Parse `--headless-analyze` from argv. None when the app should start normally.
pub fn parse_args() -> Option<HeadlessArgs> {
    let args: Vec<String> = std::env::args().collect();
    let pos = args.iter().position(|a| a == "--headless-analyze" || a == "--headless-ledger")?;
    let ledger_only = args[pos] == "--headless-ledger";
    let mut pdfs = Vec::new();
    let mut instructions = String::new();
    let mut i = pos + 1;
    while i < args.len() {
        if args[i] == "--instructions" {
            instructions = args.get(i + 1).cloned().unwrap_or_default();
            i += 2;
        } else {
            pdfs.push(args[i].clone());
            i += 1;
        }
    }
    Some(HeadlessArgs { pdfs, instructions, ledger_only })
}

/// Called from the Tauri setup hook. Hides the window, runs the job, exits the process.
pub fn run(app: &tauri::AppHandle, args: HeadlessArgs) {
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
    if args.pdfs.is_empty() {
        return Err("no PDF paths given".into());
    }
    if args.ledger_only {
        return ledger_dump(&args.pdfs);
    }
    let t = std::time::Instant::now();
    super::engine_install(app.clone()).await?;
    eprintln!("[headless] install ok ({:.1}s)", t.elapsed().as_secs_f32());
    let url = super::engine_start(app.clone()).await?;
    eprintln!("[headless] engine at {url} ({:.1}s)", t.elapsed().as_secs_f32());
    let json = super::engine_analyze(app.clone(), args.pdfs, args.instructions, 0.2, 4096).await?;
    eprintln!("[headless] analyze done ({:.1}s total)", t.elapsed().as_secs_f32());
    Ok(json)
}

/// Deterministic pass over the PDF text layers, no model involved.
fn ledger_dump(pdfs: &[String]) -> Result<String, String> {
    let mut texts: Vec<(usize, String)> = Vec::new();
    for pdf in pdfs {
        // .txt inputs are page dumps (see MCA_DUMP_PAGES), one page per file.
        if pdf.ends_with(".txt") {
            texts.push((texts.len() + 1, std::fs::read_to_string(pdf).map_err(|e| e.to_string())?));
            continue;
        }
        let n = super::pipeline::page_count(pdf)?;
        for page in 1..=n {
            texts.push((texts.len() + 1, super::pipeline::text_layer(pdf, page)?));
        }
    }
    let pages: Vec<(usize, &str)> = texts.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let ledger = super::ledger::parse(&pages);
    let metrics = super::ledger::compute_metrics(&ledger, &ledger.funding_candidates, &[]);
    let credits = ledger.transactions.iter().filter(|t| t.kind == super::ledger::Kind::Credit).count();
    let out = serde_json::json!({
        "summary": ledger.summary,
        "parsed": { "credit_lines": credits, "debit_lines": ledger.transactions.len() - credits,
                    "credit_total": ledger.parsed_credit_total, "debit_total": ledger.parsed_debit_total },
        "daily_balances": ledger.daily_balances.len(),
        "recurring_debits": ledger.recurring_debits,
        "nsf_items": ledger.nsf_items.iter().map(|i| &ledger.transactions[*i]).collect::<Vec<_>>(),
        "funding_candidates": ledger.funding_candidates.iter().map(|i| &ledger.transactions[*i]).collect::<Vec<_>>(),
        "large_unlabeled_credits": ledger.large_unlabeled_credits.iter().map(|i| &ledger.transactions[*i]).collect::<Vec<_>>(),
        "payees": ledger.payees.iter().take(25).collect::<Vec<_>>(),
        "metrics_if_all_candidates_are_funding": metrics,
    });
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}
