//! The analysis job for the built-in engine.
//!
//! Stage 1, per page: get text. Digital pages use the PDF text layer (exact and
//! instant). Scanned pages are rendered and read by the OCR model.
//! Stage 2, once per job: the reasoning model reads all page text and fills the
//! dashboard JSON, constrained by a JSON schema so the output always parses.
//!
//! One reasoning call per job replaces the old per-page vision call plus merge call,
//! and several statements of the same merchant are analyzed together as one batch.

use super::llama::{self, ChatOptions, Message};
use super::runtime::Endpoint;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use tauri::Emitter;

/// Pages with fewer words than this in their text layer are treated as scans.
const MIN_TEXT_WORDS: usize = 40;
/// A page image at least this many pixels on both sides marks a scanned page.
const SCAN_IMAGE_MIN_PX: u32 = 1000;
/// Render resolution for OCR. 100 DPI misread digits in testing; 150 did not.
const OCR_DPI: u32 = 150;

/// Text of one page and how it was obtained.
#[derive(Debug, Clone, Serialize)]
pub struct PageText {
    pub file_name: String,
    pub page: usize,
    /// "text" (PDF text layer) or "ocr".
    pub method: &'static str,
    pub seconds: f32,
    pub text: String,
}

fn run(cmd: &str, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new(cmd).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{cmd} not found. Install Poppler (poppler-utils) and make sure it is on PATH.")
        } else {
            format!("{cmd} failed to start: {e}")
        }
    })
}

pub fn page_count(pdf: &str) -> Result<usize, String> {
    let out = run("pdfinfo", &[pdf])?;
    if !out.status.success() {
        return Err(format!("pdfinfo failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("Pages:"))
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| "pdfinfo did not report a page count".to_string())
}

fn text_layer(pdf: &str, page: usize) -> Result<String, String> {
    let p = page.to_string();
    let out = run("pdftotext", &["-layout", "-f", &p, "-l", &p, pdf, "-"])?;
    if !out.status.success() {
        return Err(format!("pdftotext failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// True when the page carries a large raster image, i.e. it is a scan (its text
/// layer, if any, came from someone else's OCR and is not trusted).
fn has_scan_image(pdf: &str, page: usize) -> bool {
    let p = page.to_string();
    let Ok(out) = run("pdfimages", &["-list", "-f", &p, "-l", &p, pdf]) else { return false };
    // Columns: page num type width height color comp bpc enc interp object ID x-ppi y-ppi size ratio
    String::from_utf8_lossy(&out.stdout).lines().skip(2).any(|l| {
        let cols: Vec<&str> = l.split_whitespace().collect();
        let w: u32 = cols.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
        let h: u32 = cols.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);
        w >= SCAN_IMAGE_MIN_PX && h >= SCAN_IMAGE_MIN_PX
    })
}

/// Render one page to a grayscale JPEG and return it as a data URI.
fn render_page_data_uri(pdf: &str, page: usize) -> Result<String, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let prefix = dir.path().join("page");
    let p = page.to_string();
    let out = run(
        "pdftocairo",
        &["-jpeg", "-gray", "-r", &OCR_DPI.to_string(), "-f", &p, "-l", &p, "-singlefile", pdf, &prefix.to_string_lossy()],
    )?;
    if !out.status.success() {
        return Err(format!("pdftocairo failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let bytes = std::fs::read(prefix.with_extension("jpg")).map_err(|e| format!("Rendered page missing: {e}"))?;
    Ok(format!("data:image/jpeg;base64,{}", BASE64.encode(bytes)))
}

async fn ocr_page(ep: &Endpoint, pdf: &str, page: usize) -> Result<String, String> {
    let uri = render_page_data_uri(pdf, page)?;
    let msgs = [Message { role: "user", text: "Text Recognition:", image_data_uri: Some(&uri) }];
    let opts = ChatOptions {
        model: "ocr",
        temperature: 0.0,
        max_tokens: 4096,
        json_schema: None,
        enable_thinking: false,
        idle_timeout: Duration::from_secs(300),
    };
    let r = llama::chat(ep, &msgs, &opts, |_, _| {}).await?;
    Ok(r.content)
}

/// Stage 1 for one file. Emits `analysis-progress` page events on `app`.
pub async fn extract_pages(
    app: &tauri::AppHandle,
    ep: &Endpoint,
    pdf: &str,
    page_offset: usize,
    total_pages: usize,
) -> Result<Vec<PageText>, String> {
    let file_name = Path::new(pdf).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let n = page_count(pdf)?;
    let mut pages = Vec::with_capacity(n);
    for page in 1..=n {
        let current = page_offset + page;
        let _ = app.emit("analysis-progress", json!({
            "type": "page_start", "current_page": current, "total_pages": total_pages,
            "message": format!("Reading {file_name} page {page} of {n}")
        }));
        let started = Instant::now();
        let layer = text_layer(pdf, page)?;
        let words = layer.split_whitespace().count();
        let (method, text) = if words < MIN_TEXT_WORDS || has_scan_image(pdf, page) {
            ("ocr", ocr_page(ep, pdf, page).await?)
        } else {
            ("text", layer)
        };
        let seconds = started.elapsed().as_secs_f32();
        println!("[Engine] {file_name} p{page}: {method} in {seconds:.1}s, {} chars", text.len());
        let _ = app.emit("analysis-progress", json!({
            "type": "page_complete", "current_page": current, "total_pages": total_pages,
            "method": method, "seconds": seconds, "page_result": "",
            "message": format!("{file_name} page {page}: {} in {seconds:.1}s", if method == "ocr" { "OCR" } else { "text layer" })
        }));
        pages.push(PageText { file_name: file_name.clone(), page, method, seconds, text });
    }
    Ok(pages)
}

/// The dashboard schema. Enforced by the server, so the frontend parser never
/// sees prose, markdown fences or truncated arrays.
pub fn result_schema() -> Value {
    json!({
      "type": "object",
      "properties": {
        "business": { "type": "object", "properties": {
            "name": { "type": ["string", "null"] },
            "account": { "type": ["string", "null"] },
            "period": { "type": ["string", "null"] }
          }, "required": ["name", "account", "period"] },
        "positions": { "type": "array", "maxItems": 12, "items": { "type": "object", "properties": {
            "lender": { "type": "string" },
            "payment": { "type": "number" },
            "frequency": { "type": "string", "enum": ["daily", "weekly", "monthly"] },
            "funded": { "type": ["number", "null"] },
            "funded_date": { "type": ["string", "null"] }
          }, "required": ["lender", "payment", "frequency", "funded", "funded_date"] } },
        "bank_metrics": { "type": "object", "properties": {
            "true_revenue": { "type": "number" },
            "negative_days": { "type": "integer" },
            "avg_daily_balance": { "type": "number" },
            "nsf_count": { "type": "integer" }
          }, "required": ["true_revenue", "negative_days", "avg_daily_balance", "nsf_count"] },
        "debt_leverage": { "type": "object", "properties": {
            "total_debt_service": { "type": "number" },
            "safe_new_payment": { "type": "number" },
            "leverage_ratio": { "type": "string" }
          }, "required": ["total_debt_service", "safe_new_payment", "leverage_ratio"] },
        "risk": { "type": "object", "properties": { "score": { "type": "integer", "minimum": 1, "maximum": 10 } }, "required": ["score"] },
        "recommendation": { "type": "string", "enum": ["APPROVE", "REVIEW", "DECLINE"] },
        "notes": { "type": "string", "maxLength": 600 }
      },
      "required": ["business", "positions", "bank_metrics", "debt_leverage", "risk", "recommendation", "notes"]
    })
}

const UNDERWRITER_SYSTEM_PROMPT: &str = r#"You are an underwriting analyst for Merchant Cash Advance (MCA) funding. You receive the text of one or more bank statement pages for one merchant and fill a JSON report.

Definitions
- MERCHANT: the account holder named on the statement. Never the bank.
- POSITION: an existing MCA or loan being repaid. It shows as a recurring debit of the same (or near-same) amount to the same payee every business day or every week. Known MCA funders include OnDeck, Kabbage, Fundbox, Forward Financing, Rapid Finance, Credibly, Fora, CAN Capital, Kapitus, Libertas, Bluevine. Unnamed recurring daily/weekly debits are "Unknown MCA". Vendor payments, payroll, floor plan or manufacturer settlements, taxes, transfers and one-off debits are NOT positions.
- FUNDING DEPOSIT: an incoming lump sum from a funder or lender (loan proceeds, MCA funding).
- TRUE REVENUE: total deposits for the period minus funding deposits and minus transfers from the merchant's own accounts.

Rules
1. Use the statement's own summary lines when present (Beginning Balance, total Deposits/Credits, total Checks/Debits, Ending Balance, days in period). Do not recompute totals the statement already states.
2. negative_days: number of calendar days whose ending balance was below zero. nsf_count: count of NSF, returned item, overdraft and insufficient funds fees.
3. avg_daily_balance: average of the daily ending balances if a daily balance section exists; otherwise the mean of beginning and ending balance.
4. total_debt_service is the sum of position payments expressed per day (weekly payment / 5, monthly / 21). safe_new_payment = (true_revenue / days_in_period) * 0.10 - total_debt_service, floored at 0. leverage_ratio = total_debt_service / (true_revenue / days_in_period) formatted like "0.4x".
5. risk.score is 1 (safest) to 10 (riskiest). Weigh negative days, NSF count, leverage, revenue level and stability, stacking of positions.
6. recommendation: APPROVE, REVIEW or DECLINE.
7. Unknown text fields are null. Unknown numbers are 0. Do not invent lenders, amounts or dates.
8. notes: two or three plain sentences an underwriter would want: what drives the score, anything to verify.
9. Several statements in the input are consecutive months of the same merchant; report period as the full span and totals across all of them.
10. Read once, decide once. Keep working silently and output only the JSON."#;

/// Stage 2. Streams reasoning to `stream-thought` and tokens to `stream-token`,
/// returns the JSON text of the report.
pub async fn underwrite(
    app: &tauri::AppHandle,
    ep: &Endpoint,
    pages: &[PageText],
    custom_instructions: &str,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    let mut input = String::new();
    for p in pages {
        input.push_str(&format!("=== {} | page {} ({}) ===\n{}\n\n", p.file_name, p.page, p.method, p.text.trim()));
    }
    let custom = custom_instructions.trim();
    let user = if custom.is_empty() || custom.starts_with("Add custom underwriting focus") {
        format!("BANK STATEMENT TEXT:\n\n{input}")
    } else {
        format!("ADDITIONAL INSTRUCTIONS FROM THE UNDERWRITER:\n{custom}\n\nBANK STATEMENT TEXT:\n\n{input}")
    };

    let msgs = [
        Message { role: "system", text: UNDERWRITER_SYSTEM_PROMPT, image_data_uri: None },
        Message { role: "user", text: &user, image_data_uri: None },
    ];
    let opts = ChatOptions {
        model: "underwriter",
        temperature,
        max_tokens,
        json_schema: Some(result_schema()),
        enable_thinking: false,
        idle_timeout: Duration::from_secs(600),
    };
    let app2 = app.clone();
    let r = llama::chat(ep, &msgs, &opts, move |reasoning, content| {
        if let Some(t) = reasoning {
            let _ = app2.emit("stream-thought", json!({ "type": "thought", "thinking": t }));
        }
        if let Some(c) = content {
            let _ = app2.emit("stream-token", json!({ "type": "token", "content": c }));
        }
    })
    .await?;
    println!("[Engine] underwrite: {} prompt tokens, {} completion tokens", r.prompt_tokens, r.completion_tokens);
    Ok(r.content)
}

/// Free-form follow-up question about a finished analysis.
pub async fn chat(app: &tauri::AppHandle, ep: &Endpoint, prompt: &str, temperature: f32, max_tokens: i32) -> Result<String, String> {
    let msgs = [Message { role: "user", text: prompt, image_data_uri: None }];
    let opts = ChatOptions {
        model: "underwriter",
        temperature,
        max_tokens,
        json_schema: None,
        enable_thinking: false,
        idle_timeout: Duration::from_secs(300),
    };
    let app2 = app.clone();
    let r = llama::chat(ep, &msgs, &opts, move |reasoning, content| {
        if let Some(t) = reasoning {
            let _ = app2.emit("stream-thought", json!({ "type": "thought", "thinking": t }));
        }
        if let Some(c) = content {
            let _ = app2.emit("stream-token", json!({ "type": "token", "content": c }));
        }
    })
    .await?;
    Ok(r.content)
}
