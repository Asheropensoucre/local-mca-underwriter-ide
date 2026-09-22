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
use super::memory;
use super::llama::{self, ChatOptions, Message};
use super::runtime::Endpoint;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

/// Text layers with fewer words than this are "thin": a scan with a bad OCR layer, or a
/// nearly empty page. Combined with the presence of an image, the page goes to OCR.
const MIN_TEXT_WORDS: usize = 40;
/// Images smaller than this on either side are logos and signature marks, not scans.
const SCAN_IMAGE_MIN_PX: u32 = 300;
/// Below this compressed size a large image is a watermark or background, not a scan.
const SCAN_IMAGE_MIN_BYTES: u64 = 60 * 1024;
/// Render resolution for OCR. 100 DPI misread digits in testing; 150 did not.
const OCR_DPI: u32 = 150;

fn ocr_dpi() -> u32 {
    std::env::var("MCA_OCR_DPI").ok().and_then(|v| v.parse().ok()).unwrap_or(OCR_DPI) // testing aid
}

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
    // Poppler prints syntax warnings for slightly damaged files and still answers; only a
    // missing page count is a failure, and then the message is one sentence, not the dump.
    let pages = String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("Pages:")).and_then(|v| v.trim().parse().ok());
    match pages {
        Some(n) => Ok(n),
        None => {
            let err = String::from_utf8_lossy(&out.stderr);
            let name = Path::new(pdf).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if err.contains("Encrypted") || err.contains("Incorrect password") {
                Err(format!("{name} is password protected. Remove the password and try again."))
            } else if err.contains("xref") || err.contains("trailer") || err.contains("Length") {
                Err(format!("{name} is damaged (broken cross-reference table) and could not be repaired. Re-save it from the bank's site or print it to a new PDF and try again."))
            } else {
                Err(format!("{name} could not be read as a PDF: {}", err.lines().last().unwrap_or("unknown error")))
            }
        }
    }
}

/// Damaged PDFs (broken cross-reference tables, bad stream lengths) are common in
/// files that went through email, scanners and portals. When Poppler cannot read one,
/// rebuild it with qpdf or Ghostscript if either is installed, into the engine's
/// `repaired/` folder, and return the new path. None when no tool could fix it.
pub fn repair_pdf(app: &tauri::AppHandle, pdf: &str) -> Option<PathBuf> {
    let dir = super::runtime::engine_dir(app).ok()?.join("repaired");
    std::fs::create_dir_all(&dir).ok()?;
    let name = Path::new(pdf).file_name()?.to_string_lossy().to_string();
    let out = dir.join(format!("{}-{name}", file_hash(pdf).unwrap_or_default()));
    let attempts: [(&str, Vec<String>); 2] = [
        ("qpdf", vec![pdf.to_string(), out.to_string_lossy().to_string()]),
        ("gs", vec!["-q".into(), "-dNOPAUSE".into(), "-dBATCH".into(), "-sDEVICE=pdfwrite".into(), format!("-sOutputFile={}", out.to_string_lossy()), pdf.to_string()]),
    ];
    for (tool, args) in &attempts {
        let _ = std::fs::remove_file(&out);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let Ok(res) = Command::new(tool).args(&args).output() else { continue };
        // qpdf exits 3 for "succeeded with warnings"; judge by whether Poppler can read the result.
        let _ = res;
        if out.exists() && page_count(&out.to_string_lossy()).is_ok() {
            println!("[Engine] {name}: repaired with {tool}");
            return Some(out);
        }
    }
    None
}

/// Paths the job will actually read: damaged files replaced by repaired copies.
pub fn prepare_inputs(app: &tauri::AppHandle, pdfs: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(pdfs.len());
    for pdf in pdfs {
        match page_count(pdf) {
            Ok(_) => out.push(pdf.clone()),
            Err(e) => match repair_pdf(app, pdf) {
                Some(fixed) => out.push(fixed.to_string_lossy().to_string()),
                None => return Err(e),
            },
        }
    }
    Ok(out)
}

pub fn text_layer(pdf: &str, page: usize) -> Result<String, String> {
    let p = page.to_string();
    let out = run("pdftotext", &["-layout", "-f", &p, "-l", &p, pdf, "-"])?;
    if !out.status.success() {
        return Err(format!("pdftotext failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// True when the page carries a raster image larger than a logo.
fn has_page_image(pdf: &str, page: usize) -> bool {
    let p = page.to_string();
    let Ok(out) = run("pdfimages", &["-list", "-f", &p, "-l", &p, pdf]) else { return false };
    // Columns: page num type width height color comp bpc enc interp object ID x-ppi y-ppi size ratio
    // A scan is a large image with real content: letterhead watermarks (Legends prints a
    // 622x860 JPEG of 12 KB on every page) must not send clean text pages to OCR.
    // Some scanners store the page as a stack of strips (Brookline court copies: nine
    // 2550x367 CCITT stencils): together they are the scan.
    let mut strips: u64 = 0;
    let whole = String::from_utf8_lossy(&out.stdout).lines().skip(2).any(|l| {
        let cols: Vec<&str> = l.split_whitespace().collect();
        let w: u32 = cols.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
        let h: u32 = cols.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);
        let size = cols.get(14).map(|v| parse_size(v)).unwrap_or(0);
        if w >= 1000 {
            strips += w as u64 * h as u64;
        }
        w >= SCAN_IMAGE_MIN_PX && h >= SCAN_IMAGE_MIN_PX && (size >= SCAN_IMAGE_MIN_BYTES || w * h >= 1_000_000)
    });
    whole || strips >= 2_000_000
}

/// pdfimages size column: "12.6K", "304K", "1.2M", "5137B".
fn parse_size(v: &str) -> u64 {
    let (num, unit) = v.split_at(v.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len());
    let n: f64 = num.parse().unwrap_or(0.0);
    match unit {
        "K" => (n * 1024.0) as u64,
        "M" => (n * 1024.0 * 1024.0) as u64,
        "G" => (n * 1024.0 * 1024.0 * 1024.0) as u64,
        _ => n as u64,
    }
}

/// Fraction of dark pixels on a low-resolution render. Cover sheets and blank pages have
/// almost none, so they skip the OCR model (30 to 50 seconds each).
/// (A court's two-line exhibit stamp alone is about half a percent; the sparsest page of
/// statement text is well over one.)
const BLANK_INK_RATIO: f64 = 0.0075;

fn page_ink_ratio(pdf: &str, page: usize) -> Result<f64, String> {
    page_ink_ratio_below(pdf, page, 128)
}

/// The same with the darkness cut-off given. At 40 dpi the strokes of a scan drawn as
/// vector outlines come out mid-grey, not black: a Home Bank page with 16 percent of its
/// pixels under 200 has 1.5 percent under 128, the same as a court stamp alone.
fn page_ink_ratio_below(pdf: &str, page: usize, cutoff: u8) -> Result<f64, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let prefix = dir.path().join("ink");
    let p = page.to_string();
    let out = run("pdftocairo", &["-png", "-gray", "-r", "40", "-f", &p, "-l", &p, "-singlefile", pdf, &prefix.to_string_lossy()])?;
    if !out.status.success() {
        return Err(format!("pdftocairo failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let img = image::open(prefix.with_extension("png")).map_err(|e| e.to_string())?.into_luma8();
    let dark = img.pixels().filter(|p| p.0[0] < cutoff).count();
    Ok(dark as f64 / img.pixels().count().max(1) as f64)
}

/// Horizontal bands of a page image (pixel rows `start..end`), split where a run of blank
/// rows at least `min_gap` tall separates them. The OCR model is a region recognizer: it
/// reads a block on its own far better than a whole page (it stops after the first table
/// of a page, and loses the second column of a wide table squeezed into the page's token
/// budget). Statements put white space between sections and never inside a table beyond
/// a row's height, so a gap of a quarter inch or more is a section boundary. Bands shorter
/// than `min_band` (a lone heading) are joined to the band below them.
pub fn page_bands(img: &image::GrayImage, min_gap: u32, min_band: u32) -> Vec<(u32, u32)> {
    let (w, h) = img.dimensions();
    let ink_threshold = (w / 200).max(1); // a few dark pixels are noise, not text
    let dark_row = |y: u32| (0..w).filter(|&x| img.get_pixel(x, y).0[0] < 128).count() as u32 > ink_threshold;
    let mut bands: Vec<(u32, u32)> = Vec::new();
    let mut start: Option<u32> = None;
    let mut blank_run = 0u32;
    for y in 0..h {
        if dark_row(y) {
            if start.is_none() {
                start = Some(y);
            }
            blank_run = 0;
        } else if let Some(s) = start {
            blank_run += 1;
            if blank_run >= min_gap {
                bands.push((s, y + 1 - blank_run));
                start = None;
                blank_run = 0;
            }
        }
    }
    if let Some(s) = start {
        bands.push((s, h));
    }
    // A short band is a heading for what follows: join it to the next band.
    let mut joined: Vec<(u32, u32)> = Vec::new();
    for (s, e) in bands {
        match joined.last_mut() {
            Some(last) if last.1 - last.0 < min_band => last.1 = e,
            _ => joined.push((s, e)),
        }
    }
    joined
}

/// Render one page to a grayscale JPEG and return it as a data URI.
fn render_page_data_uri(pdf: &str, page: usize) -> Result<String, String> {
    render_page_at(pdf, page, ocr_dpi())
}

/// Second resolution for pages whose listings came back short (see `rows_short_of_totals`):
/// at 200 dpi the model's image encoder gets its full token budget and reads both columns
/// of TD's check table; 300 dpi adds nothing (same token count).
const OCR_HIRES_DPI: u32 = 200;

fn render_page_at(pdf: &str, page: usize, dpi: u32) -> Result<String, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let prefix = dir.path().join("page");
    let p = page.to_string();
    let out = run(
        "pdftocairo",
        &["-jpeg", "-gray", "-r", &dpi.to_string(), "-f", &p, "-l", &p, "-singlefile", pdf, &prefix.to_string_lossy()],
    )?;
    if !out.status.success() {
        return Err(format!("pdftocairo failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let bytes = std::fs::read(prefix.with_extension("jpg")).map_err(|e| format!("Rendered page missing: {e}"))?;
    Ok(format!("data:image/jpeg;base64,{}", BASE64.encode(bytes)))
}

/// Render one page at `dpi` and cut it into its bands (see `page_bands`), each returned
/// as a JPEG data URI with a little white margin. One band means the page has no gap to
/// cut at; the caller then reads it whole.
fn render_page_bands(pdf: &str, page: usize, dpi: u32) -> Result<Vec<String>, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let prefix = dir.path().join("page");
    let p = page.to_string();
    let out = run("pdftocairo", &["-png", "-gray", "-r", &dpi.to_string(), "-f", &p, "-l", &p, "-singlefile", pdf, &prefix.to_string_lossy()])?;
    if !out.status.success() {
        return Err(format!("pdftocairo failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let img = image::open(prefix.with_extension("png")).map_err(|e| e.to_string())?.into_luma8();
    let (w, h) = img.dimensions();
    let min_gap = dpi / 4; // a quarter inch of white
    let min_band = dpi * 2 / 5;
    let bands = page_bands(&img, min_gap, min_band);
    let margin = dpi / 12;
    let mut uris = Vec::with_capacity(bands.len());
    for (s, e) in bands {
        let top = s.saturating_sub(margin);
        let bottom = (e + margin).min(h);
        let crop = image::imageops::crop_imm(&img, 0, top, w, bottom - top).to_image();
        let mut bytes: Vec<u8> = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90).encode_image(&crop).map_err(|e| e.to_string())?;
        uris.push(format!("data:image/jpeg;base64,{}", BASE64.encode(bytes)));
    }
    Ok(uris)
}

/// GLM-OCR task prompt. "Text Recognition:" is the model's plain-text task.
const OCR_PROMPT: &str = "Text Recognition:";

/// GLM-OCR table task: returns the page's table as HTML, every cell kept.
const OCR_TABLE_PROMPT: &str = "Table Recognition:";

/// Read one scanned page. Plain text first; when rows under a transaction table header
/// come back without amounts (the text task drops cells of wrapped rows), the table task
/// is run as well and its rows replace the table in the text.
/// Raw model outputs for one page, cached separately so post-processing can change
/// without re-reading the page. `text` is the plain task; `table` the table task.
struct RawOcr {
    text: Option<PathBuf>,
    table: Option<PathBuf>,
    /// Banded reading at `OCR_HIRES_DPI` (see `ocr_page`), only for pages whose first
    /// reading came back short or nearly empty.
    hires: Option<PathBuf>,
}

impl RawOcr {
    fn for_page(cache: Option<&(PathBuf, String)>, page: usize) -> RawOcr {
        let model = super::registry::ocr_model().id;
        let base = cache.map(|(dir, h)| dir.join(format!("{h}-p{page:03}-{}dpi-{model}", ocr_dpi())));
        let hires = cache.map(|(dir, h)| dir.join(format!("{h}-p{page:03}-{OCR_HIRES_DPI}dpi-{model}.bands.txt")));
        RawOcr {
            text: base.as_ref().map(|b| b.with_extension("txt")),
            table: base.as_ref().map(|b| b.with_extension("table.html")),
            hires,
        }
    }

    /// Text is cached; the table is only needed when the text lost amounts, and the
    /// higher-resolution read only when a listing fell short of its subtotal, so a page
    /// counts as cached when the text is there and every pass it calls for is there too.
    fn is_complete(&self) -> bool {
        match self.text.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => {
                let table_ok = !needs_table_pass(&text) || self.table.as_ref().map(|p| p.exists()).unwrap_or(false);
                let hires_ok = ledger::rows_short_of_totals(&text) == 0 && text.split_whitespace().count() >= SPARSE_PAGE_WORDS || self.hires.as_ref().map(|p| p.exists()).unwrap_or(false);
                table_ok && hires_ok
            }
            None => false,
        }
    }

    /// The plain text is cached, table or not.
    fn has_text(&self) -> bool {
        self.text.as_ref().map(|p| p.exists()).unwrap_or(false)
    }

    /// The table task would have to run and no engine will: the plain text stands in.
    fn table_missing_and_cached_only(&self) -> bool {
        cached_only() && !self.table.as_ref().map(|p| p.exists()).unwrap_or(false)
    }
}

/// MCA_OCR_CACHED_ONLY=1: corpus runs on a shared machine serve cached pages only and
/// never start the engine. A page whose text is cached but whose table pass is not is
/// read from the text alone: a partial page still beats a garbled court text layer, and
/// the server worker fills the table in later.
fn cached_only() -> bool {
    std::env::var("MCA_OCR_CACHED_ONLY").is_ok()
}

/// The plain text task is not trusted on its own when rows under a table header lost
/// their amounts, or when it returned almost nothing: on a Wells Fargo page of 30 credit
/// rows it once produced only the two "Total ..." lines below the table.
fn needs_table_pass(text: &str) -> bool {
    // "- 04/18: CCD DEPOSIT ...: 3,176.12" bullets are a table retold as prose; the rest of
    // such a page (other sections, headers) is usually gone with it.
    let bullets = text.lines().filter(|l| {
        let l = l.trim_start();
        l.starts_with("- ") && l[2..].split(':').next().map(|d| ledger::parse_date_token(d.trim()).is_some()).unwrap_or(false)
    }).count();
    ledger::rows_missing_amounts(text) > 0 || text.split_whitespace().count() < SPARSE_PAGE_WORDS || bullets >= 3
}

const SPARSE_PAGE_WORDS: usize = 60;

/// Run `prompt` on the page image, serving and filling the raw cache at `path`.
async fn ocr_cached(ep: &Endpoint, uri: &mut Option<String>, pdf: &str, page: usize, prompt: &str, path: Option<&PathBuf>, progress: &PageProgress, what: &str) -> Result<String, String> {
    if let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) {
        return Ok(text);
    }
    if uri.is_none() {
        *uri = Some(render_page_data_uri(pdf, page)?);
    }
    let out = ocr_prompt(ep, uri.as_deref().unwrap(), prompt, progress, what).await?;
    if let Some(p) = path {
        let _ = std::fs::write(p, &out);
    }
    Ok(out)
}

/// Progress sink for one page: `(what, tokens so far)`; the pipeline turns it into UI events.
type PageProgress = std::sync::Arc<dyn Fn(&str, usize) + Send + Sync>;

async fn ocr_page(ep: &Endpoint, pdf: &str, page: usize, raw: &RawOcr, progress: &PageProgress) -> Result<String, String> {
    let (text, raw_words) = ocr_page_passes(ep, pdf, page, raw, progress).await?;
    // Two signs the model dropped rows it could see: a listing that adds up to less than
    // its printed subtotal (the right column of TD's two-column check table), and a plain
    // reading that came back nearly empty (a Wells page where it stops after the first
    // table's total). GLM-OCR is a region recognizer, so the page is then read the way its
    // own SDK reads it: cut into bands at the white gaps between sections, each band on its
    // own at 200 dpi, the results joined in order. The banded reading stands when it is
    // short in fewer listings, or when it carries more dated rows than a sparse first
    // reading. Rows are never merged across readings.
    let short = ledger::rows_short_of_totals(&text);
    let sparse = raw_words < SPARSE_PAGE_WORDS;
    if short == 0 && !sparse || (cached_only() && !raw.hires.as_ref().map(|p| p.exists()).unwrap_or(false)) {
        return Ok(text);
    }
    let why = if short > 0 { format!("{short} listing(s) short of the printed subtotal") } else { "plain reading nearly empty".to_string() };
    println!("[Engine] page {page}: {why}, reading the page in bands at {OCR_HIRES_DPI} dpi");
    progress(&format!("{why}, reading the page in bands at {OCR_HIRES_DPI} dpi"), 0);
    let banded = match raw.hires.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(cached) => cached,
        None => {
            let uris = render_page_bands(pdf, page, OCR_HIRES_DPI)?;
            let mut parts = Vec::with_capacity(uris.len());
            for (i, uri) in uris.iter().enumerate() {
                let out = ocr_prompt(ep, uri, OCR_PROMPT, progress, &format!("reading band {} of {}", i + 1, uris.len())).await?;
                parts.push(out.trim().to_string());
            }
            let out = parts.join("\n\n");
            if let Some(p) = raw.hires.as_ref() {
                let _ = std::fs::write(p, &out);
            }
            out
        }
    };
    let banded = super::ocr_table::expand_markdown_tables(&super::ocr_table::expand_html_tables(&banded));
    let dated = |t: &str| t.lines().filter(|l| l.split_whitespace().next().and_then(ledger::parse_date_token).is_some()).count();
    let better = ledger::rows_short_of_totals(&banded) < short || sparse && dated(&banded) > dated(&text);
    if better && banded.split_whitespace().count() * 2 >= text.split_whitespace().count() {
        return Ok(banded);
    }
    println!("[Engine] page {page}: the banded reading did not come closer; keeping the first");
    Ok(text)
}

/// Plain text task, then the table task when the text lost amounts (see below). Returns
/// the page text and the word count of the plain task's own output.
async fn ocr_page_passes(ep: &Endpoint, pdf: &str, page: usize, raw: &RawOcr, progress: &PageProgress) -> Result<(String, usize), String> {
    let mut uri: Option<String> = None; // rendered once, only when a task is not cached
    let prompt = std::env::var("MCA_OCR_PROMPT").unwrap_or_else(|_| OCR_PROMPT.to_string()); // testing aid
    let text = ocr_cached(ep, &mut uri, pdf, page, &prompt, raw.text.as_ref(), progress, "reading").await?;
    let raw_words = text.split_whitespace().count();
    // The text task sometimes returns tables as Markdown; turn those into aligned lines.
    let text = super::ocr_table::expand_markdown_tables(&text);
    if !needs_table_pass(&text) || prompt != OCR_PROMPT || raw.table_missing_and_cached_only() {
        return Ok((text, raw_words));
    }
    let missing = ledger::rows_missing_amounts(&text);
    let why = if missing > 0 { format!("{missing} table row(s) lost their amounts in plain OCR") } else { "plain OCR returned almost nothing".to_string() };
    println!("[Engine] page {page}: {why}, reading the table");
    progress(&format!("{why}, reading the table"), 0);
    let html = ocr_cached(ep, &mut uri, pdf, page, OCR_TABLE_PROMPT, raw.table.as_ref(), progress, "reading the table").await?;
    match super::ocr_table::table_html_to_layout(&html) {
        // The text task keeps every row but drops some amounts; the table task keeps the
        // amounts in their columns but may lose the last rows. A table with at least as
        // many rows as the text replaces the block (its columns decide credit or debit);
        // a shorter one only lends its amounts to the rows that lost theirs.
        Some(table) => {
            // A table is complete when it reaches the text's last dated row; one that stops
            // earlier lost the bottom of the page (Truist). Row counts alone mislead: the
            // text task sometimes adds a line of its own.
            let last_row = |s: &str| -> Option<String> {
                s.lines().rev().find(|l| l.split_whitespace().next().and_then(ledger::parse_date_token).is_some()).map(|l| l.split_whitespace().take(4).collect::<Vec<_>>().join(" ").to_ascii_lowercase())
            };
            // (A table of some other block, TD's check table above the wrapped payments,
            // ends elsewhere and so never replaces the text.)
            let complete = last_row(&table).is_some() && last_row(&table) == last_row(&text);
            if complete {
                return Ok((splice_table(&text, &table), raw_words));
            }
            // The text task kept only the lines below the table ("Total credits: ..."): the
            // table is the page's body, the text its tail.
            if last_row(&text).is_none() && last_row(&table).is_some() {
                return Ok((format!("{table}\n{text}"), raw_words));
            }
            let patched = patch_missing_amounts(&text, &table);
            if ledger::rows_missing_amounts(&patched) > 0 {
                println!("[Engine] page {page}: {} row(s) still without an amount after the table pass", ledger::rows_missing_amounts(&patched));
            }
            Ok((patched, raw_words))
        }
        None => Ok((text, raw_words)),
    }
}

/// Give date-first rows that lost their amount in the text task the amount of the table
/// row with the same date and description start. On a flat (plain OCR) page the amount is
/// appended with one space so the page stays flat; on an aligned page the table's own
/// line replaces the row, so the amount sits under its column.
fn patch_missing_amounts(text: &str, table: &str) -> String {
    let flat = ledger::is_flat(text);
    // (date, description words, amount, whole line) per table row.
    let rows: Vec<(String, Vec<String>, String, &str)> = table
        .lines()
        .filter_map(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            let date = toks.first().filter(|t| ledger::parse_date_token(t).is_some())?;
            let amounts: Vec<&&str> = toks.iter().filter(|t| ledger::is_amount_token(t)).collect();
            let amount = amounts.first()?;
            let words = toks[1..].iter().filter(|t| !ledger::is_amount_token(t)).map(|t| t.to_ascii_lowercase()).collect();
            Some((date.to_string(), words, amount.to_string(), l))
        })
        .collect();
    let mut used = vec![false; rows.len()];
    let mut out = String::with_capacity(text.len() + 64);
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let dated = toks.first().map(|t| ledger::parse_date_token(t).is_some()).unwrap_or(false);
        let has_amount = toks.iter().any(|t| ledger::is_amount_token(t));
        if dated && !has_amount && toks.len() >= 3 {
            let words: Vec<String> = toks[1..].iter().map(|t| t.to_ascii_lowercase()).collect();
            // Same date, and the first six description words agree (three when the row is
            // that short): "Purchase authorized on" alone would match every card row.
            let hit = rows.iter().enumerate().find(|(i, (d, w, _, _))| {
                let k = w.len().min(words.len()).min(6);
                !used[*i] && *d == toks[0] && k >= 3 && w[..k] == words[..k]
            });
            if let Some((i, (_, _, amount, table_line))) = hit {
                used[i] = true;
                if flat {
                    out.push_str(&format!("{line} {amount}\n"));
                } else {
                    out.push_str(table_line);
                    out.push('\n');
                }
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Replace the transaction table in plain OCR `text` (from its header line through the
/// last date-first line) with the converted `table`.
fn splice_table(text: &str, table: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let is_date_row = |l: &&str| l.split_whitespace().next().and_then(ledger::parse_date_token).is_some();
    let header = lines.iter().position(|l| {
        let lower = l.to_ascii_lowercase();
        let kind_words = lower.contains("deposit") || lower.contains("credit") || lower.contains("debit") || lower.contains("withdrawal");
        // "Date Description Deposits Withdrawals Balance", or Truist's "DATE DESCRIPTION AMOUNT($)".
        lower.contains("date") && (lower.contains("balance") || lower.contains("amount")) && (kind_words || lower.split_whitespace().count() <= 8)
    });
    // No header: the table replaces the run of dated rows; with no dated rows it is appended.
    let Some(h) = header.or_else(|| lines.iter().position(is_date_row)) else { return format!("{text}\n{table}") };
    let last_row = lines.iter().rposition(is_date_row).unwrap_or(h);
    let mut out = String::new();
    for l in &lines[..h] {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(table);
    for l in &lines[last_row.max(h) + 1..] {
        out.push_str(l);
        out.push('\n');
    }
    out
}

async fn ocr_prompt(ep: &Endpoint, uri: &str, prompt: &str, progress: &PageProgress, what: &str) -> Result<String, String> {
    let msgs = [Message { role: "user", text: prompt, image_data_uri: Some(uri) }];
    let opts = ChatOptions {
        model: "ocr",
        temperature: 0.0,
        max_tokens: 4096,
        json_schema: None,
        enable_thinking: false,
        idle_timeout: Duration::from_secs(300),
    };
    // Heartbeat every ~3 s while tokens stream, so a long page never looks stuck.
    let tokens = std::sync::atomic::AtomicUsize::new(0);
    let last = std::sync::Mutex::new(Instant::now());
    progress(what, 0);
    let r = llama::chat(ep, &msgs, &opts, |_, _| {
        let n = tokens.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if let Ok(mut l) = last.lock() {
            if l.elapsed() >= Duration::from_secs(3) {
                *l = Instant::now();
                progress(what, n);
            }
        }
    })
    .await?;
    Ok(r.content)
}

/// How a page is read: "text", "ocr" or "blank".
/// Pages with a usable text layer use it, even court-filing scans whose layer came from
/// someone else's OCR: it costs nothing. The OCR model reads pages whose layer is thin and
/// that carry an image (a scan), or that have no text at all. Near-blank pages are skipped.
pub fn page_method(pdf: &str, page: usize, layer: &str, force_ocr: bool) -> &'static str {
    let words = layer.split_whitespace().count();
    // A scan whose text layer is someone else's failed OCR (",S,1=.or:2~ho1,Jr-a~11nl"):
    // when a quarter or more of its tokens carry characters no statement prints, the
    // layer is worthless and the page is read as a scan.
    let junk = |t: &str| !t.chars().all(|c| c.is_ascii_alphanumeric() || ".,/$#:%()'*&+\"-".contains(c));
    let junk_layer = words >= MIN_TEXT_WORDS && layer.split_whitespace().filter(|t| junk(t)).count() * 4 >= words;
    let thin = words < MIN_TEXT_WORDS || junk_layer && has_page_image(pdf, page);
    // A thin page with no image object can still be a scan drawn as vector outlines (some
    // court filings convert the scan): ink well beyond a stamp's worth says so.
    // (Such a page is never blank, whatever its share of black pixels: a sparse last page
    // of four rows has 3.4 percent of mid-grey ink and well under the blank share of black.)
    // (A faint scan image of a sparse last page, four rows and a footer, has the same
    // share of black; it is blank only when its mid-grey ink is a stamp's worth too.)
    let inked_vector = || page_ink_ratio_below(pdf, page, 200).map(|r| r >= VECTOR_SCAN_INK_RATIO).unwrap_or(false);
    let grey_blank = || page_ink_ratio_below(pdf, page, 200).map(|r| r < BLANK_GREY_INK_RATIO).unwrap_or(true);
    if force_ocr || words == 0 || thin && (has_page_image(pdf, page) || inked_vector()) {
        if !force_ocr && page_ink_ratio(pdf, page).map(|r| r < BLANK_INK_RATIO).unwrap_or(false) && grey_blank() { "blank" } else { "ocr" }
    } else {
        "text"
    }
}

/// Share of pixels under 200 above which a page with almost no text layer is treated as a
/// scan even without an image object; a court stamp alone is about half a percent, a page
/// of statement text six percent or more.
const VECTOR_SCAN_INK_RATIO: f64 = 0.03;

/// Share of pixels under 200 below which a page with almost no black is blank. A faint scan
/// of a sparse page (a fee continuation and the daily balances, Bank of America) has 0.4
/// percent of black and 2.3 percent of mid-grey; the court's stamp alone is half a percent.
const BLANK_GREY_INK_RATIO: f64 = 0.015;


/// Error returned when a page needs the OCR model but no engine endpoint was given.
pub const NEEDS_ENGINE: &str = "scanned pages need the engine";

/// Where OCR page text is kept so a statement is read by the model once. Keyed by the
/// file's content hash, page, DPI and OCR model, so an edited file or a model change
/// misses the cache. `MCA_OCR_CACHE=<dir>` overrides the location (corpus work).
fn ocr_cache_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = match std::env::var("MCA_OCR_CACHE") {
        Ok(d) => PathBuf::from(d),
        Err(_) => super::runtime::engine_dir(app).ok()?.join("ocr-cache"),
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn file_hash(pdf: &str) -> Option<String> {
    let bytes = std::fs::read(pdf).ok()?;
    Some(format!("{:x}", Sha256::digest(&bytes))[..16].to_string())
}

/// Stage 1 for one file. Decides per page whether the text layer is enough, then reads
/// the OCR pages concurrently. Emits `analysis-progress` page events on `app`.
pub async fn extract_pages(
    app: &tauri::AppHandle,
    ep: Option<&Endpoint>,
    pdf: &str,
    page_offset: usize,
    total_pages: usize,
) -> Result<Vec<PageText>, String> {
    let file_name = Path::new(pdf).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let n = page_count(pdf)?;
    let force_ocr = std::env::var("MCA_FORCE_OCR").is_ok(); // testing aid
    let mut pages: Vec<PageText> = Vec::with_capacity(n);
    let mut ocr_queue: Vec<usize> = Vec::new();

    let cache = ocr_cache_dir(app).and_then(|dir| file_hash(pdf).map(|h| (dir, h)));
    let tessdata = tessdata_dir(app);
    for page in 1..=n {
        let current = page_offset + page;
        let started = Instant::now();
        let mut layer = text_layer(pdf, page)?;
        let mut method = page_method(pdf, page, &layer, force_ocr);
        // A scanned page is read by classic OCR first (a second or two, exact on a clean
        // scan); the model reads it only when the totals say the page needs it.
        if method == "ocr" && !force_ocr {
            if let Some(text) = tesseract_layer(cache.as_ref(), tessdata.as_deref(), pdf, page) {
                layer = text;
                method = "tesseract";
            }
        }
        if method == "ocr" {
            ocr_queue.push(page);
            let _ = app.emit("analysis-progress", json!({
                "type": "page_start", "current_page": current, "total_pages": total_pages,
                "message": format!("{file_name} page {page} of {n}: scanned, queued for OCR")
            }));
        } else {
            let seconds = started.elapsed().as_secs_f32();
            emit_page_done(app, &file_name, page, n, current, total_pages, method, seconds);
        }
        pages.push(PageText { file_name: file_name.clone(), page, method, seconds: started.elapsed().as_secs_f32(), text: layer });
    }

    ocr_into(app, ep, pdf, &mut pages, &ocr_queue, page_offset, total_pages).await?;
    dump_pages(&pages, "");
    Ok(pages)
}

/// Resolution for classic OCR: Tesseract wants about 300 dpi for 10-point print.
const TESSERACT_DPI: u32 = 300;

/// Where Tesseract's language data lives: the engine directory's `tessdata` (the app
/// downloads `eng.traineddata` there), else whatever the system Tesseract finds itself.
fn tessdata_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = super::runtime::engine_dir(app).ok()?.join("tessdata");
    dir.join("eng.traineddata").exists().then_some(dir)
}

/// Classic OCR of a scanned page (Tesseract, when installed): deterministic, a second or
/// two on the CPU, and exact on a clean scan, where the vision model drops rows. None
/// when Tesseract is missing, fails, or reads fewer than `MIN_TEXT_WORDS` words (a
/// garbled or faint scan the model must read). Cached next to the model's readings.
/// `MCA_NO_TESSERACT=1` skips it (testing aid).
fn tesseract_layer(cache: Option<&(PathBuf, String)>, tessdata: Option<&Path>, pdf: &str, page: usize) -> Option<String> {
    if std::env::var("MCA_NO_TESSERACT").is_ok() {
        return None;
    }
    let path = cache.map(|(dir, h)| dir.join(format!("{h}-p{page:03}-{TESSERACT_DPI}dpi-tesseract.txt")));
    if let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
        return (text.split_whitespace().count() >= MIN_TEXT_WORDS).then_some(text);
    }
    let dir = tempfile::tempdir().ok()?;
    let prefix = dir.path().join("page");
    let p = page.to_string();
    let out = run("pdftocairo", &["-png", "-gray", "-r", &TESSERACT_DPI.to_string(), "-f", &p, "-l", &p, "-singlefile", pdf, &prefix.to_string_lossy()]).ok()?;
    if !out.status.success() {
        return None;
    }
    let png = prefix.with_extension("png");
    whiten_redactions(&png);
    let mut cmd = Command::new("tesseract");
    cmd.arg(&png).arg("stdout").args(["--psm", "6"]);
    if let Some(td) = tessdata {
        cmd.env("TESSDATA_PREFIX", td);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if let Some(p) = path {
        let _ = std::fs::write(p, &text);
    }
    (text.split_whitespace().count() >= MIN_TEXT_WORDS).then_some(text)
}

/// Paint the solid black boxes of a redacted court copy, and the page's horizontal rules,
/// white before Tesseract reads the page: a box on a row makes Tesseract drop that row and its neighbours ("8/20 [box]
/// 500.00  8,894.39" and the wire under it, Brookline). A box is a run of 8x8 blocks that
/// are nearly all ink, at least 100 px wide and 32 px tall at 300 dpi; type never fills
/// such blocks. In place; a page that cannot be read or has no boxes is left alone.
fn whiten_redactions(png: &Path) {
    let Ok(img) = image::open(png) else { return };
    let mut gray = img.into_luma8();
    let (w, h) = gray.dimensions();
    let (bw, bh) = ((w / 8) as usize, (h / 8) as usize);
    if bw == 0 || bh == 0 {
        return;
    }
    // Solid blocks: 60 or more of the 64 pixels darker than 64.
    let mut solid = vec![false; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let mut dark = 0;
            for y in 0..8 {
                for x in 0..8 {
                    if gray.get_pixel(bx as u32 * 8 + x, by as u32 * 8 + y).0[0] < 64 {
                        dark += 1;
                    }
                }
            }
            solid[by * bw + bx] = dark >= 60;
        }
    }
    // Connected runs of solid blocks (4-neighbour flood fill); keep the box-sized ones.
    let mut seen = vec![false; bw * bh];
    let mut boxes: Vec<(usize, usize, usize, usize)> = Vec::new();
    for start in 0..bw * bh {
        if !solid[start] || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        let (mut x0, mut y0, mut x1, mut y1) = (bw, bh, 0, 0);
        let mut n = 0;
        while let Some(i) = stack.pop() {
            if seen[i] || !solid[i] {
                continue;
            }
            seen[i] = true;
            n += 1;
            let (x, y) = (i % bw, i / bw);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            if x > 0 { stack.push(i - 1); }
            if x + 1 < bw { stack.push(i + 1); }
            if y > 0 { stack.push(i - bw); }
            if y + 1 < bh { stack.push(i + bw); }
        }
        let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
        // At least 100 px wide, 32 px tall, and all but filled (a redaction is solid; a
        // heading bar with white type in it, "Statement Summary", is not, and Tesseract
        // reads such bars better as they are).
        if cw >= 13 && ch >= 4 && n * 100 >= cw * ch * 97 {
            boxes.push((x0, y0, x1, y1));
        }
    }
    // Horizontal rules: a contiguous dark span (gaps of three pixels allowed) across 25
    // percent of the width or more. The rule under a table header makes Tesseract drop
    // the first row beneath it ("11/3 Zelle From Adam Spencer Willmouth ... 3,500.00",
    // Wells), so the span goes, a pixel each side. Only the span: a rule that runs into a
    // heading's box (Chase's "DAILY ENDING BALANCE") must not cut through the letters.
    let mut rules = 0;
    let min_span = w / 4;
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if gray.get_pixel(x, y).0[0] >= 128 {
                x += 1;
                continue;
            }
            let start = x;
            let mut last_dark = x;
            while x < w && x - last_dark <= 3 {
                if gray.get_pixel(x, y).0[0] < 128 {
                    last_dark = x;
                }
                x += 1;
            }
            if last_dark - start + 1 >= min_span {
                for yy in y.saturating_sub(1)..(y + 2).min(h) {
                    for xx in start..=last_dark {
                        gray.put_pixel(xx, yy, image::Luma([255]));
                    }
                }
                rules += 1;
            }
        }
    }
    if boxes.is_empty() && rules == 0 {
        return;
    }
    for (x0, y0, x1, y1) in boxes {
        // One block of margin so the box's anti-aliased edge goes too.
        let (px0, py0) = (x0.saturating_sub(1) as u32 * 8, y0.saturating_sub(1) as u32 * 8);
        let (px1, py1) = (((x1 + 2) as u32 * 8).min(w), ((y1 + 2) as u32 * 8).min(h));
        for y in py0..py1 {
            for x in px0..px1 {
                gray.put_pixel(x, y, image::Luma([255]));
            }
        }
    }
    let _ = gray.save(png);
}

/// A page nobody could read (a scan with no OCR, or a page the court blacked out entirely,
/// whose OCR holds nothing but the stamp) leaves its statement incomplete: whatever rows
/// it carried are not in the totals. The headless dump applies the same rule.
pub fn mark_unreadable_pages(ledger: &mut ledger::Ledger, pages: &[PageText]) {
    let unreadable: Vec<usize> = pages.iter().enumerate().filter(|(_, p)| {
        let words = p.text.lines().skip(2).flat_map(|l| l.split_whitespace()).count();
        p.method == "scan" || p.method != "text" && p.method != "blank" && words < 5
    }).map(|(i, _)| i + 1).collect();
    for st in &mut ledger.statements {
        if let Some((a, b)) = st.pages {
            if unreadable.iter().any(|p| (a..=b).contains(p)) {
                st.missing_pages = true;
            }
        }
    }
    if ledger.statements.is_empty() && !unreadable.is_empty() {
        ledger.summary.missing_pages = true;
    }
}

/// Testing aid: MCA_DUMP_PAGES=<dir> writes every page text to disk for parser work
/// (`suffix` tells the OCR re-read apart from the first pass).
pub fn dump_pages(pages: &[PageText], suffix: &str) {
    if let Ok(dir) = std::env::var("MCA_DUMP_PAGES") {
        let _ = std::fs::create_dir_all(&dir);
        for p in pages {
            let _ = std::fs::write(format!("{dir}/{}-p{:02}-{}{suffix}.txt", p.file_name, p.page, p.method), &p.text);
        }
    }
}

/// Read `queue` (1-based page numbers of `pdf`) with the OCR model, `OCR_CONCURRENCY` at a
/// time, and store the text into `pages`. Cached pages are served from disk.
async fn ocr_into(app: &tauri::AppHandle, ep: Option<&Endpoint>, pdf: &str, pages: &mut [PageText], queue: &[usize], page_offset: usize, total_pages: usize) -> Result<(), String> {
    if queue.is_empty() {
        return Ok(());
    }
    let cache = ocr_cache_dir(app).and_then(|dir| file_hash(pdf).map(|h| (dir, h)));
    // Without a running engine only cached pages can be served. MCA_OCR_CACHED_ONLY=1
    // (corpus runs on a shared machine) skips the rest instead of starting the engine.
    let mut queue: Vec<usize> = queue.to_vec();
    let ep = match ep {
        Some(ep) => ep.clone(),
        None => {
            let cached = |p: &usize| {
                let raw = RawOcr::for_page(cache.as_ref(), *p);
                cache.is_some() && if cached_only() { raw.has_text() } else { raw.is_complete() }
            };
            if !queue.iter().all(cached) {
                if !cached_only() {
                    return Err(NEEDS_ENGINE.to_string());
                }
                for &p in queue.iter().filter(|p| !cached(p)) {
                    pages[p - 1].method = "scan";
                }
                queue.retain(cached);
                if queue.is_empty() {
                    return Ok(());
                }
            }
            Endpoint::default()
        }
    };
    let queue = &queue[..];
    let file_name = pages.first().map(|p| p.file_name.clone()).unwrap_or_default();
    let n = pages.len();
    let ocr_start = Instant::now();
    // Slots come from the memory plan the engine started with; MCA_OCR_CONCURRENCY
    // lowers it further when the machine is shared (corpus runs).
    let planned = app.state::<super::runtime::EngineProcess>().ocr_parallel() as usize;
    let concurrency = std::env::var("MCA_OCR_CONCURRENCY").ok().and_then(|v| v.parse().ok()).unwrap_or(planned).clamp(1, planned.max(1));
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    println!("[Engine] {file_name}: {} page(s) to OCR, {concurrency} at a time", queue.len());
    let mut tasks = Vec::new();
    for &page in queue {
        let sem = sem.clone();
        let ep = ep.clone();
        let pdf = pdf.to_string();
        let raw = RawOcr::for_page(cache.as_ref(), page);
        let app_for_wait = app.clone();
        let (app_p, file_p, started_p) = (app.clone(), file_name.clone(), Instant::now());
        let progress: PageProgress = std::sync::Arc::new(move |what: &str, tokens: usize| {
            let secs = started_p.elapsed().as_secs();
            let detail = if tokens > 0 { format!("{what}, {tokens} tokens, {secs}s") } else { what.to_string() };
            let _ = app_p.emit("analysis-progress", json!({
                "type": "page_start", "current_page": page_offset + page, "total_pages": total_pages,
                "message": format!("{file_p} page {page} of {n}: {detail}")
            }));
        });
        tasks.push(tokio::spawn(async move {
            let started = Instant::now();
            if raw.is_complete() {
                let text = ocr_page(&ep, &pdf, page, &raw, &progress).await?;
                return Ok::<(usize, String, f32), String>((page, text, 0.0));
            }
            let _permit = sem.acquire().await.map_err(|e| e.to_string())?;
            // A busy machine slows the job down; the job never pushes the machine over.
            let waited = memory::wait_for_room(Duration::from_secs(180), |avail| {
                let _ = app_for_wait.emit("analysis-progress", json!({
                    "type": "page_start", "current_page": page_offset + page, "total_pages": total_pages,
                    "message": format!("Waiting for memory before page {page}: {:.1} GB free, {:.1} GB needed", avail as f64 / 1e9, memory::SOFT_FLOOR_BYTES as f64 / 1e9)
                }));
            }).await?;
            if waited.as_secs() >= 1 {
                println!("[Engine] page {page}: waited {:.0}s for memory", waited.as_secs_f32());
            }
            let text = ocr_page(&ep, &pdf, page, &raw, &progress).await?;
            Ok::<(usize, String, f32), String>((page, text, started.elapsed().as_secs_f32()))
        }));
    }
    for task in tasks {
        let (page, text, seconds) = task.await.map_err(|e| format!("OCR task failed: {e}"))??;
        let entry = &mut pages[page - 1];
        entry.text = text;
        entry.seconds = seconds;
        entry.method = "ocr";
        emit_page_done(app, &file_name, page, n, page_offset + page, total_pages, "ocr", seconds);
    }
    println!("[Engine] {file_name}: OCR of {} page(s) took {:.1}s wall", queue.len(), ocr_start.elapsed().as_secs_f32());
    Ok(())
}

/// Stage 1 for a whole job, with verification. Reads every file, parses the ledger and
/// compares the parsed totals with the totals the bank printed. When they disagree and
/// some pages came from a text layer sitting on top of a full-page image (a scan carrying
/// someone else's OCR, common in court filings), those pages are re-read with the OCR
/// model and the better-matching parse is kept. Digital statements never pay for OCR.
pub async fn read_pages(app: &tauri::AppHandle, ep: Option<&Endpoint>, pdfs: &[String], total_pages: usize) -> Result<Vec<PageText>, String> {
    let mut pages: Vec<PageText> = Vec::with_capacity(total_pages);
    let mut offsets = Vec::new();
    let mut ep: Option<Endpoint> = ep.cloned();
    for pdf in pdfs {
        offsets.push(pages.len());
        let offset = pages.len();
        // If the engine was stopped mid-file (memory watchdog), wait for memory to come
        // back, restart it and read the file again: pages already read come from the cache.
        let mut attempts = 0;
        let extracted = loop {
            match extract_pages(app, ep.as_ref(), pdf, offset, total_pages).await {
                Ok(p) => break p,
                Err(e) => {
                    attempts += 1;
                    let stopped = app.state::<super::runtime::EngineProcess>().stopped_reason.lock().ok().and_then(|g| g.clone());
                    match stopped {
                        Some(reason) if attempts <= 3 && ep.is_some() => {
                            println!("[Engine] interrupted: {reason}; waiting for memory, then resuming");
                            ep = Some(resume_engine(app, &reason, total_pages).await?);
                        }
                        _ => return Err(e),
                    }
                }
            }
        };
        pages.extend(extracted);
    }
    // No printed totals at all is a gap too when the text layer is court OCR so poor that
    // not even the balances survive ("Eeglnnirq Balance"): every image-backed page is
    // re-read and adopted as soon as it yields a summary.
    // (A classic-OCR reading with no printed totals is unverified too: the model may read
    // the summary the scan garbled for Tesseract.)
    let (gap, verified, passing) = match totals_gap(&pages) {
        Some((g, _, _)) if g <= 1.0 => return Ok(pages),
        Some((g, n, p)) => (g, n, p),
        None if has_no_balances(&pages) || pages.iter().any(|p| p.method == "tesseract") => (f64::INFINITY, 0, 0),
        None => return Ok(pages),
    };
    // First a second reading by classic OCR of every page that sits on an image (a court
    // copy's text layer is someone else's poor OCR): a second per page, adopted page by
    // page and statement by statement where it brings the totals closer. Often that is
    // enough and the model never runs.
    let cache_for = |pdf: &str| ocr_cache_dir(app).and_then(|dir| file_hash(pdf).map(|h| (dir, h)));
    let tessdata = tessdata_dir(app);
    let mut tess = pages.clone();
    let mut tess_pages = 0;
    for (pdf, &offset) in pdfs.iter().zip(&offsets) {
        let n = page_count(pdf)?;
        let cache = cache_for(pdf);
        for p in &mut tess[offset..offset + n] {
            // (A vector text page whose layer dropped a row's amount, "04/09/2025  Square Inc
            // SQ250409  [blank]  $715,889.24", is rendered and read as well: the glyphs are
            // on the page, only the text layer lost them.)
            if p.method == "text" && (has_page_image(pdf, p.page) || ledger::rows_missing_amounts(&p.text) > 0) {
                if let Some(text) = tesseract_layer(cache.as_ref(), tessdata.as_deref(), pdf, p.page) {
                    p.text = text;
                    p.method = "tesseract";
                    tess_pages += 1;
                }
            }
        }
    }
    let (mut best, mut best_gap, mut best_verified, mut best_passing) = (pages.clone(), gap, verified, passing);
    if tess_pages > 0 {
        dump_pages(&tess, "-candidate");
        let _ = app.emit("analysis-progress", json!({
            "type": "page_start", "current_page": 0, "total_pages": total_pages,
            "message": format!("Totals do not match the statement summary (off by {gap:.2}); re-reading {tess_pages} scanned page(s) with classic OCR")
        }));
        let (b, g, n, p) = adopt_readings(best, &tess, "tesseract", best_gap, best_verified, best_passing);
        let adopted = b.iter().filter(|p| p.method == "tesseract").count() - pages.iter().filter(|p| p.method == "tesseract").count();
        if adopted > 0 {
            println!("[Engine] classic OCR improved the totals gap from {best_gap:.2} to {g:.2} using {adopted} page(s)");
        }
        (best, best_gap, best_verified, best_passing) = (b, g, n, p);
        if best_gap <= 1.0 {
            return Ok(best);
        }
    }
    // Then the model, on the pages that still sit on an image, wherever the totals are
    // still off.
    let mut retry = best.clone();
    let mut queued = 0;
    for (pdf, &offset) in pdfs.iter().zip(&offsets) {
        let n = page_count(pdf)?;
        let slice = &mut retry[offset..offset + n];
        let queue: Vec<usize> = slice.iter().filter(|p| p.method == "tesseract" || p.method == "text" && has_page_image(pdf, p.page)).map(|p| p.page).collect();
        if queue.is_empty() {
            continue;
        }
        queued += queue.len();
        let _ = app.emit("analysis-progress", json!({
            "type": "page_start", "current_page": offset, "total_pages": total_pages,
            "message": if best_gap.is_finite() { format!("Totals do not match the statement summary (off by {best_gap:.2}); re-reading {} scanned page(s) with the OCR model", queue.len()) } else { format!("No balances found in the text layer; re-reading {} scanned page(s) with the OCR model", queue.len()) }
        }));
        ocr_into(app, ep.as_ref(), pdf, slice, &queue, offset, total_pages).await?;
    }
    if queued == 0 {
        return Ok(best);
    }
    // The OCR model reads the body and drops the bank's "Page N of M" footer; the text
    // layer's footer line is kept so a copy with pages missing is still recognized.
    for (r, p) in retry.iter_mut().zip(&best) {
        if r.method == "ocr" && (p.method == "text" || p.method == "tesseract") && ledger::footer_line(&r.text).is_none() {
            if let Some(footer) = ledger::footer_line(&p.text) {
                r.text = format!("{}\n{}\n", r.text.trim_end(), footer.trim());
            }
        }
    }
    dump_pages(&retry, "-retry");
    let base = best.clone();
    let (b, g, n, p) = adopt_readings(best, &retry, "ocr", best_gap, best_verified, best_passing);
    (best, best_gap, best_verified, best_passing) = (b, g, n, p);
    let _ = (best_verified, best_passing);
    // Second look: a page adopted early, while a later statement's summary was still
    // unread, may have helped the wrong total (dropping rows of a statement that was over
    // because the pages behind it had not been split off yet). Each adopted page is put
    // back to its earlier reading once; the reversal stays when the gap gets smaller.
    for i in 0..retry.len() {
        if best[i].method != "ocr" || base[i].method == "ocr" {
            continue;
        }
        let mut candidate = best.clone();
        candidate[i] = base[i].clone();
        if let Some((g, n, p)) = totals_gap(&candidate) {
            if g < best_gap && n >= best_verified && p >= best_passing {
                best = candidate;
                best_gap = g;
                best_verified = n;
                best_passing = p;
            }
        }
    }
    let adopted = best.iter().filter(|p| p.method == "ocr").count() - pages.iter().filter(|p| p.method == "ocr").count();
    if adopted == 0 {
        println!("[Engine] the OCR model's re-read did not improve the totals gap ({best_gap:.2}); keeping the earlier readings");
        return Ok(base);
    }
    println!("[Engine] the OCR model's re-read improved the totals gap from {gap:.2} to {best_gap:.2} using {adopted} page(s)");
    // The OCR model reads the transaction body and may skip the letterhead; the bank's
    // name from the text layer is kept as a line of its own.
    let texts = |ps: &[PageText]| ps.iter().map(|p| p.text.clone()).collect::<Vec<_>>();
    let (ocr_texts, layer_texts) = (texts(&best), texts(&pages));
    let named = |ts: &[String]| ledger::detect_bank(&ts.iter().map(String::as_str).collect::<Vec<_>>());
    if named(&ocr_texts).is_none() {
        if let (Some(bank), Some(first)) = (named(&layer_texts), best.first_mut()) {
            first.text = format!("{bank}\n{}", first.text);
        }
    }
    Ok(best)
}

/// Mix a second reading into `base`: every page of `candidates` read by `method` is offered
/// page by page (kept only when the totals gap shrinks without verifying or passing fewer
/// statements), then every statement still off gets all of its remaining candidate pages
/// at once, then the single pages again in case the swap unlocked one. The text task can
/// lose credit rows on one page while fixing the debit rows of another, so readings are
/// mixed, never swapped wholesale. Returns the mix and its gap, verified and passing counts.
fn adopt_readings(mut best: Vec<PageText>, candidates: &[PageText], method: &str, mut best_gap: f64, mut best_verified: usize, mut best_passing: usize) -> (Vec<PageText>, f64, usize, usize) {
    let mut try_candidate = |best: &mut Vec<PageText>, candidate: Vec<PageText>, best_gap: &mut f64, best_verified: &mut usize, best_passing: &mut usize| -> bool {
        if let Some((g, n, p)) = totals_gap(&candidate) {
            // (Or the same gap with more printed figures or more statements passing: a
            // reading whose summary the classic OCR garbled, "(827.88 -$95,111. -$381.7",
            // gets its totals from the model's page at no cost to the rest.)
            let closer = g < *best_gap && n >= *best_verified && p >= *best_passing;
            let fuller = g <= *best_gap && n >= *best_verified && p >= *best_passing && (n > *best_verified || p > *best_passing);
            if closer || fuller {
                *best = candidate;
                *best_gap = g;
                *best_verified = n;
                *best_passing = p;
                return true;
            }
        }
        false
    };
    let offer_singles = |best: &mut Vec<PageText>, best_gap: &mut f64, best_verified: &mut usize, best_passing: &mut usize, try_candidate: &mut dyn FnMut(&mut Vec<PageText>, Vec<PageText>, &mut f64, &mut usize, &mut usize) -> bool| -> bool {
        let mut any = false;
        for i in 0..candidates.len() {
            if candidates[i].method != method || best[i].method == method {
                continue;
            }
            let mut candidate = best.clone();
            candidate[i] = candidates[i].clone();
            any |= try_candidate(best, candidate, best_gap, best_verified, best_passing);
        }
        any
    };
    offer_singles(&mut best, &mut best_gap, &mut best_verified, &mut best_passing, &mut try_candidate);
    let mut improved = true;
    while improved {
        improved = false;
        let refs: Vec<(usize, &str)> = best.iter().enumerate().map(|(i, p)| (i + 1, p.text.as_str())).collect();
        let parsed = ledger::parse(&refs);
        let mut ranges: Vec<(usize, usize)> = parsed.statements.iter().filter(|st| {
            let off = st.total_credits.map(|c| (c - st.parsed_credits.unwrap_or(0.0)).abs()).unwrap_or(0.0) + st.total_debits.map(|d| (d - st.parsed_debits.unwrap_or(0.0)).abs()).unwrap_or(0.0);
            off > 1.0
        }).filter_map(|st| st.pages).collect();
        // A single statement (no per-statement list) is offered whole as well: its summary
        // page and its row pages may each be wrong alone and right together.
        if parsed.statements.is_empty() && best_gap > 1.0 {
            ranges.push((1, candidates.len()));
        }
        for (first, last) in ranges {
            let mut candidate = best.clone();
            let mut swapped = 0;
            for i in first.saturating_sub(1)..last.min(candidates.len()) {
                if candidates[i].method == method && candidate[i].method != method {
                    candidate[i] = candidates[i].clone();
                    swapped += 1;
                }
            }
            if swapped > 0 && try_candidate(&mut best, candidate, &mut best_gap, &mut best_verified, &mut best_passing) {
                improved = true;
            }
        }
        if offer_singles(&mut best, &mut best_gap, &mut best_verified, &mut best_passing, &mut try_candidate) {
            improved = true;
        }
    }
    (best, best_gap, best_verified, best_passing)
}

/// After a watchdog stop: wait (up to five minutes) until the engine's plan fits in free
/// memory again, restart it, and hand back the new endpoint. Progress events keep the
/// user informed the whole time.
async fn resume_engine(app: &tauri::AppHandle, reason: &str, total_pages: usize) -> Result<Endpoint, String> {
    let started = Instant::now();
    loop {
        let cfg = super::runtime::load_config(app);
        let uw = super::registry::underwriter_model(&cfg.underwriter_model).ok_or("unknown reasoning model")?;
        let plan = memory::plan(&super::registry::ocr_model(), if super::headless::ledger_only() { None } else { Some(&uw) });
        if plan.fits {
            break;
        }
        if started.elapsed() > Duration::from_secs(300) {
            return Err(format!("{reason}\nMemory did not come back within five minutes; pages already read are kept, run again when the machine is less busy."));
        }
        let _ = app.emit("analysis-progress", json!({
            "type": "page_start", "current_page": 0, "total_pages": total_pages,
            "message": format!("Paused: {reason} Waiting for memory ({:.1} GB free)", plan.available_bytes as f64 / 1e9)
        }));
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    if let Ok(mut g) = app.state::<super::runtime::EngineProcess>().stopped_reason.lock() {
        *g = None;
    }
    let _ = app.emit("analysis-progress", json!({
        "type": "page_start", "current_page": 0, "total_pages": total_pages,
        "message": "Memory is back; restarting the engine and resuming"
    }));
    super::ensure_running(app).await
}

/// Neither a beginning nor an ending balance was read: no statement prints neither, so
/// the text layer is unusable.
fn has_no_balances(pages: &[PageText]) -> bool {
    let refs: Vec<(usize, &str)> = pages.iter().enumerate().map(|(i, p)| (i + 1, p.text.as_str())).collect();
    let s = ledger::parse(&refs).summary;
    s.beginning_balance.is_none() && s.ending_balance.is_none()
}

/// Sum of |stated - parsed| over the totals the statement prints; None when it prints none.
/// Sum of |stated - parsed| over the totals the statement prints, how many statements
/// printed totals, and how many of those are met to the cent; None when none printed any.
/// The distance between the printed totals and the parsed rows: (gap, printed figures,
/// statements passing). The figure count is every printed total found, so a reading that
/// garbles a statement's "Totals" line (its debits then unverified, gap 0) can never look
/// like an improvement over one that reads it.
fn totals_gap(pages: &[PageText]) -> Option<(f64, usize, usize)> {
    let refs: Vec<(usize, &str)> = pages.iter().enumerate().map(|(i, p)| (i + 1, p.text.as_str())).collect();
    let ledger = ledger::parse(&refs);
    let figures = |c: Option<f64>, d: Option<f64>| c.is_some() as usize + d.is_some() as usize;
    // A bundle is judged statement by statement: parts that print no totals (an online
    // activity printout filed behind the statement) neither count nor block the others.
    if ledger.statements.len() > 1 {
        let parts: Vec<(f64, usize)> = ledger.statements.iter().filter(|st| st.total_credits.is_some() || st.total_debits.is_some()).map(|st| {
            (st.total_credits.map(|c| (c - st.parsed_credits.unwrap_or(0.0)).abs()).unwrap_or(0.0) + st.total_debits.map(|d| (d - st.parsed_debits.unwrap_or(0.0)).abs()).unwrap_or(0.0), figures(st.total_credits, st.total_debits))
        }).collect();
        return if parts.is_empty() { None } else { Some((parts.iter().map(|p| p.0).sum(), parts.iter().map(|p| p.1).sum(), parts.iter().filter(|p| p.0 <= 1.0).count())) };
    }
    let s = &ledger.summary;
    if s.total_credits.is_none() && s.total_debits.is_none() {
        return None;
    }
    let gc = s.total_credits.map(|c| (c - ledger.parsed_credit_total).abs()).unwrap_or(0.0);
    let gd = s.total_debits.map(|d| (d - ledger.parsed_debit_total).abs()).unwrap_or(0.0);
    Some((gc + gd, figures(s.total_credits, s.total_debits), (gc + gd <= 1.0) as usize))
}

fn emit_page_done(app: &tauri::AppHandle, file_name: &str, page: usize, n: usize, current: usize, total_pages: usize, method: &str, seconds: f32) {
    println!("[Engine] {file_name} p{page}/{n}: {method} in {seconds:.1}s");
    let _ = app.emit("analysis-progress", json!({
        "type": "page_complete", "current_page": current, "total_pages": total_pages,
        "method": method, "seconds": seconds, "page_result": "",
        "message": format!("{file_name} page {page}: {} in {seconds:.1}s", match method { "ocr" => "OCR", "tesseract" => "classic OCR", "blank" => "blank page skipped", _ => "text layer" })
    }));
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

/// Lines in the first pages that look like a business name: an entity suffix or "dba",
/// and not the bank's own name. Used to override a model answer that names the bank.
fn merchant_name_candidates(pages: &[PageText]) -> Vec<String> {
    const SUFFIX: &[&str] = &[" llc", " inc", " inc.", " corp", " corporation", " ltd", " co.", " company", "dba ", " d/b/a ", " l.l.c", " lp", " pllc"];
    let mut out: Vec<String> = Vec::new();
    for p in pages.iter().filter(|p| p.page <= 3) {
        for line in p.text.lines().take(60) {
            let t = line.trim();
            let l = format!(" {}", t.to_ascii_lowercase());
            if t.len() < 4 || t.len() > 70 || l.contains("bank") || l.contains("member fdic") || l.contains("filed:") {
                continue;
            }
            if SUFFIX.iter().any(|s| l.contains(s)) && !out.iter().any(|o| o == t) {
                out.push(t.to_string());
            }
        }
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
    let mut ledger = ledger::parse(&page_refs);
    mark_unreadable_pages(&mut ledger, pages);
    let recurring_ids: Vec<usize> = ledger.recurring_debits.iter().map(|r| r.id).collect();
    let funding_ids: Vec<usize> = ledger.funding_candidates.clone();

    let facts = facts_block(&ledger, pages);
    let custom = custom_instructions.trim();
    let mut user = String::new();
    if !custom.is_empty() && !custom.starts_with("Add custom underwriting focus") {
        user.push_str(&format!("ADDITIONAL INSTRUCTIONS FROM THE UNDERWRITER:\n{custom}\n\n"));
    }
    let candidates = merchant_name_candidates(pages);
    if !candidates.is_empty() {
        user.push_str(&format!("MERCHANT NAME CANDIDATES (lines with an entity suffix, parsed from the header; the bank is never the merchant):\n  {}\n\n", candidates.join("\n  ")));
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
    // The recommendation follows the adjusted score; the model's own pick is kept for review.
    let recommendation = match score {
        1..=4 => "APPROVE",
        5..=7 => "REVIEW",
        _ => "DECLINE",
    };

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

    // A model that names the bank as the merchant is overridden by the parsed candidates.
    let model_name = cls["business"]["name"].as_str().unwrap_or("").trim().to_string();
    let candidates = merchant_name_candidates(pages);
    let name: Value = if (model_name.is_empty() || model_name.to_ascii_lowercase().contains("bank") || model_name.to_ascii_lowercase().contains("not provided")) && !candidates.is_empty() {
        json!(candidates.join(" ").chars().take(80).collect::<String>())
    } else if model_name.is_empty() {
        Value::Null
    } else {
        json!(model_name)
    };

    json!({
        "business": {
            "name": name,
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
        "recommendation": recommendation,
        "notes": notes,
        "verification": {
            "stated_total_credits": s.total_credits, "parsed_total_credits": round2(ledger.parsed_credit_total),
            "stated_total_debits": s.total_debits, "parsed_total_debits": round2(ledger.parsed_debit_total),
            "beginning_balance": s.beginning_balance, "ending_balance": s.ending_balance,
            "bank": s.bank,
            "document_kind": s.document_kind,
            "statements": ledger.statements.iter().map(|st| json!({
                "bank": st.bank, "account_last4": st.account_last4, "period_start": st.period_start, "period_end": st.period_end,
                "beginning_balance": st.beginning_balance, "ending_balance": st.ending_balance,
                "total_credits": st.total_credits, "total_debits": st.total_debits,
                "missing_pages": st.missing_pages
            })).collect::<Vec<_>>(),
            "missing_pages": s.missing_pages,
            "transactions_parsed": ledger.transactions.len(),
            "daily_balances_found": ledger.daily_balances.len(),
            "funding_deposits": funding_lines,
            "large_deposits_to_verify": ledger.large_unlabeled_credits.iter().filter_map(|i| ledger.transactions.get(*i)).map(|t| json!({ "date": t.date, "amount": t.amount, "description": t.description })).collect::<Vec<_>>(),
            "nsf_items": ledger.nsf_items.iter().filter_map(|i| ledger.transactions.get(*i)).map(|t| json!({ "date": t.date, "amount": t.amount, "description": t.description })).collect::<Vec<_>>(),
            "sources": m.sources,
            "rejected_positions": rejected,
            "model_recommendation": cls["recommendation"],
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
    #[test]
    fn page_bands_split_at_white_gaps_and_join_headings() {
        // 200 wide, 300 tall: a heading (rows 10..20), a gap of 8, a table (28..120), a gap
        // of 60, a second table (180..260); row spacing inside a table (gaps of 4) never splits.
        let mut img = image::GrayImage::from_pixel(200, 300, image::Luma([255u8]));
        let ink = |img: &mut image::GrayImage, y0: u32, y1: u32| for y in y0..y1 { for x in 20..180 { img.put_pixel(x, y, image::Luma([0u8])); } };
        ink(&mut img, 10, 20);
        for y in (28..120).step_by(12) { ink(&mut img, y, y + 8); }
        for y in (180..260).step_by(12) { ink(&mut img, y, y + 8); }
        assert_eq!(super::page_bands(&img, 30, 40), vec![(10, 120), (180, 260)]);
    }

    #[test]
    fn redaction_boxes_and_rules_are_whitened_but_type_is_kept() {
        // 400 wide, 200 tall: a black box (a redacted name, 160x40), a horizontal rule across
        // the page (3 px), and a line of "type" (thin 3 px strokes) that must survive.
        let mut img = image::GrayImage::from_pixel(400, 200, image::Luma([255u8]));
        for y in 40..80 { for x in 100..260 { img.put_pixel(x, y, image::Luma([0u8])); } }
        for y in 120..123 { for x in 10..390 { img.put_pixel(x, y, image::Luma([0u8])); } }
        for x in (20..380).step_by(8) { for y in 150..160 { img.put_pixel(x, y, image::Luma([0u8])); img.put_pixel(x + 1, y, image::Luma([0u8])); } }
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("page.png");
        img.save(&png).unwrap();
        super::whiten_redactions(&png);
        let out = image::open(&png).unwrap().into_luma8();
        assert_eq!(out.get_pixel(180, 60).0[0], 255, "box stays");
        assert_eq!(out.get_pixel(200, 121).0[0], 255, "rule stays");
        assert_eq!(out.get_pixel(20, 155).0[0], 0, "type lost");
    }

    /// `MCA_BANDS_PNG=/tmp/page.png cargo test bands_of_a_page -- --ignored --nocapture`
    /// prints the bands of a rendered page (150 dpi grayscale PNG) for a look.
    #[test]
    #[ignore]
    fn bands_of_a_page() {
        let path = std::env::var("MCA_BANDS_PNG").unwrap();
        let img = image::open(&path).unwrap().into_luma8();
        let (w, h) = img.dimensions();
        for (s, e) in super::page_bands(&img, 36, 60) {
            println!("band {s:>5}..{e:<5} ({} rows) of {w}x{h}", e - s);
        }
    }

    use super::*;

    #[test]
    fn dollar_amounts_are_extracted() {
        assert_eq!(dollar_amounts("costs $9,625.85 and $113k, not $2.5M."), vec![9625.85, 113_000.0, 2_500_000.0]);
        assert!(dollar_amounts("no money here").is_empty());
    }

    fn truist_text() -> &'static str {
        "COMMERCIAL INTEREST CHECKING 7174 (continued)\n\nDATE DESCRIPTION AMOUNT($)\n03/02 PAYMENT 1489 EXTRA SPACE 2446 368.20\n03/06 ACH CORP DEBIT NPC PYMT NV ENERGY\n\ncontinued\n"
    }

    #[test]
    fn table_pass_patches_or_replaces_the_text() {
        // TD: the text already lists the checks; the table task read that same check table
        // instead of the wrapped "Electronic Payments" list below it.
        let text = "Checks Paid\nDATE SERIAL NO. AMOUNT\n03/10 10991 368.53\n03/18 11020 1,005.11\n\nElectronic Payments\nPOSTING DATE DESCRIPTION AMOUNT\n03/03 DEBIT POS AP, AUT 030125 DDA PURCHASE AP\nRESTAURANT DEPOT ALEXANDRIA * VA 142.29\n";
        // A wrapped row whose amount ends the next line is not a row that lost its amount.
        assert_eq!(ledger::rows_missing_amounts(text), 0);
        // Rows that lost their amount take it from the matching table row; the table's own
        // losses (its last rows) do not matter then.
        let patched = patch_missing_amounts(truist_text(), "Date        Description                 Debits\n03/06       ACH CORP DEBIT NPC PYMT NV ENERGY   85.65\n");
        assert!(patched.contains("NV ENERGY 85.65"), "{patched}");
        // Aligned text (pdftotext) takes the table's line so the amount sits under its column.
        let aligned = "Date        Description                                   Credits      Debits\n03/02       PAYMENT 1489 EXTRA SPACE 2446                              368.20\n03/06       ACH CORP DEBIT NPC PYMT NV ENERGY\n03/06       PAYMENT 8528 Extra Space                                    98.20\n";
        let patched = patch_missing_amounts(aligned, "Date        Description                                   Credits      Debits\n03/06       ACH CORP DEBIT NPC PYMT NV ENERGY                           85.65\n");
        assert!(patched.contains("NV ENERGY                           85.65"), "{patched}");
        assert_eq!(ledger::rows_missing_amounts(&patched), 0, "{patched}");
        // Splicing replaces the dated rows under a single-amount header instead of appending.
        let spliced = splice_table(truist_text(), "Date        Description                 Debits\n03/02       PAYMENT 1489 EXTRA SPACE     368.20\n03/06       NV ENERGY                     85.65\n");
        assert_eq!(spliced.matches("368.20").count(), 1, "{spliced}");
        assert!(spliced.contains("85.65") && spliced.trim_end().ends_with("continued"), "{spliced}");
        assert!(!needs_table_pass(&format!("{text}{}", "word ".repeat(60))));
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
