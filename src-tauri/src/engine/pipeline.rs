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

/// A page image at least this many pixels on both sides marks a scanned page.
/// Logos and signature blocks are smaller; a letter page scanned at 100 DPI is 850 x 1100.
const SCAN_IMAGE_MIN_PX: u32 = 800;
/// Render resolution for OCR. 100 DPI misread digits in testing; 150 did not.
const OCR_DPI: u32 = 150;

/// Text of one page and how it was obtained.
#[derive(Debug, Clone, Serialize)]
pub struct PageText {
    pub file_name: String,
    pub page: usize,
    /// "text" (PDF text layer), "ocr", or "blank" (near-empty page, not sent to OCR).
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
        w >= SCAN_IMAGE_MIN_PX && h >= SCAN_IMAGE_MIN_PX || (w as u64 * h as u64) >= 1_000_000
    })
}

/// Fraction of dark pixels on a low-resolution render. Cover sheets and blank pages have
/// almost none, so they skip the OCR model (30 to 50 seconds each).
const BLANK_INK_RATIO: f64 = 0.004;

fn page_ink_ratio(pdf: &str, page: usize) -> Result<f64, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let prefix = dir.path().join("ink");
    let p = page.to_string();
    let out = run("pdftocairo", &["-png", "-gray", "-r", "40", "-f", &p, "-l", &p, "-singlefile", pdf, &prefix.to_string_lossy()])?;
    if !out.status.success() {
        return Err(format!("pdftocairo failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let img = image::open(prefix.with_extension("png")).map_err(|e| e.to_string())?.into_luma8();
    let dark = img.pixels().filter(|p| p.0[0] < 128).count();
    Ok(dark as f64 / img.pixels().count().max(1) as f64)
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
        // Scanned pages (a full-page raster image) are read by the OCR model, since any text
        // layer they carry came from someone else's OCR. Digital pages use their text layer,
        // however short: a cover sheet with twenty words is still exact. Pages with no text
        // at all and no ink are blank; pages with no text but ink are OCR'd.
        let scanned = has_scan_image(pdf, page);
        let (method, text) = if scanned || words == 0 {
            if page_ink_ratio(pdf, page).map(|r| r < BLANK_INK_RATIO).unwrap_or(false) {
                ("blank", layer)
            } else {
                ("ocr", ocr_page(ep, pdf, page).await?)
            }
        } else {
            ("text", layer)
        };
        let seconds = started.elapsed().as_secs_f32();
        println!("[Engine] {file_name} p{page}: {method} in {seconds:.1}s, {} chars", text.len());
        let _ = app.emit("analysis-progress", json!({
            "type": "page_complete", "current_page": current, "total_pages": total_pages,
            "method": method, "seconds": seconds, "page_result": "",
            "message": format!("{file_name} page {page}: {} in {seconds:.1}s", match method { "ocr" => "OCR", "blank" => "blank page skipped", _ => "text layer" })
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
        "risk_adjustment": { "type": "integer", "minimum": -2, "maximum": 2 },
        "risk_reason": { "type": "string", "maxLength": 200 },
        "recommendation": { "type": "string", "enum": ["APPROVE", "REVIEW", "DECLINE"] },
        "notes": { "type": "string", "maxLength": 600 }
      },
      "required": ["business", "recurring_debits", "other_positions", "funding_deposit_ids", "risk_adjustment", "risk_reason", "recommendation", "notes"]
    })
}

const UNDERWRITER_SYSTEM_PROMPT: &str = r#"You are an underwriting analyst for Merchant Cash Advance (MCA) funding. A parser has already read the bank statement and computed every total. You do not compute or restate numbers. You classify and judge.

Definitions
- MERCHANT: the account holder named on the statement. Never the bank.
- POSITION: an existing MCA or business loan repaid by recurring ACH debits. Typical MCA funders: OnDeck, Kabbage, Fundbox, Forward Financing, Rapid Finance, Credibly, Fora, CAN Capital, Kapitus, Libertas, Bluevine, CFG Merchant Solutions, Cromwell Capital, Everest, Mantis, Fox, Spartan, Vader, names containing "MCA", "Capital", "Funding", "Advance", "Merchant Solutions". NOT positions: vendor and supplier bills, payroll, taxes, insurance, utilities, credit cards, POS and processor fees, floor plan or manufacturer settlements (auto dealers: Nissan WFS, NMAC, Ally, CAF, Chrysler Capital), internal transfers, rent, and anything described as a sale or purchase. A bank term loan payment counts as a position, named after the bank.
- FUNDING DEPOSIT: borrowed money coming in (MCA funding, loan proceeds, line draw, floor plan advance). Card settlements, customer payments, sales proceeds, incentives, rebates, refunds and owner transfers are not funding.

Tasks
1. business: merchant name and account last four from the header text; industry in a few words from the payees; the statement period.
2. recurring_debits: for every candidate id, is_position true or false, and a clean lender name ("CFG Merchant Solutions", not the raw ACH text). Use the id numbers exactly as given.
3. other_positions: debt payments to MCA funders or lenders that appear only once in SINGLE DEBITS. Copy the payment amount exactly from that line and quote the line in evidence. Leave the list empty if there are none. Never list a vendor, a sale, a purchase, a tax or a transfer.
4. funding_deposit_ids: the candidate ids that are borrowed money. Use the id numbers exactly as given.
5. risk_adjustment: the facts include a computed RISK BASELINE with its factors. Return 0 unless something the baseline cannot see justifies moving it, then -2 to +2 with the reason in risk_reason (one sentence).
6. recommendation: APPROVE if the adjusted score is 1 to 4, REVIEW for 5 to 7, DECLINE for 8 to 10, unless you state a reason to deviate in notes.
7. notes: two to four short sentences an underwriter needs: industry and nature of the cash flow, what drives the risk, what to verify. Do not write dollar amounts in notes; the dashboard shows them. Mention data limits stated in the facts (partial statement, no daily balances).

Example (abbreviated). Facts: candidate id 0: 2 x 3599.00 weekly | ACH Withdrawal CFG MERCHANT SOL ACHPAYMENT; candidate id 1: 2 x 466.13 biweekly | FEDERATED ACH OFFSET; single debit 18750.00 | ACH Withdrawal MCA Servicing Co DR; single debit 1725.00 | Integrity 1st PR Sale; funding id 7: 57592.12 | PNCBANK-PROCEEDS LOAN FUND; funding id 9: 3000.00 | NISSAN INCENTIVES.
Answer: recurring_debits [{id 0, true, "CFG Merchant Solutions"}, {id 1, false, "Federated Insurance"}]; other_positions [{"MCA Servicing Co", 18750.00, "monthly", "ACH Withdrawal MCA Servicing Co DR"}] (the Integrity line is a sale, not a position); funding_deposit_ids [7] (incentives are not borrowed money).

Output only the JSON."#;

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
    let prelim = ledger::compute_metrics(ledger, &[], &[]);
    let base = ledger::risk_baseline(&prelim, ledger.recurring_debits.len(), !ledger.daily_balances.is_empty());
    out.push_str(&format!("\nRISK BASELINE (computed before your classification, assuming every recurring candidate is a position and no deposit is funding): {}/10\n", base.score));
    for f in &base.factors {
        out.push_str(&format!("  {f}\n"));
    }
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
    let _ = temperature; // classification is deterministic; the UI temperature applies to chat
    let opts = ChatOptions {
        model: "underwriter",
        temperature: 0.0,
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
    let mut rejected: Vec<Value> = Vec::new();
    for item in cls["other_positions"].as_array().cloned().unwrap_or_default() {
        let payment = item["payment"].as_f64().unwrap_or(0.0);
        let freq = item["frequency"].as_str().unwrap_or("monthly").to_string();
        let lender = item["lender"].as_str().unwrap_or("").to_string();
        let evidence = item["evidence"].as_str().unwrap_or("").to_string();
        // The payment must be a real debit line and the lender name must come from that line.
        match matching_debit(ledger, payment, &lender, &evidence) {
            Some(t) => {
                confirmed.push((payment, freq.clone()));
                positions.push(json!({
                    "lender": lender, "payment": t.amount, "frequency": freq,
                    "funded": null, "funded_date": null,
                    "occurrences": 1, "date": t.date, "evidence": t.description, "source": "single payment identified by model"
                }));
            }
            None => rejected.push(json!({ "lender": lender, "payment": payment, "evidence": evidence, "reason": "no matching debit line, or the line is a vendor, transfer, tax or sale" })),
        }
    }

    let base_positions = positions.len();
    let funding_ids: Vec<usize> = cls["funding_deposit_ids"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_u64()).map(|v| v as usize).filter(|id| ledger.funding_candidates.contains(id)).collect();
    let m = ledger::compute_metrics(ledger, &funding_ids, &confirmed);

    let funding_lines: Vec<Value> = funding_ids.iter().filter_map(|id| ledger.transactions.get(*id)).map(|t| json!({ "date": t.date, "amount": t.amount, "description": t.description })).collect();

    let base = ledger::risk_baseline(&m, base_positions, !ledger.daily_balances.is_empty());
    let adjustment = cls["risk_adjustment"].as_i64().unwrap_or(0).clamp(-2, 2);
    let score = (base.score as i64 + adjustment).clamp(1, 10);
    let (notes, dropped) = filter_notes(cls["notes"].as_str().unwrap_or(""), ledger, &m);

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
        "risk": {
            "score": score,
            "baseline": base.score,
            "adjustment": adjustment,
            "reason": cls["risk_reason"],
            "factors": base.factors
        },
        "recommendation": cls["recommendation"],
        "notes": notes,
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
            "rejected_positions": rejected,
            "notes_dropped": dropped,
            "pages": pages.iter().map(|p| json!({ "file": p.file_name, "page": p.page, "method": p.method, "seconds": p.seconds })).collect::<Vec<_>>()
        }
    })
}

/// Find the debit line a model-reported single position refers to. Requires an exact
/// amount match and a shared word with the lender or evidence text, and rejects lines
/// that are plainly not debt payments.
fn matching_debit<'a>(ledger: &'a ledger::Ledger, payment: f64, lender: &str, evidence: &str) -> Option<&'a ledger::Txn> {
    const REJECT: &[&str] = &["sale", "purchase", "payroll", "tax", "transfer", "tr to acct", "vendor", "insurance", "utilit", "amex", "fee", "irs"];
    let words: Vec<String> = format!("{lender} {evidence}")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 4)
        .map(|w| w.to_ascii_lowercase())
        .collect();
    ledger
        .transactions
        .iter()
        .filter(|t| t.kind == ledger::Kind::Debit && (t.amount - payment).abs() < 0.01)
        .find(|t| {
            let l = t.description.to_ascii_lowercase();
            !REJECT.iter().any(|r| l.contains(r)) && words.iter().any(|w| l.contains(w.as_str()))
        })
}

/// Drop note sentences that quote a dollar amount the statement does not contain.
/// Small models invent figures; the dashboard already shows the real ones.
fn filter_notes(notes: &str, ledger: &ledger::Ledger, m: &ledger::Metrics) -> (String, Vec<String>) {
    let mut known: Vec<f64> = ledger.transactions.iter().map(|t| t.amount).collect();
    let s = &ledger.summary;
    known.extend([s.beginning_balance, s.ending_balance, s.total_credits, s.total_debits, s.average_balance, s.minimum_balance].into_iter().flatten().map(f64::abs));
    known.extend([m.true_revenue, m.total_credits, m.funding_deposits, m.avg_daily_balance, m.total_debt_service_daily, m.safe_new_payment]);
    let is_known = |v: f64| known.iter().any(|k| (k - v).abs() < 0.5 || (v >= 1000.0 && (k - v).abs() / v < 0.02));

    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for sentence in notes.split_inclusive(|c| c == '.' || c == '!' || c == '?') {
        let mut ok = true;
        for amount in dollar_amounts(sentence) {
            if !is_known(amount) {
                ok = false;
                break;
            }
        }
        if ok {
            kept.push(sentence.trim());
        } else {
            dropped.push(sentence.trim().to_string());
        }
    }
    (kept.join(" "), dropped)
}

/// "$113,045.99", "$9,625", "$113k", "$2.5M" style amounts in free text.
fn dollar_amounts(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b',' || bytes[j] == b'.') {
                j += 1;
            }
            let num: String = text[i + 1..j].chars().filter(|c| *c != ',').collect();
            if let Ok(mut v) = num.trim_end_matches('.').parse::<f64>() {
                let suffix = text[j..].chars().next().map(|c| c.to_ascii_lowercase());
                match suffix {
                    Some('k') => v *= 1_000.0,
                    Some('m') => v *= 1_000_000.0,
                    _ => {}
                }
                out.push(v);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollar_amounts_are_extracted() {
        assert_eq!(dollar_amounts("costs $9,625.85 and $113k, not $2.5M."), vec![9625.85, 113_000.0, 2_500_000.0]);
        assert!(dollar_amounts("no money here").is_empty());
    }
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
