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
}

/// Parse `--headless-analyze` from argv. None when the app should start normally.
pub fn parse_args() -> Option<HeadlessArgs> {
    let args: Vec<String> = std::env::args().collect();
    let pos = args.iter().position(|a| a == "--headless-analyze")?;
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
    Some(HeadlessArgs { pdfs, instructions })
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
    let t = std::time::Instant::now();
    super::engine_install(app.clone()).await?;
    eprintln!("[headless] install ok ({:.1}s)", t.elapsed().as_secs_f32());
    let url = super::engine_start(app.clone()).await?;
    eprintln!("[headless] engine at {url} ({:.1}s)", t.elapsed().as_secs_f32());
    let json = super::engine_analyze(app.clone(), args.pdfs, args.instructions, 0.2, 4096).await?;
    eprintln!("[headless] analyze done ({:.1}s total)", t.elapsed().as_secs_f32());
    Ok(json)
}
