//! Deterministic statement parsing. Everything numeric the report needs is computed
//! here from the page text; the model never adds numbers.
//!
//! Bank layouts differ, so this is heuristic but conservative: a line is a transaction
//! only when it starts with a date and ends with an amount. Section headers decide
//! whether a line is a credit or a debit, with keyword fallback. Statement summary
//! lines (beginning balance, total credits, ...) are captured separately and are
//! treated as the authoritative totals when present.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Credit,
    Debit,
}

/// One parsed transaction line (plus its continuation lines).
#[derive(Debug, Clone, Serialize)]
pub struct Txn {
    pub id: usize,
    /// ISO date when the year is known, otherwise "MM/DD".
    pub date: String,
    /// Days since 1970-01-01 when the date could be resolved; used for cadence.
    pub day: Option<i64>,
    pub kind: Kind,
    pub amount: f64,
    pub description: String,
    pub page: usize,
    /// Which listing on the statement the line came from. Many banks print the same
    /// transaction twice (a running-balance table, then per-type lists or check tables);
    /// `dedup_across_tables` drops repeats that come from a different table.
    pub table: usize,
}

/// Figures printed by the bank itself.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub beginning_balance: Option<f64>,
    pub ending_balance: Option<f64>,
    pub total_credits: Option<f64>,
    pub total_debits: Option<f64>,
    pub days_in_period: Option<u32>,
    pub average_balance: Option<f64>,
    pub minimum_balance: Option<f64>,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    /// Last four digits of the account, when a masked or labeled account number is printed.
    pub account_last4: Option<String>,
    /// Bank named on the statement, from a fixed keyword list (see `detect_bank`).
    pub bank: Option<String>,
    /// Banks that split debits into "Checks" and "Other withdrawals" (Truist) print two
    /// figures; this holds the checks part until both are known.
    #[serde(skip)]
    checks_total: Option<f64>,
    /// Bank of America prints "Service fees -16.00" as a third debit figure.
    #[serde(skip)]
    fees_total: Option<f64>,
    /// The key that gave `total_debits`; decides whether checks and fees are already in it.
    #[serde(skip)]
    debits_key: &'static str,
    /// Page the debit total was read from. Separate checks and fee figures count only from
    /// the same page, so a fee line on a later bundled page is not added.
    #[serde(skip)]
    debits_page: Option<usize>,
    /// Distinct "beginning balance" figures seen. More than one means the file bundles
    /// several statements or accounts, which the parser does not separate yet.
    pub beginning_balances_seen: Vec<f64>,
}

/// A group of debits to the same payee with the same amount, i.e. a possible position.
#[derive(Debug, Clone, Serialize)]
pub struct RecurringDebit {
    pub id: usize,
    pub payee: String,
    pub amount: f64,
    pub count: usize,
    /// "daily", "weekly", "monthly" or "irregular" from the gaps between occurrences.
    pub cadence: &'static str,
    pub dates: Vec<String>,
}

/// Payee seen more than once with varying amounts (vendors, floor plan, loan payments).
#[derive(Debug, Clone, Serialize)]
pub struct PayeeTotal {
    pub payee: String,
    pub count: usize,
    pub total: f64,
    pub kind: Kind,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyBalance {
    pub date: String,
    pub balance: f64,
}

/// Everything the deterministic pass knows about a statement set.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Ledger {
    pub transactions: Vec<Txn>,
    /// Figures for the whole set. With several statements in the input (a bundle of
    /// months, or several accounts) this is the combination of `statements`.
    pub summary: Summary,
    /// One summary per statement found in the input; empty when there is only one.
    pub statements: Vec<Summary>,
    pub daily_balances: Vec<DailyBalance>,
    pub recurring_debits: Vec<RecurringDebit>,
    pub payees: Vec<PayeeTotal>,
    /// Debits whose description names an NSF, returned item or overdraft fee.
    pub nsf_items: Vec<usize>,
    /// Credits whose description uses loan/funding wording. Only these can become funding.
    pub funding_candidates: Vec<usize>,
    /// Large credits with no explanatory description. Listed for the underwriter to verify;
    /// never subtracted from revenue automatically.
    pub large_unlabeled_credits: Vec<usize>,
    pub parsed_credit_total: f64,
    pub parsed_debit_total: f64,
}

// ─── Line parsing ─────────────────────────────────────────────────────────

/// Parse "1,234.56", "$1,234.56", "1,234.56-", "-1,234.56", "(1,234.56)", "$.00" and the
/// OCR form "2.197.40" where a comma was read as a period.
pub fn parse_amount(raw: &str) -> Option<f64> {
    let t = raw.trim();
    let neg = t.ends_with('-') || t.starts_with('-') || (t.starts_with('(') && t.ends_with(')'));
    let digits: String = t.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    let dot = digits.rfind('.')?;
    let joined: String = digits[..dot].chars().filter(|c| *c != '.').chain(digits[dot..].chars()).collect();
    let v: f64 = format!("0{joined}").parse().ok()?;
    Some(if neg { -v } else { v })
}

pub fn is_amount_token(tok: &str) -> bool {
    let t = tok.trim_start_matches(|c| c == '$' || c == '-').trim_end_matches('-').trim_matches(|c| c == '(' || c == ')');
    if t.is_empty() {
        return false;
    }
    let first = t.chars().next().unwrap();
    // "$.00" is how some banks print zero; otherwise an amount starts with a digit.
    if !(first.is_ascii_digit() || (first == '.' && tok.starts_with('$'))) {
        return false;
    }
    // Groups between separators: "1,234.56" -> [1, 234, 56]; "2.197.40" -> [2, 197, 40].
    let groups: Vec<&str> = t.split(|c| c == ',' || c == '.').collect();
    if groups.iter().any(|g| !g.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    let decimals_ok = groups.last().map(|g| g.len() == 2).unwrap_or(false) && (t.contains('.'));
    // Every inner group is a thousands group of exactly three digits.
    let inner_ok = groups.len() < 3 || groups[1..groups.len() - 1].iter().all(|g| g.len() == 3);
    // Period-separated thousands only when no comma is present (otherwise "1.5.00" is noise).
    let periods = t.matches('.').count();
    let period_thousands_ok = periods == 1 || (!t.contains(',') && groups[0].len() <= 3 && inner_ok);
    decimals_ok && inner_ok && period_thousands_ok
}

const MONTHS: &[&str] = &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Mailing barcodes rendered as text in the left margin ("ACEMBHDOODOPKMBLFEBEPIPK  Jun 14
/// PREAUTHORIZED CREDIT $2,584.81") would hide the date. Blank out a leading run of 12 or
/// more uppercase letters when a date follows, keeping the width.
fn strip_margin_barcode(line: &str) -> String {
    let trimmed = line.trim_start();
    let first = trimmed.split_whitespace().next().unwrap_or("");
    if first.len() >= 12 && first.chars().all(|c| c.is_ascii_uppercase()) {
        let rest = trimmed[first.len()..].trim_start();
        if rest.split_whitespace().next().and_then(parse_date_token).is_some() {
            let indent = line.len() - trimmed.len();
            return format!("{}{}{}", " ".repeat(indent), " ".repeat(first.len()), &trimmed[first.len()..]);
        }
    }
    line.to_string()
}

/// Rewrite "Jun 03", "Jun 3, 2024" and "June 3 2024" as "06/03" / "06/03/2024" in place,
/// keeping the line width so column offsets still line up. Wintrust and a few others
/// print month names in the date column.
pub fn normalize_month_dates(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let at_word_start = i == 0 || !chars[i - 1].is_alphanumeric();
        if at_word_start && chars[i].is_ascii_alphabetic() {
            // month word
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect::<String>().to_ascii_lowercase();
            let month = if word.len() >= 3 { MONTHS.iter().position(|m| word.starts_with(m) && (word.len() == 3 || full_month(&word))) } else { None };
            if let Some(m) = month {
                // optional ".", then spaces, then day digits, optional ",", optional year
                let mut k = j;
                if k < chars.len() && chars[k] == '.' {
                    k += 1;
                }
                let mut sp = k;
                while sp < chars.len() && chars[sp] == ' ' {
                    sp += 1;
                }
                let day_start = sp;
                let mut de = day_start;
                while de < chars.len() && chars[de].is_ascii_digit() && de - day_start < 2 {
                    de += 1;
                }
                let day_ok = de > day_start && sp - k <= 2 && (de == chars.len() || !chars[de].is_alphanumeric());
                if day_ok {
                    let day: u32 = chars[day_start..de].iter().collect::<String>().parse().unwrap_or(0);
                    if (1..=31).contains(&day) {
                        // year: ", 2024" or " 2024"
                        let mut ye = de;
                        let mut y2 = ye;
                        if y2 < chars.len() && chars[y2] == ',' {
                            y2 += 1;
                        }
                        let mut ys = y2;
                        while ys < chars.len() && chars[ys] == ' ' && ys - y2 <= 2 {
                            ys += 1;
                        }
                        let mut yend = ys;
                        while yend < chars.len() && chars[yend].is_ascii_digit() {
                            yend += 1;
                        }
                        let mut date = format!("{:02}/{:02}", m + 1, day);
                        if yend - ys == 4 && ys > de {
                            date.push('/');
                            date.extend(chars[ys..yend].iter());
                            ye = yend;
                        }
                        let span = ye - i;
                        if date.chars().count() <= span {
                            out.push_str(&date);
                            for _ in date.chars().count()..span {
                                out.push(' ');
                            }
                            i = ye;
                            continue;
                        }
                    }
                }
            }
            out.extend(chars[i..j].iter());
            i = j;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn full_month(word: &str) -> bool {
    ["january", "february", "march", "april", "may", "june", "july", "august", "september", "sept", "october", "november", "december"].contains(&word)
}

/// (month, day, year) from MM/DD, MM/DD/YY, MM/DD/YYYY.
pub fn parse_date_token(tok: &str) -> Option<(u32, u32, Option<i32>)> {
    let parts: Vec<&str> = tok.split('/').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let m: u32 = parts[0].parse().ok()?;
    let d: u32 = parts[1].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = match parts.get(2) {
        None => None,
        Some(y) if y.len() == 4 => Some(y.parse().ok()?),
        Some(y) if y.len() == 2 => Some(2000 + y.parse::<i32>().ok()?),
        _ => return None,
    };
    Some((m, d, y))
}

/// Days since the Unix epoch for a civil date (proleptic Gregorian).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn weekday(day: i64) -> i64 {
    // 0 = Monday
    (day + 3).rem_euclid(7)
}

/// Section a header line switches to, if it is a header.
fn section_for(line: &str) -> Option<Kind> {
    let l = line.to_ascii_lowercase();
    let toks: Vec<&str> = l.split_whitespace().collect();
    let starts_with_date = toks.first().and_then(|t| parse_date_token(t)).is_some();
    let ends_with_amount = toks.last().map(|t| is_amount_token(t)).unwrap_or(false);
    // Headers carry no reference or account numbers ("TRANSFER TO DEPOSIT SYSTEM ACCOUNT
    // XXXXXX4516" is a description continuation, not a section).
    let has_reference = toks.iter().any(|t| t.chars().filter(|c| c.is_ascii_digit()).count() >= 4 || t.contains("xxx"));
    let header_like = (l.contains("---") || toks.len() <= 6) && !starts_with_date && !ends_with_amount && !has_reference;
    if !header_like {
        return None;
    }
    if l.contains("daily balance") || l.contains("balance summary") {
        return None;
    }
    if l.contains("deposit") || l.contains("credit") {
        return Some(Kind::Credit);
    }
    if l.contains("debit") || l.contains("withdrawal") || l.contains("checks") || l.contains("fees") {
        return Some(Kind::Debit);
    }
    None
}

fn kind_from_words(desc: &str, section: Option<Kind>) -> Kind {
    kind_and_confidence(desc, section).0
}

/// Kind from the description words, and whether a word actually decided it (true) or
/// the section / default did (false). Weak kinds are the ones balance arithmetic may flip.
fn kind_and_confidence(desc: &str, section: Option<Kind>) -> (Kind, bool) {
    let l = desc.to_ascii_lowercase();
    // Explicit words on the line win over the section, since some banks mix them.
    const DEBIT_WORDS: &[&str] = &["withdrawal", " debit", "purchase", " fee", "charge", "check ", "payment to", "zelle to", "transfer to", "payment authorized", "pmt to", "bill pay", "wire out", "outgoing wire", "atm "];
    const CREDIT_WORDS: &[&str] = &["deposit", " credit", "zelle from", "transfer from", "pmt from", "payment from", "wire in", "incoming wire", "refund", "reversal", "cashback", "cash back"];
    if l.starts_with("debit") || DEBIT_WORDS.iter().any(|w| l.contains(w)) {
        return (Kind::Debit, true);
    }
    if l.starts_with("credit") || CREDIT_WORDS.iter().any(|w| l.contains(w)) || l.contains("interest") && section.is_none() {
        return (Kind::Credit, true);
    }
    (section.unwrap_or(Kind::Debit), section.is_some())
}

const NSF_WORDS: &[&str] = &["nsf", "insufficient", "overdraft", "od fee", "returned", "return item", "item ret", "ret chrg", "ret-r", "non check return", "uncollected"];

/// Case-insensitive phrase match on word boundaries, so "transfer" never matches "nsf".
fn has_phrase(lower: &str, phrase: &str) -> bool {
    let mut start = 0;
    while let Some(p) = lower[start..].find(phrase) {
        let i = start + p;
        let j = i + phrase.len();
        let before_ok = i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric();
        let after_ok = j == lower.len() || !lower.as_bytes()[j].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = j;
    }
    false
}
/// Recurring debits with these words are never MCA positions: the merchant moving its own
/// money, payroll, taxes, card bills, utilities, insurance. They stay out of the candidate list.
const NOT_POSITION_WORDS: &[&str] = &["tr to acct", "transfer", "xfer", "payroll", "irs ", "usataxpymt", "tax", "dept of reven", "amex", "american express", "credit card", "utilit", "insurance", " ins ", "401k", "ach offset", "fee"];

const FUNDING_WORDS: &[&str] = &["loan", "funding", "proceeds", "advance", "capital", "mca", "fund ", "financ", "lending", "kabbage", "ondeck", "fundbox", "bluevine", "credibly", "kapitus", "libertas", "forward fin", "rapid fin"];

/// Character end offsets of amount columns from a table header such as
/// "Date  Description  Deposits/Credits  Withdrawals/Debits  Ending daily balance".
/// Statements laid out this way (Wells Fargo, many credit unions) print credits and
/// debits in separate columns and a running balance last, so the column an amount
/// sits in decides its kind, not words in the description.
#[derive(Debug, Clone, Default)]
pub struct Columns {
    credit: Option<usize>,
    debit: Option<usize>,
    balance: Option<usize>,
}

impl Columns {
    /// Column labels found on one header line, by character offset. Headers often wrap
    /// over two lines ("Deposits/ Withdrawals/ Ending daily" above "Credits Debits balance"),
    /// so callers merge two consecutive label lines with `merge`.
    fn labels(line: &str) -> Columns {
        let lower = line.to_ascii_lowercase();
        // Amounts are right-aligned under their label, so compare against label end offsets.
        let find = |keys: &[&str]| keys.iter().filter_map(|k| lower.find(k).map(|p| p + k.len())).min();
        Columns {
            credit: find(&["deposits/credits", "deposits/ credits", "credits", "deposits", "additions"]),
            debit: find(&["withdrawals/debits", "withdrawals/ debits", "debits", "withdrawals", "subtractions", "payments"]),
            balance: find(&["ending daily balance", "daily balance", "running balance", "balance"]),
        }
    }

    fn merge(&self, other: &Columns) -> Columns {
        Columns {
            credit: self.credit.or(other.credit),
            debit: self.debit.or(other.debit),
            balance: self.balance.or(other.balance),
        }
    }

    fn count(&self) -> usize {
        [self.credit, self.debit, self.balance].iter().filter(|c| c.is_some()).count()
    }

    /// Which columns are present, independent of their offsets (indentation shifts between pages).
    fn key(&self) -> String {
        format!("{}{}{}", if self.credit.is_some() { "c" } else { "" }, if self.debit.is_some() { "d" } else { "" }, if self.balance.is_some() { "b" } else { "" })
    }

    /// A usable transaction table header: a date column plus at least two amount columns,
    /// one of them credits or debits.
    fn is_complete(&self, has_date: bool) -> bool {
        has_date && self.count() >= 2 && (self.credit.is_some() || self.debit.is_some())
    }

    /// Kind of an amount printed ending at character `end`, by nearest column label.
    fn kind_at(&self, end: usize) -> Option<Kind> {
        let dist = |c: Option<usize>| c.map(|x| (x as i64 - end as i64).abs()).unwrap_or(i64::MAX);
        let (dc, dd, db) = (dist(self.credit), dist(self.debit), dist(self.balance));
        if db < dc && db < dd {
            return None; // running balance, not a transaction amount
        }
        Some(if dc <= dd { Kind::Credit } else { Kind::Debit })
    }
}

/// Parser state that carries across pages: the current section, whether we are inside a
/// daily balance block, and which listing (table) lines belong to.
#[derive(Default)]
struct State {
    section: Option<Kind>,
    in_daily: bool,
    table: usize,
    table_key: String,
    /// Inside a block that repeats or explains transactions without being one
    /// ("Items returned unpaid", "Monthly service fee summary"); lines there are skipped.
    informational: bool,
    /// Last running balance seen on a flat (OCR) page, and the transactions read since,
    /// with whether their kind came from a word. See `resolve_group`.
    last_balance: Option<f64>,
    open_group: Vec<(usize, bool)>,
}

/// pdftotext -layout keeps columns aligned with runs of spaces; OCR output does not.
/// On a flat page the character offset of an amount says nothing, so kinds come from
/// words and running-balance arithmetic instead.
fn is_flat(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return false;
    }
    let aligned = lines.iter().filter(|l| l.contains("   ")).count();
    aligned * 10 < lines.len()
}

/// Running-balance check for transactions read from a flat page. Between two printed
/// balances, credits minus debits must equal the balance change. Kinds decided by a word
/// are trusted; the others are flipped, in every combination, until exactly one
/// assignment fits. With no unique fit the word-based kinds stay.
fn resolve_group(ledger: &mut Ledger, group: &[(usize, bool)], prev: f64, next: f64) {
    let target = ((next - prev) * 100.0).round() as i64;
    let signed = |ledger: &Ledger, flips: u64| -> i64 {
        group.iter().enumerate().map(|(i, (id, _))| {
            let mut k = ledger.transactions[*id].kind;
            if flips & (1 << i) != 0 {
                k = if k == Kind::Credit { Kind::Debit } else { Kind::Credit };
            }
            let a = (ledger.transactions[*id].amount * 100.0).round() as i64;
            if k == Kind::Credit { a } else { -a }
        }).sum()
    };
    if group.is_empty() || group.len() > 20 || signed(ledger, 0) == target {
        return;
    }
    // Try flipping only weak lines first, then all lines (small groups only).
    let weak_mask: u64 = group.iter().enumerate().filter(|(_, (_, strong))| !strong).map(|(i, _)| 1u64 << i).sum();
    let all_mask: u64 = if group.len() <= 12 { (1u64 << group.len()) - 1 } else { weak_mask };
    for mask_space in [weak_mask, all_mask] {
        let mut found: Option<u64> = None;
        let mut sub = mask_space;
        loop {
            if sub != 0 && signed(ledger, sub) == target {
                if found.is_some() {
                    found = None; // ambiguous
                    break;
                }
                found = Some(sub);
            }
            if sub == 0 {
                break;
            }
            sub = (sub - 1) & mask_space;
        }
        if let Some(flips) = found {
            for (i, (id, _)) in group.iter().enumerate() {
                if flips & (1 << i) != 0 {
                    let t = &mut ledger.transactions[*id];
                    t.kind = if t.kind == Kind::Credit { Kind::Debit } else { Kind::Credit };
                }
            }
            return;
        }
        if mask_space == all_mask {
            break;
        }
    }
}

/// Headers of blocks whose dated, amounted lines are not transactions.
const INFORMATIONAL_HEADERS: &[&str] = &["items returned unpaid", "monthly service fee summary", "account transaction fees summary", "fee period", "overdraft protection", "interest summary"];

impl State {
    /// Switch to the table identified by `key`. A header repeated on the next page
    /// ("Credits (continued)") keeps the same table so cross-page repeats are not
    /// mistaken for a second listing.
    fn enter_table(&mut self, key: &str) {
        let key = key.to_ascii_lowercase().replace("(continued)", "").replace("continued", "");
        let key = key.split_whitespace().collect::<Vec<_>>().join(" ");
        if key != self.table_key {
            self.table += 1;
            self.table_key = key;
        }
        self.informational = false;
        self.open_group.clear();
    }
}

/// Parse one page. `year_hint` fills in years for MM/DD dates.
fn parse_page(text: &str, page: usize, year_hint: Option<i32>, ledger: &mut Ledger, st: &mut State) {
    let mut columns: Option<Columns> = None;
    let flat = is_flat(text);
    let mut pending_header: Option<Columns> = None;
    let mut last_txn: Option<usize> = None;
    // Column-style summaries ("Previous Balance  Total Credits  Total Debits  Current Balance")
    // put the labels on one line and the values on the next.
    let mut pending_columns: Vec<&'static str> = Vec::new();
    let mut pending_has_checks = false;
    for raw in text.lines() {
        let normalized = normalize_month_dates(raw);
        let stripped = strip_margin_barcode(normalized.trim_end());
        let line: &str = stripped.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();

        if !pending_columns.is_empty() {
            let amounts: Vec<f64> = trimmed.split_whitespace().filter(|t| is_amount_token(t)).filter_map(parse_amount).collect();
            // Second header line ("balance  other credits  other debits  balance"): keep waiting.
            if amounts.is_empty() && trimmed.split_whitespace().count() <= 8 && ["balance", "credits", "debits", "other"].iter().any(|w| lower.contains(w)) {
                continue;
            }
            if amounts.len() >= pending_columns.len() {
                for (label, value) in pending_columns.iter().zip(amounts) {
                    match *label {
                        "beginning" => ledger.summary.beginning_balance.get_or_insert(value),
                        "credits" => ledger.summary.total_credits.get_or_insert(value.abs()),
                        "debits" => {
                            if ledger.summary.total_debits.is_none() {
                                ledger.summary.debits_key = if pending_has_checks { "checks and other debits" } else { "debits" };
                                ledger.summary.debits_page = Some(page);
                            }
                            ledger.summary.total_debits.get_or_insert(value.abs())
                        }
                        _ => ledger.summary.ending_balance.get_or_insert(value),
                    };
                }
            }
            pending_columns.clear();
            continue;
        }
        let labels = column_labels(&lower);
        if labels.len() >= 2 && !trimmed.split_whitespace().any(is_amount_token) {
            pending_columns = labels;
            pending_has_checks = lower.contains("check");
            continue;
        }

        capture_summary(&lower, trimmed, &mut ledger.summary, page);

        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.len() <= 6 && INFORMATIONAL_HEADERS.iter().any(|h| lower.starts_with(h)) {
            st.informational = true;
            columns = None;
            last_txn = None;
            continue;
        }
        // Daily balance tables: a "Daily Balance" heading, or a header repeating "Date ...
        // balance" for several columns ("Date  Ledger balance  Date  Ledger balance").
        let has_amount = tokens.iter().any(|t| is_amount_token(t));
        let repeated_date_balance_header = !has_amount && lower.matches("date").count() >= 2 && lower.contains("balance") && tokens.len() <= 12;
        // A transaction table header naming a credit or debit column ("... Ending daily balance") is not a daily balance block.
        let hdr = Columns::labels(line);
        let names_txn_columns = hdr.credit.is_some() || hdr.debit.is_some();
        if !names_txn_columns && (lower.contains("daily balance") || lower.contains("daily ending balance") || lower.contains("daily ledger balance") || repeated_date_balance_header) {
            st.enter_table("daily balances");
            st.in_daily = true;
            last_txn = None;
            continue;
        }
        // Column header for a transaction table with separate credit/debit/balance columns,
        // possibly wrapped over two lines.
        {
            let has_amount = tokens.iter().any(|t| is_amount_token(t));
            let labels = Columns::labels(line);
            let has_date = lower.contains("date");
            if !has_amount && labels.count() >= 1 && tokens.len() <= 12 {
                let merged = pending_header.as_ref().map(|p| p.merge(&labels)).unwrap_or(labels.clone());
                if merged.is_complete(has_date) {
                    st.enter_table(&format!("columns {} {:?}", merged.key(), st.section));
                    columns = Some(merged);
                    pending_header = None;
                    st.in_daily = false;
                    last_txn = None;
                    continue;
                }
                if labels.count() >= 2 || has_date {
                    pending_header = Some(merged);
                    continue;
                }
            } else {
                pending_header = None;
            }
        }

        if let Some(k) = section_for(trimmed) {
            st.enter_table(trimmed);
            st.section = Some(k);
            st.in_daily = false;
            last_txn = None;
            continue;
        }
        // Long check-table titles ("Summary of checks written (checks listed are also
        // displayed in the preceding Transaction history)") start a new listing too.
        if !has_amount && lower.contains("check") && (lower.contains("summary of") || lower.contains("checks paid") || lower.contains("checks cleared") || lower.contains("checks written")) && tokens.len() <= 16 {
            st.enter_table(trimmed);
            st.section = Some(Kind::Debit);
            st.in_daily = false;
            last_txn = None;
            continue;
        }
        if lower.starts_with("total") || lower.starts_with("minimum balance") || lower.contains("continued on") {
            // "Totals  $62,461.80  $66,931.38" under a credit/debit column header.
            if let Some(c) = &columns {
                if lower.starts_with("totals") {
                    for (end, tok) in amount_spans(line) {
                        match (c.kind_at(end), parse_amount(tok)) {
                            (Some(Kind::Credit), Some(v)) => ledger.summary.total_credits.get_or_insert(v.abs()),
                            (Some(Kind::Debit), Some(v)) => ledger.summary.total_debits.get_or_insert(v.abs()),
                            _ => continue,
                        };
                    }
                }
            }
            last_txn = None;
            continue;
        }

        if st.informational {
            continue;
        }

        if st.in_daily {
            // Columns of "date balance date balance ...".
            let mut i = 0;
            let mut any = false;
            while i + 1 < tokens.len() {
                if let (Some(_), true) = (parse_date_token(tokens[i]), is_amount_token(tokens[i + 1])) {
                    if let Some(bal) = parse_amount(tokens[i + 1]) {
                        ledger.daily_balances.push(DailyBalance { date: iso_or_raw(tokens[i], year_hint), balance: bal });
                        any = true;
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if any {
                continue;
            }
            // Real content (an amount, or a long line) ends the daily balance block; short
            // header words ("Ledger", "Date Balance Date Balance") do not.
            let header_words = tokens.len() <= 6 && !tokens.iter().any(|t| is_amount_token(t));
            if tokens.first().and_then(|t| parse_date_token(t)).is_none() && !header_words {
                st.in_daily = false;
            }
        }

        // Transaction under a column header. On an aligned line (pdftotext -layout, or a
        // converted OCR table) the column an amount ends in decides its kind. On a flat
        // line (plain OCR) the amounts sit at the end: transaction amount, then running
        // balance when the table has one, and the kind comes from words. Either way the
        // running balance is checked: between two printed balances, credits minus debits
        // must equal the change, and `resolve_group` flips word-based kinds to make it fit.
        if let Some(c) = &columns {
            let starts_with_date = tokens.first().and_then(|t| parse_date_token(t)).is_some();
            let spans: Vec<(usize, &str)> = amount_spans(line);
            let aligned = !flat && line.contains("   ");
            if starts_with_date && !spans.is_empty() {
                let mut txn: Option<(f64, Kind, bool, usize)> = None; // amount, kind, strong, desc end
                let mut running: Option<f64> = None;
                if aligned {
                    for (end, tok) in &spans {
                        match c.kind_at(*end) {
                            None => running = parse_amount(tok),
                            Some(k) => {
                                if txn.is_none() {
                                    txn = parse_amount(tok).map(|v| (v.abs(), k, true, line.find(spans[0].1).unwrap_or(line.len())));
                                }
                            }
                        }
                    }
                } else {
                    let trailing = tokens.iter().rev().take_while(|t| is_amount_token(t)).count();
                    let (amount_idx, bal) = if trailing >= 2 && c.balance.is_some() {
                        (tokens.len() - 2, parse_amount(tokens[tokens.len() - 1]))
                    } else {
                        (tokens.len() - 1, None)
                    };
                    running = bal;
                    let desc = tokens[1..amount_idx].join(" ");
                    let (kind, strong) = kind_and_confidence(&desc, st.section);
                    let desc_end = line.find(tokens[amount_idx]).unwrap_or(line.len());
                    txn = parse_amount(tokens[amount_idx]).map(|v| (v.abs(), kind, strong, desc_end));
                }
                let (date, day) = resolve_date(tokens[0], year_hint);
                if let Some((amount, kind, strong, desc_end)) = txn {
                    let desc: String = line[..desc_end].split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
                    let id = ledger.transactions.len();
                    ledger.transactions.push(Txn { id, date: date.clone(), day, kind, amount, description: desc, page, table: st.table });
                    st.open_group.push((id, strong));
                    last_txn = Some(id);
                } else {
                    last_txn = None; // balance-only row ("Beginning Balance")
                }
                if let Some(bal) = running {
                    if c.balance.is_some() {
                        if let Some(prev) = st.last_balance.or(ledger.summary.beginning_balance) {
                            let group = std::mem::take(&mut st.open_group);
                            resolve_group(ledger, &group, prev, bal);
                        }
                        st.open_group.clear();
                        st.last_balance = Some(bal);
                    }
                    ledger.daily_balances.push(DailyBalance { date, balance: bal });
                }
                if txn.is_some() || running.is_some() {
                    continue;
                }
            }
        }

        // Image captions ("Regular Deposit  Date: 12/04  Amount: $2,364.21") repeat items
        // already listed; they are not transactions.
        if lower.contains("date:") && lower.contains("amount:") {
            last_txn = None;
            continue;
        }

        // Multi-column check tables: two or more (date, amount) pairs on one line, in either
        // "date check# amount" or "check# date amount" order.
        let date_idx: Vec<usize> = tokens.iter().enumerate().filter(|(_, t)| parse_date_token(t).is_some()).map(|(i, _)| i).collect();
        let amt_idx: Vec<usize> = tokens.iter().enumerate().filter(|(_, t)| is_amount_token(t)).map(|(i, _)| i).collect();
        // Between each date and its amount there is at most a check number and a gap marker;
        // prose there ("Fee period 11/01 - 11/30 ... $5.00") means this is not a check table.
        let check_table_shape = date_idx.iter().zip(&amt_idx).all(|(d, a)| d < a && a - d <= 3);
        if date_idx.len() >= 2 && date_idx.len() == amt_idx.len() && check_table_shape {
            let mut prev_end = 0usize;
            let mut seen_on_line: Vec<(String, f64)> = Vec::new();
            for (&d, &a) in date_idx.iter().zip(&amt_idx) {
                let mut desc: Vec<&str> = tokens[d + 1..a].to_vec();
                // A check number printed just before the date belongs to this entry.
                // (Reference numbers are longer; check numbers have at most seven digits.)
                let n = check_no(tokens[d.saturating_sub(1)]);
                if d > prev_end && d >= 1 && !n.is_empty() && n.len() <= 7 && n.chars().all(|c| c.is_ascii_digit()) {
                    desc.insert(0, tokens[d - 1]);
                }
                let desc: Vec<&str> = desc.into_iter().filter(|t| *t != "*").collect();
                let label = if desc.len() == 1 && check_no(desc[0]).chars().all(|c| c.is_ascii_digit()) {
                    format!("Check {}", check_no(desc[0]))
                } else {
                    desc.join(" ")
                };
                let amount = parse_amount(tokens[a]).unwrap_or(0.0).abs();
                prev_end = a + 1;
                // Check image pages caption the front and back with the same "#3214 09/19/2022 $17,230.00".
                if seen_on_line.contains(&(label.clone(), amount)) {
                    continue;
                }
                seen_on_line.push((label.clone(), amount));
                let id = ledger.transactions.len();
                let (date, day) = resolve_date(tokens[d], year_hint);
                ledger.transactions.push(Txn { id, date, day, kind: st.section.unwrap_or(Kind::Debit), amount, description: label, page, table: st.table });
            }
            last_txn = None;
            continue;
        }

        // The same table in "number amount date" order: "285 733.58 05/03 290 700.00 05/05".
        let number_amount_date = date_idx.len() >= 1 && date_idx.len() == amt_idx.len()
            && amt_idx.iter().zip(&date_idx).all(|(a, d)| a < d && d - a == 1 && *a >= 1 && check_no(tokens[a - 1]).chars().all(|c| c.is_ascii_digit()) && check_no(tokens[a - 1]).len() <= 7 && !check_no(tokens[a - 1]).is_empty());
        if number_amount_date && (date_idx.len() >= 2 || st.section == Some(Kind::Debit)) {
            for (&a, &d) in amt_idx.iter().zip(&date_idx) {
                let id = ledger.transactions.len();
                let (date, day) = resolve_date(tokens[d], year_hint);
                ledger.transactions.push(Txn { id, date, day, kind: Kind::Debit, amount: parse_amount(tokens[a]).unwrap_or(0.0).abs(), description: format!("Check {}", check_no(tokens[a - 1])), page, table: st.table });
            }
            last_txn = None;
            continue;
        }

        // Lone check entry "365989* 11/17 20,754.66".
        if tokens.len() == 3 && tokens[0].trim_end_matches('*').chars().all(|c| c.is_ascii_digit()) && parse_date_token(tokens[1]).is_some() && is_amount_token(tokens[2]) {
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[1], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind: Kind::Debit, amount: parse_amount(tokens[2]).unwrap_or(0.0).abs(), description: format!("Check {}", tokens[0].trim_end_matches('*')), page, table: st.table });
            last_txn = None;
            continue;
        }

        // Transaction line: date first, amount last.
        let starts_with_date = tokens.first().and_then(|t| parse_date_token(t)).is_some();
        let ends_with_amount = tokens.last().map(|t| is_amount_token(t)).unwrap_or(false);

        // PNC corporate: "06/03  28,273.92  Corporate ACH Txns/Fees  00024155901130577" and
        // "06/21  12490  450.00  017261553": date first, exactly one amount, a reference
        // number last. The reference (nine or more digits) is dropped from the description.
        let amount_positions: Vec<usize> = (1..tokens.len()).filter(|&i| is_amount_token(tokens[i])).collect();
        let summary_row = lower.contains("beginning balance") || lower.contains("ending balance") || lower.contains("previous balance") || lower.contains("balance forward");
        if starts_with_date && !ends_with_amount && tokens.len() >= 3 && amount_positions.len() == 1 && !summary_row {
            let a = amount_positions[0];
            let rest: Vec<&str> = tokens[1..].iter().enumerate().filter(|(i, t)| *i + 1 != a && !(t.len() >= 9 && t.chars().all(|c| c.is_ascii_digit()))).map(|(_, t)| *t).collect();
            let desc = if rest.len() == 1 && rest[0].len() <= 7 && rest[0].chars().all(|c| c.is_ascii_digit()) { format!("Check {}", rest[0]) } else { rest.join(" ") };
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[0], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind: kind_from_words(&desc, st.section), amount: parse_amount(tokens[a]).unwrap_or(0.0).abs(), description: desc, page, table: st.table });
            last_txn = Some(id);
            continue;
        }
        if starts_with_date && ends_with_amount && tokens.len() >= 2 {
            let amount = parse_amount(tokens[tokens.len() - 1]).unwrap_or(0.0).abs();
            // Statement summary rows also start with a date ("11/01/2025 Beginning Balance"); skip them.
            if summary_row {
                last_txn = None;
                continue;
            }
            let desc: String = tokens[1..tokens.len() - 1].join(" ");
            // Check tables print "check# date amount" pairs without a description; skip those.
            let kind = kind_from_words(&desc, st.section);
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[0], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind, amount, description: desc, page, table: st.table });
            last_txn = Some(id);
            continue;
        }

        // Continuation line: text right after a transaction with no date and no amount adds
        // to its description. Indentation is not required because OCR output has none.
        // A lone all-caps token with no digits is a page footer artifact, not a description.
        if let Some(id) = last_txn {
            let has_amount = tokens.iter().any(|t| is_amount_token(t));
            let footer_artifact = tokens.len() == 1 && tokens[0].len() >= 6 && tokens[0].chars().all(|c| c.is_ascii_uppercase());
            let boilerplate = lower.contains("member fdic") || lower.contains("page ") && lower.contains(" of ") || lower.starts_with("pg ");
            if !starts_with_date && !has_amount && tokens.len() <= 12 && !footer_artifact && !boilerplate {
                let t = &mut ledger.transactions[id];
                t.description.push(' ');
                t.description.push_str(trimmed);
                continue;
            }
        }
        last_txn = None;
    }
}

/// Amount tokens on a line with the character offset where each ends.
fn amount_spans(line: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut pos = 0;
    for tok in line.split(' ') {
        let end = pos + tok.len();
        if !tok.is_empty() && is_amount_token(tok) {
            out.push((end, tok));
        }
        pos = end + 1;
    }
    out
}

/// "#3214*" -> "3214": check numbers as printed in check tables and image captions.
fn check_no(t: &str) -> &str {
    t.trim_start_matches('#').trim_end_matches('*')
}

fn iso_or_raw(tok: &str, year_hint: Option<i32>) -> String {
    resolve_date(tok, year_hint).0
}

fn resolve_date(tok: &str, year_hint: Option<i32>) -> (String, Option<i64>) {
    match parse_date_token(tok) {
        Some((m, d, y)) => match y.or(year_hint) {
            Some(y) => (format!("{y:04}-{m:02}-{d:02}"), Some(days_from_civil(y, m, d))),
            None => (format!("{m:02}/{d:02}"), None),
        },
        None => (tok.to_string(), None),
    }
}

fn last_amount(line: &str) -> Option<f64> {
    line.split_whitespace().rev().find(|t| is_amount_token(t)).and_then(parse_amount)
}

fn capture_summary(lower: &str, line: &str, s: &mut Summary, page: usize) {
    if lower.contains("beginning balance") || lower.contains("previous balance") || lower.contains("opening ledger balance") || lower.contains("opening balance") {
        // Sunrise puts the values on the next line; Legends on the same line.
        let v = first_amount_after(line, &["beginning balance", "previous balance", "opening ledger balance", "opening balance"]);
        if let Some(v) = v {
            if !s.beginning_balances_seen.iter().any(|b| (b - v).abs() < 0.005) {
                s.beginning_balances_seen.push(v);
            }
        }
        if s.beginning_balance.is_none() {
            s.beginning_balance = v;
        }
    }
    let ntok = lower.split_whitespace().count();
    if s.ending_balance.is_none() && (lower.contains("ending balance") || lower.contains("current balance") || lower.contains("new balance") || lower.contains("ending ledger balance") || lower.contains("closing balance")) {
        s.ending_balance = last_amount(line);
    }
    // Summary totals. Two-column summaries put unrelated figures to the right of the value
    // ("2 Deposits/Credits  53,633.89  Average Ledger  154,454"), so the first amount after
    // the key is the value. Keys, by bank: Legends "Deposits/Other Credits", Sunrise "Total
    // Credits", Wells "Deposits/Additions", Webster "26 Credit(s) this period", Truist
    // "Deposits, credits and interest", Chase "Deposits and Credits", BofA "Deposits and other
    // credits", Mabrey "Deposits/Credits", Pinnacle "Credits + $.00".
    const CREDIT_KEYS: &[&str] = &["deposits/other credits", "total credits", "total deposits", "deposits/additions", "deposits and additions", "credit(s) this period", "deposits, credits and interest", "deposits and credits", "deposits and other credits", "deposits/credits"];
    const DEBIT_KEYS: &[&str] = &["checks/other debits", "total debits", "total withdrawals", "withdrawals/subtractions", "withdrawals and subtractions", "debit(s) this period", "other withdrawals, debits and service charges", "withdrawals and debits", "withdrawals and other debits", "checks/debits", "withdrawals/debits"];
    if s.total_credits.is_none() && !lower.contains("---") {
        if CREDIT_KEYS.iter().any(|k| lower.contains(k)) {
            s.total_credits = first_amount_after(line, CREDIT_KEYS).map(f64::abs);
        } else if lower.starts_with("credits") && ntok <= 5 {
            s.total_credits = first_amount_after(line, &["credits"]).map(f64::abs);
        }
    }
    if s.total_debits.is_none() && !lower.contains("---") {
        if let Some(k) = DEBIT_KEYS.iter().find(|k| lower.contains(*k)) {
            s.total_debits = first_amount_after(line, DEBIT_KEYS).map(f64::abs);
            s.debits_key = k;
            s.debits_page = Some(page);
        } else if lower.starts_with("debits") && ntok <= 5 {
            s.total_debits = first_amount_after(line, &["debits"]).map(f64::abs);
            s.debits_key = "debits";
            s.debits_page = Some(page);
        }
    }
    // Pinnacle-style summary cells anywhere on the line: "Credits + $.00", "Debits - $94,340.67".
    let toks: Vec<&str> = line.split_whitespace().collect();
    for (i, t) in toks.iter().enumerate() {
        let tl = t.to_ascii_lowercase();
        if tl != "credits" && tl != "debits" {
            continue;
        }
        let next = toks.get(i + 1).copied().unwrap_or("");
        let value = if next == "+" || next == "-" { toks.get(i + 2).copied().unwrap_or("") } else { next };
        if is_amount_token(value) {
            let v = parse_amount(value).map(f64::abs);
            if tl == "credits" && s.total_credits.is_none() {
                s.total_credits = v;
            } else if tl == "debits" && s.total_debits.is_none() {
                s.total_debits = v;
            }
        }
    }
    // Truist lists "Checks - 0.00" and Chase "Checks Paid 16 $17,652.08" as a separate debit
    // figure next to "Other withdrawals" / "Withdrawals and Debits"; the two are summed.
    // Two-column summaries put unrelated figures to the right, so take the first amount after the key.
    let same_block = s.debits_page.map(|p| p == page).unwrap_or(true);
    if same_block && s.checks_total.is_none() && (lower.starts_with("checks") && !lower.starts_with("checks paid") || lower.starts_with("checks paid") && ntok <= 5) {
        s.checks_total = first_amount_after(line, &["checks"]).map(f64::abs);
    }
    if same_block && s.fees_total.is_none() && (lower.starts_with("service fees") || lower.starts_with("service charge") || lower.starts_with("- service charge") || lower.starts_with("analysis or maintenance fee")) {
        s.fees_total = first_amount_after(line, &["service fees", "service charges", "service charge", "fees for period"]).map(f64::abs);
    }
    // "Beginning balance on 11/1" / "Ending balance on 11/30" carry the period.
    if lower.contains("beginning balance on ") || lower.contains("ending balance on ") {
        if let Some(d) = line.split_whitespace().find(|t| parse_date_token(t).is_some()) {
            if lower.contains("beginning") {
                s.period_start.get_or_insert(d.to_string());
            } else {
                s.period_end.get_or_insert(d.to_string());
            }
        }
    }
    if s.days_in_period.is_none() && lower.contains("days in") {
        // "30 Days in Statement Period" or "Total Days In Statement Period ...: 31"
        let toks: Vec<&str> = line.split_whitespace().collect();
        let after_colon = line.rsplit(':').next().and_then(|t| t.trim().parse::<u32>().ok());
        let before = toks.iter().position(|t| t.eq_ignore_ascii_case("days")).and_then(|i| i.checked_sub(1)).and_then(|i| toks[i].parse::<u32>().ok());
        s.days_in_period = after_colon.or(before).filter(|d| (1..=366).contains(d));
    }
    if s.average_balance.is_none() && (lower.contains("average balance") || lower.contains("average ledger balance") || lower.contains("avg daily balance")) {
        s.average_balance = first_amount_after(line, &["average balance", "average ledger balance", "avg daily balance"]);
    }
    if s.minimum_balance.is_none() && lower.contains("minimum balance") {
        s.minimum_balance = line.split_whitespace().find(|t| is_amount_token(t)).and_then(parse_amount);
    }
    if s.account_last4.is_none() && lower.contains("account") {
        // "Primary Account: XXXXXXXX1177", "Account: ****1234", "Account Number 123456789"
        let after = &line[lower.find("account").unwrap() + 7..];
        let cand = after
            .split(|c: char| c.is_whitespace() || c == ':')
            .filter(|t| t.len() >= 4)
            .find(|t| t.chars().all(|c| c.is_ascii_digit() || c == 'X' || c == 'x' || c == '*' || c == '-') && t.chars().rev().take(4).all(|c| c.is_ascii_digit()));
        if let Some(c) = cand {
            s.account_last4 = Some(c.chars().rev().take(4).collect::<String>().chars().rev().collect());
        }
    }
    if lower.contains("period") && lower.contains("through") || lower.contains("statement period") {
        let dates: Vec<&str> = line.split_whitespace().filter(|t| parse_date_token(t).is_some()).collect();
        if dates.len() >= 2 {
            s.period_start = Some(dates[0].to_string());
            s.period_end = Some(dates[1].to_string());
        }
    }
}

/// Summary column labels in left-to-right order, for two-line summaries.
fn column_labels(lower: &str) -> Vec<&'static str> {
    let mut found: Vec<(usize, &'static str)> = Vec::new();
    for (needle, label) in [
        ("previous balance", "beginning"), ("beginning balance", "beginning"),
        ("total credits", "credits"), ("total deposits", "credits"), ("deposits and other credits", "credits"), ("deposits/credits", "credits"),
        ("total debits", "debits"), ("total withdrawals", "debits"), ("checks and other debits", "debits"), ("withdrawals/debits", "debits"), ("checks/debits", "debits"),
        ("current balance", "ending"), ("ending balance", "ending"), ("new balance", "ending"),
    ] {
        if let Some(p) = lower.find(needle) {
            found.push((p, label));
        }
    }
    // PNC wraps the labels: "Beginning   Deposits and   Checks and   Ending" over
    // "balance   other credits   other debits   balance". Single words carry the order.
    if found.len() < 2 {
        let mut words: Vec<(usize, &'static str)> = Vec::new();
        for (needle, label) in [("beginning", "beginning"), ("deposits", "credits"), ("credits", "credits"), ("checks", "debits"), ("withdrawals", "debits"), ("debits", "debits"), ("ending", "ending")] {
            if let Some(p) = lower.find(needle) {
                if !words.iter().any(|(_, l)| *l == label) {
                    words.push((p, label));
                }
            }
        }
        if words.len() >= 3 && words.iter().any(|(_, l)| *l == "beginning") && words.iter().any(|(_, l)| *l == "ending") {
            found = words;
        }
    }
    found.sort();
    found.into_iter().map(|(_, l)| l).collect()
}

fn first_amount_after(line: &str, keys: &[&str]) -> Option<f64> {
    let lower = line.to_ascii_lowercase();
    let pos = keys.iter().filter_map(|k| lower.find(k).map(|p| p + k.len())).min()?;
    line[pos..].split_whitespace().find(|t| is_amount_token(t)).and_then(parse_amount)
}

// ─── Derived facts ────────────────────────────────────────────────────────

/// Payee key: description without dates, reference numbers and account fragments.
pub fn payee_key(desc: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for w in desc.split_whitespace() {
        let clean: String = w.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
        if clean.len() < 2 {
            continue;
        }
        // Skip transaction-type boilerplate so "ACH Withdrawal Nissan WFS" == "Nissan WFS".
        if matches!(clean.as_str(), "ach" | "withdrawal" | "deposit" | "debit" | "credit" | "internet" | "payment" | "pmt" | "purchase" | "sig" | "pin" | "recur" | "electronic" | "trans" | "with" | "kbd" | "rcr") {
            continue;
        }
        words.push(clean);
        if words.len() == 3 {
            break;
        }
    }
    words.join(" ")
}

fn cadence(days: &[i64]) -> &'static str {
    if days.len() < 2 {
        return "irregular";
    }
    let mut d = days.to_vec();
    d.sort_unstable();
    let gaps: Vec<i64> = d.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 0).collect();
    if gaps.is_empty() {
        return "irregular";
    }
    let all_business_daily = gaps.iter().all(|g| *g == 1 || (*g == 3 && true)) && d.iter().all(|x| weekday(*x) < 5);
    if all_business_daily {
        return "daily";
    }
    if gaps.iter().all(|g| (6..=8).contains(g)) {
        return "weekly";
    }
    if gaps.iter().all(|g| (13..=16).contains(g)) {
        return "biweekly";
    }
    if gaps.iter().all(|g| (27..=32).contains(g)) {
        return "monthly";
    }
    "irregular"
}

/// Running balances yield several entries per date; keep the last one as that day's
/// ending balance. Explicit daily balance tables already have one entry per date.
fn collapse_daily_balances(ledger: &mut Ledger) {
    let mut last: BTreeMap<String, f64> = BTreeMap::new();
    for b in &ledger.daily_balances {
        last.insert(b.date.clone(), b.balance);
    }
    if last.len() < ledger.daily_balances.len() {
        ledger.daily_balances = last.into_iter().map(|(date, balance)| DailyBalance { date, balance }).collect();
    }
}

fn derive(ledger: &mut Ledger) {
    collapse_daily_balances(ledger);
    ledger.parsed_credit_total = ledger.transactions.iter().filter(|t| t.kind == Kind::Credit).map(|t| t.amount).sum();
    ledger.parsed_debit_total = ledger.transactions.iter().filter(|t| t.kind == Kind::Debit).map(|t| t.amount).sum();

    ledger.nsf_items = ledger
        .transactions
        .iter()
        .filter(|t| {
            let l = t.description.to_ascii_lowercase();
            NSF_WORDS.iter().any(|w| has_phrase(&l, w)) && !l.contains("total")
        })
        .map(|t| t.id)
        .collect();

    // Funding candidates: wording, or unusually large relative to the median credit.
    let mut credits: Vec<f64> = ledger.transactions.iter().filter(|t| t.kind == Kind::Credit).map(|t| t.amount).collect();
    credits.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = credits.get(credits.len() / 2).copied().unwrap_or(0.0);
    // OCR text layers often drop spaces ("FUNDINGAL WEST"), so longer words match as
    // substrings; short ones need word boundaries to avoid "sloan" or "amcaster".
    let has_funding_word = |t: &Txn| {
        let l = t.description.to_ascii_lowercase();
        FUNDING_WORDS.iter().any(|w| {
            let w = w.trim();
            if w.len() <= 4 { has_phrase(&l, w) } else { l.contains(w) }
        })
    };
    ledger.funding_candidates = ledger.transactions.iter().filter(|t| t.kind == Kind::Credit && has_funding_word(t)).map(|t| t.id).collect();
    ledger.large_unlabeled_credits = ledger
        .transactions
        .iter()
        .filter(|t| t.kind == Kind::Credit && !has_funding_word(t) && median > 0.0 && t.amount >= 8.0 * median && t.amount >= 5000.0)
        .map(|t| t.id)
        .collect();

    // Recurring debits: same payee key and same amount (within 1%) at least twice.
    let mut groups: BTreeMap<(String, i64), Vec<&Txn>> = BTreeMap::new();
    for t in ledger.transactions.iter().filter(|t| t.kind == Kind::Debit && !t.description.starts_with("Check ")) {
        let lower = t.description.to_ascii_lowercase();
        if NOT_POSITION_WORDS.iter().any(|w| lower.contains(w)) {
            continue;
        }
        let key = payee_key(&t.description);
        if key.is_empty() {
            continue;
        }
        // Bucket amounts to the nearest 1% so 3,599.00 and 3,600.00 land together.
        let bucket = (t.amount.ln() * 100.0).round() as i64;
        groups.entry((key, bucket)).or_default().push(t);
    }
    let mut id = 0;
    for ((payee, _), txns) in groups {
        if txns.len() < 2 {
            continue;
        }
        let days: Vec<i64> = txns.iter().filter_map(|t| t.day).collect();
        let amount = txns.iter().map(|t| t.amount).sum::<f64>() / txns.len() as f64;
        ledger.recurring_debits.push(RecurringDebit {
            id,
            payee: txns[0].description.clone(),
            amount: (amount * 100.0).round() / 100.0,
            count: txns.len(),
            cadence: cadence(&days),
            dates: txns.iter().map(|t| t.date.clone()).collect(),
        });
        let _ = payee;
        id += 1;
    }
    ledger.recurring_debits.sort_by(|a, b| (b.count, b.amount).partial_cmp(&(a.count, a.amount)).unwrap());

    // Payee totals (any kind) for payees seen at least twice.
    let mut totals: BTreeMap<(String, Kind), (usize, f64, String)> = BTreeMap::new();
    for t in &ledger.transactions {
        if t.description.starts_with("Check ") {
            continue;
        }
        let key = payee_key(&t.description);
        if key.is_empty() {
            continue;
        }
        let e = totals.entry((key, t.kind)).or_insert((0, 0.0, t.description.clone()));
        e.0 += 1;
        e.1 += t.amount;
    }
    ledger.payees = totals
        .into_iter()
        .filter(|(_, (n, _, _))| *n >= 2)
        .map(|((_, kind), (count, total, payee))| PayeeTotal { payee, count, total: (total * 100.0).round() / 100.0, kind })
        .collect();
    ledger.payees.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap());
}

/// Find a year to attach to MM/DD dates: first 4-digit year in a date token, else
/// from "Statement Date: 09/30/2024"-style lines, else from "Nov 30, 2025".
fn year_hint(texts: &[&str]) -> Option<i32> {
    // Court filings and fax headers carry their own dates, so take the most common year,
    // giving lines that mention "statement" or "period" a heavy vote.
    let mut votes: BTreeMap<i32, usize> = BTreeMap::new();
    for text in texts {
        for line in text.lines() {
            let lower = line.to_ascii_lowercase();
            let weight = if lower.contains("statement") || lower.contains("period") { 10 } else { 1 };
            for tok in line.split_whitespace() {
                if let Some((_, _, Some(y))) = parse_date_token(tok) {
                    *votes.entry(y).or_default() += weight;
                }
            }
        }
    }
    // "November 30, 2024" style statement dates: a strong vote for that year.
    const MONTHS: &[&str] = &["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
    for text in texts {
        let lower = text.to_ascii_lowercase();
        let toks: Vec<&str> = lower.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).collect();
        for w in toks.windows(3) {
            if MONTHS.iter().any(|m| m.starts_with(w[0]) && w[0].len() >= 3) && w[1].chars().all(|c| c.is_ascii_digit()) && w[2].len() == 4 {
                if let Ok(y) = w[2].parse::<i32>() {
                    if (2000..=2100).contains(&y) {
                        *votes.entry(y).or_default() += 10;
                    }
                }
            }
        }
    }
    if let Some((y, _)) = votes.into_iter().max_by_key(|(_, n)| *n) {
        return Some(y);
    }
    for text in texts {
        for tok in text.split(|c: char| !c.is_ascii_digit()) {
            if tok.len() == 4 {
                if let Ok(y) = tok.parse::<i32>() {
                    if (2000..=2100).contains(&y) {
                        return Some(y);
                    }
                }
            }
        }
    }
    None
}

/// Some banks (Hancock Whitney, small banks) print two transaction columns side by side:
/// "Date  Amount  Description        Date  Amount  Description". Split each line of such a
/// block at the start of the right header and emit the left column, then the right, so the
/// line parser sees one transaction per line. A block ends at a line that crosses the gap.
pub fn unfold_two_columns(text: &str) -> String {
    let mut out = String::new();
    let mut split: Option<usize> = None;
    let mut left: Vec<String> = Vec::new();
    let mut right: Vec<String> = Vec::new();
    let flush = |out: &mut String, left: &mut Vec<String>, right: &mut Vec<String>| {
        for l in left.drain(..).chain(right.drain(..)) {
            out.push_str(&l);
            out.push('\n');
        }
    };
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let toks: Vec<&str> = line.split_whitespace().collect();
        // A header naming date/amount/description twice. The right column starts at the
        // second occurrence of whichever word repeats ("Description  Date  Amount  Description").
        let repeated = ["date", "description", "amount"].into_iter().find(|w| lower.matches(w).count() >= 2);
        let is_header = toks.len() <= 10 && !toks.iter().any(|t| is_amount_token(t)) && repeated.is_some() && lower.contains("date") && (lower.contains("amount") || lower.contains("serial")) && !lower.contains("balance");
        if is_header {
            flush(&mut out, &mut left, &mut right);
            // The right column starts at the second "Date"; when only one "Date" is printed
            // (left header partly missing) it is that one, as long as a label precedes it.
            let first = lower.find("date").unwrap();
            split = match lower[first + 4..].find("date") {
                Some(p) => Some(first + 4 + p),
                None if !lower[..first].trim().is_empty() => Some(first),
                None => None,
            };
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // A new section title ("• Checks", "Daily Balance", "Withdrawals and Debits") ends the block.
        let has_date_or_amount = toks.iter().any(|t| is_amount_token(t) || parse_date_token(t).is_some());
        let section_title = !has_date_or_amount && !toks.is_empty() && (line.trim_start().starts_with('•') || line.trim_start().starts_with('*') || section_for(line).is_some() || lower.contains("balance") || lower.contains("summary"));
        if split.is_some() && section_title {
            flush(&mut out, &mut left, &mut right);
            split = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        match split {
            Some(at) if line.len() + 8 > at => {
                // The right column's data starts inside the gap before the right header, not
                // exactly under it, so split at the whitespace run nearest the header position.
                match gap_near(line, at) {
                    Some(cut) => {
                        let (l, r) = line.split_at(cut);
                        if !l.trim().is_empty() {
                            left.push(l.to_string());
                        }
                        if !r.trim().is_empty() {
                            right.push(r.to_string());
                        }
                    }
                    None => {
                        // Text running through the gap: a title or footer ends the block.
                        flush(&mut out, &mut left, &mut right);
                        split = None;
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
            Some(_) => {
                if !line.trim().is_empty() {
                    left.push(line.to_string());
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    flush(&mut out, &mut left, &mut right);
    out
}

/// End offset of a run of three or more spaces that lies within 14 characters before
/// `at` or 4 after it; None when text runs through that region.
fn gap_near(line: &str, at: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let lo = at.saturating_sub(14);
    let hi = (at + 4).min(bytes.len());
    // Text that starts in or after the gap is the right column alone.
    let first = line.len() - line.trim_start().len();
    if first >= lo {
        return Some(first);
    }
    let mut best: Option<(usize, usize)> = None; // (distance to `at`, cut)
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            let start = i;
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            let run = i - start;
            // A gap ending at the end of the line is not a column boundary.
            if run >= 3 && start > 0 && i < bytes.len() && i >= lo && start <= hi {
                let d = (i as i64 - at as i64).unsigned_abs() as usize;
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, i));
                }
            }
        } else {
            i += 1;
        }
    }
    if let Some((_, cut)) = best {
        return Some(cut);
    }
    // Line ends before the right column: left only.
    if line.len() <= hi {
        return Some(line.len());
    }
    None
}

/// Parse a whole statement set. `pages` are (page number, text) in reading order. A file
/// that bundles several statements (months, or accounts) is split where a new statement
/// starts and each part is parsed on its own; the parts are then combined.
pub fn parse(pages: &[(usize, &str)]) -> Ledger {
    let segments = segment_statements(pages);
    if segments.len() <= 1 {
        let mut ledger = parse_one(pages);
        derive(&mut ledger);
        return ledger;
    }
    let mut combined = Ledger::default();
    for seg in &segments {
        let part = parse_one(seg);
        let (id_off, table_off) = (combined.transactions.len(), combined.transactions.iter().map(|t| t.table).max().unwrap_or(0) + 1);
        combined.transactions.extend(part.transactions.into_iter().map(|mut t| {
            t.id += id_off;
            t.table += table_off;
            t
        }));
        combined.daily_balances.extend(part.daily_balances);
        combined.statements.push(part.summary);
    }
    combined.summary = combine_summaries(&combined.statements);
    combined.summary.bank = detect_bank(&pages.iter().map(|(_, t)| *t).collect::<Vec<_>>());
    derive(&mut combined);
    combined
}

/// One statement's worth of pages, no derived facts yet.
fn parse_one(pages: &[(usize, &str)]) -> Ledger {
    let mut ledger = Ledger::default();
    let texts: Vec<&str> = pages.iter().map(|(_, t)| *t).collect();
    let year = year_hint(&texts);
    ledger.summary.bank = detect_bank(&texts);
    let mut st = State::default();
    for (page, text) in pages {
        let unfolded = unfold_two_columns(text);
        parse_page(&unfolded, *page, year, &mut ledger, &mut st);
    }
    // Checks and fees printed as separate figures are added unless the debit key already
    // covers them ("Checks and other debits", "... debits and service charges").
    if let Some(other) = ledger.summary.total_debits {
        let key = ledger.summary.debits_key;
        let checks = if key.contains("check") { 0.0 } else { ledger.summary.checks_total.unwrap_or(0.0) };
        let fees = if key.contains("service") { 0.0 } else { ledger.summary.fees_total.unwrap_or(0.0) };
        ledger.summary.total_debits = Some(other + checks + fees);
    }
    dedup_across_tables(&mut ledger);
    ledger
}

/// Split pages into statements. A page whose own text yields a beginning balance starts a
/// new statement, except for the first such page, which starts the first one along with
/// any cover pages before it. Pages of one statement never repeat its beginning balance
/// with a different value, so a repeated figure (Webster prints it twice) does not split.
fn segment_statements<'a>(pages: &[(usize, &'a str)]) -> Vec<Vec<(usize, &'a str)>> {
    let mut segments: Vec<Vec<(usize, &str)>> = Vec::new();
    let mut current: Vec<(usize, &str)> = Vec::new();
    let mut current_beginning: Option<f64> = None;
    for &(page, text) in pages {
        let mut probe = Ledger::default();
        let mut st = State::default();
        parse_page(&unfold_two_columns(text), page, None, &mut probe, &mut st);
        let begins = probe.summary.beginning_balance;
        let starts_new = match (begins, current_beginning) {
            (Some(b), Some(cur)) if (b - cur).abs() >= 0.005 => true,
            _ => false,
        };
        if starts_new && !current.is_empty() {
            segments.push(std::mem::take(&mut current));
        }
        if begins.is_some() && (starts_new || current_beginning.is_none()) {
            current_beginning = begins;
        }
        current.push((page, text));
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// Whole-set figures from per-statement summaries: totals add up (only when every
/// statement printed one), balances run from the first beginning to the last ending,
/// the period spans them all.
fn combine_summaries(parts: &[Summary]) -> Summary {
    let sum = |f: fn(&Summary) -> Option<f64>| -> Option<f64> {
        if parts.iter().all(|p| f(p).is_some()) { Some(parts.iter().filter_map(f).sum()) } else { None }
    };
    let days = if parts.iter().all(|p| p.days_in_period.is_some()) { Some(parts.iter().filter_map(|p| p.days_in_period).sum()) } else { None };
    Summary {
        beginning_balance: parts.first().and_then(|p| p.beginning_balance),
        ending_balance: parts.last().and_then(|p| p.ending_balance),
        total_credits: sum(|p| p.total_credits),
        total_debits: sum(|p| p.total_debits),
        days_in_period: days,
        average_balance: None,
        minimum_balance: parts.iter().filter_map(|p| p.minimum_balance).reduce(f64::min),
        period_start: parts.iter().find_map(|p| p.period_start.clone()),
        period_end: parts.iter().rev().find_map(|p| p.period_end.clone()),
        account_last4: parts.iter().find_map(|p| p.account_last4.clone()),
        bank: None,
        checks_total: None,
        fees_total: None,
        debits_key: "",
        debits_page: None,
        beginning_balances_seen: parts.iter().flat_map(|p| p.beginning_balances_seen.iter().copied()).collect(),
    }
}

/// Drop a transaction that repeats (same date, kind and amount) one read from an earlier
/// table. Webster prints a running-balance table and then per-type lists; Pinnacle and
/// Legends print check tables and check-image captions; all of these would double the
/// totals. Repeats inside one table (two identical card charges on one day) are kept:
/// each earlier line can absorb at most one later copy.
fn dedup_across_tables(ledger: &mut Ledger) {
    let mut available: BTreeMap<(String, Kind, i64), Vec<(usize, usize)>> = BTreeMap::new();
    let mut keep = vec![true; ledger.transactions.len()];
    for (i, t) in ledger.transactions.iter().enumerate() {
        let key = (t.date.clone(), t.kind, (t.amount * 100.0).round() as i64);
        let slot = available.entry(key).or_default();
        if let Some(pos) = slot.iter().position(|(table, _)| *table != t.table) {
            slot.remove(pos);
            keep[i] = false;
        } else {
            slot.push((t.table, i));
        }
    }
    if keep.iter().any(|k| !k) {
        let mut i = 0;
        ledger.transactions.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
        for (id, t) in ledger.transactions.iter_mut().enumerate() {
            t.id = id;
        }
    }
}

/// Number of rows under a transaction table header that start with a date but carry no
/// amount. On plain OCR output of a scanned table these are rows whose amount cells were
/// dropped; the caller then asks the OCR model for the table itself.
pub fn rows_missing_amounts(text: &str) -> usize {
    let mut under_header = false;
    let mut missing = 0;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        if !toks.iter().any(|t| is_amount_token(t)) && Columns::labels(line).is_complete(lower.contains("date")) {
            under_header = true;
            continue;
        }
        if under_header && parse_date_token(toks[0]).is_some() && toks.len() >= 3 && !toks.iter().any(|t| is_amount_token(t)) {
            missing += 1;
        }
    }
    missing
}

/// Bank named on the statement. Counts mentions on the first pages and picks the most
/// frequent name from a fixed list, so a Wells Fargo statement that mentions Zelle or a
/// wire to Chase still reads "Wells Fargo".
pub fn detect_bank(texts: &[&str]) -> Option<String> {
    const BANKS: &[(&str, &str)] = &[
        ("wells fargo", "Wells Fargo"), ("truist", "Truist"), ("jpmorgan chase", "Chase"), ("chase.com", "Chase"),
        ("bank of america", "Bank of America"), ("pnc bank", "PNC"), ("pnc.com", "PNC"), ("td bank", "TD Bank"), ("u.s. bank", "U.S. Bank"), ("usbank.com", "U.S. Bank"),
        ("capital one", "Capital One"), ("citibank", "Citibank"), ("regions bank", "Regions"), ("fifth third", "Fifth Third"),
        ("huntington", "Huntington"), ("keybank", "KeyBank"), ("citizens bank", "Citizens"), ("m&t bank", "M&T Bank"), ("bmo", "BMO"),
        ("webster", "Webster Bank"), ("pinnacle", "Pinnacle Bank"), ("legends bank", "Legends Bank"), ("sunrise bank", "Sunrise Banks"),
        ("ally bank", "Ally"), ("frost bank", "Frost Bank"), ("frostbank", "Frost Bank"), ("first citizens", "First Citizens"), ("comerica", "Comerica"),
        ("zions", "Zions"), ("synovus", "Synovus"), ("santander", "Santander"), ("navy federal", "Navy Federal"), ("bluevine", "Bluevine"),
        ("mercury", "Mercury"), ("novo", "Novo"), ("relay", "Relay"), ("axos", "Axos"), ("live oak", "Live Oak"), ("first horizon", "First Horizon"),
        ("flagstar", "Flagstar"), ("valley national", "Valley National"), ("east west bank", "East West Bank"), ("cathay", "Cathay Bank"),
        ("customers bank", "Customers Bank"), ("signature bank", "Signature Bank"), ("silicon valley bank", "Silicon Valley Bank"),
        ("hancock whitney", "Hancock Whitney"), ("hancockwhitney", "Hancock Whitney"), ("mabrey", "Mabrey Bank"), ("wintrust", "Wintrust"),
        ("byline", "Byline Bank"), ("old national", "Old National"), ("associated bank", "Associated Bank"),
        ("first republic", "First Republic"), ("umpqua", "Umpqua"), ("banner bank", "Banner Bank"), ("amerant", "Amerant"), ("city national", "City National"),
        ("credit union", "Credit Union"),
    ];
    let mut votes: BTreeMap<&str, usize> = BTreeMap::new();
    for text in texts.iter().take(3) {
        let lower = text.to_ascii_lowercase();
        for (needle, name) in BANKS {
            let n = lower.matches(needle).count();
            if n > 0 {
                *votes.entry(name).or_default() += n;
            }
        }
    }
    votes.into_iter().max_by_key(|(_, n)| *n).map(|(name, _)| name.to_string())
}

// ─── Report math ──────────────────────────────────────────────────────────

/// Numbers for the dashboard, computed from the ledger and the model's classification.
#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    pub total_credits: f64,
    pub funding_deposits: f64,
    pub true_revenue: f64,
    pub negative_days: u32,
    pub avg_daily_balance: f64,
    pub nsf_count: u32,
    pub days_in_period: u32,
    pub total_debt_service_daily: f64,
    pub safe_new_payment: f64,
    pub leverage_ratio: f64,
    /// Where each figure came from, for the UI and the memo.
    pub sources: Vec<String>,
}

pub fn per_day(amount: f64, cadence: &str) -> f64 {
    match cadence {
        "daily" => amount,
        "weekly" => amount / 5.0,
        "biweekly" => amount / 10.0,
        "monthly" => amount / 21.0,
        _ => amount / 21.0,
    }
}

pub fn compute_metrics(ledger: &Ledger, funding_ids: &[usize], confirmed_positions: &[(f64, String)]) -> Metrics {
    let mut sources = Vec::new();
    let total_credits = match ledger.summary.total_credits {
        Some(v) => {
            sources.push(format!("total credits {v:.2} from the statement summary (parsed lines sum to {:.2})", ledger.parsed_credit_total));
            v
        }
        None => {
            sources.push(format!("total credits {:.2} summed from {} parsed credit lines", ledger.parsed_credit_total, ledger.transactions.iter().filter(|t| t.kind == Kind::Credit).count()));
            ledger.parsed_credit_total
        }
    };
    let funding_deposits: f64 = ledger.transactions.iter().filter(|t| funding_ids.contains(&t.id)).map(|t| t.amount).sum();
    let true_revenue = (total_credits - funding_deposits).max(0.0);

    let negative_days = if !ledger.daily_balances.is_empty() {
        sources.push(format!("negative days counted over {} daily balances", ledger.daily_balances.len()));
        ledger.daily_balances.iter().filter(|b| b.balance < 0.0).count() as u32
    } else if let Some(min) = ledger.summary.minimum_balance {
        sources.push(format!("no daily balance table; minimum balance {min:.2} used for negative days"));
        if min < 0.0 { 1 } else { 0 }
    } else {
        sources.push("no daily balance table and no minimum balance: negative days unknown, reported as 0".into());
        0
    };

    let avg_daily_balance = match ledger.summary.average_balance {
        Some(v) => v,
        None if !ledger.daily_balances.is_empty() => {
            sources.push("average balance is the mean of the daily balance table".into());
            ledger.daily_balances.iter().map(|b| b.balance).sum::<f64>() / ledger.daily_balances.len() as f64
        }
        None => match (ledger.summary.beginning_balance, ledger.summary.ending_balance) {
            (Some(b), Some(e)) => {
                sources.push("average balance approximated as the mean of beginning and ending balance".into());
                (b + e) / 2.0
            }
            _ => 0.0,
        },
    };

    let days_in_period = ledger.summary.days_in_period.unwrap_or_else(|| {
        // Span of the period dates when printed, else of the transaction dates, else 30.
        let span = |a: Option<i64>, b: Option<i64>| match (a, b) {
            (Some(a), Some(b)) if b >= a => Some((b - a + 1) as u32),
            _ => None,
        };
        let year = ledger.transactions.iter().find_map(|t| t.date.get(..4)).and_then(|y| y.parse::<i32>().ok());
        let to_day = |d: &Option<String>| d.as_ref().and_then(|d| parse_date_token(d)).and_then(|(m, dd, y)| y.or(year).map(|y| days_from_civil(y, m, dd)));
        span(to_day(&ledger.summary.period_start), to_day(&ledger.summary.period_end))
            .or_else(|| {
                let days: Vec<i64> = ledger.transactions.iter().filter_map(|t| t.day).collect();
                span(days.iter().min().copied(), days.iter().max().copied()).filter(|d| *d >= 20)
            })
            .unwrap_or(30)
    });
    let nsf_count = ledger.nsf_items.len() as u32;
    let total_debt_service_daily: f64 = confirmed_positions.iter().map(|(amt, cad)| per_day(*amt, cad)).sum();
    let daily_revenue = true_revenue / days_in_period as f64;
    let safe_new_payment = (daily_revenue * 0.10 - total_debt_service_daily).max(0.0);
    let leverage_ratio = if daily_revenue > 0.0 { total_debt_service_daily / daily_revenue } else { 0.0 };

    Metrics {
        total_credits,
        funding_deposits,
        true_revenue,
        negative_days,
        avg_daily_balance,
        nsf_count,
        days_in_period,
        total_debt_service_daily,
        safe_new_payment,
        leverage_ratio,
        sources,
    }
}

/// Transparent baseline risk score from the computed metrics. The model may move it by
/// at most two points and must say why. Every point is explained in `factors`.
#[derive(Debug, Clone, Serialize)]
pub struct RiskBaseline {
    pub score: u8,
    pub factors: Vec<String>,
}

pub fn risk_baseline(m: &Metrics, positions: usize, has_daily_balances: bool) -> RiskBaseline {
    let mut score: i32 = 3;
    let mut factors = Vec::new();

    let neg = match m.negative_days {
        0 => 0,
        1..=2 => 1,
        3..=5 => 2,
        _ => 3,
    };
    if neg > 0 {
        factors.push(format!("+{neg}: {} negative balance day(s)", m.negative_days));
    } else if has_daily_balances {
        factors.push("+0: no negative balance days".into());
    } else {
        factors.push("+0: no daily balance table; negative days only inferred from the minimum balance".into());
    }
    score += neg;

    let nsf = match m.nsf_count {
        0 => 0,
        1..=2 => 1,
        _ => 2,
    };
    if nsf > 0 {
        factors.push(format!("+{nsf}: {} NSF / returned item(s)", m.nsf_count));
    }
    score += nsf;

    let stack = match positions {
        0 => 0,
        1 => 1,
        2..=3 => 2,
        _ => 3,
    };
    if stack > 0 {
        factors.push(format!("+{stack}: {positions} existing position(s)"));
    }
    score += stack;

    let lev = if m.leverage_ratio < 0.05 {
        0
    } else if m.leverage_ratio < 0.15 {
        1
    } else if m.leverage_ratio < 0.30 {
        2
    } else {
        3
    };
    if lev > 0 {
        factors.push(format!("+{lev}: debt service is {:.0}% of daily revenue", m.leverage_ratio * 100.0));
    }
    score += lev;

    let daily_rev = if m.days_in_period > 0 { m.true_revenue / m.days_in_period as f64 } else { 0.0 };
    if daily_rev > 0.0 && m.avg_daily_balance < 3.0 * daily_rev {
        score += 1;
        factors.push(format!("+1: average balance {:.0} is under three days of revenue ({:.0}/day)", m.avg_daily_balance, daily_rev));
    }

    RiskBaseline { score: score.clamp(1, 10) as u8, factors }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGENDS: &str = r#"
                  BUSINESSPREFERRED   CHECKING
                  11/01/2025 Beginning Balance                                         57,739.72
                                70 Deposits/Other Credits                      +    1,321,117.77
                               136 Checks/Other Debits                         -    1,365,636.99
                  11/30/2025 Ending Balance        30 Days in Statement Period         13,220.50
                  ----------------------------          Deposits/Other Credits          ----------------------------
                  11/03/2025 Deposit                                                                                       754.44
                  11/03/2025 ACH Deposit                                                                               57,592.12
                    PNCBANK-PROCEEDS  LOAN FUND Al West Nissan
                  ------- Checks listed in numerical order;    (*)   indicates    gap in sequence -------
                              6020    11/26    23,300.00                 365872       11/07             1,550.00
                  ---------------------------------           Other Debits  ---------------------------------
                  11/18/2025 ACH Withdrawal                                                                 3,599.00
                    CFG MERCHANT    SOL ACHPAYMENT   AL WESTINC 14
                  11/25/2025 ACH Withdrawal                                                                 3,599.00
                    CFG MERCHANT    SOL ACHPAYMENT   AL WESTINC 14
                  11/10/2025 Dep Item Ret Chrg                                                                3.00
"#;

    const SUNRISE: &str = r#"
       Last Statement Previous Balance                  Total Credits                      Total Debits          This Statement          Current Balance
           08/30/24           $3,702.38               $113,045.99 (141)                  $107,697.51 (154)            09/30/24               $9,050.86
       Minimum Balance                          $4,404.84-
       Average Balance                           $3,980.14
       Total Days In Statement Period 08/31/24 Through 09/30/24:                                       31
        OTHER CREDITS
       Date Description                                                                                                                            Amount
       09/03 Toast Dep Sep 02 XXXXXX0000OPHBG                                                                                                     $3,600.00
        OTHER DEBITS
        09/05 Returned Checks NSF Charge                                                                  $35.00
        09/05 Payment To Commercial Non Re Loan XXXXXXXXXX00461                                        $3,272.60
         DAILY BALANCE
           Date                    Balance         Date                   Balance                    Date              Balance
           09/03                   $9,251.89       09/12                 $7,038.70                   09/23             $2,475.59
           09/04                   $4,404.84-      09/13                 $7,082.35                   09/24             $1,497.56-
"#;

    const OCR_STYLE: &str = "11/01/2025 Beginning Balance 57,739.72
70 Deposits/Other Credits + 1,321,117.77
136 Checks/Other Debits - 1,365,636.99
11/30/2025 Ending Balance 30 Days in Statement Period 13,220.50

------------------------------------------
Deposits/Other Credits ------------------------------------------
11/03/2025 Deposit 754.44
11/03/2025 ACH Deposit 57,592.12
PNCBANK-PROCEEDS LOAN FUND Al West Nissan
11/04/2025 ACH Deposit 207.56
MERCHANT SVCS IPSMXASETL AL WEST NISSAN WARR
Pg 1 of 8
MEMBER FDIC
";

    const COLUMN_STYLE: &str = "Transaction history
                    Check                                                                    Deposits/      Withdrawals/      Ending daily
      Date        Number Description                                                        Credits           Debits          balance
      1/2                Purchase authorized on 12/31 Costco Whse #0123 Seattle WA                              145.67          1,234.56
      1/3                Deposit Made In A Branch/Store                                    2,500.00                            3,734.56
      1/3                Everest Business Fundi Everest Bu 220103                                               399.00          3,335.56
      1/4                Everest Business Fundi Everest Bu 220104                                               399.00          2,936.56
      1/4         1021   Check                                                                                  800.00          2,136.56
      Ending balance on 1/4                                                                                                     2,136.56
      Totals                                                                             $2,500.00        $1,743.67
";

    const WELLS_SUMMARY: &str = "November 30, 2024       Page 2 of 6
Statement period activity summary                                                         Account number:         1196
     Beginning balance on 11/1                                           -$2.71
     Deposits/Additions                                              20,110.00
     Withdrawals/Subtractions                                      - 17,760.14
     Ending balance on 11/30                                         $2,347.15
";

    #[test]
    fn wells_summary_block_and_period() {
        let l = parse(&[(1, WELLS_SUMMARY)]);
        assert_eq!(l.summary.beginning_balance, Some(-2.71));
        assert_eq!(l.summary.total_credits, Some(20110.0));
        assert_eq!(l.summary.total_debits, Some(17760.14));
        assert_eq!(l.summary.ending_balance, Some(2347.15));
        assert_eq!(l.summary.account_last4.as_deref(), Some("1196"));
        assert_eq!(l.summary.period_start.as_deref(), Some("11/1"));
        assert_eq!(l.summary.period_end.as_deref(), Some("11/30"));
        assert_eq!(year_hint(&[WELLS_SUMMARY]), Some(2024));
        let m = compute_metrics(&l, &[], &[]);
        assert_eq!(m.days_in_period, 30);
    }

    #[test]
    fn column_layout_assigns_kind_by_column_and_collects_running_balance() {
        let l = parse(&[(1, COLUMN_STYLE)]);
        let credits: Vec<&Txn> = l.transactions.iter().filter(|t| t.kind == Kind::Credit).collect();
        let debits: Vec<&Txn> = l.transactions.iter().filter(|t| t.kind == Kind::Debit).collect();
        assert_eq!(credits.len(), 1, "{:?}", l.transactions);
        assert_eq!(credits[0].amount, 2500.0);
        assert_eq!(debits.len(), 4, "{:?}", debits);
        assert_eq!(debits[0].amount, 145.67, "transaction amount, not the running balance");
        assert!(debits[1].description.starts_with("Everest"));
        // Running balances collapse to one ending balance per day.
        assert_eq!(l.daily_balances.len(), 3);
        assert_eq!(l.daily_balances.last().unwrap().balance, 2136.56);
        // Two identical daily debits form a recurring candidate.
        assert_eq!(l.recurring_debits.len(), 1);
        assert_eq!(l.recurring_debits[0].amount, 399.0);
    }

    #[test]
    fn ocr_style_text_without_indentation_parses() {
        let l = parse(&[(1, OCR_STYLE)]);
        assert_eq!(l.summary.total_credits, Some(1321117.77));
        let credits: Vec<&Txn> = l.transactions.iter().filter(|t| t.kind == Kind::Credit).collect();
        assert_eq!(credits.len(), 3);
        assert!(credits[1].description.contains("PNCBANK-PROCEEDS"));
        assert!(credits[2].description.contains("MERCHANT SVCS"));
        assert!(!credits[2].description.contains("Pg 1"), "footer not attached");
    }

    #[test]
    fn amounts_parse_with_signs_and_symbols() {
        assert_eq!(parse_amount("$1,234.56"), Some(1234.56));
        assert_eq!(parse_amount("4,404.84-"), Some(-4404.84));
        assert_eq!(parse_amount("(12.00)"), Some(-12.0));
        assert!(is_amount_token("1,321,117.77"));
        assert!(!is_amount_token("365872"));
        assert!(!is_amount_token("11/07"));
    }

    #[test]
    fn legends_summary_and_recurring_debits() {
        let l = parse(&[(1, LEGENDS)]);
        assert_eq!(l.summary.beginning_balance, Some(57739.72));
        assert_eq!(l.summary.total_credits, Some(1321117.77));
        assert_eq!(l.summary.total_debits, Some(1365636.99));
        assert_eq!(l.summary.ending_balance, Some(13220.50));
        assert_eq!(l.summary.days_in_period, Some(30));
        let credits: Vec<&Txn> = l.transactions.iter().filter(|t| t.kind == Kind::Credit).collect();
        assert_eq!(credits.len(), 2);
        assert!(credits[1].description.contains("PNCBANK-PROCEEDS"), "continuation line attached");
        assert!(l.funding_candidates.contains(&credits[1].id));
        // Two check-register entries plus three debit lines.
        assert_eq!(l.transactions.iter().filter(|t| t.kind == Kind::Debit).count(), 5);
        assert_eq!(l.recurring_debits.len(), 1);
        assert_eq!(l.recurring_debits[0].amount, 3599.0);
        assert_eq!(l.recurring_debits[0].cadence, "weekly");
        assert_eq!(l.nsf_items.len(), 1);
    }

    #[test]
    fn risk_baseline_is_explained() {
        let l = parse(&[(1, SUNRISE)]);
        let m = compute_metrics(&l, &[], &[]);
        let r = risk_baseline(&m, 0, true);
        // 3 + 1 (two negative days) + 1 (one NSF) + 1 (thin balance vs 113k/31 days)
        assert_eq!(r.score, 6);
        assert_eq!(r.factors.len(), 3);
    }

    #[test]
    fn sunrise_summary_daily_balances_and_nsf() {
        let l = parse(&[(1, SUNRISE)]);
        assert_eq!(l.summary.total_credits, Some(113045.99));
        assert_eq!(l.summary.total_debits, Some(107697.51));
        assert_eq!(l.summary.days_in_period, Some(31));
        assert_eq!(l.summary.average_balance, Some(3980.14));
        assert_eq!(l.summary.minimum_balance, Some(-4404.84));
        assert_eq!(l.daily_balances.len(), 6);
        assert_eq!(l.daily_balances.iter().filter(|b| b.balance < 0.0).count(), 2);
        assert_eq!(l.nsf_items.len(), 1);
        assert_eq!(l.transactions[0].date, "2024-09-03");
        let m = compute_metrics(&l, &[], &[]);
        assert_eq!(m.negative_days, 2);
        assert_eq!(m.total_credits, 113045.99);
    }

    const WEBSTER: &str = r#"
Account Summary
Date          Description
06/01/2024    Beginning Balance                     $141,391.00
              2 Debit(s) this period                $8,674.10
              3 Credit(s) this period               $11,142.21
06/30/2024    Ending Balance                        $143,859.11
Transaction Activity
 Transaction Date     Description                                         Debits              Credits            Balance
 06/01/2024           Beginning Balance                                                                    $141,391.00
 06/03/2024           HRTLAND PMT SYS TXNS/FEES THE HEALTHY                               $10,995.75       $152,386.75
                      CHOICE APO XXXXXXXXXXX2222
 06/03/2024           WEPAY PAYMENTS NTE*ZZZ*Payouts\                                          $23.07      $152,409.82
 06/03/2024           HRTLAND PMT SYS TXNS/FEES THE HEALTHY           -$8,384.73                           $144,025.09
 06/05/2024           VANTIV_INTG_PYMT BILLNG Merch Bankcard             -$289.37                          $143,735.72
 06/06/2024           WePay PAYMENTS NTE*ZZZ*Payouts\                                         $123.39      $143,859.11
 Debits
 Date               Description                                                                           Amount
 06/03/2024         HRTLAND PMT SYS TXNS/FEES THE HEALTHY CHOICE APO                                    -$8,384.73
 06/05/2024         VANTIV_INTG_PYMT BILLNG Merch Bankcard 286809 The Heal                                -$289.37
 Credits
 Date               Description                                                                           Amount
 06/03/2024         HRTLAND PMT SYS TXNS/FEES THE HEALTHY CHOICE APO                                    $10,995.75
 06/03/2024         WEPAY PAYMENTS NTE*ZZZ*Payouts\                                                        $23.07
 06/06/2024         WePay PAYMENTS NTE*ZZZ*Payouts\                                                       $123.39
"#;

    #[test]
    fn webster_per_type_lists_do_not_double_the_running_balance_table() {
        let l = parse(&[(1, WEBSTER)]);
        assert_eq!(l.summary.total_debits, Some(8674.10));
        assert_eq!(l.summary.total_credits, Some(11142.21));
        assert_eq!(l.summary.bank, None); // no bank name in the snippet
        assert_eq!(l.transactions.len(), 5, "{:?}", l.transactions);
        assert!((l.parsed_credit_total - 11142.21).abs() < 0.001);
        assert!((l.parsed_debit_total - 8674.10).abs() < 0.001);
        assert_eq!(l.daily_balances.len(), 4); // 06/01 (beginning row), 06/03, 06/05, 06/06
    }

    const PINNACLE: &str = r#"
Statement of Account
       Balance 9/01/22                            Summary
       $ 127,574.68
                                                  Credits      + $.00
       Balance 10/02/22                           Interest     + $.00
       $ 33,234.01                                Debits       - $94,340.67
Debit Transactions
Other Debits
9/20            BOOKING.COM B.V. 1035078125 10000706034723              4,917.05
Checks
9/19            Check 3214                                             17,230.00
9/27            Check 3218*                                            17,035.65
Total Debits                                                          $39,182.70
DAILY BALANCE INFORMATION
9/01                          127,574.68   9/20                 105,427.63      9/27                  66,230.67
  #3214                 09/19/2022       $17,230.00   #3214             09/19/2022           $17,230.00
  #3218                 09/27/2022       $17,035.65   #3218             09/27/2022           $17,035.65
"#;

    #[test]
    fn pinnacle_zero_credits_and_check_image_captions() {
        let l = parse(&[(1, PINNACLE)]);
        assert_eq!(l.summary.total_credits, Some(0.0));
        assert_eq!(l.summary.total_debits, Some(94340.67));
        assert_eq!(l.transactions.len(), 3, "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 39182.70).abs() < 0.001);
    }

    // GLM-OCR output of a Wells Fargo page: no alignment, amounts at the end of the line,
    // running balance only on the last transaction of a day.
    const WELLS_OCR: &str = r#"
Statement period activity summary
Beginning balance on 7/1 $2,014.94
Deposits/Additions 12,758.10
Withdrawals/Subtractions - 14,536.85
Ending balance on 7/31 $236.19
Transaction history
Date Check Number Description Deposits/Additions Withdrawals/Subtractions Ending daily balance
7/2 Zelle From Zermay Law A Professional Corpo on 07/02 Ref # Jpm99Becvg6T 2,000.00
7/2 Recurring Payment authorized on 06/30 Google *Youtube G.CO/Helppay# CA S305181667547988 Card 9798 106.44
7/2 Zelle to Carr Mike on 07/02 Ref #Rp0Yzclgvs 2,000.00 1,908.50
7/15 Lateral Link Gro Payroll 12728400007698x Jowers, Evan P 10,000.00
7/15 Recurring Payment authorized on 07/14 El Car Wash Lantan 305-603-9565 FL S585195372421625 Card 9798 42.78 11,865.72
"#;

    #[test]
    fn flat_ocr_page_uses_words_and_running_balance_arithmetic() {
        let l = parse(&[(1, WELLS_OCR)]);
        assert_eq!(l.summary.total_credits, Some(12758.10));
        assert_eq!(l.transactions.len(), 5, "{:?}", l.transactions);
        let kinds: Vec<Kind> = l.transactions.iter().map(|t| t.kind).collect();
        // "Payroll" carries no credit word; the balance change (+9,957.22) proves it is a credit.
        assert_eq!(kinds, vec![Kind::Credit, Kind::Debit, Kind::Debit, Kind::Credit, Kind::Debit]);
        assert_eq!(l.daily_balances.len(), 2);
        assert!((l.parsed_credit_total - 12000.0).abs() < 0.001);
    }

    #[test]
    fn ocr_amounts_with_period_thousands_and_dollar_zero() {
        assert_eq!(parse_amount("2.197.40"), Some(2197.40));
        assert!(is_amount_token("2.197.40"));
        assert!(is_amount_token("$.00"));
        assert_eq!(parse_amount("$.00"), Some(0.0));
        assert!(!is_amount_token("1.5.00"));
        assert!(!is_amount_token("10.5"));
    }

    #[test]
    fn bank_is_detected_by_most_mentions() {
        assert_eq!(detect_bank(&["Wells Fargo Bank, N.A.\nZelle to Chase user\nwellsfargo.com Wells Fargo"]), Some("Wells Fargo".into()));
        assert_eq!(detect_bank(&["Nothing here"]), None);
    }

    const PNC: &str = r#"
Account Summary Information
Balance Summary
                                  Beginning                   Deposits and                    Checks and                     Ending
                                    balance                   other credits                   other debits                  balance
                             125,033.13                        43,000.00                       2,555.00             165,478.13
Deposits and Other Credits                                                    Checks and Other Debits
Description                           Items                   Amount          Description                          Items                    Amount
Deposits                                  1               4,224.50            Checks                                  2                  2,105.00
Ledger Balance
Date             Ledger balance                    Date                Ledger balance                Date            Ledger balance
06/01            125,033.13                        06/11             201,416.46                      06/21          689,173.34
Deposits and Other Credits
ACH Credits                                         2 transactions for a total of $43,000.00
Date                                               Transaction                                                      Reference
posted                                    Amount   description                                                        number
06/03                                 28,273.92    Corporate ACH Txns/Fees                                  00024155901130577
                                                   Hrtland Pmt Sys 650000011702126
06/04                                 14,726.08    Corporate ACH Cardinalcp                                 00024155906523827
Checks and Other Debits
Checks and Substitute Checks                                 2 transactions for a total of $2,105.00
Date   Check                 Reference       Date   Check                             Reference
posted number         Amount   number        posted number                  Amount      number
06/14    12486         625.00   012830651    06/24   12489                 1,480.00     017680958
ACH Debits                                                   1 transactions for a total of $450.00
06/21    12490         450.00   017261553
"#;

    #[test]
    fn pnc_two_line_summary_reference_numbers_and_ledger_balance_table() {
        let l = parse(&[(1, PNC)]);
        assert_eq!(l.summary.beginning_balance, Some(125033.13));
        assert_eq!(l.summary.total_credits, Some(43000.0));
        // "Checks and other debits" already includes the checks figure: not added twice.
        assert_eq!(l.summary.total_debits, Some(2555.0));
        assert_eq!(l.summary.ending_balance, Some(165478.13));
        assert_eq!(l.daily_balances.len(), 3);
        assert!((l.parsed_credit_total - 43000.0).abs() < 0.001, "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 2555.0).abs() < 0.001, "{:?}", l.transactions);
        let check = l.transactions.iter().find(|t| t.amount == 1480.0).unwrap();
        assert_eq!(check.description, "Check 12489");
        assert!(!l.transactions.iter().any(|t| t.description.contains("00024155901130577")));
    }

    const TWO_COLUMN: &str = r#"
                 PREVIOUS BALANCE                  67,330.01                      AVERAGE BALANCE
                    +       22 CREDITS             10,585.79                              59,856.14
                              9 DEBITS                 53.00                     YTD INTEREST PAID
                    - SERVICE CHARGES                 247.03                                    .00
                   ENDING BALANCE                  77,615.77
           • Deposits and Other Credits
          Date     Amount Description                                          Date       Amount Description
         06/03        8,010.79    EDI PYMNTS EPIC Pharmacy Ne              06/17          875.00 DEPOSIT
                                  024155004613040CCD                       06/17        1,700.00 EDI PYMNTS EPIC Pharmacy Ne
           • Debits
                Description                                          Date             Amount      Description
06/04         8.00      2667586742 USPS9000077422                                                     024159006477916CCD
                            024156005333687CCD                       06/10              45.00     2671017582 USPS9000077422
 • Balance Bx Date
   Date         Balance                Date         Balance
   05/31       67,330.01               06/10       41,678.18
"#;

    #[test]
    fn two_column_transaction_lists_are_unfolded() {
        let l = parse(&[(1, TWO_COLUMN)]);
        assert_eq!(l.summary.total_credits, Some(10585.79));
        // debits + service charges
        assert_eq!(l.summary.total_debits, Some(300.03));
        assert_eq!(l.summary.average_balance, None); // value is on the next line, not guessed
        let credits: Vec<f64> = l.transactions.iter().filter(|t| t.kind == Kind::Credit).map(|t| t.amount).collect();
        assert_eq!(credits, vec![8010.79, 875.0, 1700.0], "{:?}", l.transactions);
        let debits: Vec<f64> = l.transactions.iter().filter(|t| t.kind == Kind::Debit).map(|t| t.amount).collect();
        assert_eq!(debits, vec![8.0, 45.0], "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 2);
    }

    #[test]
    fn month_name_dates_are_rewritten_in_place() {
        assert_eq!(normalize_month_dates("Jun 03    PREAUTHORIZED DEBIT"), "06/03     PREAUTHORIZED DEBIT");
        assert_eq!(normalize_month_dates("May 31, 2024 balance"), "05/31/2024   balance");
        assert_eq!(normalize_month_dates("June 3 2024"), "06/03/2024 ");
        assert_eq!(normalize_month_dates("Mayfield Ave 12"), "Mayfield Ave 12");
        assert_eq!(normalize_month_dates("Marching band 4"), "Marching band 4");
        assert_eq!(normalize_month_dates("Dec 5"), "12/05");
    }

    #[test]
    fn bundled_statements_are_segmented_and_combined() {
        let l = parse(&[(1, LEGENDS), (2, SUNRISE)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        assert_eq!(l.statements[0].beginning_balance, Some(57739.72));
        assert_eq!(l.statements[1].beginning_balance, Some(3702.38));
        assert_eq!(l.summary.beginning_balance, Some(57739.72));
        assert_eq!(l.summary.ending_balance, Some(9050.86));
        assert_eq!(l.summary.total_credits, Some(1321117.77 + 113045.99));
        // Legends' first-page lines and Sunrise's lines both survive, ids stay unique.
        let ids: std::collections::BTreeSet<usize> = l.transactions.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), l.transactions.len());
        assert!(l.transactions.iter().any(|t| t.page == 1) && l.transactions.iter().any(|t| t.page == 2));
    }
}
