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

### Living with the rest of the machine

On laptops the GPU has no memory of its own; models and caches come out of system RAM, and a graphics driver that runs out of it can stall the whole desktop. The engine runs at full speed and is built so that can never happen:

- At start it measures the RAM that is actually free and takes as much as fits: up to 4 OCR pages at a time on the GPU, one model resident at a time (the pipeline needs the OCR model, then the reasoning model, never both), 8-bit KV cache. Full CPU, no priority tricks. `--headless-plan` prints the plan.
- Before each OCR page it checks free memory and waits (with a message) when the machine is busy, instead of piling on. A busy machine slows the job; the job never pushes the machine over.
- The engine process tree runs inside an operating-system memory limit (a systemd scope on Linux: throttled first, killed only as a last resort). A watchdog checks free memory twice a second and stops the engine under 1 GB free. If either fires mid-job, the job does not fail: pages already read are cached, the app waits for memory to come back, restarts the engine and resumes.
- It refuses to start only when even one OCR page at a time would not fit, with the numbers in the message.

## Features

- Batch: several months of statements are analyzed as one job and one report.
- Positions: recurring debits detected by payee and amount, cadence (daily, weekly, biweekly, monthly) derived from dates; the model names the lender and rejects vendors, payroll, taxes, floor plan and transfers.
- True revenue: statement total credits minus deposits the model confirms as borrowed money. Only credits whose description carries loan/funding wording can be excluded; large unlabeled deposits are listed for verification instead.
- Negative days from the daily balance table when the statement has one, otherwise from the printed minimum balance, with the source shown.
- Verification block on every report: stated vs parsed credit and debit totals, transaction count, page read method and timing.
- Follow-up chat about the finished analysis, prompt templates, analysis history, JSON/CSV export, print view.

## Statement layouts the parser understands

Every statement is checked against itself: the parser sums the transaction lines and compares them with the totals the bank printed. The verification block in the report shows both numbers, so a layout the parser does not fully understand is visible, never silent. Layouts verified to the cent so far (`scripts/parser_check.py` over public court-filing exhibits and the test statements; 403 statements to the cent as of September 23, 2026, over about 6,570 court-filing PDFs (127,800 pages), 80 of them with funder activity, 243 more documents recognized as not bank statements and 28 court copies reported as incomplete):

| Bank | Layout features |
|---|---|
| Wells Fargo (business and consumer, digital and scanned) | credit, debit and running-balance columns; two-line column headers; "Totals" row; "Items returned unpaid" and fee summaries skipped |
| Chase (commercial) | "Deposits and Credits / Withdrawals and Debits / Checks Paid" summary, multi-column check table, two-column daily balance table, multi-line ACH descriptions |
| Bank of America (business) | "Withdrawals and other debits" plus "Checks" plus "Service fees" summary parts, two-column account summary, daily ledger balances |
| PNC (corporate) | two-line "Balance Summary" header, date / amount / description / reference lines, ledger balance table, check tables with reference numbers |
| Truist (commercial) | "Checks" plus "Other withdrawals" summary, `2.197.40` OCR amounts; scanned filings need the OCR re-read (see below) |
| Webster Bank | running-balance table plus per-type lists (duplicates dropped), "N Debit(s) this period" summary, `-$` amounts |
| Legends Bank | date-first / amount-last lines, section headers, multi-column check tables, check image captions |
| Sunrise Banks | column-style two-line summary, 20-row daily balance table |
| Pinnacle Bank | `$.00` amounts, "Credits + / Debits -" summary, check image pages |
| TD Bank | "DEBIT / CREDIT / BALANCE" columns, "Statement Balance as of" beginning and ending, three-line descriptions |
| Chase (business) | summary with one line per debit category (card, electronic, checks, fees) summed; sectioned lists where the section decides credit or debit |
| Chase (business, print-stream PDF) | rows broken across the "*end*" section marker at a page break rejoined; "Check No. / Description / Date Paid / Amount" table, including the check's own date in the description column |
| Chase online activity printout | date printed once per day and carried to the rows below it, also across pages; descriptions wrapped over several lines; "Pending" rows left out |
| Bank verification reports (DecisionLogic) | "EOD / Balance" header stacked over three lines; one amount column that signs its debits "($918.75)"; the running balance printed on the last row of each day |
| Wintrust | "Jun 03" dates, mailing barcodes in the margin, "Analysis or Maintenance Fees" counted with debits |
| Hancock Whitney | two transaction columns side by side (unfolded), "22 CREDITS / 9 DEBITS / SERVICE CHARGES" summary |
| Mabrey Bank | "2 Deposits/Credits / 31 Checks/Debits" summary, check table, image caption pages skipped |
| Fifth Third (business) | scanned first page: "Beginning Balance" with the amount on the next line, "N items totaling $X" section headers, dated "06/30 Ending Balance", fee analysis block already inside the withdrawals |
| Synovus | "06-01" dashed dates, "Transaction Type" column, "Balance Summary" daily table, check table with skipped-number markers |
| PNC (corporate, scanned filings) | underscored form rules glued to dates (`03/31_____`), cent-only amounts (`.15`), sections resumed after another header on the next page |
| U.S. Bank (Uni-Statement) | trailing "-" debit markers in the summary ("Other Withdrawals 962.49-"), two-column summary with interest figures to the right, "Check Date Ref Number Amount" check rows, "Balance Summary" daily table |
| Frost Bank | "BALANCE LAST STATEMENT / BALANCE THIS STATEMENT" over the figures, dashed section rules, "07-29" dates, letterhead without the bank name (P.O. Box 1600 San Antonio) |
| Valley National | "Deposits & Other Credits" summary, bank named only in the print file path |
| BMO, Capital One | single-page exhibits, "($15.00)" debits, summary only |
| KeyBank (business) | "Beginning balance 9-30-24" and "10-3" dashed dates, "1 Addition +7,170.00 / 3 Subtractions -7,065.85 / Net fees and charges" categories with no summary heading, Additions / Subtractions / Paper Checks sections; a quiet month ending where it began is still its own statement |
| First State Bank | "ALL CREDIT ACTIVITY" and check tables three entries to a line (`Date Type Amount` three times), the columns one space apart |
| Capitol Credit Union | several sub-accounts on one statement ("KASASA CASH (0008)"), each with its own summary; transaction and effective dates side by side; descriptions printed above their dated row; loan sub-accounts left out |
| Court scans read as Markdown by the OCR model | pipe summary tables (`Beginning Balance \| Deposits \| ... \| Ending Balance`), bulleted rows with month-name dates and amount-plus-balance columns, `**bold**` headings, summary labels read column by column |
| TD Bank (business, court copies) | check tables with dozens of same-day checks for the same amount (told apart by serial), serials lifted onto their own line by the OCR layer, "Checks Paid (continued)" pages, image caption pages with no heading, "Payrnents" misreads in the summary |
| Citizens (Clearly Better Business Checking) | "Balance Calculation" summary, check table with the legend or the checks total printed beside a row, doubled text layers, the same statement filed twice in two layouts (the second copy dropped), "Non Check Return" credits, "Images for Account" pages |
| Yampa Valley Bank (Colorado) | "N Debit(s) This Period" already including the service charge (decided by the balance equation), "#0000" slip captions, two months of a swept account that both begin at -$10.00 split by their statement date |
| Bank of America account-detail exports (Ocrolus style) | newest-first rows with a running balance, "Withdrawals (-)" summary, `$1185228.22` amounts without thousands separators |
| Achieva Credit Union | "BUSINESS ESSENTIAL CHECKING 0750" sub-accounts, "Total Credits for this account" totals that leave out reversal pairs (a fee and its "-- Reversed" credit), "Checks Cleared At A Glance" |
| Navy Federal Credit Union | debit sign a space after the amount (`20.00 -`), rows wrapped over two lines, a "Summary of your deposit accounts" table with one row per account, "Items Paid" recap table, letterhead with "Credit Union" that is not a section |
| Mercury (fintech) | one date per day with the day's rows undated below it, debits with a leading minus, "Trx Type" and end-of-day balance columns whose header comes out letter-spaced |
| Hi Bar Capital's own account (older commercial statement) | month-name dates without slashes ("NOV 01"), "$ 130813.77" with a space, no descriptions, figures the text layer cut short recovered from the running balance |
| Small-bank text layers (Webster-style "Account Activity") | column rows split over two or three lines (date alone, then description, then the cells), amounts the text layer garbled (`M,000.00`) recovered from the running balance and marked as such |

Scanned pages: classic OCR (Tesseract, 300 dpi) reads the page first when it is installed, and a court copy's own text layer is tried against a Tesseract reading of the same page when the totals disagree; the vision model then reads only the pages the totals still call for, and each reading is kept only where it brings the statement closer to its printed totals. The OCR model reads plain text first; when rows under a transaction table lose their amounts (wrapped descriptions), or the page comes back nearly empty, the table is read as a table and every amount lands in its column. GLM-OCR is a region recognizer (its own SDK runs a layout detector first and reads each block on its own), so when a listing adds up to less than the subtotal printed under it, or the plain reading comes back nearly empty (the model dropped rows it could see: the right column of a two-column check table, everything after the first table of a page), the page is cut into bands at the white gaps between its sections and each band is read on its own at 200 dpi; that reading stands only when it comes closer to the printed figures, and readings are never merged. Statements whose text layer is someone else's poor OCR (court filings) are detected by the totals mismatch, or by a text layer with neither a beginning nor an ending balance, and re-read with the OCR model automatically; re-read pages are adopted one at a time, only when they bring the totals closer.

Court-filed copies often carry a poor OCR text layer; the parser repairs what it can before reading: split amounts (`20. 00`, `-1 ,100.00`, `3,051 .38`), split dates (`11 /21 /22`), form rules glued to dates (`03/31_____`), smeared section headers (`!OTHER WITHDRAWALS, FEES & C H A R G E S`), check numbers glued to labels (`24490Check`), stacked cells (two lone dates over two lone amounts), summary labels stacked over their amounts, tables retold as bullets (`- 04/18: ...: 3,176.12`, `- Oct 02: ... | 1,651.07 | 14,478.08`), pipe tables, en-dash minus signs, lone `^` / `*` footnote marks, and summary labels and month names that lost a letter (`Begi ning balance`, `End ng balance`, `Ser ice fees`, `for eptember 1, 2022`). Rows on a page that lost its section header follow their own words instead of the section inherited from the page before. When the totals still do not match, the page is re-read with the OCR model; OCR pages are adopted one at a time and, when a statement is still off, all of its pages together, never when they break a statement already to the cent. A copy that skips pages of a statement (a one-sided scan of a two-sided statement: the bank's own footers jump from `Page 3 of 14` to `Page 5 of 14` on neighbouring pages) is reported as missing pages, since its totals cannot be met from the lines that survive; `parser_check.py` counts those as incomplete copies, not failures.

The balance equation is the second witness: beginning balance plus the parsed credits minus the parsed debits must equal the ending balance. A statement passes when its printed totals are met, or when the equation closes to the cent and at most one printed total disagrees (two printed balances and one total agreeing with the rows outrank one misread total). The same rule governs the parser's repairs of a scan's misread figure: the printed daily balance table, a section's printed total ("Total checks = $3,130.00") or a second listing of the same item (a check's image caption against the checks-paid table) may change one digit of one row, and the change stays only when the statement's printed totals are then met exactly. A statement with rows missing is left as it is rather than have another row bent to cover them. The witnesses also work the other way: when every section of a kind prints its total, the rows meet those totals to the cent, and the statement's own figure is one digit off their sum, the statement figure is the misread one; and an online printout that prints no totals but a running balance on every row takes where it began and ended from the balance column, so its rows are held to the equation like any statement's.

Files that bundle several statements (months, or several banks) are split where a new statement starts (a new beginning balance, a different bank in the letterhead, a different account number, a different statement date, a sub-account heading such as Truist's `DYNAMIC BUSINESS CHECKING - CORE TIER 4952` above its own account summary, or a second summary page) and each part is parsed on its own; the report lists them. Paper filed around the statements is left out: court forms and their exhibits, reconciliation reports and general ledgers, check registers, online-banking printouts (parsed, but they carry no totals), sweep and brokerage statements. Documents that are not bank statements at all are named (`document_kind`: reconciliation report, trustee ledger, monthly operating report, ACH activity report, brokerage, credit card and PayPal statements). Not handled yet: statements with no printed totals at all (parsed lines are still shown, but cannot be verified), text layers whose amounts are missing or garbled beyond the running-balance repair (those go to the OCR model), and layouts not seen in the corpus. Adding a bank means adding its statement to the corpus, fixing the parser and adding a fixture test in `src-tauri/src/engine/ledger.rs`.

## Prerequisites

- Poppler (`pdftocairo`, `pdftotext`, `pdfinfo`, `pdfimages`) on PATH. Ubuntu/Debian: `sudo apt install poppler-utils`. Arch: `sudo pacman -S poppler`. macOS: `brew install poppler`. Windows: a Poppler build such as https://github.com/oschwartz10612/poppler-windows, folder added to PATH.
- Tesseract 5 (`tesseract` on PATH) with English data, optional but recommended: scanned pages are read by classic OCR first (a second or two, exact on clean scans) and the vision model reads only what the totals say still needs it. The app looks for `eng.traineddata` in its engine folder's `tessdata/` (the 4 MB `tessdata_fast` file), else wherever the system Tesseract keeps its languages. Ubuntu/Debian: `sudo apt install tesseract-ocr tesseract-ocr-eng`. Arch: `sudo pacman -S tesseract tesseract-data-eng`. macOS: `brew install tesseract`. Windows: the UB Mannheim installer. Without it every scanned page goes to the model.
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
./src-tauri/target/debug/local-mca-underwriter-ide --headless-ledger statement.pdf          # parser only, text layers
./src-tauri/target/debug/local-mca-underwriter-ide --headless-ledger statement.pdf --ocr    # parser with OCR of scanned pages
python3 scripts/parser_check.py <folder of PDFs> [--ocr] [--markdown]                        # parsed vs printed totals per statement
```

OCR page text is cached under `engine/ocr-cache/` (keyed by file hash, page, DPI and model), so re-analyzing a statement never pays for OCR twice.

## Measured on a laptop with an AMD Radeon 680M (integrated) and 14 GB RAM

- Digital page via text layer: 0.1 s.
- Scanned pages via GLM-OCR at 150 DPI: a dense 6-page statement in 210 s with 4 pages in flight (35 s per page effective), no digit errors; 100 and 125 DPI lost digits or rows.
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
