//! The analysis job for the built-in engine.
//!
//! Stage 1, per page: get text. Digital pages use the PDF text layer (exact and
//! instant). Scanned pages are rendered and read by the OCR model.
//! Stage 2, once per job: `ledger.rs` parses every transaction and computes the
//! totals; the reasoning model only classifies (which debits are positions, which
//! deposits are funding) and writes the risk judgment, constrained by a JSON schema.
//! The report is assembled from the computed numbers.
//!
//! One reasoning call per job replaces the old per-page vision call plus merge call,
//! and several statements of the same merchant are analyzed together as one batch.

use super::ledger;
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

pub fn text_layer(pdf: &str, page: usize) -> Result<String, String> {
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

/// What the model is asked for. Only classification and judgment: which recurring
/// debits are positions, which large deposits are funding, business identity, risk
/// score, recommendation and notes. Every dollar figure in the report is computed in
/// `ledger.rs` from these answers.
pub fn classification_schema(recurring_ids: &[usize], funding_ids: &[usize]) -> Value {
    let rec_ids: Vec<Value> = recurring_ids.iter().map(|i| json!(i)).collect();
    let fund_ids: Vec<Value> = funding_ids.iter().map(|i| json!(i)).collect();
    json!({
      "type": "object",
      "properties": {
        "business": { "type": "object", "properties": {
            "name": { "type": ["string", "null"] },
            "account_last4": { "type": ["string", "null"], "maxLength": 4 },
            "industry": { "type": ["string", "null"], "maxLength": 60 },
            "period_start": { "type": ["string", "null"] },
            "period_end": { "type": ["string", "null"] }
          }, "required": ["name", "account_last4", "industry", "period_start", "period_end"] },
        "recurring_debits": { "type": "array", "maxItems": recurring_ids.len().max(1), "items": { "type": "object", "properties": {
            "id": { "type": "integer", "enum": if rec_ids.is_empty() { vec![json!(0)] } else { rec_ids } },
            "is_position": { "type": "boolean" },
            "lender": { "type": "string", "maxLength": 60 }
          }, "required": ["id", "is_position", "lender"] } },
        "other_positions": { "type": "array", "maxItems": 8, "items": { "type": "object", "properties": {
            "lender": { "type": "string", "maxLength": 60 },
            "payment": { "type": "number" },
            "frequency": { "type": "string", "enum": ["daily", "weekly", "biweekly", "monthly"] },
            "evidence": { "type": "string", "maxLength": 120 }
          }, "required": ["lender", "payment", "frequency", "evidence"] } },
        "funding_deposit_ids": { "type": "array", "maxItems": funding_ids.len().max(1),
            "items": { "type": "integer", "enum": if fund_ids.is_empty() { vec![json!(-1)] } else { fund_ids } } },
        "risk_score": { "type": "integer", "minimum": 1, "maximum": 10 },
        "recommendation": { "type": "string", "enum": ["APPROVE", "REVIEW", "DECLINE"] },
        "notes": { "type": "string", "maxLength": 700 }
      },
      "required": ["business", "recurring_debits", "other_positions", "funding_deposit_ids", "risk_score", "recommendation", "notes"]
    })
}

const UNDERWRITER_SYSTEM_PROMPT: &str = r#"You are an underwriting analyst for Merchant Cash Advance (MCA) funding. A parser has already read the bank statement and computed the totals. You do not compute numbers. You classify and judge.

Definitions
- MERCHANT: the account holder named on the statement. Never the bank.
- POSITION: an existing MCA or business loan being repaid by recurring ACH debits. Typical MCA funders: OnDeck, Kabbage, Fundbox, Forward Financing, Rapid Finance, Credibly, Fora, CAN Capital, Kapitus, Libertas, Bluevine, CFG Merchant Solutions, Cromwell Capital, Everest, Mantis, Fox, Spartan, Vader, "MCA Servicing", "Capital", "Funding", "Advance". Vendor bills, payroll, taxes, insurance, utilities, credit cards, floor plan or manufacturer settlements (auto dealers: Nissan WFS, NMAC, Ally, CAF, Chrysler Capital), internal transfers between the merchant's own accounts, and bank loan interest are NOT MCA positions. A conventional bank term loan payment is a position only if it is a recurring debt payment; mark it with the bank's name.
- FUNDING DEPOSIT: an incoming lump sum that is borrowed money (MCA funding, loan proceeds, line of credit draw). Manufacturer incentives, floor plan advances that fund inventory, customer payments, card settlements, sales proceeds and owner transfers are revenue or working capital, not MCA funding, unless the description says loan/funding/proceeds/advance/capital.

Tasks
1. business: merchant name and account last four from the header text; industry in a few words from the payees; the statement period.
2. recurring_debits: for every candidate id, say whether it is a position and name the lender (short, cleaned up: "CFG Merchant Solutions", not the raw ACH text).
3. other_positions: debt payments to MCA funders or lenders that appear only once in the period (so the parser could not see a cadence). Give payment, likely frequency and the line you saw.
4. funding_deposit_ids: the subset of candidate ids that are borrowed money.
5. risk_score 1 (safest) to 10 (riskiest), recommendation, and notes: two to four plain sentences an underwriter needs: what drives the score, what to verify. Mention data limits (e.g. partial statement, no daily balances) when the facts block says so.
Do not restate the numbers in notes beyond what is needed. Output only the JSON."#;

fn money(v: f64) -> String {
    format!("{:.2}", v)
}

/// Compact facts block the model reads instead of raw statement text.
fn facts_block(ledger: &ledger::Ledger, pages: &[PageText]) -> String {
    let s = &ledger.summary;
    let mut out = String::new();
    let files: Vec<String> = {
        let mut v: Vec<String> = pages.iter().map(|p| p.file_name.clone()).collect();
        v.dedup();
        v
    };
    let ocr_pages = pages.iter().filter(|p| p.method == "ocr").count();
    out.push_str(&format!("PAGES READ: {} pages from {} file(s) ({} via OCR). All pages of the provided files were read.\n", pages.len(), files.len(), ocr_pages));
    if let Some(a) = &s.account_last4 {
        out.push_str(&format!("ACCOUNT LAST 4 (parsed): {a}\n"));
    }
    out.push_str("STATEMENT SUMMARY (from the bank):\n");
    out.push_str(&format!("  beginning balance: {}\n", s.beginning_balance.map(money).unwrap_or("not printed".into())));
    out.push_str(&format!("  ending balance: {}\n", s.ending_balance.map(money).unwrap_or("not printed".into())));
    out.push_str(&format!("  total credits: {}   (parser summed {} credit lines = {})\n", s.total_credits.map(money).unwrap_or("not printed".into()), ledger.transactions.iter().filter(|t| t.kind == ledger::Kind::Credit).count(), money(ledger.parsed_credit_total)));
    out.push_str(&format!("  total debits: {}   (parser summed {} debit lines = {})\n", s.total_debits.map(money).unwrap_or("not printed".into()), ledger.transactions.iter().filter(|t| t.kind == ledger::Kind::Debit).count(), money(ledger.parsed_debit_total)));
    out.push_str(&format!("  days in period: {}   period: {} to {}\n", s.days_in_period.map(|d| d.to_string()).unwrap_or("not printed".into()), s.period_start.clone().unwrap_or("?".into()), s.period_end.clone().unwrap_or("?".into())));
    out.push_str(&format!("  average balance: {}   minimum balance: {}\n", s.average_balance.map(money).unwrap_or("not printed".into()), s.minimum_balance.map(money).unwrap_or("not printed".into())));
    out.push_str(&format!("  daily balance table: {} entries, {} negative days\n", ledger.daily_balances.len(), ledger.daily_balances.iter().filter(|b| b.balance < 0.0).count()));
    out.push_str(&format!("  NSF / returned item lines: {}\n", ledger.nsf_items.len()));
    for i in &ledger.nsf_items {
        let t = &ledger.transactions[*i];
        out.push_str(&format!("    {} {} {}\n", t.date, money(t.amount), t.description.chars().take(70).collect::<String>()));
    }

    out.push_str("\nRECURRING DEBIT CANDIDATES (same payee, same amount, seen more than once):\n");
    if ledger.recurring_debits.is_empty() {
        out.push_str("  none\n");
    }
    for r in &ledger.recurring_debits {
        out.push_str(&format!("  id {}: {} x {} {} | {} | dates {}\n", r.id, r.count, money(r.amount), r.cadence, r.payee.chars().take(80).collect::<String>(), r.dates.join(", ")));
    }

    out.push_str("\nFUNDING DEPOSIT CANDIDATES (credits with loan/funding wording; pick the ones that are borrowed money):\n");
    if ledger.funding_candidates.is_empty() {
        out.push_str("  none\n");
    }
    for i in &ledger.funding_candidates {
        let t = &ledger.transactions[*i];
        out.push_str(&format!("  id {}: {} {} | {}\n", t.id, t.date, money(t.amount), t.description.chars().take(80).collect::<String>()));
    }
    if !ledger.large_unlabeled_credits.is_empty() {
        out.push_str("\nLARGE DEPOSITS WITHOUT A DESCRIPTION (counted as revenue; mention in notes if they need verification):\n");
        for i in &ledger.large_unlabeled_credits {
            let t = &ledger.transactions[*i];
            out.push_str(&format!("  {} {} | {}\n", t.date, money(t.amount), t.description.chars().take(80).collect::<String>()));
        }
    }

    out.push_str("\nPAYEES SEEN MORE THAN ONCE (count, total, side):\n");
    for p in ledger.payees.iter().take(30) {
        out.push_str(&format!("  {} x {} {:?} | {}\n", p.count, money(p.total), p.kind, p.payee.chars().take(80).collect::<String>()));
    }

    out.push_str("\nSINGLE DEBITS OF 500 OR MORE (not checks):\n");
    let mut singles: Vec<&ledger::Txn> = ledger
        .transactions
        .iter()
        .filter(|t| t.kind == ledger::Kind::Debit && t.amount >= 500.0 && !t.description.starts_with("Check "))
        .filter(|t| {
            let key = ledger::payee_key(&t.description);
            ledger.transactions.iter().filter(|o| o.kind == ledger::Kind::Debit && ledger::payee_key(&o.description) == key).count() == 1
        })
        .collect();
    singles.sort_by(|a, b| b.amount.partial_cmp(&a.amount).unwrap());
    for t in singles.iter().take(40) {
        out.push_str(&format!("  {} {} | {}\n", t.date, money(t.amount), t.description.chars().take(80).collect::<String>()));
    }
    out
}

/// Header text (top of the first three pages of each file, since court exhibits and
/// cover sheets often precede the statement) so the model can read the merchant name.
fn header_text(pages: &[PageText]) -> String {
    let mut out = String::new();
    for p in pages.iter().filter(|p| p.page <= 3) {
        let head: String = p.text.lines().filter(|l| !l.trim().is_empty()).take(30).collect::<Vec<_>>().join("\n");
        out.push_str(&format!("--- {} page {} ---\n{}\n", p.file_name, p.page, head));
    }
    out
}

/// Stage 2. Deterministic parse, one classification call, then the report is assembled
/// from computed numbers. Streams model output to `stream-thought` / `stream-token`.
pub async fn underwrite(
    app: &tauri::AppHandle,
    ep: &Endpoint,
    pages: &[PageText],
    custom_instructions: &str,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    let page_refs: Vec<(usize, &str)> = pages.iter().enumerate().map(|(i, p)| (i + 1, p.text.as_str())).collect();
    let ledger = ledger::parse(&page_refs);
    let recurring_ids: Vec<usize> = ledger.recurring_debits.iter().map(|r| r.id).collect();
    let funding_ids: Vec<usize> = ledger.funding_candidates.clone();

    let facts = facts_block(&ledger, pages);
    let custom = custom_instructions.trim();
    let mut user = String::new();
    if !custom.is_empty() && !custom.starts_with("Add custom underwriting focus") {
        user.push_str(&format!("ADDITIONAL INSTRUCTIONS FROM THE UNDERWRITER:\n{custom}\n\n"));
    }
    user.push_str(&format!("STATEMENT HEADER TEXT:\n{}\n\nFACTS FROM THE PARSER:\n{facts}", header_text(pages)));
    println!("[Engine] facts block {} chars, {} recurring candidates, {} funding candidates", facts.len(), recurring_ids.len(), funding_ids.len());

    let msgs = [
        Message { role: "system", text: UNDERWRITER_SYSTEM_PROMPT, image_data_uri: None },
        Message { role: "user", text: &user, image_data_uri: None },
    ];
    let opts = ChatOptions {
        model: "underwriter",
        temperature,
        max_tokens,
        json_schema: Some(classification_schema(&recurring_ids, &funding_ids)),
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
    println!("[Engine] classify: {} prompt tokens, {} completion tokens", r.prompt_tokens, r.completion_tokens);
    let cls: Value = serde_json::from_str(&r.content).map_err(|e| format!("Model returned invalid JSON despite schema: {e}"))?;

    Ok(assemble_report(&ledger, &cls, pages).to_string())
}

/// Build the dashboard JSON from the ledger and the model's classification.
fn assemble_report(ledger: &ledger::Ledger, cls: &Value, pages: &[PageText]) -> Value {
    // Positions: confirmed recurring candidates plus model-identified single payments.
    let mut positions: Vec<Value> = Vec::new();
    let mut confirmed: Vec<(f64, String)> = Vec::new();
    for item in cls["recurring_debits"].as_array().cloned().unwrap_or_default() {
        if item["is_position"].as_bool() != Some(true) {
            continue;
        }
        let Some(id) = item["id"].as_u64() else { continue };
        let Some(r) = ledger.recurring_debits.iter().find(|r| r.id as u64 == id) else { continue };
        let lender = item["lender"].as_str().unwrap_or("Unknown MCA").to_string();
        confirmed.push((r.amount, r.cadence.to_string()));
        positions.push(json!({
            "lender": lender, "payment": r.amount, "frequency": r.cadence,
            "funded": null, "funded_date": null,
            "occurrences": r.count, "dates": r.dates, "source": "recurring debit detected by parser"
        }));
    }
    for item in cls["other_positions"].as_array().cloned().unwrap_or_default() {
        let payment = item["payment"].as_f64().unwrap_or(0.0);
        let freq = item["frequency"].as_str().unwrap_or("monthly").to_string();
        if payment <= 0.0 {
            continue;
        }
        confirmed.push((payment, freq.clone()));
        positions.push(json!({
            "lender": item["lender"], "payment": payment, "frequency": freq,
            "funded": null, "funded_date": null,
            "occurrences": 1, "evidence": item["evidence"], "source": "single payment identified by model"
        }));
    }

    let funding_ids: Vec<usize> = cls["funding_deposit_ids"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_u64()).map(|v| v as usize).filter(|id| ledger.funding_candidates.contains(id)).collect();
    let m = ledger::compute_metrics(ledger, &funding_ids, &confirmed);

    let funding_lines: Vec<Value> = funding_ids.iter().filter_map(|id| ledger.transactions.get(*id)).map(|t| json!({ "date": t.date, "amount": t.amount, "description": t.description })).collect();

    let s = &ledger.summary;
    let period = match (&s.period_start, &s.period_end) {
        (Some(a), Some(b)) => Some(format!("{a} to {b}")),
        _ => match (cls["business"]["period_start"].as_str(), cls["business"]["period_end"].as_str()) {
            (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => Some(format!("{a} to {b}")),
            _ => None,
        },
    };
    let account = s
        .account_last4
        .clone()
        .or_else(|| cls["business"]["account_last4"].as_str().filter(|a| a.len() == 4 && a.chars().all(|c| c.is_ascii_digit())).map(String::from))
        .map(|a| format!("****{a}"));

    json!({
        "business": {
            "name": cls["business"]["name"],
            "account": account,
            "period": period,
            "industry": cls["business"]["industry"]
        },
        "positions": positions,
        "bank_metrics": {
            "true_revenue": round2(m.true_revenue),
            "negative_days": m.negative_days,
            "avg_daily_balance": round2(m.avg_daily_balance),
            "nsf_count": m.nsf_count,
            "total_credits": round2(m.total_credits),
            "funding_deposits": round2(m.funding_deposits),
            "days_in_period": m.days_in_period
        },
        "debt_leverage": {
            "total_debt_service": round2(m.total_debt_service_daily),
            "safe_new_payment": round2(m.safe_new_payment),
            "leverage_ratio": format!("{:.2}x", m.leverage_ratio)
        },
        "risk": { "score": cls["risk_score"] },
        "recommendation": cls["recommendation"],
        "notes": cls["notes"],
        "verification": {
            "stated_total_credits": s.total_credits, "parsed_total_credits": round2(ledger.parsed_credit_total),
            "stated_total_debits": s.total_debits, "parsed_total_debits": round2(ledger.parsed_debit_total),
            "beginning_balance": s.beginning_balance, "ending_balance": s.ending_balance,
            "transactions_parsed": ledger.transactions.len(),
            "daily_balances_found": ledger.daily_balances.len(),
            "funding_deposits": funding_lines,
            "large_deposits_to_verify": ledger.large_unlabeled_credits.iter().filter_map(|i| ledger.transactions.get(*i)).map(|t| json!({ "date": t.date, "amount": t.amount, "description": t.description })).collect::<Vec<_>>(),
            "nsf_items": ledger.nsf_items.iter().filter_map(|i| ledger.transactions.get(*i)).map(|t| json!({ "date": t.date, "amount": t.amount, "description": t.description })).collect::<Vec<_>>(),
            "sources": m.sources,
            "pages": pages.iter().map(|p| json!({ "file": p.file_name, "page": p.page, "method": p.method, "seconds": p.seconds })).collect::<Vec<_>>()
        }
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
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
