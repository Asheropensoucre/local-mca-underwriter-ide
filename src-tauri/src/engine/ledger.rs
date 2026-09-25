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
    /// Set when the document is not a bank statement but parses like one: a bookkeeping
    /// "reconciliation report" (QuickBooks) lists cleared checks and deposits under the
    /// bank's name. The report must say so instead of scoring it.
    pub document_kind: Option<String>,
    /// First and last page of this statement within the file (bundles of statements).
    pub pages: Option<(usize, usize)>,
    /// The balance equation, beginning + parsed credits - parsed debits = ending, checked
    /// against the rows as parsed: Some(true) when it closes to the cent, Some(false) when
    /// both balances are printed and it does not, None without both balances. Two printed
    /// figures agreeing with the rows outrank one misread printed total.
    pub balance_check: Option<bool>,
    /// Banks that split debits into "Checks" and "Other withdrawals" (Truist) print two
    /// figures; this holds the checks part until both are known.
    #[serde(skip)]
    checks_total: Option<f64>,
    /// Bank of America prints "Service fees -16.00" as a third debit figure.
    #[serde(skip)]
    fees_total: Option<f64>,
    /// "Interest Earned This Period 53.12+" beside "0 Other Credits 0.00" (First Citizens):
    /// a credit the printed credit total leaves out, added when the balance equation needs it.
    #[serde(skip)]
    interest_total: Option<f64>,
    /// The key that gave `total_debits`; decides whether checks and fees are already in it.
    #[serde(skip)]
    debits_key: &'static str,
    /// Page the debit total was read from. Separate checks and fee figures count only from
    /// the same page, so a fee line on a later bundled page is not added.
    #[serde(skip)]
    debits_page: Option<usize>,
    /// Inside the account summary block ("CHECKING SUMMARY" ... "Ending Balance").
    #[serde(skip)]
    in_summary_block: bool,
    #[serde(skip)]
    summary_lines: usize,
    /// Negative figures listed in the summary block: Chase prints one line per debit
    /// category (card withdrawals, electronic withdrawals, checks, fees); their sum is the
    /// debit total when two or more are present.
    #[serde(skip)]
    debit_parts: Vec<f64>,
    /// Unsigned category lines in the summary block, told apart by their words (TD
    /// business: "Deposits", "Electronic Deposits" / "Checks Paid", "Electronic Payments").
    #[serde(skip)]
    credit_parts: Vec<f64>,
    #[serde(skip)]
    debit_parts_unsigned: Vec<f64>,
    /// Distinct "beginning balance" figures seen. More than one means the file bundles
    /// several statements or accounts, which the parser does not separate yet.
    pub beginning_balances_seen: Vec<f64>,
    /// Sums of this statement's own parsed lines (filled for the per-statement entries of
    /// a bundle, so each statement can be verified on its own).
    pub parsed_credits: Option<f64>,
    pub parsed_debits: Option<f64>,
    /// The copy skips pages of this statement: its own "Page N of M" footers jump over
    /// numbers the file does not hold (a one-sided scan of a two-sided statement). Its
    /// totals cannot be met from the lines that survive.
    #[serde(default)]
    pub missing_pages: bool,
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
    /// Reversal pairs the bank left out of its printed totals (Achieva: a fee and its
    /// "-- Reversed" credit, a card purchase and its return). Listed, but not summed.
    #[serde(default)]
    pub netted: Vec<usize>,
    /// A second reading of a row's amount from another listing of the same item (the check
    /// table says 984.44, the image caption 984.41): (row, amount). The printed totals and
    /// daily balances choose (`use_alternates`, `meet_daily_balances`).
    #[serde(skip)]
    pub alternates: Vec<(usize, f64)>,
    /// A section's printed total ("Total checks = $3,130.00") per table id: the rows of that
    /// table must add up to it (`meet_section_totals`).
    #[serde(skip)]
    pub section_totals: Vec<(usize, f64)>,
    /// Rows whose kind came from a default rather than a word, a sign, a column or the
    /// balance arithmetic. The printed totals get the last word on them (`meet_totals`).
    #[serde(skip)]
    pub weak: Vec<usize>,
    /// The kind a section's total line names ("Total Deposits and Additions") per table
    /// id, for the rows under a section title the scan lost (`settle_weak_by_section_kinds`).
    #[serde(skip)]
    pub section_kinds: Vec<(usize, Kind)>,
}

// ─── Line parsing ─────────────────────────────────────────────────────────

/// Parse "1,234.56", "$1,234.56", "1,234.56-", "-1,234.56", "(1,234.56)", "$.00" and the
/// OCR form "2.197.40" where a comma was read as a period.
pub fn parse_amount(raw: &str) -> Option<f64> {
    let t = raw.trim();
    let neg = t.ends_with('-') || t.starts_with('-') || (t.starts_with('(') && t.ends_with(')'));
    // ("15,00-": the sign is not part of the figure.)
    let t = t.trim_matches(|c| c == '-' || c == '+' || c == '(' || c == ')');
    // "500,00": the last comma stands for the decimal point when there is no dot.
    let t: String = if !t.contains('.') && t.rsplit_once(',').map(|(_, c)| c.len() == 2).unwrap_or(false) {
        let (a, b) = t.rsplit_once(',').unwrap();
        format!("{a}.{b}")
    } else {
        t.to_string()
    };
    let t = t.as_str();
    let digits: String = t.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    let dot = digits.rfind('.')?;
    let joined: String = digits[..dot].chars().filter(|c| *c != '.').chain(digits[dot..].chars()).collect();
    let v: f64 = format!("0{joined}").parse().ok()?;
    Some(if neg { -v } else { v })
}

pub fn is_amount_token(tok: &str) -> bool {
    // Trailing '+' or '-' are credit/debit markers some community banks print ("12,821.16+");
    // KeyBank leads with them ("+7,170.00"). Parentheses may sit outside or inside the
    // sign: "($15.00)" (Capital One), "$(205,309.04)" (Fifth Third).
    let t = tok.trim_matches(|c| c == '(' || c == ')').trim_start_matches(|c| c == '$' || c == '-' || c == '+').trim_end_matches(|c| c == '-' || c == '+').trim_matches(|c| c == '(' || c == ')');
    if t.is_empty() {
        return false;
    }
    let first = t.chars().next().unwrap();
    // "$.00" is how some banks print zero and PNC prints cents alone (".15"); otherwise an
    // amount starts with a digit.
    if !(first.is_ascii_digit() || (first == '.' && (tok.starts_with('$') || t.len() == 3))) {
        return false;
    }
    // Groups between separators: "1,234.56" -> [1, 234, 56]; "2.197.40" -> [2, 197, 40].
    let groups: Vec<&str> = t.split(|c| c == ',' || c == '.').collect();
    if groups.iter().any(|g| !g.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    // A comma before the last two digits with no dot anywhere ("500,00", a wide OCR layer)
    // is a decimal point misread; `parse_amount` reads it the same way.
    let comma_decimal = !t.contains('.') && t.matches(',').count() >= 1 && groups.last().map(|g| g.len() == 2).unwrap_or(false) && groups[0].len() <= 3;
    let decimals_ok = groups.last().map(|g| g.len() == 2).unwrap_or(false) && (t.contains('.') || comma_decimal);
    // Every inner group is a thousands group of exactly three digits.
    let inner_ok = groups.len() < 3 || groups[1..groups.len() - 1].iter().all(|g| g.len() == 3);
    // Period-separated thousands only when no comma is present (otherwise "1.5.00" is noise).
    let periods = t.matches('.').count();
    let period_thousands_ok = periods == 1 || comma_decimal || (!t.contains(',') && groups[0].len() <= 3 && inner_ok);
    decimals_ok && inner_ok && period_thousands_ok
}

const MONTHS: &[&str] = &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// pdftotext sometimes breaks an amount at the decimal point ("20. 00", "1,860. 70").
/// Rejoin it and move the space after the cents so column offsets are kept.
fn join_split_amounts(line: &str) -> String {
    if !line.is_ascii() {
        return line.to_string();
    }
    // The point lost to a space at the very end of a dated row that has no other figure
    // ("02/21 CCD DEPOSIT, GRUBHUB INC FEB ACTVTY ****2119dKHGk50 424 67", TD in a scan):
    // a row lists an amount, so its last two tokens are its dollars and cents.
    let line = &{
        let toks: Vec<&str> = line.split_whitespace().collect();
        let n = toks.len();
        let digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
        if n >= 4 && parse_date_token(toks[0]).is_some() && digits(toks[n - 1]) && toks[n - 1].len() == 2 && digits(toks[n - 2].replace(',', "").as_str()) && toks[n - 2].len() <= 7
            && !toks[1..n - 2].iter().any(|t| is_amount_token(t)) && toks[1..n - 2].iter().any(|t| t.chars().any(|c| c.is_ascii_alphabetic()))
        {
            let cut = line.trim_end().rfind(toks[n - 1]).unwrap();
            format!("{}.{}", line[..cut].trim_end(), toks[n - 1])
        } else {
            line.to_string()
        }
    };
    // A decimal point lost to a space after a thousands group ("Deposits 8,498 69"): the
    // group of three after a comma, a space, two digits and no more digits is ".dd".
    let line = &{
        let b = line.as_bytes();
        let digit = |k: usize| b.get(k).map(|c| c.is_ascii_digit()).unwrap_or(false);
        let mut out = String::with_capacity(line.len());
        for i in 0..b.len() {
            let lost_point = b[i] == b' ' && i >= 4 && b[i - 4] == b',' && digit(i - 3) && digit(i - 2) && digit(i - 1) && digit(i + 1) && digit(i + 2) && !digit(i + 3) && b.get(i + 3).map(|c| *c == b' ' || *c == b'-').unwrap_or(true);
            out.push(if lost_point { '.' } else { b[i] as char });
        }
        out
    };
    // A decimal point read as two commas ("74,,84-"), a run of points ("1, 175...00") or a
    // mix of punctuation ("2,053:..01"): a digit, the run, two digits, then no digit.
    let punct = |c: u8| c == b',' || c == b'.' || c == b':' || c == b';';
    let line = &if line.as_bytes().windows(2).any(|w| punct(w[0]) && punct(w[1])) {
        let b = line.as_bytes();
        let digit = |k: usize| b.get(k).map(|c| c.is_ascii_digit()).unwrap_or(false);
        let mut out = String::with_capacity(line.len());
        let mut i = 0;
        while i < b.len() {
            if punct(b[i]) && i >= 1 && digit(i - 1) {
                let run = b[i..].iter().take_while(|c| punct(**c)).count();
                if run >= 2 && digit(i + run) && digit(i + run + 1) && !digit(i + run + 2) {
                    out.push('.');
                    i += run;
                    continue;
                }
            }
            out.push(b[i] as char);
            i += 1;
        }
        out
    } else {
        line.to_string()
    };
    let b = line.as_bytes();
    let aligned = line.contains("   ");
    let numberish = |c: u8| c.is_ascii_digit() || c == b'.' || c == b',' || c == b'/';
    // A space inside a number: "20. 00" (digit '.' ' ' digit digit, then no digit), or
    // "-1 ,100.00" and "11 /21 /22" (digit ' ' [,/] digit).
    let split_at = |i: usize| -> bool {
        let digit = |k: usize| b.get(k).map(|c| c.is_ascii_digit()).unwrap_or(false);
        if b.get(i) != Some(&b' ') || i == 0 {
            return false;
        }
        let decimal = i >= 2 && b[i - 1] == b'.' && b[i - 2].is_ascii_digit() && digit(i + 1) && digit(i + 2) && !digit(i + 3);
        // "3,051 .38": the space before the decimal point. (Not after a reference number:
        // "POS DEB 0922 07/31/21 67109900 .21-" lost the dollars of a small amount; six
        // digits in a row is the most a figure has without commas.)
        let run_before = (0..i).rev().take_while(|&k| b[k].is_ascii_digit()).count();
        // (Not after a date: "03/28 .05 Interest Payment" is a date and a cents-only amount.)
        let after_date = i > run_before && matches!(b[i - run_before - 1], b'/' | b'-');
        // (Not after a figure that already has its cents: "80,689.22 .00" is two figures.)
        let after_cents = i >= 3 && b[i - 3] == b'.' && b[i - 2].is_ascii_digit() && b[i - 1].is_ascii_digit();
        let before_point = b[i - 1].is_ascii_digit() && run_before <= 6 && !after_date && !after_cents && b.get(i + 1) == Some(&b'.') && digit(i + 2) && digit(i + 3) && !digit(i + 4);
        // "15 ..00-": the same with the point read twice (the second point goes below).
        let before_points = b[i - 1].is_ascii_digit() && b.get(i + 1) == Some(&b'.') && b.get(i + 2) == Some(&b'.') && digit(i + 3) && digit(i + 4) && !digit(i + 5);
        // ("27 , 893.44": the thousands comma spaced on both sides; the second space is
        // `after_comma`.)
        // (Only a thousands group of one to three digits joins: "Card 5059 , 136.47" is a
        // card number and an amount.)
        let group = b[i - 1].is_ascii_digit() && matches!(b.get(i + 1), Some(b',') | Some(b'/')) && (b.get(i + 1) == Some(&b'/') || run_before <= 3) && (digit(i + 2) || b.get(i + 1) == Some(&b',') && b.get(i + 2) == Some(&b' ') && digit(i + 3) && digit(i + 4) && digit(i + 5) && !digit(i + 6));
        // "1, 000.00": the space after the thousands comma, three digits following
        // (or "27 , 893.44", the comma spaced on both sides).
        let after_comma = i >= 2 && b[i - 1] == b',' && (b[i - 2].is_ascii_digit() || i >= 3 && b[i - 2] == b' ' && b[i - 3].is_ascii_digit()) && digit(i + 1) && digit(i + 2) && digit(i + 3) && !digit(i + 4);
        // "09/ 18": the space after a date's slash, at the start of the line's first token,
        // two digits on each side (Wintrust court copies).
        let after_slash = i >= 3 && b[i - 1] == b'/' && b[i - 2].is_ascii_digit() && b[i - 3].is_ascii_digit() && digit(i + 1) && digit(i + 2) && !digit(i + 3)
            && b[..i - 3].iter().all(|c| *c == b' ');
        // "1 0/24/17": a space inside the month of the line's first token.
        let in_month = i >= 1 && b[i - 1].is_ascii_digit() && digit(i + 1) && b.get(i + 2) == Some(&b'/') && digit(i + 3) && b[..i - 1].iter().all(|c| *c == b' ');
        // "$27 373.34": the thousands comma read as a space, the figure led by a dollar sign
        // (one to three digits, the space, three digits and the cents).
        let dollar_gap = b[i - 1].is_ascii_digit() && digit(i + 1) && digit(i + 2) && digit(i + 3) && b.get(i + 4) == Some(&b'.') && digit(i + 5) && digit(i + 6) && !digit(i + 7) && {
            let start = (0..i).rev().take_while(|&k| b[k].is_ascii_digit()).count();
            (1..=3).contains(&start) && i >= start + 1 && b[i - start - 1] == b'$'
        };
        // ("Check   8 368.00": the same figure after a word, one space inside it where the
        // columns are set apart by two or more; not after a month word, "Jul 12 345.00".
        // Only on an aligned line: flat OCR text spaces a count and an amount the same way,
        // "Deposits and Additions 1 500.00".)
        let word_gap = aligned && b[i - 1].is_ascii_digit() && digit(i + 1) && digit(i + 2) && digit(i + 3) && b.get(i + 4) == Some(&b'.') && digit(i + 5) && digit(i + 6) && !digit(i + 7) && {
            let start = (0..i).rev().take_while(|&k| b[k].is_ascii_digit()).count();
            (1..=3).contains(&start) && i >= start + 2 && b[i - start - 1] == b' ' && {
                let word: String = line[..i - start - 1].split_whitespace().last().unwrap_or("").to_ascii_lowercase();
                word.chars().all(|c| c.is_ascii_alphabetic()) && word.len() >= 3 && !MONTHS.iter().any(|m| word.starts_with(m))
            }
        };
        decimal || before_point || before_points || group || after_comma || after_slash || in_month || dollar_gap || word_gap
    };
    // A doubled decimal point after a removed space ("15 ..00-") is one point.
    let doubled_point = |i: usize| -> bool {
        let digit = |k: usize| b.get(k).map(|c| c.is_ascii_digit()).unwrap_or(false);
        i >= 2 && b[i] == b'.' && b[i - 1] == b'.' && b[i - 2] == b' ' && digit(i + 1) && digit(i + 2) && !digit(i + 3)
    };
    let mut out = String::with_capacity(line.len());
    let mut owed = 0; // spaces removed from inside a number, re-added after it (keeps the width)
    let mut i = 0;
    while i < b.len() {
        if split_at(i) || doubled_point(i) {
            owed += 1;
            i += 1;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
        let number_goes_on = b.get(i).map(|c| numberish(*c)).unwrap_or(false) || split_at(i);
        if owed > 0 && !number_goes_on {
            out.push_str(&" ".repeat(owed));
            owed = 0;
        }
    }
    out
}

/// A text layer that lifts a row's amount onto the line above it ("N      6.98" over
/// "(0   Jun 03   QT 168   KANSAS CIT MO", UMB, with the scan's margin marks in front):
/// a line holding one amount and nothing else but a short mark, right above a dated row
/// with no amount, gives the row its amount at the same column.
fn lower_lifted_amounts(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    // (A bullet is a list item, "• $14,432.66" under "This fee period", not a lifted cell.)
    let mark = |t: &str| t.len() <= 3 && !is_amount_token(t) && parse_date_token(t).is_none() && t != "\u{2022}" && t != "*" && t != "-";
    fn lone_amount<'a>(l: &'a str, mark: &dyn Fn(&str) -> bool) -> Option<(usize, &'a str)> {
        let toks: Vec<&str> = l.split_whitespace().collect();
        let amount = match toks.as_slice() {
            [a] if is_amount_token(a) => *a,
            [m, a] if mark(m) && is_amount_token(a) => *a,
            _ => return None,
        };
        l.rfind(amount).map(|at| (at + amount.len(), amount))
    }
    let dated_without_amount = |l: &str| -> bool {
        let toks: Vec<&str> = l.split_whitespace().collect();
        let date_at = if toks.first().map(|t| parse_date_token(t).is_some()).unwrap_or(false) { 0 } else if toks.len() > 1 && mark(toks[0]) && parse_date_token(toks[1]).is_some() { 1 } else { return false };
        // (Never a page footer: "January 31, 2022 @ Page 2 of 4".)
        toks.len() >= date_at + 2 && !toks.iter().any(|t| is_amount_token(t)) && l.contains("   ") && footer_numbers(l).is_none()
    };
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if let Some((end, amount)) = lone_amount(lines[i], &mark) {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if j < lines.len() && dated_without_amount(lines[j]) {
                let row = lines[j].trim_end();
                let pad = end.saturating_sub(row.len() + amount.len()).max(3);
                out.push(format!("{row}{}{amount}", " ".repeat(pad)));
                i = j + 1;
                continue;
            }
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    out.join("\n")
}

/// A column header whose last cell is stacked over the lines around it: DecisionLogic
/// prints "EOD" above its header row and "Balance" below it, so the header itself reads
/// "Date  Codes  Description  Category  Amount" and the balance column goes unnamed, which
/// leaves every row's running balance read as its amount.
///
/// A lone header word standing to the right of the header's last cell is written into the
/// header at its own column and blanked where it stood.
fn fold_stacked_header_words(text: &str) -> String {
    const NAMES: &[&str] = &["balance", "amount", "date", "description", "debit", "debits", "credit", "credits", "withdrawal", "withdrawals", "deposit", "deposits", "paid", "posted", "type", "category", "reference", "serial", "check", "checks", "number", "code", "codes"];
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for i in 0..lines.len() {
        let toks: Vec<&str> = lines[i].split_whitespace().collect();
        let low = lines[i].to_ascii_lowercase();
        if toks.len() < 3 || toks.iter().any(|t| is_amount_token(t)) || !low.contains("date") || !(low.contains("amount") || low.contains("description")) {
            continue;
        }
        let last = toks[toks.len() - 1];
        let right_of = lines[i].rfind(last).unwrap() + last.len();
        // The line below first: where two stacked words share a column ("EOD" over
        // "Balance"), the one that names the column is the one that must land.
        let above = (0..i).rev().find(|&j| !lines[j].trim().is_empty());
        let below = (i + 1..lines.len()).find(|&j| !lines[j].trim().is_empty());
        for j in below.into_iter().chain(above) {
            let words: Vec<&str> = lines[j].split_whitespace().collect();
            if words.is_empty() || words.len() > 2 || !words.iter().all(|w| NAMES.contains(&w.to_ascii_lowercase().as_str())) {
                continue;
            }
            let mut target: Vec<char> = out[i].chars().collect();
            let mut placed = false;
            for w in &words {
                let at = lines[j].find(w).unwrap();
                if at < right_of + 1 {
                    continue;
                }
                if target.len() < at + w.len() {
                    target.resize(at + w.len(), ' ');
                }
                if target[at..at + w.len()].iter().all(|c| *c == ' ') && target.get(at + w.len()).map(|c| *c == ' ').unwrap_or(true) {
                    for (k, c) in w.chars().enumerate() {
                        target[at + k] = c;
                    }
                    placed = true;
                }
            }
            if placed {
                out[i] = target.into_iter().collect::<String>().trim_end().to_string();
                out[j] = String::new();
            }
        }
    }
    out.join("\n")
}

/// An online banking printout ("Printed from Chase for Business") prints the date once
/// for each day and leaves the cell blank on the rows below it, wrapping every description
/// over several lines. Under a "Date  Description  Type  Amount  Balance" header, a line
/// whose date cell is blank but whose last two cells are figures is the next row of that
/// day, so the date above it is written into its own cell.
///
/// Wrapped description lines carry no figures and are left alone, and a "Pending" cell
/// where the date belongs clears the carried date: nothing under it is posted yet.
fn carry_column_dates(text: &str, carried: &mut Option<String>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let header = lines.iter().position(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        let low = l.to_ascii_lowercase();
        (2..=8).contains(&t.len()) && low.contains("date") && low.contains("amount") && low.contains("balance") && !t.iter().any(|x| is_amount_token(x))
    });
    let Some(h) = header else { return text.to_string() };
    let Some(date_at) = lines[h].to_ascii_lowercase().find("date") else { return text.to_string() };
    let desc_at = lines[h].split_whitespace().nth(1).and_then(|w| lines[h].find(w)).unwrap_or(date_at + 4);
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for i in h + 1..lines.len() {
        let toks: Vec<&str> = lines[i].split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        let first_at = lines[i].find(toks[0]).unwrap();
        if first_at < desc_at {
            // The row's own first cell. A date is the day to carry. Anything else that
            // opens a row with figures ("Pending") ends the day above; a repeated header
            // or a page footer carries no figures and leaves the day standing.
            if parse_date_token(toks[0]).is_some() {
                *carried = Some(toks[0].to_string());
            } else if toks.iter().any(|t| is_amount_token(t)) {
                *carried = None;
            }
            continue;
        }
        let Some(date) = carried.clone() else { continue };
        if toks.len() < 3 || !is_amount_token(toks[toks.len() - 1]) || !is_amount_token(toks[toks.len() - 2]) {
            continue;
        }
        let mut target: Vec<char> = out[i].chars().collect();
        let fits = |w: &str| target.len() >= date_at + w.len() + 1 && target[..date_at + w.len() + 1].iter().all(|c| *c == ' ');
        // A narrow printout rules its date column to "04/20" and prints the year in the
        // page header; the short form is written when the whole date would not fit.
        let short = parse_date_token(&date).map(|(m, d, _)| format!("{m:02}/{d:02}"));
        let Some(write) = Some(date.clone()).filter(|w| fits(w)).or_else(|| short.filter(|w| fits(w))) else { continue };
        for (k, c) in write.chars().enumerate() {
            target[date_at + k] = c;
        }
        out[i] = target.into_iter().collect();
    }
    out.join("\n")
}

/// A print stream that marks its sections ("*start*deposits and additions",
/// "*end*daily ending balance") can break the last row of a page across the marker: the
/// date prints, then the marker, then the row's body.
///
/// ```text
///     03/09
/// *end*deposits and additions
///                     Online Transfer From Chk ...6372          6,000.00
/// ```
/// The date is written back into the body line at its own column, so the row reads whole.
/// Only a marker line may stand between the two; anything else means these are two rows.
fn rejoin_marked_rows(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let marker = |l: &str| { let t = l.trim(); t.starts_with('*') && t.len() > 1 && t[1..].contains('*') };
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for i in 0..lines.len() {
        let toks: Vec<&str> = lines[i].split_whitespace().collect();
        if toks.len() != 1 || parse_date_token(toks[0]).is_none() {
            continue;
        }
        let mut j = i + 1;
        let mut marked = false;
        while j < lines.len() && (lines[j].trim().is_empty() || marker(lines[j])) {
            marked |= marker(lines[j]);
            j += 1;
        }
        if !marked || j >= lines.len() {
            continue;
        }
        let body: Vec<&str> = lines[j].split_whitespace().collect();
        if body.is_empty() || !is_amount_token(body[body.len() - 1]) {
            continue;
        }
        // The body must start to the right of the date column, so nothing of the row is
        // overwritten and the date the body quotes ("02/23  02/21 Online Transfer To ...")
        // stays where the bank printed it, in the description.
        let at = lines[i].find(toks[0]).unwrap();
        let mut target: Vec<char> = out[j].chars().collect();
        if target.len() < at + toks[0].len() + 1 || !target[..at + toks[0].len() + 1].iter().all(|c| *c == ' ') {
            continue;
        }
        for (k, c) in toks[0].chars().enumerate() {
            target[at + k] = c;
        }
        out[j] = target.into_iter().collect();
        out[i] = String::new();
    }
    out.join("\n")
}

/// A poor text layer sometimes stacks the cells of neighbouring rows: two lines holding
/// only "06/04" and "06/05", then two lines holding only "2,566.85" and "1,171.23". Equal
/// runs of lone dates and lone amounts are zipped back into "date amount" rows.
fn zip_stacked_cells(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let lone = |l: &str, f: &dyn Fn(&str) -> bool| -> bool {
        let toks: Vec<&str> = l.split_whitespace().collect();
        toks.len() == 1 && f(toks[0])
    };
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        // (A stray margin mark of one or two characters before the date does not count.)
        let lone_date = |l: &str| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            match toks.as_slice() {
                [d] => parse_date_token(d).is_some(),
                [junk, d] => junk.len() <= 2 && parse_date_token(junk).is_none() && parse_date_token(d).is_some(),
                _ => false,
            }
        };
        let date_of = |l: &str| l.split_whitespace().last().unwrap_or("").to_string();
        let dates = lines[i..].iter().take_while(|l| lone_date(l)).count();
        if dates >= 1 {
            // A lone "Balance" (or "Amount") label may sit between the dates and their amounts
            // (UMB's "End of Day" table read column by column).
            let label_between = lines.get(i + dates).map(|l| { let t = l.trim().to_ascii_lowercase(); t == "balance" || t == "amount" }).unwrap_or(false) as usize;
            let amounts = lines[i + dates + label_between..].iter().take_while(|l| lone(l, &|t| is_amount_token(t))).count();
            if amounts == dates {
                for k in 0..dates {
                    out.push(format!("{:>16}{:>12}", date_of(lines[i + k]), lines[i + dates + label_between + k].trim()));
                }
                i += dates * 2 + label_between;
                continue;
            }
            // Lone dates over as many description lines over as many lone amounts (a court
            // scan's text layer pulling the dates of a few rows out of line): one row each.
            let is_desc = |l: &str| {
                let toks: Vec<&str> = l.split_whitespace().collect();
                toks.len() >= 2 && parse_date_token(toks[0]).is_none() && !toks.iter().any(|t| is_amount_token(t))
            };
            // The amount line may carry the running balance too ("$2,500.00      $838.16"
            // under Debits / Credits / Balance): its cells stay at their columns when the
            // date and description fit in front of them, so the column still names the kind.
            let amount_cells = |l: &str| {
                let toks: Vec<&str> = l.split_whitespace().collect();
                (1..=3).contains(&toks.len()) && toks.iter().all(|t| is_amount_token(t))
            };
            let descs = lines[i + dates..].iter().take_while(|l| is_desc(l)).count();
            if descs == dates {
                let amounts = lines[i + dates + descs..].iter().take_while(|l| amount_cells(l)).count();
                if amounts == dates {
                    for k in 0..dates {
                        let amt = lines[i + dates + descs + k];
                        let head = format!("{:>16} {}", date_of(lines[i + k]), lines[i + dates + k].trim());
                        let at = amt.len() - amt.trim_start().len();
                        if head.len() + 1 < at {
                            out.push(format!("{head}{}{}", " ".repeat(at - head.len()), amt.trim_start().trim_end()));
                        } else {
                            out.push(format!("{head} {}", amt.trim()));
                        }
                    }
                    i += dates * 3;
                    continue;
                }
            }
        }
        // The same for a summary block read column by column: the labels ("Beginning
        // Balance", "Deposits and Additions", ...) then their amounts, one per line.
        let label = |l: &str| {
            let n = l.split_whitespace().count();
            (1..=5).contains(&n) && !l.chars().any(|c| c.is_ascii_digit()) && !l.trim_end().ends_with(':')
        };
        let labels = lines[i..].iter().take_while(|l| label(l)).count();
        if labels >= 2 {
            let amounts = lines[i + labels..].iter().take_while(|l| lone(l, &|t| is_amount_token(t))).count();
            if amounts == labels {
                for k in 0..labels {
                    out.push(format!("{}{:>14}", lines[i + k].trim_end(), lines[i + labels + k].trim()));
                }
                i += labels * 2;
                continue;
            }
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    out.join("\n")
}

/// Mercury (a fintech account) prints one date per day, the rows of that day undated
/// below it, debits with a leading minus, credits unsigned, and the end-of-day balance on
/// the day's last row. Its header comes out letter-spaced ("Dat e  De s cript ion  T rx T ype
/// A mou n t  En d of Day Balan ce"). Each row is rewritten with the day's date and its
/// sign as a word ("... Debit" / "... Credit"), the balance dropped, so the plain rules read it.
/// `carried` is the last date of the page before (a day's rows run over the page break);
/// the date this page ends on comes back with the text.
fn unfold_mercury_rows(text: &str, carried: Option<&str>) -> (String, Option<String>) {
    let squashed: String = text.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if !(squashed.contains("trxtype") && squashed.contains("endofdaybalance")) {
        return (text.to_string(), None);
    }
    let mut out: Vec<String> = Vec::with_capacity(text.lines().count());
    let mut date: Option<String> = carried.map(str::to_string);
    let mut in_table = false;
    for line in text.lines() {
        let sq: String = line.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        if sq.contains("trxtype") && sq.contains("endofdaybalance") {
            in_table = true;
            out.push(line.to_string());
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if !in_table || toks.is_empty() {
            out.push(line.to_string());
            continue;
        }
        let dated = toks.first().and_then(|t| parse_date_token(t)).is_some();
        let amounts: Vec<usize> = toks.iter().enumerate().filter(|(_, t)| is_amount_token(t)).map(|(i, _)| i).collect();
        if dated {
            date = Some(toks[0].to_string());
        }
        let Some(&a) = amounts.first() else {
            // A line of prose or a footer ends the table; a wrapped payee name does not.
            if toks.len() > 6 || sq.contains("bankingservices") {
                in_table = false;
            }
            out.push(line.to_string());
            continue;
        };
        let (Some(d), true) = (date.as_ref(), a >= 1) else { out.push(line.to_string()); continue };
        let desc_from = if dated { 1 } else { 0 };
        let desc: String = toks[desc_from..a].join(" ");
        let dl = desc.to_ascii_lowercase();
        if dl.contains("total") || dl.contains("balance") {
            out.push(line.to_string());
            continue;
        }
        let kind = if toks[a].starts_with('-') || toks[a].starts_with("-$") { "Debit" } else { "Credit" };
        out.push(format!("{d} {desc} {kind} {}", toks[a].trim_start_matches('-')));
    }
    (out.join("\n"), date)
}

/// The OCR model sometimes retells a one-row table as a field list:
/// "- **Date:** 11/04" / "- **Description:** Electronic transfer" / "- **Credits:** $25,000.00"
/// / "- **Debits:** $0.00". Each such block becomes a column header and one aligned row, so
/// the column rules read it like the table it was.
fn fold_field_lists(text: &str) -> String {
    // ("Date 10/03" / "Description Bank Service Fee" / "Credits $513.01" / "Debits" without
    // colons is the same list; there the first word must be one of the known keys.)
    const KEYS: &[&str] = &["date", "description", "credits", "debits", "credit", "debit", "amount", "transaction"];
    // ("POSTING DATE" and "SERIAL NO." are TD's names for the same cells.)
    let canon = |k: &str| -> String {
        match k {
            "posting date" | "date posted" | "post date" => "date".into(),
            "serial no." | "serial no" | "serial number" | "check number" | "check #" => "serial".into(),
            "transaction detail" | "transaction description" => "description".into(),
            other => other.to_string(),
        }
    };
    let field = |l: &str| -> Option<(String, String)> {
        let t = l.trim().trim_start_matches("- ").trim_start_matches('•').trim().replace("**", "");
        if let Some((k, v)) = t.split_once(':') {
            let k = canon(&k.trim().to_ascii_lowercase());
            if k.split_whitespace().count() > 3 || k.is_empty() {
                return None;
            }
            return Some((k, v.trim().to_string()));
        }
        let (k, v) = t.split_once(' ').unwrap_or((t.as_str(), ""));
        let k = k.to_ascii_lowercase();
        if KEYS.contains(&k.as_str()) && t.split_whitespace().count() <= 12 {
            return Some((k, v.trim().to_string()));
        }
        None
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let starts = field(lines[i]).map(|(k, v)| k == "date" && parse_date_token(&v).is_some()).unwrap_or(false);
        if starts {
            let mut fields: Vec<(String, String)> = Vec::new();
            let mut j = i;
            while j < lines.len() {
                match field(lines[j]) {
                    Some(f) if !(f.0 == "date" && j > i) => { fields.push(f); j += 1; }
                    _ => break,
                }
            }
            let get = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
            let amount_of = |v: Option<String>| v.and_then(|v| v.split_whitespace().find(|t| is_amount_token(t)).and_then(parse_amount)).filter(|a| a.abs() >= 0.005);
            let (credit, debit) = (amount_of(get("credits").or_else(|| get("credit"))), amount_of(get("debits").or_else(|| get("debit"))));
            let amount = amount_of(get("amount"));
            if let Some(date) = get("date") {
                if credit.is_some() || debit.is_some() || amount.is_some() {
                    let mut desc = get("description").or_else(|| get("transaction")).unwrap_or_default();
                    if let Some(serial) = get("serial") {
                        // A check row ("SERIAL NO.: 1002"): the number is the description.
                        desc = if desc.is_empty() { serial } else { format!("{serial} {desc}") };
                    }
                    let cell = |v: Option<f64>| v.map(|a| format!("{:.2}", a)).unwrap_or_default();
                    if let Some(a) = amount {
                        // One "Amount" cell: a plain row; the section it sits in decides the kind.
                        out.push(format!("{date}  {}  {:.2}", desc.chars().take(60).collect::<String>().trim(), a));
                    } else {
                        // (A credit cell is written with a leading "+" and a debit row's
                        // description ends in "Debit", so a flat page, where words and signs
                        // decide, reads it the same way as the column rules do.)
                        out.push(format!("{:<12}{:<44}{:>20}{:>20}", "Date", "Description", "Credits", "Debits"));
                        let (c, d) = (credit.map(|a| format!("+{:.2}", a)).unwrap_or_default(), cell(debit));
                        let desc = format!("{} {}", desc.chars().take(36).collect::<String>().trim(), if c.is_empty() { "Debit" } else { "Credit" });
                        out.push(format!("{:<12}{:<44}{:>20}{:>20}", date, desc, c, d));
                    }
                    i = j;
                    continue;
                }
            }
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    out.join("\n")
}

/// A wide OCR layer lifts a check's serial ("361*", "372*") onto its own line above the
/// row it belongs to ("04/01   2,000.00"). One or two such lone serials are written into
/// the next dated line at the same offsets when that space is blank, so the row reads
/// "04/01   361*   2,000.00" again.
fn merge_stacked_serials(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    // (A lone "0" is a stray mark of the text layer, not a serial.)
    let serial = |t: &str| { let n = t.trim_end_matches('*'); n.len() >= 2 && n.len() <= 7 && n.chars().all(|c| c.is_ascii_digit()) && n.chars().any(|c| c != '0') && !t.contains('.') };
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for i in 0..lines.len() {
        let toks: Vec<&str> = lines[i].split_whitespace().collect();
        if toks.is_empty() || toks.len() > 2 || !toks.iter().all(|t| serial(t)) {
            continue;
        }
        let Some(j) = (i + 1..lines.len()).find(|&j| !lines[j].trim().is_empty()) else { continue };
        let next: Vec<&str> = lines[j].split_whitespace().collect();
        if !next.iter().any(|t| parse_date_token(t).is_some()) || !next.iter().any(|t| is_amount_token(t)) {
            continue;
        }
        let mut target: Vec<char> = out[j].chars().collect();
        let mut placed = false;
        for tok in &toks {
            let at = lines[i].find(tok).unwrap();
            if target.len() < at + tok.len() {
                target.resize(at + tok.len(), ' ');
            }
            if target[at..at + tok.len()].iter().all(|c| *c == ' ') && (at == 0 || target[at - 1] == ' ') && target.get(at + tok.len()).map(|c| *c == ' ').unwrap_or(true) {
                for (k, c) in tok.chars().enumerate() {
                    target[at + k] = c;
                }
                placed = true;
            }
        }
        if placed {
            out[j] = target.into_iter().collect();
            out[i] = String::new();
        }
    }
    out.join("\n")
}

/// Meaning of one header cell of a summary table, if it names a figure we keep.
fn summary_cell_label(cell: &str) -> Option<&'static str> {
    if cell.contains("beginning balance") || cell.contains("previous balance") {
        Some("beginning")
    } else if cell.contains("ending balance") || cell.contains("new balance") || cell.contains("current balance") {
        Some("ending")
    } else if cell.contains("interest") || cell.contains("fee") || cell.contains("charge") {
        None // its own category; the totals carry it when they exist
    } else if cell.contains("deposit") || cell.contains("credit") {
        Some("credits")
    } else if cell.contains("withdrawal") || cell.contains("debit") || cell.contains("check") {
        Some("debits")
    } else {
        None
    }
}

/// Credit unions print a transaction date and an effective date side by side ("Sep 03
/// Sep 03   -34.50   -1,513.00"). The second date is blanked, width kept, so the row has
/// one date and its columns stay put.
fn drop_second_date(line: &str) -> String {
    let mut it = line.split_whitespace();
    let (Some(first), Some(second)) = (it.next(), it.next()) else { return line.to_string() };
    if it.next().is_none() || parse_date_token(first).is_none() || parse_date_token(second).is_none() {
        return line.to_string();
    }
    let start = line.find(first).unwrap() + first.len();
    let at = start + line[start..].find(second).unwrap();
    format!("{}{}{}", &line[..at], " ".repeat(second.len()), &line[at + second.len()..])
}

/// Court scans carry stray marks down the left margin that the text layer turns into a
/// short token in front of the date ("0   Mar 20 DEPOSIT ... 2,100.00", "c':,  Mar 25 ...").
/// A token of up to four characters that is not a check number (three or more digits)
/// right before a date is blanked, width kept.
/// Under a deposits section a check number cannot start a row, so there a longer digit
/// run before the date ("100000    07/05    5.72", a mailing code) is junk as well.
/// A month no calendar has at the start of a line ("42/09 4455950653 3,250.00 12/24 ...",
/// a 1 read as 4) is the month of the other date on the line, when there is one.
fn repair_bad_month(line: &str) -> String {
    // (A scan's margin glyph in front of the date, "| 05/03. Deposit": the repairs apply to
    // the line behind it.)
    if let Some(first) = line.split_whitespace().next() {
        if first.chars().count() == 1 && !first.chars().all(|c| c.is_alphanumeric()) {
            let at = line.find(first).unwrap();
            let rest = &line[at + first.len()..];
            let repaired = repair_bad_month(rest.trim_start());
            if repaired != rest.trim_start() {
                return format!("{}{first} {repaired}", &line[..at]);
            }
            return line.to_string();
        }
    }
    let Some(first) = line.split_whitespace().next() else { return line.to_string() };
    // ("dan 31 489 36,210.81 Jan 29 508 5,555.41": a month name with one letter misread in
    // front of a day, when the same line prints the month right further on.)
    let month_word = |t: &str| MONTHS.iter().position(|m| t.len() == 3 && t.eq_ignore_ascii_case(m));
    if first.len() == 3 && first.chars().all(|c| c.is_ascii_alphabetic()) && month_word(first).is_none() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let day_next = toks.get(1).and_then(|d| d.parse::<u32>().ok()).map(|d| (1..=31).contains(&d)).unwrap_or(false);
        let mut later: Vec<usize> = toks.iter().skip(2).filter_map(|t| month_word(t)).collect();
        later.dedup();
        if day_next && later.len() == 1 {
            let m = MONTHS[later[0]];
            let off = first.to_ascii_lowercase().chars().zip(m.chars()).filter(|(a, b)| a != b).count();
            if off == 1 {
                let at = line.find(first).unwrap();
                let cased: String = if first.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) { format!("{}{}", m[..1].to_ascii_uppercase(), &m[1..]) } else { m.to_string() };
                return format!("{}{cased}{}", &line[..at], &line[at + first.len()..]);
            }
        }
    }
    // ("02/17. Card Purchase With Pin": a speck after the date.)
    if let Some(bare) = first.strip_suffix('.').or_else(|| first.strip_suffix(',')) {
        if bare.len() >= 3 && parse_date_token(bare).is_some() && parse_date_token(first).is_none() {
            let at = line.find(first).unwrap();
            return repair_bad_month(&format!("{}{bare} {}", &line[..at], &line[at + first.len()..]));
        }
    }
    // ("o9/11 206 200.00": a zero read as the letter o in front of the month.)
    if first.len() == 5 && (first.starts_with('o') || first.starts_with('O')) && first.as_bytes()[2] == b'/' && parse_date_token(&format!("0{}", &first[1..])).is_some() {
        let at = line.find(first).unwrap();
        return format!("{}0{}", &line[..at], &line[at + 1..]);
    }
    let shape = (4..=5).contains(&first.len()) && first.as_bytes()[2] == b'/' && first[..2].chars().all(|c| c.is_ascii_digit()) && first[3..].chars().all(|c| c.is_ascii_digit());
    if !shape || first[..2].parse::<u32>().map(|m| m <= 12).unwrap_or(true) {
        return line.to_string();
    }
    let month = line.split_whitespace().skip(1).find_map(|t| parse_date_token(t).map(|_| t.split('/').next().unwrap_or(""))).filter(|m| m.len() == 2);
    match month {
        Some(m) if parse_date_token(&format!("{m}/{}", &first[3..])).is_some() => {
            let at = line.find(first).unwrap();
            format!("{}{m}{}", &line[..at], &line[at + 2..])
        }
        _ => line.to_string(),
    }
}

/// A court text layer that read every slash as a 1 ("411", "412", "416", "4112" for 4/1,
/// 4/2, 4/6, 4/12 on a Wells page): when no line on the page starts with a real date and
/// four or more start with such a token sharing one month, the middle 1 is the slash.
fn repair_slashless_dates(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    // (Some slashes survive on the same page: "7/9" and "7/12" beside "712" and "7122".
    // The real dates' month must agree with the slashless ones; the statement date "April
    // 30, 2021", already rewritten, may be the odd one out.)
    let real_months: Vec<u32> = lines.iter().filter_map(|l| l.split_whitespace().next().and_then(parse_date_token).map(|(m, _, _)| m)).collect();
    // (month, day) read from "M1D", "M1DD", "MM1D", "MM1DD". "1105" reads as 1/05 or 11/05:
    // the real dates on the page decide (11/07 beside it), else the one-digit month.
    let split = |t: &str, other_date: bool| -> Option<(u32, u32)> {
        if !(3..=5).contains(&t.len()) || !t.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut readings: Vec<(u32, u32)> = Vec::new();
        // (The slash read as a 7 as well, "579" for 5/9: only when the real dates confirm it.)
        let mut sevens: Vec<(u32, u32)> = Vec::new();
        for m_len in [1usize, 2] {
            if t.len() > m_len + 1 && (&t[m_len..m_len + 1] == "1" || &t[m_len..m_len + 1] == "7") {
                if let (Ok(m), Ok(d)) = (t[..m_len].parse::<u32>(), t[m_len + 1..].parse::<u32>()) {
                    if (1..=12).contains(&m) && (1..=31).contains(&d) && t[m_len + 1..].len() <= 2 {
                        if &t[m_len..m_len + 1] == "1" { readings.push((m, d)); } else { sevens.push((m, d)); }
                    }
                }
            }
        }
        // (Or the slash simply gone, "1105" for 11/05: four digits, only when the real
        // dates confirm the month and no date follows within three tokens. "1004 ^ 02/02
        // $308.00" is check 1004 with its date, not October 4; PNC's three-column check
        // row "1105 1005 * 800.00 077074442] 11/07 ..." has a whole column before the next.)
        let mut dropped: Vec<(u32, u32)> = Vec::new();
        if t.len() == 4 && !other_date {
            if let (Ok(m), Ok(d)) = (t[..2].parse::<u32>(), t[2..].parse::<u32>()) {
                if (1..=12).contains(&m) && (1..=31).contains(&d) {
                    dropped.push((m, d));
                }
            }
        }
        let confirmed = |(m, _): &(u32, u32)| !real_months.is_empty() && real_months.iter().all(|r| r == m);
        readings.iter().copied().find(confirmed).or_else(|| dropped.iter().chain(sevens.iter()).copied().find(confirmed)).or_else(|| readings.first().copied())
    };
    let firsts: Vec<Option<(u32, u32)>> = lines.iter().map(|l| {
        let mut toks = l.split_whitespace();
        let first = toks.next()?;
        // (A check number with its amount right after it, "1031 1,528.00 Paid Check", is
        // not a date either.)
        let rest: Vec<&str> = toks.take(3).collect();
        let other_date = rest.iter().any(|t| parse_date_token(t).is_some()) || rest.first().map(|t| is_amount_token(t)).unwrap_or(false);
        split(first, other_date)
    }).collect();
    let months: Vec<u32> = firsts.iter().flatten().map(|(m, _)| *m).collect();
    // (Two or three slashless dates are enough when every real date on the page shares
    // their month: "1105", "1106", "1108" beside "11/05" and "11/07" in a PNC check table.)
    let confirmed = months.len() >= 2 && real_months.len() >= 2 && real_months.iter().all(|m| *m == months[0]);
    // (A single one is repaired when the real dates confirm its month and the line has the
    // shape of a row with its running balance, two figures at the end: "579 Legal Order
    // Debit ... 50.00 0.00" beside 5/16 and 5/18.)
    let lone_row = months.len() == 1 && real_months.len() >= 2 && real_months.iter().all(|m| *m == months[0]) && lines.iter().zip(&firsts).any(|(l, f)| {
        let toks: Vec<&str> = l.split_whitespace().collect();
        f.is_some() && toks.len() >= 4 && is_amount_token(toks[toks.len() - 1]) && is_amount_token(toks[toks.len() - 2])
    });
    if months.is_empty() || months.len() < 4 && !confirmed && !lone_row || months.iter().any(|m| *m != months[0]) {
        return text.to_string();
    }
    let agreeing = real_months.iter().filter(|m| **m == months[0]).count();
    if real_months.len() > 1 && agreeing * 2 < real_months.len() {
        return text.to_string();
    }
    lines.iter().zip(&firsts).map(|(l, f)| match f {
        Some((m, d)) => {
            let first = l.split_whitespace().next().unwrap();
            let at = l.find(first).unwrap();
            format!("{}{m:02}/{d:02}{}", &l[..at], &l[at + first.len()..])
        }
        None => l.to_string(),
    }).collect::<Vec<_>>().join("\n")
}

/// The same for a line with no other date on it ("42/22 ACH DEPOSIT, VENMO CASHOUT ...
/// 868.00"): the month of the nearest dated lines above and below, when they agree.
fn repair_bad_months(text: &str) -> String {
    // (A scan's margin glyph in front of the date, "| 05/05 Deposit", "‘11-04 ...": blanked
    // first so the dates behind it are seen by every repair here; bullets and signs stay.)
    let glyphs = ['|', '!', '¦', '[', ']', '{', '}', '\'', '\u{2018}', '\u{2019}', '"', '\u{201c}', '\u{201d}', '_', '~', '='];
    let unglyphed: String = text.lines().map(|l| {
        match l.split_whitespace().next() {
            Some(t) if t.chars().count() == 1 && glyphs.contains(&t.chars().next().unwrap()) => {
                let at = l.find(t).unwrap();
                format!("{}{}{}", &l[..at], " ".repeat(t.len()), &l[at + t.len()..])
            }
            _ => l.to_string(),
        }
    }).collect::<Vec<_>>().join("\n");
    let text = &repair_slashless_dates(&unglyphed);
    // A zero read as the letter o anywhere in a leading date ("oo/o9 7238 130.00"): the
    // letters become zeros; a month of 00 is then repaired from the neighbours below.
    let text = &text.lines().map(|l| {
        let Some(first) = l.split_whitespace().next() else { return l.to_string() };
        let shape = first.len() <= 5 && first.contains('/') && first.chars().any(|c| c == 'o' || c == 'O') && first.chars().all(|c| c == '/' || c == 'o' || c == 'O' || c.is_ascii_digit());
        if shape {
            let at = l.find(first).unwrap();
            format!("{}{}{}", &l[..at], first.replace(['o', 'O'], "0"), &l[at + first.len()..])
        } else {
            l.to_string()
        }
    }).collect::<Vec<_>>().join("\n");
    let lines: Vec<&str> = text.lines().collect();
    // ("21/11/15", a reference line's year-first date, is November too.)
    let month_of = |l: &str| l.split_whitespace().next().and_then(parse_date_token).map(|(m, _, _)| format!("{m:02}"));
    // The month's width when it cannot be one: two digits over 12, or three digits with one
    // read in ("121/15 NATIONWIDE EDI PYMNTS 169.25-").
    let bad = |l: &str| -> Option<usize> {
        let first = l.split_whitespace().next()?;
        let (m, rest) = first.split_once('/')?;
        // ("42/22/2021", Heritage Bank in a scan: the day is what comes before the year;
        // "95/09." a speck after the day.)
        let rest = rest.trim_end_matches(|c| c == '.' || c == ',');
        let d = rest.split('/').next().unwrap_or(rest);
        if !(m.len() == 2 || m.len() == 3) || d.is_empty() || d.len() > 2 || !m.chars().chain(d.chars()).all(|c| c.is_ascii_digit()) || rest.split('/').nth(1).map(|y| !(y.len() == 2 || y.len() == 4) || !y.chars().all(|c| c.is_ascii_digit())).unwrap_or(false) {
            return None;
        }
        (m.len() == 3 || m.parse::<u32>().map(|v| v > 12 || v == 0).unwrap_or(false)).then_some(m.len())
    };
    // A letter in the day ("10/a1 ENTERGY MISSISSIBANK DRAFT 857.19-"): the date of the
    // nearest dated line above or below whose day fits the digit that survived (10/21,
    // not 10/22), when only one does.
    let bad_day = |l: &str| -> bool {
        let Some(first) = l.split_whitespace().next() else { return false };
        let Some((m, d)) = first.split_once('/') else { return false };
        // (Or a day no month has, "02/41" for 02/11: one digit misread.)
        let over = d.len() == 2 && d.chars().all(|c| c.is_ascii_digit()) && d.parse::<u32>().map(|v| v > 31).unwrap_or(false);
        // (Or a digit read in twice, "03/117" for 03/17.)
        let extra = d.len() == 3 && d.chars().all(|c| c.is_ascii_digit());
        m.len() <= 2 && !m.is_empty() && m.chars().all(|c| c.is_ascii_digit()) && ((1..=2).contains(&d.len()) && (d.chars().filter(|c| c.is_ascii_alphabetic()).count() == 1 && d.chars().all(|c| c.is_ascii_alphanumeric()) || over) || extra)
    };
    if !lines.iter().any(|l| bad(l).is_some() || bad_day(l)) {
        return text.to_string();
    }
    let date_of = |l: &str| l.split_whitespace().next().and_then(parse_date_token).map(|(m, d, _)| (m, d));
    let out: Vec<String> = lines.iter().enumerate().map(|(i, l)| {
        if bad_day(l) {
            let first = l.split_whitespace().next().unwrap();
            let (month, day) = first.split_once('/').unwrap();
            // (A day with a digit too many: the days left by dropping one digit that fall
            // between the row above, or the row's own date, and the row below; one such day
            // is the day. "03/117" between 03/17 and 03/22 is 03/17, not 03/11.)
            if day.len() == 3 {
                let m: u32 = month.parse().unwrap_or(0);
                let low = l.split_whitespace().skip(1).find_map(parse_date_token).map(|(m, d, _)| (m, d)).or_else(|| lines[..i].iter().rev().find_map(|x| date_of(x)));
                let high = lines[i + 1..].iter().find_map(|x| date_of(x));
                let mut days: Vec<(u32, u32)> = (0..3).filter_map(|k| format!("{}{}", &day[..k], &day[k + 1..]).parse::<u32>().ok()).filter(|d| (1..=31).contains(d)).map(|d| (m, d)).filter(|c| low.map(|a| *c >= a).unwrap_or(false) && high.map(|b| *c <= b).unwrap_or(false)).collect();
                days.sort_unstable();
                days.dedup();
                if days.len() == 1 {
                    let at = l.find(first).unwrap();
                    return format!("{}{:02}/{:02}{}", &l[..at], days[0].0, days[0].1, &l[at + first.len()..]);
                }
                return l.to_string();
            }
            // (A day of digits alone, over 31, may differ from the neighbour's in one digit.)
            let slack = if day.chars().all(|c| c.is_ascii_digit()) { 1 } else { 0 };
            let fits = |(m, d): &(u32, u32)| -> bool {
                let printed = if day.len() == 2 { format!("{d:02}") } else { d.to_string() };
                month.parse::<u32>().ok() == Some(*m) && printed.len() == day.len() && printed.chars().zip(day.chars()).filter(|(p, c)| !(c.is_ascii_alphabetic() || p == c)).count() <= slack
            };
            let above = lines[..i].iter().rev().find_map(|x| date_of(x));
            let below = lines[i + 1..].iter().find_map(|x| date_of(x));
            // (The three nearest dated lines each way: the row's own section may end before
            // a date that fits appears, "03/1C" over a withdrawal dated 03/22 and the daily
            // table's 03/10.)
            let near: Vec<(u32, u32)> = lines[..i].iter().rev().filter_map(|x| date_of(x)).take(3).chain(lines[i + 1..].iter().filter_map(|x| date_of(x)).take(3)).collect();
            let mut fitting: Vec<(u32, u32)> = near.iter().copied().filter(fits).collect();
            fitting.sort_unstable();
            fitting.dedup();
            // (A day over 31 with no neighbour to copy: the one-digit repairs that fall
            // between the row above and the row below, the row's own transaction date
            // ("02/41 Card Purchase 02/10 ...") standing in for a missing row above; one
            // such day is the day.)
            if fitting.is_empty() && slack == 1 {
                // (The row's own date first: the row above may close another section.)
                let own = l.split_whitespace().skip(1).find_map(parse_date_token).map(|(m, d, _)| (m, d));
                let low = own.or(above);
                let m: u32 = month.parse().unwrap_or(0);
                let mut days: Vec<(u32, u32)> = (1..=31u32).filter(|d| {
                    let printed = format!("{d:02}");
                    printed.chars().zip(day.chars()).filter(|(p, c)| p != c).count() == 1
                }).map(|d| (m, d)).filter(|c| low.map(|a| *c >= a).unwrap_or(false) && below.map(|b| *c <= b).unwrap_or(false)).collect();
                days.dedup();
                if days.len() == 1 {
                    fitting = days;
                }
            }
            if fitting.len() == 1 {
                let at = l.find(first).unwrap();
                return format!("{}{:02}/{:02}{}", &l[..at], fitting[0].0, fitting[0].1, &l[at + first.len()..]);
            }
            return l.to_string();
        }
        let Some(width) = bad(l) else { return l.to_string() };
        let above = lines[..i].iter().rev().find_map(|x| month_of(x));
        let below = lines[i + 1..].iter().find_map(|x| month_of(x));
        // (When the two disagree, "05/31 Balance Forward" above "96/03 Deposit" over June's
        // rows, the month most of the page's dated lines carry decides, if it is one digit
        // off the misread one.)
        let majority = || -> Option<String> {
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for m in lines.iter().filter_map(|x| month_of(x)) { *counts.entry(m).or_insert(0) += 1; }
            let (best, n) = counts.iter().max_by_key(|(_, n)| **n)?;
            let total: usize = counts.values().sum();
            let bad_m = &l.split_whitespace().next()?[..2];
            (n * 2 > total && bad_m.len() == 2 && best.len() == 2 && best.chars().zip(bad_m.chars()).filter(|(a, b)| a != b).count() == 1).then(|| best.clone())
        };
        let month = match (above, below) {
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(_), Some(_)) => majority(),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            _ => None,
        };
        match month {
            // (Dropping one of the three digits must leave the month: "121" for 11 or 12,
            // never "345".)
            Some(m) if width == 2 || { let t = &l.split_whitespace().next().unwrap()[..3]; (0..3).any(|k| format!("{}{}", &t[..k], &t[k + 1..]) == m) } => {
                let first = l.split_whitespace().next().unwrap();
                let day = first[width + 1..].trim_end_matches(|c| c == '.' || c == ',');
                if parse_date_token(&format!("{m}/{day}")).is_some() {
                    let at = l.find(first).unwrap();
                    return format!("{}{m}/{day}{}", &l[..at], &l[at + first.len()..]);
                }
                l.to_string()
            }
            _ => l.to_string(),
        }
    }).collect();
    out.join("\n")
}

fn strip_margin_junk(line: &str, credit_section: bool) -> String {
    let mut it = line.split_whitespace();
    let (Some(first), Some(second), Some(_)) = (it.next(), it.next(), it.next()) else { return line.to_string() };
    // A margin digit glued to the date ("411/15 DBT CRD ...", a scan's edge): three digits
    // before the slash can only be one too many.
    if first.len() >= 5 && first.as_bytes()[..3].iter().all(|c| c.is_ascii_digit()) && first.as_bytes()[3] == b'/' && parse_date_token(&first[1..]).is_some() {
        let at = line.find(first).unwrap();
        return format!("{} {}", &line[..at], &line[at + 1..]);
    }
    // ("Date 11/30/21 Page 4" heads Community Bank's pages, "From 11/01/2025 To 11/30/2025"
    // Trustmark's period: labels, not stray marks.)
    if ["date", "from", "thru", "to", "for"].iter().any(|w| first.eq_ignore_ascii_case(w)) {
        return line.to_string();
    }

    let all_digits = first.chars().all(|c| c.is_ascii_digit());
    // ("#6" on a check image caption is check number 6, not a stray mark.)
    let numbered = first.len() >= 2 && first.starts_with('#') && first[1..].chars().all(|c| c.is_ascii_digit());
    // (SunTrust sets its section label in the margin over two lines, "Deposits/" then
    // "Credits" beside the first row: "Credits 04/18 284.88 DEPOSIT".)
    let margin_label = ["credits", "debits", "deposits", "withdrawals", "checks", "paid", "history"].iter().any(|w| first.eq_ignore_ascii_case(w));
    let short_junk = (first.len() <= 4 && (!all_digits || first.len() <= 2) || margin_label) && !numbered;
    let code_in_credits = credit_section && all_digits && first.len() >= 5;
    let junk = (short_junk || code_in_credits) && parse_date_token(first).is_none() && parse_date_token(second).is_some() && !is_amount_token(first);
    if !junk {
        return line.to_string();
    }
    let at = line.find(first).unwrap();
    format!("{}{}{}", &line[..at], " ".repeat(first.len()), &line[at + first.len()..])
}

/// "6-3 ..." at the start of a line becomes "06-03 ...", same width kept by eating spaces.
fn pad_short_dashed_date(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let Some(tok) = rest.split_whitespace().next() else { return line.to_string() };
    let Some((m, d)) = tok.split_once('-') else { return line.to_string() };
    let short = |p: &str| !p.is_empty() && p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit());
    if !(short(m) && short(d) && (m.len() == 1 || d.len() == 1)) || rest.len() == tok.len() {
        return line.to_string();
    }
    let padded = format!("{:0>2}-{:0>2}", m, d);
    let after = &rest[tok.len()..];
    let extra = padded.len() - tok.len();
    // (The columns keep their places only when there are spaces to spare; flat OCR's
    // single space stays, "12-1 Direct Deposit" must not become "12-01Direct Deposit".)
    let spare = after.len() - after.trim_start().len();
    let trimmed_after = if spare > extra { after.strip_prefix(&" ".repeat(extra)).unwrap_or(after) } else { after };
    format!("{}{}{}", &line[..indent], padded, trimmed_after)
}

/// Figures at the end of a row whose commas and points a scan turned into spaces ("-6 789
/// 84 25 409.88"): read from the right, a figure is a cents pair (or an amount with its
/// point) behind groups of three digits behind a leading group of one to three, signed or
/// not. Two figures must come out of the tail, the row's amount and its balance, and the
/// plain reading must not already give them; otherwise the line is left alone.
fn rejoin_split_figures(line: &str) -> Option<String> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    let group3 = |t: &str| t.len() == 3 && digits(t);
    let lead = |t: &str| { let b = t.trim_start_matches('-'); (1..=3).contains(&b.len()) && digits(b) };
    if toks.iter().rev().take_while(|t| is_amount_token(t) && t.contains('.')).count() >= 2 {
        return None;
    }
    let mut i = toks.len();
    let mut figures: Vec<String> = Vec::new();
    while figures.len() < 2 && i > 0 {
        let t = toks[i - 1];
        let (mut int_groups, cents): (Vec<&str>, String) = if is_amount_token(t) && t.contains('.') && !t.contains(',') {
            // "409.88": a whole figure, or the last group of one that lost its commas.
            let (int, c) = t.rsplit_once('.').unwrap();
            if int.len() != 3 || !digits(int) {
                figures.push(t.to_string());
                i -= 1;
                continue;
            }
            (vec![int], c.to_string())
        } else if t.len() == 2 && digits(t) {
            (Vec::new(), t.to_string())
        } else if t.len() == 3 && digits(t) && t.starts_with('0') && figures.is_empty() {
            // ("014" at the very end: a balance of 0.14 whose point went too.)
            figures.push(format!("0.{}", &t[1..]));
            i -= 1;
            continue;
        } else {
            break;
        };
        i -= 1;
        // Groups of three (an unsigned group of three may be the leading group itself),
        // then a shorter or signed leading group.
        while i > 0 {
            let p = toks[i - 1];
            if group3(p) {
                int_groups.insert(0, p);
                i -= 1;
                continue;
            }
            if lead(p) {
                int_groups.insert(0, p);
                i -= 1;
            }
            break;
        }
        if int_groups.is_empty() {
            return None;
        }
        let sign = if int_groups[0].starts_with('-') { "-" } else { "" };
        let mut int = int_groups.iter().map(|g| g.trim_start_matches('-')).collect::<Vec<_>>().join(",");
        if int.is_empty() { int = "0".into(); }
        let figure = format!("{sign}{int}.{cents}");
        if !is_amount_token(&figure) {
            return None;
        }
        figures.push(figure);
    }
    if figures.len() != 2 || i == 0 {
        return None;
    }
    figures.reverse();
    Some(format!("{} {} {}", toks[..i].join(" "), figures[0], figures[1]))
}

/// A summary label that some banks print as a dated row of the activity table.
fn is_balance_label(lower: &str) -> bool {
    // ("Beginning      Balance" in a wide text layer: runs of spaces count as one.)
    let squeezed = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    ["beginning balance", "ending balance", "opening balance", "closing balance", "balance forward", "previous balance"].iter().any(|k| squeezed.contains(k))
}

/// Some filings carry a doubled text layer: every line drawn twice with a shift, so
/// pdftotext yields fragments that overlap ("06/04  Online Domestic Wire Transfer" /
/// "Transfer Via:" / "Via: TD Bank," / ...). When a page shows this pattern on at least
/// five lines, a fragment that begins with the previous line's last token continues that
/// line, and a fragment that repeats the previous line's tail is dropped.
/// Lines that begin with the previous line's last token: the mark of a doubled text layer.
fn doubled_overlaps(text: &str) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    let last_tok = |l: &str| l.split_whitespace().last().map(str::to_string);
    lines.windows(2).filter(|w| {
        let (a, b) = (w[0].trim(), w[1].trim());
        let bt: Vec<&str> = b.split_whitespace().collect();
        !a.is_empty() && bt.len() >= 2 && last_tok(a).as_deref() == Some(bt[0]) && a.len() > bt[0].len()
    }).count()
}

fn merge_doubled_fragments(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let last_tok = |l: &str| l.split_whitespace().last().map(str::to_string);
    if doubled_overlaps(text) < 5 {
        return text.to_string();
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        let t = line.trim();
        if let Some(prev) = out.last_mut() {
            let pt = prev.trim_end();
            let bt: Vec<&str> = t.split_whitespace().collect();
            if !bt.is_empty() && !pt.is_empty() {
                // The fragment repeats the previous line's tail (or all of it).
                if pt.ends_with(t) && t.len() < pt.len() || pt == t {
                    continue;
                }
                // The fragment starts with the previous line's last token: a continuation.
                // (Not after a complete two-column check row, "1907 300.00 08/08 1912 430.00
                // 08/21": the fragment "08/21 -- 75,808.50" is the other layer's copy of the
                // column beside it, and would break the row's shape.)
                let check_row = pt.split_whitespace().filter(|t| parse_date_token(t).is_some()).count() >= 2 && pt.split_whitespace().filter(|t| is_amount_token(t)).count() >= 2;
                if check_row && bt.len() >= 2 && last_tok(pt).as_deref() == Some(bt[0]) {
                    continue;
                }
                // A previous line that was only that token ("1952*" over "1952*  400.00  06/06
                // 1996 ...") is the other layer's stray cell: the fragment is the whole row,
                // kept as printed so its columns still line up.
                if bt.len() >= 2 && pt.trim() == bt[0] {
                    *prev = line.to_string();
                    continue;
                }
                if bt.len() >= 2 && last_tok(pt).as_deref() == Some(bt[0]) && pt.len() > bt[0].len() && !is_amount_token(bt[0]) {
                    let rest = t[bt[0].len()..].trim_start();
                    prev.push(' ');
                    prev.push_str(rest);
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

/// Remove lone "^" and "*" tokens (footnote marks on check rows), keeping the spacing of
/// everything else so aligned pages keep their columns.
fn drop_footnote_marks(line: &str) -> String {
    if !line.contains(" ^") && !line.contains(" *") && !line.contains("*") {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    chars.iter().enumerate().map(|(i, &c)| {
        let lone = (c == '^' || c == '*') && (i == 0 || chars[i - 1] == ' ') && (i + 1 == chars.len() || chars[i + 1] == ' ');
        // A star glued to a date ("01/03*", BankNorth's out-of-sequence check marker).
        let glued = c == '*' && i >= 5 && chars[i - 1].is_ascii_digit() && chars[i - 3] == '/' && (i + 1 == chars.len() || chars[i + 1] == ' ');
        if lone || glued { ' ' } else { c }
    }).collect()
}

/// "$ 130813.77" -> "$130813.77 " (the space moves after the figure so widths hold).
fn glue_dollar_sign(line: &str) -> String {
    if !line.contains("$ ") {
        return line.to_string();
    }
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b' ' {
            let mut k = i + 1;
            while k < b.len() && b[k] == b' ' {
                k += 1;
            }
            if k < b.len() && k - i <= 8 && (b[k].is_ascii_digit() || b[k] == b'.' || b[k] == b'-') {
                out.push('$');
                let mut j = k;
                while j < b.len() && b[j] != b' ' {
                    out.push(b[j] as char);
                    j += 1;
                }
                for _ in 0..(k - i - 1) {
                    out.push(' ');
                }
                i = j;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Navy Federal prints the debit sign a space after the amount ("20.00 -   21,290.87").
/// The dash moves onto the amount ("20.00-  ") so the signed-amount rules read it, width kept.
/// En and em dashes and the minus sign become "-" where they lead a figure ("–$1,500.00",
/// online exports) and a space elsewhere ("600,000.00 —", a margin mark Tesseract reads
/// off a scan's edge, which as "-" would make the deposit a debit).
fn dashes_to_signs(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    chars.iter().enumerate().map(|(i, &c)| {
        if c == '\u{2013}' || c == '\u{2014}' || c == '\u{2212}' {
            let next = chars.get(i + 1).copied().unwrap_or(' ');
            if next.is_ascii_digit() || next == '$' || next == '(' { '-' } else { ' ' }
        } else {
            c
        }
    }).collect()
}

/// "11.414": digits, thousands commas allowed, a point and exactly three digits.
fn three_decimals(t: &str) -> bool {
    let Some((whole, frac)) = t.rsplit_once('.') else { return false };
    frac.len() == 3 && frac.chars().all(|c| c.is_ascii_digit()) && !whole.is_empty() && whole.chars().all(|c| c.is_ascii_digit() || c == ',') && whole.chars().next().unwrap().is_ascii_digit() && is_amount_token(&format!("{whole}.{}", &frac[..2]))
}

/// Where a one-to-three-letter code starts after a signed amount ("22.00-SC" -> 6), if any.
fn glued_code(t: &str) -> Option<usize> {
    let letters = t.chars().rev().take_while(|c| c.is_ascii_alphabetic()).count();
    if !(1..=3).contains(&letters) || t.len() <= letters + 2 {
        return None;
    }
    let at = t.len() - letters;
    (t[..at].ends_with('-') && is_amount_token(&t[..at])).then_some(at)
}

/// A section sign for a five ("§,442.13") or a pound sign glued to a figure ("£28.48"),
/// Tesseract's readings of a worn 5 and a stray mark: the token is an amount once the
/// glyph is a 5 or gone. Runs before the page's non-ASCII characters are blanked.
/// (Also a letter read into the cents of a signed figure, "7.1i1-": three characters
/// after the point, one of them an i, l or I, are the two cents digits with an insertion.)
/// A text layer whose font map lost its figures ("$o.oo", "$431.3s", "O9lO1l24" for
/// $0.00, $431.35, 09/01/24; Wintrust's second account page): letters that stand in for
/// figures are put back when the whole token then reads as an amount (behind its "$") or a
/// date. Other tokens are left alone.
fn unfont(t: &str) -> Option<String> {
    let has_letter = t.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    if !has_letter {
        return None;
    }
    let figure = |c: char| match c { 'o' | 'O' => '0', 'l' | 'I' => '1', 's' | 'S' => '5', 'z' | 'Z' => '2', other => other };
    if let Some(body) = t.strip_prefix('$') {
        let mapped: String = body.chars().map(figure).collect();
        return (mapped.contains('.') && is_amount_token(&format!("${mapped}"))).then(|| format!("${mapped}"));
    }
    // (A date's slashes read as "l": two of them, with digits or figure letters around.)
    if has_digit && t.matches('l').count() == 2 && t.chars().all(|c| c.is_ascii_digit() || matches!(c, 'o' | 'O' | 'l' | 'z' | 's' | 'S' | 'Z' | 'I')) {
        let mapped: String = t.chars().map(|c| if c == 'l' { '/' } else { figure(c) }).collect();
        return parse_date_token(&mapped).map(|_| mapped);
    }
    None
}

fn glyph_figures(line: &str) -> String {
    let line = &if line.contains('$') || line.contains('l') {
        line.split(' ').map(|t| unfont(t).unwrap_or_else(|| t.to_string())).collect::<Vec<_>>().join(" ")
    } else {
        line.to_string()
    };
    let stray_in_cents = |t: &str| -> Option<String> {
        let body = t.strip_suffix('-').or_else(|| t.strip_suffix('+'))?;
        let (dollars, cents) = body.rsplit_once('.')?;
        let letters: Vec<usize> = cents.char_indices().filter(|(_, c)| matches!(c, 'i' | 'l' | 'I' | '|')).map(|(i, _)| i).collect();
        if cents.len() != 3 || letters.len() != 1 || dollars.is_empty() {
            return None;
        }
        let fixed = format!("{dollars}.{}{}", &cents[..letters[0]], &cents[letters[0] + 1..]);
        is_amount_token(&fixed).then(|| format!("{fixed}{}", &t[body.len()..]))
    };
    if !(line.contains('§') || line.contains('£') || line.contains(".") && (line.contains('i') || line.contains('l') || line.contains('I') || line.contains('|'))) {
        return line.to_string();
    }
    line.split(' ').map(|t| {
        if let Some(fixed) = stray_in_cents(t) {
            return fixed;
        }
        if !(t.contains('§') || t.contains('£')) { return t.to_string(); }
        // ("§0,000:00" for 50,000.00: the colon for the point is put right here too.)
        let five = t.replace('§', "5").replace('£', "5").replace(':', ".");
        let gone = t.replace(['§', '£'], "");
        if is_amount_token(&five) && t.contains('§') { five } else if is_amount_token(&gone) { gone } else { t.to_string() }
    }).collect::<Vec<_>>().join(" ")
}

fn attach_trailing_sign(line: &str) -> String {
    // A comma, semicolon or point glued to an amount ("188.47, VOYAHSA", "22,336.57. IOD",
    // Hancock Whitney scans) is noise; so is a tilde read into a trailing sign ("4,022.84~-", Brookline court scans).
    // Both go when the rest of the token is an amount.
    // (A table's vertical rule read as a bracket after a reference number, "077074442]" in
    // PNC's three-column check table, goes the same way.)
    let line = &if line.contains(",") || line.contains(";") || line.contains(".") || line.contains("]") || line.contains("}") {
        line.split(' ').map(|t| {
            let bare = t.trim_end_matches(|c| c == ',' || c == ';' || c == '.' || c == ']' || c == '}');
            if bare != t && !bare.is_empty() && bare.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false) && (is_amount_token(bare) || bare.len() >= 6 && bare.chars().all(|c| c.is_ascii_digit()) && t.len() == bare.len() + 1) { format!("{bare} ") } else { t.to_string() }
        }).collect::<Vec<_>>().join(" ")
    } else {
        line.to_string()
    };
    // A stray character after the last amount ("100.00 100.00 S", "10.00 90.00 &": a
    // scan's margin marks read as glyphs) is dropped; a sign there stays for the rule below.
    // (The same between two amounts: "AES STDNT LOAN $188.11 : $221.66", "CHECK #1527
    // $48.50 . $104.05".)
    // (The dropped marks are blanked in place, so the amounts keep their columns: "$4.99 D
    // $32.00" under Pinnacle's Credits / Debits / Balance header is a $32.00 debit.)
    let line = &{
        let spans: Vec<(usize, &str)> = line.split_whitespace().map(|t| (t.as_ptr() as usize - line.as_ptr() as usize, t)).collect();
        let mut toks: Vec<&str> = spans.iter().map(|(_, t)| *t).collect();
        let mut removed: Vec<usize> = Vec::new();
        let stray = |t: &str| t.chars().count() == 1 && !t.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == ')');
        // (At the very end also a lone digit, signed or not: "425 * 04/25 $26,295.28 ——4",
        // a rule's tail read as a figure.)
        let stray_digit = |t: &str| { let d = t.trim_start_matches('-'); d.len() == 1 && d.chars().all(|c| c.is_ascii_digit()) && t.len() <= 2 };
        let mut dropped = false;
        while toks.len() >= 3 && (stray(toks[toks.len() - 1]) || stray_digit(toks[toks.len() - 1])) && is_amount_token(toks[toks.len() - 2]) {
            toks.pop();
            removed.push(toks.len());
            dropped = true;
        }
        // (The tail of a rule read as a short scribble after a check row's amount, "225 4“
        // 02/22 139,667.00 ES", "232 *A 02/22 3,095.00 s3", "234 4 02/18 5,143.94 rr": one
        // or two tokens of three characters or fewer, not figures, after the amount of a
        // line that starts with a check number and carries a date.)
        let scribble = |t: &str| t.len() <= 3 && !t.chars().all(|c| c.is_ascii_digit()) && !t.chars().all(|c| c.is_ascii_uppercase() && c.is_ascii_alphabetic()) || t.len() <= 2 && t.chars().all(|c| c.is_ascii_alphabetic());
        let check_row = toks.len() >= 4 && toks[0].len() <= 7 && toks[0].chars().all(|c| c.is_ascii_digit()) && toks.iter().skip(1).take(3).any(|t| parse_date_token(t).is_some());
        if check_row {
            let run = toks.iter().rev().take(3).take_while(|t| scribble(t) || stray(t)).count();
            if run >= 1 && toks.len() > run + 2 && is_amount_token(toks[toks.len() - 1 - run]) {
                removed.extend(toks.len() - run..toks.len());
                toks.truncate(toks.len() - run);
                dropped = true;
            }
        }
        // (Indexes into `spans`: nothing before the tail has been removed yet.)
        let mut i = 1;
        let mut kept: Vec<usize> = (0..toks.len()).collect();
        while i + 1 < toks.len() {
            if stray(toks[i]) && is_amount_token(toks[i - 1]) && is_amount_token(toks[i + 1]) {
                toks.remove(i);
                removed.push(kept.remove(i));
                dropped = true;
            } else {
                i += 1;
            }
        }
        if dropped {
            let mut out = line.to_string();
            removed.sort_unstable_by(|a, b| b.cmp(a));
            for idx in removed {
                let (start, tok) = spans[idx];
                out.replace_range(start..start + tok.len(), &" ".repeat(tok.chars().count()));
            }
            out
        } else {
            line.to_string()
        }
    };
    // An overdrawn balance marked "OD" ("3.16-OD", "10.59-0D", a Puerto Rico bank): the
    // marker goes, the minus stays. (Without the minus, "3.160D" in its daily balance
    // table, the marker becomes one: the balance is overdrawn either way.)
    let line = &if line.contains("OD") || line.contains("0D") {
        line.split(' ').map(|t| {
            let Some(bare) = t.strip_suffix("OD").or_else(|| t.strip_suffix("0D")) else { return t.to_string() };
            if bare.ends_with('-') && is_amount_token(bare) {
                bare.to_string()
            } else if bare.contains('.') && is_amount_token(bare) && !bare.starts_with('-') {
                format!("{bare}-")
            } else {
                t.to_string()
            }
        }).collect::<Vec<_>>().join(" ")
    } else {
        line.to_string()
    };
    // A semicolon for a comma and a colon for a point ("1;199:05", a TD check table in a
    // scan): the token is an amount once they are swapped back.
    let line = &if line.contains(';') || line.contains(':') {
        line.split(' ').map(|t| {
            // (Never a plain time, "11:50" in "POS DEB 11:50 12/26/24": the token must carry
            // a semicolon or a thousands comma to be a figure.)
            // (Or the figure before the colon is no hour: "221:39" is 221.39.)
            let no_hour = t.split_once(':').map(|(h, m)| h.len() >= 3 && h.chars().all(|c| c.is_ascii_digit()) && m.len() == 2 && m.chars().all(|c| c.is_ascii_digit())).unwrap_or(false);
            // ("$60;000.00-", Flushing Bank in a scan: the dollar sign and the trailing
            // minus stay around the repaired figure.)
            let (head, body, tail) = { let b = t.strip_prefix('$').unwrap_or(t); let (b, tail) = b.strip_suffix('-').map(|x| (x, "-")).unwrap_or((b, "")); (if t.starts_with('$') { "$" } else { "" }, b, tail) };
            if (body.contains(';') || body.contains(':') && body.contains(',') || no_hour) && body.chars().all(|c| c.is_ascii_digit() || c == ';' || c == ':' || c == ',' || c == '.') && body.chars().any(|c| c.is_ascii_digit()) {
                let fixed = format!("{head}{}{tail}", body.replace(';', ",").replace(':', "."));
                if is_amount_token(&fixed) { return fixed; }
            }
            t.to_string()
        }).collect::<Vec<_>>().join(" ")
    } else {
        line.to_string()
    };
    // Three decimals ("11.414", Chase's footnote glyph read as a digit glued to the cents):
    // no bank prints a third decimal, so the amount ends after two.
    let line = &if line.split(' ').any(|t| three_decimals(t)) {
        line.split(' ').map(|t| if three_decimals(t) { format!("{} ", &t[..t.len() - 1]) } else { t.to_string() }).collect::<Vec<_>>().join(" ")
    } else {
        line.to_string()
    };
    // "2,915.71 CR" / "375.00 DR" (Community Bank): the marker after the amount is the sign.
    let line = &{
        let toks: Vec<&str> = line.split_whitespace().collect();
        let n = toks.len();
        if n >= 3 && (toks[n - 1] == "CR" || toks[n - 1] == "DR") && is_amount_token(toks[n - 2]) && !toks[n - 2].ends_with('-') && !toks[n - 2].ends_with('+') {
            let sign = if toks[n - 1] == "CR" { '+' } else { '-' };
            let cut = line.trim_end().rfind(toks[n - 1]).unwrap_or(line.len());
            format!("{}{sign}", line[..cut].trim_end())
        } else {
            line.to_string()
        }
    };
    let line = &if line.contains('~') {
        line.split(' ').map(|t| if t == "~-" || t == "-~" { "- ".to_string() } else if t.contains('~') && is_amount_token(&t.replace('~', "")) { format!("{} ", t.replace('~', "")) } else { t.to_string() }).collect::<Vec<_>>().join(" ")
    } else {
        line.to_string()
    };
    // A code glued to a signed amount ("22.00-SC", Brookline's service-charge marker) is
    // its own token, moved before the amount.
    let line = &if line.contains("-") && line.split(' ').any(|t| glued_code(t).is_some()) {
        // (The code goes before the amount so the amount still stands next to the balance.)
        line.split(' ').map(|t| glued_code(t).map(|at| format!("{} {}", &t[at..], &t[..at])).unwrap_or_else(|| t.to_string())).collect::<Vec<_>>().join(" ")
    } else {
        line.to_string()
    };
    if !line.contains(" - ") && !line.trim_end().ends_with(" -") {
        return line.to_string();
    }
    let toks: Vec<&str> = line.split_whitespace().collect();
    let mut out = line.to_string();
    for i in 1..toks.len() {
        let next_ok = toks.get(i + 1).map(|t| is_amount_token(t)).unwrap_or(true);
        if toks[i] == "-" && is_amount_token(toks[i - 1]) && !toks[i - 1].ends_with('-') && next_ok {
            // The amount then exactly one space then the dash: swap the two characters.
            if let Some(at) = out.find(&format!("{} -", toks[i - 1])) {
                let a = at + toks[i - 1].len();
                out.replace_range(a..a + 2, "- ");
            }
        }
    }
    out
}

/// Table rules in some text layers come out as the letter I, glued to the date it borders
/// ("I 09/10I            $100.00", a Sunrise Banks deposits grid). When a token is a date
/// followed by "I", that "I" and every lone "I" on the line are blanked, width kept.
fn drop_rule_glyphs(line: &str) -> String {
    let glued = line.split_whitespace().any(|t| t.len() >= 4 && t.ends_with('I') && parse_date_token(&t[..t.len() - 1]).is_some());
    if !glued {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    chars.iter().enumerate().map(|(i, &c)| {
        let ends_token = i + 1 == chars.len() || chars[i + 1] == ' ';
        let lone = i == 0 || chars[i - 1] == ' ';
        let after_date = i > 0 && chars[i - 1].is_ascii_digit();
        if c == 'I' && ends_token && (lone || after_date) { ' ' } else { c }
    }).collect()
}

/// OCR sometimes retells a table as bullets: "- 04/18: CCD DEPOSIT, TOAST DEP: 3,176.12".
/// Drop the bullet and the colon after the date so the line reads as a row.
fn unbullet(line: &str) -> String {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("- ") {
        let mut it = rest.splitn(2, ' ');
        if let (Some(first), Some(tail)) = (it.next(), it.next()) {
            // "- 04/18: ..." or, after a month name was rewritten, "- 10/02 : ...".
            let (date, tail) = match first.strip_suffix(':') {
                Some(d) => (Some(d), tail),
                None if tail.trim_start().starts_with(": ") => (Some(first), tail.trim_start()[2..].trim_start()),
                None => (None, tail),
            };
            if let Some(date) = date {
                if parse_date_token(date).is_some() {
                    let indent = line.len() - t.len();
                    // "...JL33W: 3,176.12": the colon before the amount goes too.
                    let tail = match tail.trim_end().rfind(": ") {
                        Some(i) if is_amount_token(tail[i + 2..].trim()) => format!("{} {}", &tail[..i], tail[i + 2..].trim()),
                        _ => tail.trim_end().trim_end_matches(':').to_string(),
                    };
                    return format!("{}{} {}", " ".repeat(indent), date, tail);
                }
            }
        }
    }
    line.to_string()
}

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
            // A month name inside prose ("Hcclaimmpt May 4 002624241") stays text: a date
            // column follows the line start, a digit, a bullet, another date or a mailing
            // barcode in capitals ("ACEMBHDOODOPKMBLFEBEPIPK  Jun 14", Wintrust), not a word.
            // (One or two lowercase letters before it, "o Oct 15 DEPOSIT", "ae Oct 31
            // INTEREST EARNED", are a scan's margin marks, not a word.)
            let prev_word = {
                let before: Vec<&char> = chars[..i].iter().rev().skip_while(|c| c.is_whitespace()).take_while(|c| !c.is_whitespace()).collect();
                before.first().map(|c| c.is_ascii_lowercase()).unwrap_or(false) && before.len() >= 3
            };
            let month = if prev_word { None } else { month };
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
                        // (A four-digit number after the day is a year only when it reads as
                        // one: "Jan 07 1135 21,705.13" is check 1135, not the year 1135.)
                        let year_ok = yend - ys == 4 && chars[ys..yend].iter().collect::<String>().parse::<u32>().map(|y| (1990..=2100).contains(&y)).unwrap_or(false);
                        if year_ok && ys > de {
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
    // Synovus writes dates as "06-01", KeyBank as "10-3" and "9-30-24". The dashed form
    // needs a two-digit month or day (never "1-2") so ranges and phone numbers stay out.
    let dashed = tok.split('-').collect::<Vec<_>>();
    let short = |p: &str| !p.is_empty() && p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit());
    // ("8-8-24" with a year is a date too; the year is checked below.)
    let dashed_date = (2..=3).contains(&dashed.len()) && short(dashed[0]) && short(dashed[1]) && (dashed[0].len() == 2 || dashed[1].len() == 2 || dashed.len() == 3);
    let parts: Vec<&str> = if dashed_date { dashed } else { tok.split('/').collect() };
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
/// Full section phrases the banks print, matched even through a scan's glyph noise.
const SECTION_PHRASES: &[(&str, Kind)] = &[("checks and other debits", Kind::Debit), ("deposits and other credits", Kind::Credit), ("all credit activity", Kind::Credit), ("all debit activity", Kind::Debit), ("deposits and additions", Kind::Credit), ("withdrawals and debits", Kind::Debit), ("electronic withdrawals", Kind::Debit), ("checks paid", Kind::Debit)];

fn section_for(line: &str) -> Option<Kind> {
    let mut l = line.to_ascii_lowercase();
    // Fifth Third prints the section total on the header line: "Deposits / Credits
    // 46 items totaling $137,498.52". Only the words before the count name the section.
    if let Some(p) = l.find(" items total") {
        let head = l[..p].trim_end();
        let cut = head.rfind(' ').filter(|_| head.split_whitespace().last().map(|t| t.chars().all(|c| c.is_ascii_digit())).unwrap_or(false));
        l = cut.map(|c| head[..c].to_string()).unwrap_or_else(|| head.to_string());
    }
    // PNC runs the count into the header on a flat page: "Deposits and Other Additions
    // There were 10 Deposits and Other Additions totaling $54,232.55".
    if let Some(p) = l.find(" there were ").or_else(|| l.find(" there was ")) {
        l = l[..p].trim_end().to_string();
    }
    // Court OCR smears headers ("!OTHER WITHDRAWALS, FEES & C H A R G E S - I - - -"):
    // single-character tokens are noise there.
    let toks: Vec<&str> = l.split_whitespace().filter(|t| t.chars().count() > 1 || t.chars().all(|c| c.is_ascii_digit())).collect();
    let starts_with_date = toks.first().and_then(|t| parse_date_token(t)).is_some();
    // ("- 0 DEBITS -00 YTD INTEREST PAID": a summary category, its zero without its point,
    // carries a figure and is no header.)
    let ends_with_amount = toks.last().map(|t| is_amount_token(t) || lost_zero(t)).unwrap_or(false) || toks.iter().any(|t| lost_zero(t));
    // Headers carry no reference or account numbers ("TRANSFER TO DEPOSIT SYSTEM ACCOUNT
    // XXXXXX4516" is a description continuation, not a section).
    // (A header may name its account after a dotted leader: BankNorth's "CHECKS / DEBITS
    // ..... ACCOUNT 02229319"; the number after "account" is not a reference there. Without
    // the leader, "TRANSFER TO DEPOSIT SYSTEM ACCOUNT XXXXXX3134" is a wire's continuation.)
    let leader = l.contains("...") || l.contains("---");
    // ("Account:-------1079", Huntington's title, glues the leader and number to the word.)
    let before_account = if leader { toks.iter().position(|t| *t == "account" || t.starts_with("account:")).unwrap_or(toks.len()) } else { toks.len() };
    let has_reference = toks[..before_account].iter().any(|t| t.chars().filter(|c| c.is_ascii_digit()).count() >= 4 || t.contains("xxx"));
    // (First American's recaps run to seven words in capitals: "SUMMARY OF ELECTRONIC
    // DEBITS AND OTHER WITHDRAWALS".)
    let shouted = line.split_whitespace().all(|t| t.chars().all(|c| !c.is_ascii_lowercase()));
    // A full section phrase drowned in a scan's glyph noise ("ESE =~ CHECKS AND OTHER
    // DEBITS Ssainieacme mee ont ie iwc mis", Premier Bank) is still the header.
    if !starts_with_date && !ends_with_amount && !toks.iter().any(|t| is_amount_token(t)) {
        if let Some((_, k)) = SECTION_PHRASES.iter().find(|(p, _)| l.contains(p)) {
            if !l.contains("total") && !l.contains("summary") {
                return Some(*k);
            }
        }
        // (Union Bank's "Purchases ATM card and Debit card™ purchases" runs to seven words.)
        if l.starts_with("purchases") && !l.contains("total") {
            return Some(Kind::Debit);
        }
    }
    // (A wire's wrapped detail line starting with a figure, "347 '43ELEKTA INC DEPOSIT",
    // is no header either.)
    let digit_led = !leader && toks.first().map(|t| t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).unwrap_or(false);
    let header_like = (l.contains("---") || toks.len() <= 6 || toks.len() <= 8 && shouted) && !starts_with_date && !ends_with_amount && !has_reference && !digit_led;
    if !header_like {
        return None;
    }
    if l.contains("daily balance") || l.contains("balance summary") {
        return None;
    }
    // ("Credit Union   Statement Period" is Navy Federal's letterhead, not a section.)
    if l.contains("credit union") {
        return None;
    }
    if l.contains("deposit") || l.contains("credit") || l.contains("additions") {
        return Some(Kind::Credit);
    }
    if l.contains("debit") || l.contains("withdrawal") || l.contains("checks") || l.contains("fees") || l.contains("charges") || l.contains("payments") || l.contains("subtractions") || l.starts_with("items paid") {
        return Some(Kind::Debit);
    }
    None
}

/// Kind from the description words, and whether a word actually decided it (true) or
/// the section / default did (false). Weak kinds are the ones balance arithmetic may flip.
fn kind_and_confidence(desc: &str, section: Option<Kind>) -> (Kind, bool) {
    let l = desc.to_ascii_lowercase();
    // Inside a credit or debit section the section decides: "ATM Check Deposit" and "Card
    // Purchase Return" under DEPOSITS are credits, a reversed provisional credit under
    // WITHDRAWALS is a debit. Words only decide on unsectioned lists (flat OCR pages,
    // "Transactions by Date").
    if let Some(k) = section {
        return (k, true);
    }
    // (Wells names the account holder's card and bill payments "American Express ACH
    // Pmt", "Citi Card Online Payment", "Macys Online Pmt", "Chase Credit Crd Epay".)
    const DEBIT_WORDS: &[&str] = &["withdrawal", " debit", "purchase", " fee", "charge", "check ", "payment to", "zelle to", "transfer to", "payment authorized", "pmt to", "bill pay", "wire out", "outgoing wire", "ach pmt", "online pmt", "online payment", "epay"];
    const CREDIT_WORDS: &[&str] = &["deposit", " credit", "zelle from", "transfer from", "pmt from", "payment from", "wire in", "incoming wire", "refund", "cashback", "cash back"];
    // Phrases that contain a debit word but are credits: Wells "ATM Check Deposit",
    // "Purchase Return authorized" on a flat OCR page; Wells incoming wires name the
    // originator ("WT ... Morgan Stanley /Org=..."), outgoing ones the beneficiary (/Bnf=).
    // Legends lists a returned ACH pull under deposits as "Non Check Return Ret-R08".
    // (Signature's "DEBIT CARD REFUND" is money back on the card: a credit.)
    const CREDIT_PHRASES: &[&str] = &["check deposit", "purchase return", "/org=", "non check return", "payment from", "pmt from", "refund"];
    if CREDIT_PHRASES.iter().any(|w| l.contains(w)) {
        return (Kind::Credit, true);
    }
    // A pull that came back (BMO: "RETURNED ACH DEBIT NSF  WEB COMCAST") is money returned
    // to the account, and "TRANSFER IN" is money arriving. (Not the fee a bank charges for a
    // returned item: "RETURNED ACH DEBIT FEE" stays a debit.)
    // A payment app's cash-out ("VENMO CASHOUT", "CASH APP*CASH OUT", "PAYPAL TRANSFER ...
    // CASHOUT") moves the balance from the app into this account: money in.
    let app = l.contains("venmo") || l.contains("cash app") || l.contains("paypal") || l.contains("square inc");
    if app && (l.contains("cashout") || l.contains("cash out")) && !l.contains(" fee") {
        return (Kind::Credit, true);
    }
    // Square's payout of the merchant's card sales ("SQUARE INC  SQ240101  A COMPANY", the
    // settlement date after "SQ") is money in. (A card purchase at another Square seller reads
    // "SQ *NAME" and stays a debit, and so does any Square fee.)
    // (A scan reads the "S" as a dollar sign: "$Q240101".)
    if l.contains("square inc") && l.split_whitespace().any(|w| w.len() == 8 && (w.starts_with("sq") || w.starts_with("$q")) && w[2..].chars().all(|c| c.is_ascii_digit())) && !l.contains(" fee") {
        return (Kind::Credit, true);
    }
    // A card processor paying out the merchant's sales ("PODIUM PAYMENTS PODIUM PAY",
    // "STRIPE TRANSFER", "SHOPIFY PAYOUT") is money in; the same companies' software charges
    // read differently ("WWW.PODIUM.COM", "STRIPE BILLING") and stay debits.
    if ["podium payments", "stripe transfer", "shopify payout", "shopify payments"].iter().any(|w| l.contains(w)) && !l.contains(" fee") && !l.contains("billing") {
        return (Kind::Credit, true);
    }
    let returned_pull = l.contains("returned ach debit") || l.contains("returned debit") || l.contains("ach debit return");
    if (returned_pull || l.contains("transfer in ") || l.ends_with("transfer in")) && !l.contains(" fee") && !l.contains("charge") {
        return (Kind::Credit, true);
    }
    // (Heritage Bank's "INET XFER 12-02 FROM XXXXXXXX0686" / "INET XFER 12-02 TO ...": the
    // direction word comes after the date.)
    if l.contains("xfer") && !l.contains("transfer") {
        if l.contains(" from ") { return (Kind::Credit, true); }
        if l.contains(" to ") { return (Kind::Debit, true); }
    }
    if l.starts_with("debit") || DEBIT_WORDS.iter().any(|w| l.contains(w)) {
        return (Kind::Debit, true);
    }
    if l.starts_with("credit") || CREDIT_WORDS.iter().any(|w| l.contains(w)) || l.contains("interest") && section.is_none() {
        return (Kind::Credit, true);
    }
    (Kind::Debit, false)
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
        // Singular "Withdrawal  Deposit  Balance" (credit unions) only when no plural matched,
        // so the end offsets of the usual headers do not move.
        Columns {
            credit: find(&["deposits/credits", "deposits/ credits", "credits", "credit", "deposits", "additions"]).or_else(|| find(&["deposit"])),
            debit: find(&["withdrawals/debits", "withdrawals/ debits", "debits", "debit", "withdrawals", "subtractions", "payments"]).or_else(|| find(&["withdrawal"])),
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

    /// An amount that ends well left of the first amount column is part of the description
    /// ("Overdraft Fee for a Transaction Posted on 09/16 $100.00      35.00": the fee is 35.00).
    fn before_columns(&self, end: usize) -> bool {
        match [self.credit, self.debit].into_iter().flatten().min() {
            Some(first) => end + 4 < first,
            None => false,
        }
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
    /// Page the section header was read on. On later pages the section is inherited and
    /// a row's own words outrank it (see `row_kind`).
    section_page: Option<usize>,
    /// A dashed date ("5-31-24", "6-10") was read: short "6-3" dates are dates too.
    dashed_dates: bool,
    /// Page headed "Images": check and deposit pictures with captions, nothing to parse.
    images_page: Option<usize>,
    /// Mercury: the last day seen, for the rows of that day that run onto the next page.
    mercury_date: Option<String>,
    /// Online printout: the day whose date cell was last printed, for the rows of that day
    /// that run onto the next page (see `carry_column_dates`).
    column_date: Option<String>,
    /// The listing's one amount column signs its debits (see `signs_the_debits`), so the
    /// sign is the kind wherever a row is read.
    signed_amounts: bool,
    /// The table header put the amount before the description ("Effective date  Posted
    /// date  Amount  Transaction detail", Wells; "Date  Amount  Description", Citizens):
    /// on a flat line the first amount after the date is the transaction's, whatever
    /// dollar figures the description quotes ("NSF Return Item Fee ... $23,530.00").
    amount_first: bool,
    /// Decided-by-word row counts on the current page under an inherited section.
    words_page: Option<usize>,
    words_credit: usize,
    words_debit: usize,
    in_daily: bool,
    table: usize,
    table_key: String,
    /// Table id per normalized header, so a listing resumed after another header
    /// ("Checks and Other Debits continued" then "Funds Transfers Out - continued") is
    /// still the same table.
    tables: BTreeMap<String, usize>,
    /// Inside a block that repeats or explains transactions without being one
    /// ("Items returned unpaid", "Monthly service fee summary"); lines there are skipped.
    informational: bool,
    /// Last running balance seen on a flat (OCR) page, and the transactions read since,
    /// with whether their kind came from a word. See `resolve_group`.
    last_balance: Option<f64>,
    /// Amount of the last row that carried a running balance.
    last_amount: Option<f64>,
    /// This page's rows are headed "Date  Description  Amount  Balance": two figures at the
    /// end of a row are its amount and the balance after it, whether or not they chain.
    amount_balance: bool,
    /// PNC's two summary boxes read as one line ("Deposits and Other Additions   Checks and
    /// Other Deductions"): the "Total ... Total ..." line below carries both totals.
    two_box_summary: bool,
    open_group: Vec<(usize, bool)>,
    /// Inside a paid-checks listing ("Summary of checks written", "CHECKS IN CHECK NO.
    /// ORDER"): a row whose date the scan garbled is still a check.
    check_table: bool,
    /// A fintech export's "Name  Date  Status  Amount  Balance" table (Relay): the
    /// description comes first, then the date, "Settled", the signed amount and the balance.
    status_table: bool,
}

/// pdftotext -layout keeps columns aligned with runs of spaces; OCR output does not.
/// On a flat page the character offset of an amount says nothing, so kinds come from
/// words and running-balance arithmetic instead.
pub fn is_flat(text: &str) -> bool {
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
    if group.is_empty() || group.len() > 20 {
        return;
    }
    if signed(ledger, 0) == target {
        ledger.weak.retain(|id| !group.iter().any(|(g, _)| g == id));
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
            // The arithmetic settled every row of the group.
            ledger.weak.retain(|id| !group.iter().any(|(g, _)| g == id));
            return;
        }
        if mask_space == all_mask {
            break;
        }
    }
}

/// Headers of blocks whose dated, amounted lines are not transactions.
const INFORMATIONAL_HEADERS: &[&str] = &["items returned unpaid", "monthly service fee summary", "account transaction fees summary", "fee period", "overdraft protection", "interest summary", "automatic transactions", "service charge summary", "service fee calculation"];

impl State {
    /// Kind of a row under the current section. A section header on this page decides;
    /// one inherited from an earlier page only fills in when the row's words do not,
    /// because OCR pages sometimes drop their own header ("Electronic Payments" rows
    /// after a "Deposits" page would all become credits).
    fn row_kind(&mut self, page: usize, desc: &str) -> (Kind, bool) {
        if self.section.is_some() && self.section_page != Some(page) {
            if self.words_page != Some(page) {
                self.words_page = Some(page);
                self.words_credit = 0;
                self.words_debit = 0;
            }
            let (k, strong) = kind_and_confidence(desc, None);
            if strong {
                match k {
                    Kind::Credit => self.words_credit += 1,
                    Kind::Debit => self.words_debit += 1,
                }
                return (k, true);
            }
            // Rows without a deciding word follow the page's decided rows when those
            // contradict the inherited section (a lost "Electronic Payments" header).
            if self.words_debit > self.words_credit && self.section == Some(Kind::Credit) {
                return (Kind::Debit, false);
            }
            if self.words_credit > self.words_debit && self.section == Some(Kind::Debit) {
                return (Kind::Credit, false);
            }
        }
        kind_and_confidence(desc, self.section)
    }


    /// Switch to the table identified by `key`. A header repeated on the next page
    /// ("Credits (continued)") keeps the same table so cross-page repeats are not
    /// mistaken for a second listing.
    fn enter_table(&mut self, key: &str) {
        let key = key.to_ascii_lowercase().replace("(continued)", "").replace("continued", "");
        let key = key.split_whitespace().filter(|t| *t != "-").collect::<Vec<_>>().join(" ");
        if key != self.table_key {
            let next = self.tables.len() + 1;
            self.table = *self.tables.entry(key.clone()).or_insert(next);
            self.table_key = key;
        }
        self.informational = false;
        self.open_group.clear();
        self.check_table = false;
        self.status_table = false;
    }
}

/// Parse one page. `year_hint` fills in years for MM/DD dates.
fn parse_page(text: &str, page: usize, year_hint: Option<i32>, ledger: &mut Ledger, st: &mut State) {
    st.amount_balance = false;
    // Month-name dates and split amounts are normalized line by line first, so stacked
    // "Apr02" / "1,395 .37" cells zip like any other (the per-line pass below is idempotent).
    let text = &merge_doubled_fragments(text);
    // (Dash variants become "-" here already, so a pre-pass sees "-$30,000.00" as an amount.)
    // Other symbols (arrows, glyphs the text layer made of icons) become spaces, so the
    // ASCII-only repairs still run on their lines.
    let ascii = |l: &str| -> String { l.chars().map(|c| if c.is_ascii() || c == '\u{2022}' { c } else { ' ' }).collect() };
    // ("Dally Balance Summary": Tesseract's i as l in a heading the parser keys on.)
    let headings = |l: &str| -> String { if l.contains("Dally") || l.contains("DALLY") { l.replace("Dally", "Daily").replace("DALLY", "DAILY") } else { l.to_string() } };
    let pre: String = text.lines().map(|l| join_split_amounts(&normalize_month_dates(&ascii(&dashes_to_signs(&glyph_figures(&headings(l))))))).collect::<Vec<_>>().join("\n");
    let pre = repair_bad_months(&pre);
    let pre = pre.lines().map(repair_check_pairs).collect::<Vec<_>>().join("\n");
    let (pre, mercury_date) = unfold_mercury_rows(&pre, st.mercury_date.as_deref());
    if mercury_date.is_some() {
        st.mercury_date = mercury_date;
    }
    let text = &zip_stacked_cells(&merge_stacked_serials(&fold_field_lists(&lower_lifted_amounts(&carry_column_dates(&fold_stacked_header_words(&rejoin_marked_rows(&pre)), &mut st.column_date)))));
    let mut columns: Option<Columns> = None;
    let flat = is_flat(text);
    // Account-detail reports (Ocrolus-style Bank of America exports) list newest first.
    // There a row's balance is checked against the row above it, whose amount is not
    // this row's, so balance arithmetic must not flip kinds on such a page.
    let newest_first = {
        let days: Vec<i64> = text.lines().filter_map(|l| l.split_whitespace().next()).filter_map(|t| parse_date_token(t)).filter_map(|(m, d, y)| Some(days_from_civil(y?, m, d))).collect();
        let down = days.windows(2).filter(|w| w[1] < w[0]).count();
        let up = days.windows(2).filter(|w| w[1] > w[0]).count();
        down >= 2 && down > up * 3
    };
    // Check image pages without a heading (TD): three or more caption lines "#361  04/01
    // $9,000.00", each a number sign, a date and an amount, mean the page is pictures.
    let caption_lines = text.lines().filter(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        t.len() >= 3 && t[0].len() >= 2 && t[0].starts_with('#') && t[0][1..].chars().all(|c| c.is_ascii_digit()) && parse_date_token(t[1]).is_some() && t.iter().any(|x| is_amount_token(x))
    }).count();
    // BankNorth's image pages: check and receipt pictures captioned "1068  1/25/2024  Paid
    // 8100.00" (the check number, its date and "Paid"), or "1/10/2024  700.00" under a
    // "Record Of Deposit" receipt. Two captions of either kind mean pictures.
    let paid_captions = text.lines().filter(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        t.len() >= 3 && t[t.len() - 2].eq_ignore_ascii_case("paid") && is_amount_token(t[t.len() - 1]) && t[..t.len() - 2].iter().any(|x| parse_date_token(x).is_some())
    }).count();
    // (Teller receipts carry "TRAN DATE:" and "PREPARED BY:" too.)
    let lower_text = text.to_ascii_lowercase();
    let receipts = ["record of deposit", "prepared by:", "tran date:"].iter().map(|w| lower_text.matches(w).count()).sum::<usize>();
    // (Flushing Bank titles its check picture pages "Image Statement".)
    let image_statement = text.lines().take(4).any(|l| l.to_ascii_lowercase().contains("image statement"));
    if caption_lines >= 3 || paid_captions >= 2 || receipts >= 2 || image_statement {
        st.images_page = Some(page);
    }
    // A page whose dated rows end in signed amounts ("$-500.00") for at least three rows and
    // in unsigned ones for others is a signed layout: the sign decides, not the words.
    let signed_page = {
        // (Rows with one trailing figure only: where a running balance follows the amount,
        // an overdrawn month's "-528.63" balances are not signed amounts.)
        // (Nor check-table rows, "9/15  1213*  1,000.00": a check table never signs its
        // amounts, whatever the listing above it does.)
        let check_row = |t: &[&str]| t.len() == 3 && { let n = t[1].trim_end_matches('*'); (3..=7).contains(&n.len()) && n.chars().all(|c| c.is_ascii_digit()) };
        let ends: Vec<&str> = text.lines().filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            (t.len() >= 3 && parse_date_token(t[0]).is_some() && is_amount_token(t[t.len() - 1]) && !is_amount_token(t[t.len() - 2]) && !check_row(&t)).then(|| t[t.len() - 1])
        }).collect();
        // (Trailing signs count too: Community Bank's "375.00-" debits and "2,915.71 CR"
        // credits, the marker already turned into a "+".)
        // (Trailing signs only on a flat OCR page: on an aligned page the daily balance
        // table's overdrawn "11,921.52-" and the unsplit two-column check lines would vote too.)
        let flat_page = is_flat(text);
        let signed = |a: &&str| a.starts_with("$-") || a.starts_with("-$") || a.starts_with('-') || a.starts_with('+') || a.starts_with("$+") || flat_page && (a.ends_with('-') || a.ends_with('+'));
        let neg = ends.iter().filter(|a| a.starts_with("$-") || a.starts_with("-$") || a.starts_with('-') || flat_page && a.ends_with('-')).count();
        // Signed and unsigned rows must mix in one run, or the signed ones must be the
        // majority: a page whose unsigned "Subtractions" list is followed by a short signed
        // fee list (KeyBank) changes sign once and is not signed.
        let changes = ends.windows(2).filter(|w| signed(&w[0]) != signed(&w[1])).count();
        neg >= 3 && neg < ends.len() && (changes >= 2 || neg * 2 > ends.len())
    };
    let signed_amounts = st.signed_amounts;
    // The same for rows that end in a running balance (Great Southern, Community Bank:
    // "1/02 ATM Service Charge 2.50- 9,775.44" under "1/02 FED SALARY ... 5,113.06
    // 9,777.94"): three or more amounts signed "-" before the balance make the unsigned
    // ones credits.
    let signed_balance_page = {
        let amounts: Vec<&str> = text.lines().filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            (t.len() >= 4 && parse_date_token(t[0]).is_some() && is_amount_token(t[t.len() - 1]) && is_amount_token(t[t.len() - 2])).then(|| t[t.len() - 2])
        }).collect();
        let neg = amounts.iter().filter(|a| a.ends_with('-')).count();
        neg >= 3 && neg < amounts.len() && amounts.iter().all(|a| !a.starts_with('-') && !a.starts_with('+') && !a.ends_with('+'))
    };
    let mut pending_header: Option<Columns> = None;
    let mut last_txn: Option<usize> = None;
    // Dated OCR line waiting for its amount on a following line (date token, description).
    let mut pending_flat: Option<(String, String)> = None;
    // Undated text lines taken into the pending row's description so far.
    let mut pending_flat_lines = 0usize;
    // Column table rows broken over two lines: a dated line without amounts ("12/02/2024"
    // alone, or "12/02/2024   XX2823CHKPURCHSIG SP FRAGRANT"), then an undated line with
    // the cells ("JEWE   $96.58   $30,997.94"). The date is written into the second line's
    // indentation so the cells keep their columns; the first line's words lead the row.
    let mut pending_row: Option<(String, String)> = None;
    let mut pending_lead: Option<String> = None;
    let mut desc_indent: Option<usize> = None;
    // Whether the line before the current one was blank (flat OCR keeps no indent, so a
    // wrapped description straight under its row is told from a title by that alone).
    let mut blank_before = false;
    // Column-style summaries ("Previous Balance  Total Credits  Total Debits  Current Balance")
    // put the labels on one line and the values on the next.
    let mut pending_columns: Vec<&'static str> = Vec::new();
    let mut pending_has_checks = false;
    // Description printed on the line above its dated row (credit unions: "Withdrawal
    // RETURNED ACH FEE In the amount $100.00 Diverse" over "Sep 03  -34.50  -1,513.00").
    let mut lead_desc: Option<String> = None;
    // Markdown summary table from OCR ("Previous Date | Beginning Balance | Deposits | ...
    // | Ending Balance" over "10/01/2023 | 16,129.15 | 3,160.60 | ... | 12,757.34"): the
    // cells are explicit, so labels and values pair by cell.
    let mut pending_cells: Vec<Option<&'static str>> = Vec::new();
    let raw_lines: Vec<&str> = text.lines().collect();
    // Dated rows listed newest first (an online printout): the first dated line's date is
    // later than the last's. Running balances then chain backwards.
    // (Counted step by step, so a statement running from December into January, whose
    // last date is "smaller" than its first, is not taken for one.)
    let newest_first = {
        let dated: Vec<(u32, u32)> = raw_lines.iter().filter_map(|l| l.split_whitespace().next().and_then(parse_date_token).map(|(m, d, _)| (m, d))).collect();
        let down = dated.windows(2).filter(|w| w[1] < w[0]).count();
        let up = dated.windows(2).filter(|w| w[1] > w[0]).count();
        dated.len() >= 3 && down >= 1 && up == 0
    };
    for (line_no, &raw) in raw_lines.iter().enumerate() {
        if raw.matches('|').count() >= 2 {
            let cells: Vec<String> = raw.split('|').map(|c| c.trim().to_ascii_lowercase()).filter(|c| !c.is_empty()).collect();
            if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':')) {
                continue; // "--- | --- | ---" separator
            }
            if pending_cells.is_empty() && cells.iter().any(|c| c.contains("beginning balance") || c.contains("previous balance")) && cells.iter().any(|c| c.contains("ending balance")) {
                pending_cells = cells.iter().map(|c| summary_cell_label(c)).collect();
                continue;
            }
            if !pending_cells.is_empty() {
                if cells.len() == pending_cells.len() {
                    for (label, cell) in pending_cells.iter().zip(&cells) {
                        let value = cell.split_whitespace().find(|t| is_amount_token(t)).and_then(parse_amount);
                        if let (Some(label), Some(v)) = (label, value) {
                            match *label {
                                "beginning" => ledger.summary.beginning_balance.get_or_insert(v),
                                "credits" => ledger.summary.total_credits.get_or_insert(v.abs()),
                                "debits" => ledger.summary.total_debits.get_or_insert(v.abs()),
                                _ => ledger.summary.ending_balance.get_or_insert(v),
                            };
                        }
                    }
                }
                pending_cells.clear();
                continue;
            }
        }
        // Form rules scanned as underscores glue to the date ("03/31_____  217,945.04");
        // online activity exports print negatives with an en dash ("–$1,500.00"); Chase
        // marks checks with lone "^" and "*" footnote symbols ("1447 * ^ 09/17 159.05").
        // Markdown-style OCR separates cells with pipes ("| 1,651.07 | 14,478.08").
        // (A dash glued to a figure or a dollar sign is a sign; a dash standing alone is a
        // scan's margin mark, "600,000.00 —", or a rule, and is blanked, never a sign.)
        let raw = dashes_to_signs(&raw.replace('_', " ")).replace('|', " ");
        // A dollar sign a space before its figure ("$ 130813.77", an older commercial
        // statement) is glued back on; the width shift is one character.
        let raw = glue_dollar_sign(&raw);
        let raw = attach_trailing_sign(&drop_rule_glyphs(&drop_footnote_marks(&raw)));
        // Month names first, so a bullet "- Oct 02: ..." reads as "- 10/02: ..." for unbullet.
        let normalized = drop_second_date(&strip_margin_junk(&join_split_amounts(&unbullet(&normalize_month_dates(&raw))), st.section == Some(Kind::Credit)));
        // KeyBank writes "6-3" once its dashed dates are established ("Beginning balance
        // 5-31-24", "6-10"): a one-digit-by-one-digit dash at the start of a line is a date
        // then, never a range. Padded to "06-03" so the token rules apply.
        let normalized = if st.dashed_dates { pad_short_dashed_date(&normalized) } else { normalized };
        let mut stripped = strip_margin_barcode(normalized.trim_end());
        // Citizens prints the checks total in the margin beside a row ("1952*  400.00  06/06
        // 1996  1,750.00  06/17   --   57,479.00"): a trailing dash and amount after a row
        // that already has its date and amount are cut off.
        {
            let toks: Vec<&str> = stripped.split_whitespace().collect();
            let n = toks.len();
            if n >= 5 && (toks[n - 2] == "-" || toks[n - 2] == "--") && is_amount_token(toks[n - 1]) && toks[..n - 2].iter().any(|t| parse_date_token(t).is_some()) && toks[..n - 2].iter().any(|t| is_amount_token(t)) {
                if let Some(at) = stripped.rfind(toks[n - 2]) {
                    stripped.truncate(at);
                }
            }
        }
        if columns.is_some() && !flat && stripped.contains("   ") {
            let toks: Vec<&str> = stripped.split_whitespace().collect();
            let starts_with_date = toks.first().and_then(|t| parse_date_token(t)).is_some();
            let has_amount = toks.iter().any(|t| is_amount_token(t));
            let indent = stripped.len() - stripped.trim_start().len();
            let joins = matches!(&pending_row, Some((date_tok, _)) if !starts_with_date && has_amount && indent > date_tok.len());
            if joins {
                let (date_tok, lead) = pending_row.take().unwrap();
                stripped = format!("{date_tok}{}{}", " ".repeat(indent - date_tok.len()), stripped.trim_start());
                pending_lead = if lead.is_empty() { None } else { Some(lead) };
            } else if starts_with_date && !has_amount && toks.len() <= 8 && footer_numbers(&stripped).is_none() {
                // (Wells' page header "May31,2021 • Page3of4" is dated but never a row.)
                pending_row = Some((toks[0].to_string(), toks[1..].join(" ")));
            } else if starts_with_date {
                pending_row = None;
            }
        }
        let line: &str = stripped.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_before = true;
            continue;
        }
        let blank_above = blank_before;
        blank_before = false;
        let lower = trimmed.to_ascii_lowercase();
        // Where the rows' descriptions start on this page (the first word after the date
        // and any amount or reference): a "header" printed at that column, straight after
        // a row, is the row's description continuing.
        let indent = line.len() - line.trim_start().len();
        if trimmed.split_whitespace().next().and_then(parse_date_token).is_some() {
            let word_at = trimmed.split_whitespace().skip(1).find(|t| t.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) && !is_amount_token(t));
            desc_indent = word_at.and_then(|w| trimmed.find(w)).map(|p| p + indent);
        }
        // (A bulleted title, "• Other Debits", is never a continuation, whatever its indent:
        // rows the two-column unfolding rewrote start their descriptions further left.)
        let bulleted = trimmed.starts_with('\u{2022}') || trimmed.starts_with("* ");
        let continuation_position = last_txn.is_some() && !bulleted && desc_indent.map(|d| d >= 4 && indent + 2 >= d).unwrap_or(false);
        // (Flat OCR: a line of three or more words at the margin straight under a row, with
        // no blank line between and not one of the banks' section phrases, is the row's
        // wrapped description, "PAYMENT ELEKTA INC DEPOSIT" under a wire, not a section.)
        // (Two or more words outside the banks' heading vocabulary make it a wrap: "Other
        // withdrawals, debits and service charges" has none.)
        const HEADING_WORDS: &[&str] = &["deposits", "deposit", "credits", "credit", "debits", "debit", "withdrawals", "withdrawal", "checks", "check", "other", "and", "additions", "subtractions", "electronic", "paid", "fees", "fee", "charges", "charge", "service", "activity", "transactions", "transaction", "summary", "of", "the", "date", "amount", "description", "serial", "no", "number", "balance", "daily", "posted", "cleared", "atm", "card", "purchases", "payments", "payment", "items", "returned", "wire", "transfers", "ach", "for", "in", "out", "detail", "account", "history", "continued", "misc", "miscellaneous", "cash", "mobile", "online", "banking", "bill", "pay", "total", "all"];
        // (Counted before the first section word: a heading leads with its word, "Banking/
        // Debit Card Withdrawals ..."; a wrap reaches it after the payee's name.)
        let flat_wrap = last_txn.is_some() && !bulleted && !blank_above && indent == 0 && trimmed.split_whitespace().count() >= 3 && !SECTION_PHRASES.iter().any(|(p, _)| lower.contains(p)) && {
            const SECTION_WORDS: &[&str] = &["deposit", "deposits", "credit", "credits", "debit", "debits", "withdrawal", "withdrawals", "checks", "fees", "charges", "payments", "subtractions", "additions"];
            let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
            let first_section = words.iter().position(|w| SECTION_WORDS.contains(w)).unwrap_or(words.len());
            words[..first_section].iter().filter(|w| !w.chars().all(|c| c.is_ascii_digit()) && !HEADING_WORDS.contains(w)).count() >= 2
        };
        if !st.dashed_dates && trimmed.split_whitespace().any(|t| t.contains('-') && t.len() >= 5 && parse_date_token(t).is_some()) {
            st.dashed_dates = true;
        }

        // Flushing Bank wraps its six summary labels over two lines, "Beginning  Interest
        // Service  Ending" over "Balance + Deposits + Paid - Withdrawals ~ Charge = Balance",
        // the figures beneath ("634,907.25  120,084.03  00  80,689.22  .00  674,302.06").
        {
            let words: Vec<&str> = lower.split_whitespace().collect();
            // ("Beginning Interest. i Service Ending" in a scan: specks and lone letters aside.)
            let words: Vec<&str> = words.iter().map(|w| w.trim_matches(|c: char| !c.is_ascii_alphabetic())).filter(|w| w.len() > 1).collect();
            if words == ["beginning", "interest", "service", "ending"] {
                pending_columns = vec!["beginning", "credits", "interest", "debits", "fees", "ending"];
                pending_has_checks = false;
                continue;
            }
        }
        // A lone "Beginning Balance" label only takes a line that is nothing but the
        // amount, or "as of 03/01/24   1,310,488.01" (a money market statement); anything
        // else is parsed as usual.
        let as_of_amount = {
            let t: Vec<&str> = trimmed.split_whitespace().collect();
            t.len() == 4 && t[0].eq_ignore_ascii_case("as") && t[1].eq_ignore_ascii_case("of") && parse_date_token(t[2]).is_some() && is_amount_token(t[3])
        };
        if pending_columns.len() == 1 && trimmed.split_whitespace().count() != 1 && !as_of_amount {
            pending_columns.clear();
        }
        if !pending_columns.is_empty() {
            // ("00" and ".00": a zero whose point the scan lost, see `lost_zero`.)
            // ("$114,633.19." with a speck after it, Flushing Bank in a scan: the figure.)
            let amounts: Vec<f64> = trimmed.split_whitespace().map(|t| if t.len() > 4 && (t.ends_with('.') || t.ends_with(',')) && is_amount_token(&t[..t.len() - 1]) { &t[..t.len() - 1] } else { t }).filter(|t| is_amount_token(t) || lost_zero(t)).filter_map(|t| if lost_zero(t) { Some(0.0) } else { parse_amount(t) }).collect();
            // Second header line ("balance  other credits  other debits  balance"): keep waiting.
            // So does an account name between the header and its figures (Navy Federal:
            // "Business Checking" over "7125242482  $17,360.42  $395,422.08 ...").
            // (Flushing Bank's second line runs to eleven tokens with its signs: "Balance +
            // Deposits + Paid - Withdrawals ~ Charge = Balance".)
            if amounts.is_empty() && trimmed.split_whitespace().count() <= 12 && ["balance", "credits", "debits", "other", "deposits", "withdrawals"].iter().any(|w| lower.contains(w)) {
                continue;
            }
            if amounts.is_empty() && trimmed.split_whitespace().count() <= 4 && !trimmed.chars().any(|c| c.is_ascii_digit()) {
                continue;
            }
            let columns = std::mem::take(&mut pending_columns);
            if amounts.len() >= columns.len() {
                for (label, value) in columns.iter().zip(amounts) {
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
                        "fees" => ledger.summary.fees_total.get_or_insert(value.abs()),
                        "interest" => ledger.summary.interest_total.get_or_insert(value.abs()),
                        _ => ledger.summary.ending_balance.get_or_insert(value),
                    };
                }
            }
            continue;
        }
        let labels = column_labels(&lower);
        // Fifth Third OCR: "06/01 Beginning Balance Checks" with "$95,550.90" on the next
        // line; Frost: "BALANCE LAST STATEMENT" / "BALANCE THIS STATEMENT" over the figure.
        // The label must be the line ("Balance Summary" or "Daily Ending Balance" are headings).
        let label_line = ["beginning balance", "balance last statement", "balance this statement", "ending balance"].iter().any(|k| lower.starts_with(k) || lower.split_once(' ').map(|(d, rest)| parse_date_token(d).is_some() && rest.starts_with(k)).unwrap_or(false));
        let lone_beginning = (labels == ["beginning"] || labels == ["ending"]) && label_line && !lower.contains("daily") && trimmed.split_whitespace().count() <= 6;
        // A transaction table header ("Date Check Number Description Deposits/Credits
        // Withdrawals/Debits Ending daily balance") names columns, not summary values.
        let table_header = lower.contains("description") || lower.contains("check number");
        if (labels.len() >= 2 || lone_beginning) && !table_header && !trimmed.split_whitespace().any(is_amount_token) {
            pending_columns = labels;
            pending_has_checks = lower.contains("check");
            continue;
        }

        // (Summary keys match on single-spaced text: a wide OCR layer prints "Deposits   &
        // Credit   +   135,188.00".)
        let squeezed_line = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        capture_summary(&squeezed_line.to_ascii_lowercase(), &squeezed_line, &mut ledger.summary, page);

        // A dated row whose amount lost its point behind a long reference ("06/14/23
        // Preencoded Deposit 0000000001 813108252112849 27725"): the row carries no other
        // figure and a reference of nine digits or more, so the tail is its amount in cents.
        let pointless_tail: Option<String> = {
            let t: Vec<&str> = trimmed.split_whitespace().collect();
            let n = t.len();
            let tail_ok = n >= 4 && (4..=7).contains(&t[n - 1].len()) && t[n - 1].chars().all(|c| c.is_ascii_digit()) && !t[n - 1].starts_with('0');
            if tail_ok && parse_date_token(t[0]).is_some() && !t.iter().any(|x| is_amount_token(x)) && t[1..n - 1].iter().any(|x| x.len() >= 9 && x.chars().all(|c| c.is_ascii_digit())) {
                let last = t[n - 1];
                let cut = trimmed.rfind(last).unwrap();
                Some(format!("{}{}.{}", &trimmed[..cut], &last[..last.len() - 2], &last[last.len() - 2..]))
            } else {
                None
            }
        };
        let trimmed: &str = pointless_tail.as_deref().unwrap_or(trimmed);
        // (Under an amount-and-balance header, a scan's figures that lost their commas and
        // points to spaces, "-6 789 84 25 409.88" for -6,789.84 and 25,409.88, are put back
        // together before the line is read.)
        let rejoined: String;
        let trimmed: &str = if st.amount_balance && trimmed.split_whitespace().next().and_then(parse_date_token).is_some() {
            match rejoin_split_figures(trimmed) {
                Some(r) => { rejoined = r; &rejoined }
                None => trimmed,
            }
        } else {
            trimmed
        };
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        // A table header names the column order (see `State::amount_first`).
        // (A check table header, "Date posted  Check number  Amount  Reference number" three
        // times over on a flat OCR page, ends an amount-first table that came before it.)
        // (PNC's margin summary runs into the header on a flat page: "Date Amount Description
        // withdrawals totaling $626.50." The header is the words before "totaling".)
        let header_part = lower.find(" totaling ").map(|p| &lower[..p]).unwrap_or(&lower);
        let header_amounts = header_part.split_whitespace().any(|t| is_amount_token(t));
        if !header_amounts && tokens.len() <= 24 && header_part.contains("date") && header_part.contains("amount") {
            if lower.contains("description") || lower.contains("detail") {
                let a = lower.find("amount").unwrap_or(usize::MAX);
                let d = lower.find("description").or_else(|| lower.find("detail")).unwrap_or(usize::MAX);
                st.amount_first = a < d;
            } else if lower.contains("check") || lower.contains("serial") || lower.contains("reference") {
                st.amount_first = false;
            }
        }
        // "COMMERCIAL INTEREST CHECKING (continued)" at the top of a page confirms the
        // section carried over from the page before: its rows follow it as if the header
        // were printed here (see `row_kind`).
        if st.section.is_some() && lower.contains("continued") && tokens.len() <= 12 && !tokens.iter().any(|t| is_amount_token(t)) {
            st.section_page = Some(page);
        }
        // Dated balance rows inside an activity table ("11/01/2025 Beginning Balance
        // $323.01", "11/30/2025 Ending Balance $323.02") are summary lines, not
        // transactions; the summary already took their figures.
        // (Under a running-balance table the row still feeds the daily balances below.)
        if tokens.len() <= 6 && columns.is_none() && !st.in_daily && tokens.first().and_then(|t| parse_date_token(t)).is_some() && is_balance_label(&lower) {
            // ("08-01  Beginning Balance  17,360.42" opens the running balance the rows
            // below chain from.)
            // (An ending balance closes a chain rather than opening one: the summary's
            // "08/27/2025 Ending Balance $202.05" must not be what the first row chains from.)
            if let Some(b) = tokens.last().filter(|t| is_amount_token(t)).and_then(|t| parse_amount(t)) {
                st.last_balance = if lower.contains("ending") || lower.contains("closing") { None } else { Some(b) };
            }
            last_txn = None;
            continue;
        }
        // (Right after a transaction the same words are a description continuation:
        // TD prints "CREDIT FUNDING," over "OVERDRAFT PROTECTION FROM".)
        // ("Images" / "Check Images" heads UMB's check image pages, "Images for Account ..." Citizens'; their captions repeat
        // the checks; it counts even right after a transaction.)
        let images_heading = tokens.len() <= 2 && (lower == "images" || lower == "check images" || lower == "deposit images") || lower.starts_with("image number ") || lower.starts_with("images for account");
        if images_heading {
            st.images_page = Some(page);
        }
        // ("Items returned unpaid" is a heading even right after a row: Wells prints it
        // straight under the "Summary of checks written" table.)
        // (BankNorth rules its heading: "-------- AUTOMATIC TRANSACTIONS ------ -  DEBITS  CREDITS",
        // a detailed re-listing of the rows already counted under "CHECKS / DEBITS".)
        let bare = lower.trim_start_matches(|c| c == '-' || c == ' ');
        let informational = INFORMATIONAL_HEADERS.iter().any(|h| bare.starts_with(h)) && (last_txn.is_none() || !lower.starts_with("overdraft protection"));
        let word_tokens = tokens.iter().filter(|t| !t.chars().all(|c| c == '-')).count();
        if images_heading || word_tokens <= 6 && informational {
            st.informational = true;
            columns = None;
            last_txn = None;
            continue;
        }
        // The rest of an image page is captions and stamp text ("CHECKING DEPOSIT" on the
        // image itself would otherwise open a deposits section).
        if st.images_page == Some(page) {
            continue;
        }
        // Chase commercial: "02/19  List Posted Items  Quantity 10  $17,924.58" under
        // Withdrawals and Debits restates the checks paid below it (its own "Total*" is
        // $0.00 and excludes it); the checks are the transactions.
        if lower.contains("list posted items") {
            last_txn = None;
            continue;
        }
        // Daily balance tables: a "Daily Balance" heading, or a header repeating "Date ...
        // balance" for several columns ("Date  Ledger balance  Date  Ledger balance").
        let has_amount = tokens.iter().any(|t| is_amount_token(t));
        // ("‘Date -Lediger balanée Bate: Ledger balance Dats: Ledger. batence", a scan of
        // PNC's three-column table: the misread "Date"s count, one "balance" is enough.)
        let date_like = lower.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| matches!(*w, "date" | "dale" | "data" | "dato" | "oate" | "datc" | "dats" | "bate")).count();
        let repeated_date_balance_header = !has_amount && (lower.matches("date").count() >= 2 || date_like >= 3) && lower.contains("balance") && tokens.len() <= 12;
        // A transaction table header naming a credit or debit column ("... Ending daily balance") is not a daily balance block.
        let hdr = Columns::labels(line);
        let names_txn_columns = hdr.credit.is_some() || hdr.debit.is_some();
        // Synovus heads its daily table "Balance Summary" over "Date Amount Date Amount".
        let balance_summary_heading = lower.starts_with("balance summ") && tokens.len() <= 3; // ("Balance Summa": the scan cut the word)
        // Court OCR breaks the heading's letters apart ("I DAIL y ENDING BALANCE I"): the
        // spaceless form still reads.
        let squashed: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        // ("it eersre= sess sssessseceessssc= Daily Bal ance Information s=es===ss====": a
        // scan's rule noise around the heading runs to a dozen tokens.)
        let smeared_daily = !has_amount && tokens.len() <= 12 && (squashed.contains("dailyendingbalance") || squashed.contains("dailybalance") || squashed.contains("dailyledgerbalance"));
        // (UMB heads its table "End of Day - Current Balance".)
        let end_of_day = lower.starts_with("end of day") && lower.contains("balance") && !has_amount;
        // (BMO's "CLOSING DAILY BALANCES AND DEBIT TOTALS" over "DATE  BALANCE  DEBITS" names a
        // debit column too, but a daily table has no description or amount column.)
        let txn_header = names_txn_columns && (lower.contains("description") || lower.contains("amount"));
        // A daily balance table whose heading the OCR lost or cut short (TD prints "DAILY
        // BALANCE SUMMARY" in pale green; Synovus' "Balance Summary" comes back as "Balance
        // Summa"): a line made only of date and amount pairs, two or more of them, is that
        // table. A transaction row always carries some description between its date and
        // its amount, so nothing else looks like this.
        let bare_pairs = !st.in_daily && tokens.len() >= 4 && tokens.len() % 2 == 0 && tokens.chunks(2).all(|p| parse_date_token(p[0]).is_some() && is_amount_token(p[1]));
        // (Chase's single-column daily table whose "DAILY ENDING BALANCE" heading the scan
        // lost: a bare "DATE  AMOUNT" header over two or more lines of a date and a figure
        // alone. Rows without any description are balances, not transactions.)
        // (Or the three-column "Date Amount ~ Date Amount Date Amount", a stray glyph between
        // the groups, whose columns the unfolding has already cut into such lines.)
        let bare_words: Vec<&str> = lower.split_whitespace().filter(|t| t.chars().count() > 1 || t.chars().all(|c| c.is_ascii_alphanumeric())).collect();
        let bare_daily_header = !st.in_daily && bare_words.len() >= 2 && bare_words.iter().all(|t| *t == "date" || *t == "amount") && bare_words.contains(&"date") && bare_words.contains(&"amount") && {
            let following = raw_lines[line_no + 1..].iter().filter(|l| !l.trim().is_empty()).take(2).filter(|l| {
                let t: Vec<&str> = l.split_whitespace().collect();
                t.len() == 2 && parse_date_token(t[0]).is_some() && is_amount_token(t[1])
            }).count();
            following >= 2
        };
        if !txn_header && (lower.contains("daily balance") || lower.contains("daily ending balance") || lower.contains("daily ledger balance") || repeated_date_balance_header || balance_summary_heading || smeared_daily || end_of_day || bare_pairs) || bare_daily_header {
            st.enter_table("daily balances");
            st.in_daily = true;
            last_txn = None;
            if !bare_pairs {
                continue;
            }
        }
        if !tokens.iter().any(|t| is_amount_token(t)) && lower.contains("deposits and other additions") && (lower.contains("checks and other deductions") || lower.contains("checks and other debits")) {
            st.two_box_summary = true;
        }
        // "Date  Description  Amount  Balance" (online printouts, credit unions): the page's
        // rows end in their amount and the balance after it. (Not consumed here: the column
        // logic below still keeps it as a pending header.)
        if !tokens.iter().any(|t| is_amount_token(t)) && tokens.len() <= 6 && lower.contains("date") && lower.contains("amount") && lower.contains("balance") && !["credit", "debit", "deposit", "withdrawal", "check"].iter().any(|w| lower.contains(w)) {
            st.amount_balance = true;
        }
        // Relay's export heads its rows "Name  Date  Status  Amount  Balance": the
        // description comes before the date (rows are read below).
        if !tokens.iter().any(|t| is_amount_token(t)) && tokens.len() <= 6 && lower.contains("status") && lower.contains("amount") && lower.contains("date") && lower.contains("name") {
            st.enter_table(trimmed);
            st.status_table = true;
            columns = None;
            last_txn = None;
            continue;
        }
        // Column header for a transaction table with separate credit/debit/balance columns,
        // possibly wrapped over two lines.
        {
            let has_amount = tokens.iter().any(|t| is_amount_token(t));
            let labels = Columns::labels(line);
            // ("Dale  Number  Description  Credits  Debits  balance": a scan's "Date".)
            let has_date = lower.contains("date") || lower.split_whitespace().any(|t| matches!(t, "dale" | "data" | "dato" | "oate" | "datc" | "dats"));
            if !has_amount && labels.count() >= 1 && tokens.len() <= 12 {
                // (SunTrust sets the section's name in the margin beside its column header,
                // "Deposits/   Date   Amount  Serial #  Description", with "Credits" beside
                // the first row: the words before "Date" name the section.)
                if let Some(k) = lower.find("date").map(|p| lower[..p].trim()).filter(|lead| !lead.is_empty()).and_then(section_for) {
                    st.section = Some(k);
                    st.section_page = Some(page);
                }
                let merged = pending_header.as_ref().map(|p| p.merge(&labels)).unwrap_or(labels.clone());
                if merged.is_complete(has_date) {
                    // A table with its own credit and debit columns is mixed: no section applies.
                    if merged.credit.is_some() && merged.debit.is_some() {
                        st.section = None;
                    }
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
                // A plain "Date  Description  Amount" header after a column table (Webster's
                // per-type lists under its running-balance table): the columns no longer apply.
                if !has_amount && has_date && lower.contains("amount") && tokens.len() <= 6 && columns.is_some() {
                    columns = None;
                    st.enter_table(&format!("amount list {:?}", st.section));
                    last_txn = None;
                    continue;
                }
            }
        }

        // (Right after a transaction, at the description's indent, the same words continue
        // the description: "BARCLAYCARD" over "US CREDITCARD 250313 1241539854".)
        // (A lone "Credit" straight after a row is the payee's name wrapping, "Credit One
        // Bank Payment" split over two lines, not a section.)
        let wrapped_word = last_txn.is_some() && tokens.len() == 1 && (lower == "credit" || lower == "debit");
        // The OCR model sometimes retells a table as prose and bullets: "The daily account
        // activity ... includes the following electronic deposits:" names the section of
        // the bullets that follow.
        // (Only a sentence that announces a list: a wire's continuation line ending in
        // "Ref: 3Rd Deposit 18305 Biscayne Imad:" is not one.)
        let lead_in = tokens.len() >= 5 && lower.ends_with(':') && !has_amount && lower.contains("following") && !continuation_position && {
            if lower.contains("deposit") || lower.contains("credit") { Some(Kind::Credit) }
            else if lower.contains("payment") || lower.contains("withdrawal") || lower.contains("debit") || lower.contains("check") { Some(Kind::Debit) }
            else { None }
        }.is_some();
        let lead_in_kind = if lower.contains("deposit") || lower.contains("credit") { Kind::Credit } else { Kind::Debit };
        // (A short title at the description's indent, "Other Debits" under a deposits list
        // whose wrapped lines sit at the same indent, is still the section, not a wrap.)
        let short_title = tokens.len() <= 3 && tokens.iter().all(|t| t.chars().all(|c| c.is_ascii_alphabetic() || c == '/')) && (lower.starts_with("other ") || lower.contains('/'));
        if let Some(k) = section_for(trimmed).filter(|_| (!continuation_position && !flat_wrap || short_title) && !wrapped_word).or(if lead_in { Some(lead_in_kind) } else { None }) {
            st.enter_table(trimmed);
            st.section = Some(k);
            st.section_page = Some(page);
            st.in_daily = false;
            last_txn = None;
            continue;
        }
        // Long check-table titles ("Summary of checks written (checks listed are also
        // displayed in the preceding Transaction history)") start a new listing too.
        // (A wide OCR layer spreads the words: "Checks        Paid          No. Checks: 15".)
        let squeezed_lower = lower.split_whitespace().collect::<Vec<_>>().join(" ");
        let lower = &squeezed_lower;
        // (TD's check table header "DATE  SERIAL NO.  AMOUNT" names a checks listing on its
        // own, for when the title above it is misread: "Checks Pald".)
        // (Not under a deposits title of this page: Huntington's "Date  Amount  Serial #  Type"
        // heads its deposits, the serial being the deposit slip's.)
        let serial_header = !has_amount && tokens.len() <= 8 && lower.contains("serial") && lower.contains("amount") && lower.contains("date") && !(st.section == Some(Kind::Credit) && st.section_page == Some(page));
        // (A doubled "Check  Date  Amount  Check  Date  Amount" header names a two-column
        // checks listing too, and "Checks listed in numerical order" is its title.)
        // (Three groups and the section's name beside them, "Checks  Check Amount Date
        // Check Amount Date  Check Amount Date", SunTrust, run to ten words.)
        let doubled_check_header = !has_amount && tokens.len() <= 12 && lower.matches("check").count() >= 2 && lower.matches("date").count() >= 2 && lower.matches("amount").count() >= 2;
        // (Chase names the listing by its first column, "Check No.  Description  Date Paid
        // Amount"; the word "Date" is printed on the line above the rest of the header.)
        let check_no_header = !has_amount && tokens.len() <= 8 && (lower.starts_with("check no") || lower.starts_with("check number")) && lower.contains("amount");
        if serial_header || doubled_check_header || check_no_header || !has_amount && lower.contains("check") && (lower.starts_with("checks paid") || lower.starts_with("checks listed") || (lower.contains("summary of") || lower.contains("checks paid") || lower.contains("checks cleared") || lower.contains("checks written") || lower.contains("checks posted")) && tokens.len() <= 16) {
            st.enter_table(trimmed);
            st.section = Some(Kind::Debit);
            st.section_page = Some(page);
            st.in_daily = false;
            columns = None; // check rows are not under the transaction table's columns
            pending_flat = None; // a dated line held above the title was no row
            st.check_table = true;
            last_txn = None;
            continue;
        }
        // PNC heads a section with its total: "Funds Transfers Out   3 transactions for a
        // total of $299,103.30". The rows follow in the same table.
        if lower.contains("transactions for a total of") && !st.in_daily && !ledger.section_totals.iter().any(|(tb, _)| *tb == st.table) {
            if let Some(total) = lower.split("total of").nth(1).and_then(|rest| rest.split_whitespace().next()).filter(|t| is_amount_token(t)).and_then(parse_amount) {
                if total > 0.0 {
                    ledger.section_totals.push((st.table, total));
                }
            }
        }
        if lower.starts_with("total") || lower.starts_with("subtotal") || lower.starts_with("minimum balance") || lower.contains("continued on") {
            // "Total checks = $3,130.00" closing a section: one figure, rows listed above it
            // in the same table, the first such total for the table.
            let figures: Vec<f64> = tokens.iter().filter(|t| is_amount_token(t)).filter_map(|t| parse_amount(t)).collect();
            if lower.starts_with("total") && !lower.starts_with("totals") && figures.len() == 1 && figures[0] > 0.0 && !st.in_daily && ledger.transactions.iter().any(|t| t.table == st.table) && !ledger.section_totals.iter().any(|(tb, _)| *tb == st.table) {
                ledger.section_totals.push((st.table, figures[0].abs()));
                // (The words after "Total" and before the figure name the section.)
                let label = trimmed.split_whitespace().skip(1).take_while(|t| !is_amount_token(t) && *t != "=" && *t != "$").collect::<Vec<_>>().join(" ");
                if let Some(k) = section_for(&label) {
                    ledger.section_kinds.push((st.table, k));
                }
                // (The section is closed: rows after its total, under a header the scan
                // stripped of its title, are another section's.)
                st.enter_table(&format!("after {}", trimmed));
            }
            // PNC's two summary boxes squashed onto one line by a poor text layer: "Deposits
            // and Other Additions   Checks and Other Deductions" over the categories, then
            // "Total  400,378.67  Total  6  388,717.00": the credits' total, then the item
            // count and the debits' total. These outrank a category taken for the total.
            if st.two_box_summary && lower.matches("total").count() == 2 && !lower.contains("balance") {
                let mut halves = lower.splitn(3, "total").skip(1);
                let amount_in = |part: &str| part.split_whitespace().filter(|t| is_amount_token(t) && t.contains('.')).last().and_then(parse_amount).map(f64::abs);
                if let (Some(left), Some(right)) = (halves.next(), halves.next()) {
                    if let (Some(c), Some(d)) = (amount_in(left), amount_in(right)) {
                        ledger.summary.total_credits = Some(c);
                        ledger.summary.total_debits = Some(d);
                        ledger.summary.debits_key = "two-box summary (checks and service fees included)";
                    }
                }
                st.two_box_summary = false;
            }
            // "Totals  $62,461.80  $66,931.38" under a credit/debit column header (Citi:
            // "Total Debits/Credits  2,022.00  907.75").
            if let Some(c) = &columns {
                if lower.starts_with("totals") || lower.starts_with("total debits/credits") || lower.starts_with("total credits/debits") {
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
            // A row of the table whose dates the OCR garbled ("I912 117,075.89 og/21
            // 173,718:29 O9/29 156,906.18", Tesseract on a Chase page): figures with no
            // word among them is still the table, not a transaction. Its balances are lost.
            // (Or a month name read as another word, "gan 31 1,050,488.04 Feb 14 ...": four
            // letters in the whole line at most, where a transaction row has a description.)
            let garbled_row = tokens.iter().any(|t| is_amount_token(t)) && (tokens.iter().all(|t| t.chars().filter(|c| c.is_ascii_alphabetic()).count() <= 2) || trimmed.chars().filter(|c| c.is_ascii_alphabetic()).count() <= 4);
            if garbled_row {
                continue;
            }
            // Real content (an amount, or a long line) ends the daily balance block; short
            // header words ("Ledger", "Date Balance Date Balance") do not.
            // (Citizens prints the summary's right-hand column through the header: "Date
            // Balance  Date  Balance  Date  Balance   =   170,036.28" is still the header.)
            // (TriState rules its tables; Tesseract reads the rule as glyph noise with hardly
            // a letter or digit in it, "————E——EeEEE~_——&—_zx————>>>>": not content either.)
            // (A word is letters only, four or more, in one case or capitalised; "EeEEE"
            // and "eiiEEiEIEIEIEq" are what a rule reads as.)
            let symbol_run = |t: &str| t.chars().fold((0usize, 0usize), |(best, cur), c| if c.is_alphanumeric() { (best, 0) } else { (best.max(cur + 1), cur + 1) }).0 >= 3;
            let clean_word = |t: &str| {
                let n = t.chars().count();
                let rest: Vec<char> = t.chars().skip(1).collect();
                n >= 4 && t.chars().all(|c| c.is_alphabetic()) && (rest.iter().all(|c| c.is_lowercase()) || t.chars().all(|c| c.is_uppercase()))
            };
            let mixed_case = |t: &str| { let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect(); letters.len() >= 4 && letters.iter().skip(1).any(|c| c.is_uppercase()) && letters.iter().any(|c| c.is_lowercase()) };
            let noise = !tokens.is_empty() && !lower.chars().any(|c| c.is_ascii_digit()) && (tokens.iter().any(|t| symbol_run(t)) || tokens.iter().any(|t| mixed_case(t))) && !tokens.iter().any(|t| clean_word(t));
            let header_words = tokens.len() <= 6 && !tokens.iter().any(|t| is_amount_token(t)) || lower.matches("date").count() >= 2 && (lower.contains("balance") || lower.contains("amount")) || noise;
            // (A lone amount is a stray cell of the table, a doubled layer's second copy.)
            if tokens.len() == 1 && is_amount_token(tokens[0]) {
                continue;
            }
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
            // (A dated line held for its amount is no row once a complete row follows it:
            // the page header "August 31, 2019" above the table must not take the amount of
            // a later row that lost its date.)
            if starts_with_date && tokens.iter().any(|t| is_amount_token(t)) {
                pending_flat = None;
            }
            let aligned = !flat && line.contains("   ");
            // On an aligned line only the trailing run of amounts is in the columns; an amount
            // inside the description ("ACH Pmt ... $2,300.00 Usd, ID: E9F3E6   1,863.00") is text.
            let mut spans: Vec<(usize, &str)> = amount_spans(line);
            if aligned {
                let trailing = tokens.iter().rev().take_while(|t| is_amount_token(t)).count();
                spans = spans.split_off(spans.len().saturating_sub(trailing));
            }
            // "08/01/2025  BEGINNING BALANCE  $9,500.00" opens M&T's table as a dated row; with
            // the balance column's header smeared ("BALANr.E") its figure would land in the
            // debit column. A balance row's only figure is its running balance.
            let balance_row = { let d = tokens.iter().skip(1).take_while(|t| !is_amount_token(t)).map(|t| t.to_ascii_lowercase()).collect::<Vec<_>>().join(" "); ["beginning balance", "ending balance", "balance forward", "previous balance"].contains(&d.as_str()) };
            if starts_with_date && !spans.is_empty() {
                let mut txn: Option<(f64, Kind, bool, usize)> = None; // amount, kind, strong, desc end
                let mut running: Option<f64> = None;
                let mut repaired_from_balance = false;
                if aligned {
                    for (end, tok) in spans.iter().filter(|(end, _)| !c.before_columns(*end)) {
                        match c.kind_at(*end) {
                            _ if balance_row => running = parse_amount(tok),
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
                    // (A balance row's one figure is the running balance: "07/28/2025 Beginning
                    // Balance $260.76" opens the chain on a flat page too.)
                    let (amount_idx, bal) = if trailing >= 2 && c.balance.is_some() {
                        (tokens.len() - 2, parse_amount(tokens[tokens.len() - 1]))
                    } else if balance_row && c.balance.is_some() && !lower.contains("ending") && !lower.contains("closing") {
                        (tokens.len() - 1, parse_amount(tokens[tokens.len() - 1]))
                    } else {
                        (tokens.len() - 1, None)
                    };
                    running = bal;
                    let desc = tokens[1..amount_idx].join(" ");
                    // (A leading "+" marks a credit here too, as KeyBank prints and as the
                    // folded field lists write their credit cells.)
                    let (kind, strong) = if tokens[amount_idx].starts_with('+') {
                        (Kind::Credit, true)
                    } else if signed_amounts {
                        (if parse_amount(tokens[amount_idx]).unwrap_or(0.0) < 0.0 { Kind::Debit } else { Kind::Credit }, true)
                    } else {
                        st.row_kind(page, &desc)
                    };
                    let desc_end = line.find(tokens[amount_idx]).unwrap_or(line.len());
                    // "11/01/2025 Beginning Balance $323.01" in the activity table: a
                    // balance row, not a transaction (its running balance still counts).
                    txn = if is_balance_label(&desc.to_ascii_lowercase()) { None } else { parse_amount(tokens[amount_idx]).map(|v| (v.abs(), kind, strong, desc_end)) };
                    // On a flat page the balance arithmetic outranks the words: "OLB XFER FR
                    // DDA ... $90.00 $350.76" after a $260.76 balance is a credit whatever
                    // "XFER" suggests.
                    // (Both only when no unbalanced rows sit between this one and the last
                    // balance: with rows in the open group the change belongs to all of them.)
                    let direct = st.open_group.is_empty();
                    if let (Some((v, _, _, end)), Some(bal), Some(prev), true) = (txn, bal, st.last_balance, direct) {
                        if (prev + v - bal).abs() < 0.005 && v > 0.0 {
                            txn = Some((v, Kind::Credit, true, end));
                        } else if (prev - v - bal).abs() < 0.005 && v > 0.0 {
                            txn = Some((v, Kind::Debit, true, end));
                        }
                    }
                    // One digit misread in the amount ("7,975.97" where the balances fell by
                    // 7,575.97, a scan's 5 read as 9): when the change has the same digits but
                    // one, the change is the amount. See the flat rule below for the same repair.
                    if let (Some((v, k, strong, end)), Some(bal), Some(prev), true) = (txn, bal, st.last_balance, direct) {
                        let change = ((bal - prev).abs() * 100.0).round() / 100.0;
                        let (cc, tc) = (format!("{}", (change * 100.0).round() as i64), format!("{}", (v * 100.0).round() as i64));
                        if change > 0.0 && (change - v).abs() > 0.005 && cc.len() == tc.len() && cc.chars().zip(tc.chars()).filter(|(a, b)| a != b).count() == 1 {
                            txn = Some((change, k, strong, end));
                            repaired_from_balance = true;
                        }
                    }
                }
                // A cell the text layer garbled ("M,000.00") next to a readable running
                // balance: the amount is the balance change, when the change lands in
                // that cell's column. The row says so in its description.
                let mut from_balance = repaired_from_balance;
                if let (None, Some(bal), true) = (txn, running, aligned) {
                    if let (Some((end, tok)), Some(prev)) = (garbled_amount_span(line), st.last_balance) {
                        let diff = bal - prev;
                        if let Some(k) = c.kind_at(end) {
                            if diff.abs() >= 0.005 && (diff > 0.0) == (k == Kind::Credit) && !c.before_columns(end) {
                                txn = Some(((diff * 100.0).round().abs() / 100.0, k, true, line.find(tok).unwrap_or(line.len())));
                                from_balance = true;
                            }
                        }
                    }
                    // An amount cell blacked out by the court (Gulf Coast, "04/09/2025  Square
                    // Inc SQ250409  [box]  $715,889.24") or dropped by the text layer: the
                    // balance change is the amount, its sign the kind.
                    // (Only in a table with a balance column beside debit or credit columns:
                    // a per-type list's "Amount" is not a balance.)
                    let balance_table = c.balance.is_some() && (c.debit.is_some() || c.credit.is_some());
                    if let (None, Some(prev), false, true) = (txn, st.last_balance, balance_row, balance_table) {
                        let diff = bal - prev;
                        let worded = tokens[1..].iter().any(|t| t.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 3);
                        if diff.abs() >= 0.005 && worded {
                            let k = if diff > 0.0 { Kind::Credit } else { Kind::Debit };
                            txn = Some(((diff * 100.0).round().abs() / 100.0, k, true, line.find(spans[0].1).unwrap_or(line.len())));
                            from_balance = true;
                        }
                    }
                }
                let (date, day) = resolve_date(tokens[0], year_hint);
                if let Some((amount, kind, strong, desc_end)) = txn {
                    let mut desc: String = line[..desc_end].split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
                    // A transaction type that ends in the word itself ("Wire Transfer Debit",
                    // "Wire Transfer Credit", "ACH Credit") outranks the column an OCR page put
                    // it in. (Not a payee's name wrapping there: "... $30.00 Credit" One Bank.)
                    let words: Vec<String> = desc.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
                    let typed = words.len() >= 2 && ["transfer", "wire", "ach", "electronic", "misc", "miscellaneous", "book"].contains(&words[words.len() - 2].as_str());
                    let kind = match words.last().map(String::as_str) {
                        Some("debit") if typed => Kind::Debit,
                        Some("credit") if typed => Kind::Credit,
                        _ => kind,
                    };
                    if from_balance {
                        desc = format!("{desc} (amount read from the running balance)").trim().to_string();
                    }
                    if let Some(lead) = pending_lead.take() {
                        desc = format!("{lead} {desc}").trim().to_string();
                    }
                    if desc.is_empty() {
                        if let Some(lead) = lead_desc.take() {
                            desc = lead;
                        }
                    }
                    let id = ledger.transactions.len();
                    ledger.transactions.push(Txn { id, date: date.clone(), day, kind, amount, description: desc, page, table: st.table });
                    st.open_group.push((id, strong));
                    if !strong {
                        ledger.weak.push(id);
                    }
                    last_txn = Some(id);
                } else {
                    last_txn = None; // balance-only row ("Beginning Balance")
                }
                if let Some(bal) = running {
                    if c.balance.is_some() {
                        if let Some(prev) = st.last_balance.or(ledger.summary.beginning_balance).filter(|_| !newest_first) {
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
            // An undated line whose single amount sits in a credit or debit column, right
            // after a dated row on this page, is the next row with the same date (a court
            // scan's text layer dropped the date: ",J   PIN THE HOME DEPOT ...   11.94").
            // (With the running balance after the amount too: "XX2823DDAPOSCREDITSP ...
            // $59.08   $15,013.36" in a layer that printed the row's date on the line below.)
            let with_balance = spans.len() == 2 && c.balance.is_some() && c.kind_at(spans[1].0).is_none();
            if aligned && !starts_with_date && (spans.len() == 1 || with_balance) && tokens.len() >= 3 {
                let summary_like = lower.contains("total") || lower.contains("balance") || lower.contains("subtotal");
                let prev = last_txn.map(|id| ledger.transactions[id].clone()).filter(|t| t.page == page);
                if let (Some(prev), Some(kind), false) = (prev, c.kind_at(spans[0].0), summary_like) {
                    if !c.before_columns(spans[0].0) {
                        let desc_end = line.find(spans[0].1).unwrap_or(line.len());
                        let desc: String = line[..desc_end].split_whitespace().filter(|t| t.len() > 2 || t.chars().all(|ch| ch.is_ascii_alphanumeric())).collect::<Vec<_>>().join(" ");
                        let amount = parse_amount(spans[0].1).unwrap_or(0.0).abs();
                        let id = ledger.transactions.len();
                        ledger.transactions.push(Txn { id, date: prev.date.clone(), day: prev.day, kind, amount, description: desc, page, table: st.table });
                        if with_balance {
                            if let Some(bal) = parse_amount(spans[1].1) {
                                st.last_balance = Some(bal);
                            }
                        }
                        last_txn = Some(id);
                        continue;
                    }
                }
            }
        }

        // Image captions ("Regular Deposit  Date: 12/04  Amount: $2,364.21") repeat items
        // already listed; they are not transactions. Yampa Valley captions deposit and
        // withdrawal slips "#0000  04/02/2025  $22,000.00": no check number, same rule.
        let zero_caption = tokens.first().map(|t| t.len() >= 3 && t.starts_with('#') && t[1..].chars().all(|c| c == '0')).unwrap_or(false);
        if lower.contains("date:") && lower.contains("amount:") || zero_caption && tokens.len() >= 3 {
            last_txn = None;
            continue;
        }

        // Multi-column check tables: two or more (date, amount) pairs on one line, in either
        // "date check# amount" or "check# date amount" order.
        let date_idx: Vec<usize> = tokens.iter().enumerate().filter(|(_, t)| parse_date_token(t).is_some()).map(|(i, _)| i).collect();
        let amt_idx: Vec<usize> = tokens.iter().enumerate().filter(|(_, t)| is_amount_token(t)).map(|(i, _)| i).collect();
        // Between each date and its amount there is at most a check number and a gap marker;
        // prose there ("Fee period 11/01 - 11/30 ... $5.00") means this is not a check table.
        // Prose between the pairs ("NSF Return Item Fee for a Transaction Received on 12/29
        // $23,530.00") means one row quoting another transaction, not a check table.
        let check_table_shape = date_idx.iter().zip(&amt_idx).all(|(d, a)| d < a && a - d <= 3)
            && date_idx.windows(2).zip(&amt_idx).all(|(w, a)| w[1] <= a + 3);
        // (Never inside a daily balance table: its garbled row "gan 31 1,050,488.04 Feb 14
        // 1,3129,847.30" is no check 31.)
        if date_idx.len() >= 2 && date_idx.len() == amt_idx.len() && check_table_shape && !st.amount_first && !st.in_daily {
            let mut prev_end = 0usize;
            let mut seen_on_line: Vec<(String, f64)> = Vec::new();
            for (&d, &a) in date_idx.iter().zip(&amt_idx) {
                let mut desc: Vec<&str> = tokens[d + 1..a].to_vec();
                // A check number printed just before the date belongs to this entry.
                // (Reference numbers are longer; check numbers have at most seven digits.)
                let n = check_no(tokens[d.saturating_sub(1)]);
                if d > prev_end && d >= 1 && !n.is_empty() && n.len() <= 8 && n.chars().all(|c| c.is_ascii_digit()) {
                    desc.insert(0, tokens[d - 1]);
                }
                let desc: Vec<&str> = desc.into_iter().filter(|t| *t != "*").collect();
                // (Under a deposits section the number is a deposit ticket reference: U.S.
                // Bank's "Customer Deposits" table "Apr 7  8356110329  37,409.66".)
                let label = if desc.len() == 1 && check_no(desc[0]).chars().all(|c| c.is_ascii_digit()) {
                    if st.section == Some(Kind::Credit) && st.section_page == Some(page) { format!("Deposit {}", check_no(desc[0])) } else { format!("Check {}", check_no(desc[0])) }
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
                // A check number entry is a paid check, whatever section the page was in.
                let kind = if label.starts_with("Check ") { Kind::Debit } else { st.section.unwrap_or(Kind::Debit) };
                ledger.transactions.push(Txn { id, date, day, kind, amount, description: label, page, table: st.table });
            }
            last_txn = None;
            continue;
        }

        // The same table in "number amount date" order: "285 733.58 05/03 290 700.00 05/05".
        let number_amount_date = date_idx.len() >= 1 && date_idx.len() == amt_idx.len()
            && amt_idx.iter().zip(&date_idx).all(|(a, d)| a < d && d - a == 1 && *a >= 1 && check_no(tokens[a - 1]).chars().all(|c| c.is_ascii_digit()) && check_no(tokens[a - 1]).len() <= 7 && !check_no(tokens[a - 1]).is_empty());
        if number_amount_date && (date_idx.len() >= 2 || st.section == Some(Kind::Debit)) && !st.in_daily {
            for (&a, &d) in amt_idx.iter().zip(&date_idx) {
                let id = ledger.transactions.len();
                let (date, day) = resolve_date(tokens[d], year_hint);
                ledger.transactions.push(Txn { id, date, day, kind: Kind::Debit, amount: parse_amount(tokens[a]).unwrap_or(0.0).abs(), description: format!("Check {}", check_no(tokens[a - 1])), page, table: st.table });
            }
            last_txn = None;
            continue;
        }

        // Lone check entry "365989* 11/17 20,754.66" (Chase prints the gap marker as its own
        // token: "2846 * 10/18 5,821.77").
        // U.S. Bank adds a reference number and, in OCR, the rest of a two-column line:
        // "5001 Nov 26 8651583986 113.19 Conventional Checks Paid (2) $1,495.73-".
        let mut no_star: Vec<&str> = tokens.iter().copied().filter(|t| *t != "*").collect();
        // (A scan reads Chase's gap and electronic-check marks as stray glyphs between the
        // check number and its date: "9126 4“ 10/20 568.31", "9129 A 10/22 2,714.82"; a
        // token of one or two characters there is such a mark. "6909 “ 10/28 10/28 $2,500.00"
        // then repeats the date.)
        // (Up to three such marks in a row: "60541 A” a —_ 01/26 984.41".)
        for _ in 0..3 {
            if no_star.len() >= 4 && no_star[1].chars().count() <= 2 && !check_no(no_star[0]).is_empty() && check_no(no_star[0]).chars().all(|c| c.is_ascii_digit()) && parse_date_token(no_star[1]).is_none() && (parse_date_token(no_star[2]).is_some() || no_star.len() >= 5 && no_star[2].chars().count() <= 2 && parse_date_token(no_star[2]).is_none()) {
                no_star.remove(1);
            } else {
                break;
            }
        }
        if no_star.len() == 4 && no_star[1] == no_star[2] && parse_date_token(no_star[1]).is_some() && is_amount_token(no_star[3]) {
            no_star.remove(2);
        }
        if no_star.len() >= 4 && no_star[2].len() >= 9 && no_star[2].chars().all(|c| c.is_ascii_digit()) && is_amount_token(no_star[3]) && !check_no(no_star[0]).is_empty() && check_no(no_star[0]).chars().all(|c| c.is_ascii_digit()) && parse_date_token(no_star[1]).is_some() {
            no_star = vec![no_star[0], no_star[1], no_star[3]];
        }
        // UMB: "129  Mar 04  1,500.00  00081094018", the reference after the amount (one
        // or two digit groups); a further amount would make it a two-column line, left alone.
        if no_star.len() >= 4 && no_star.len() <= 5 && is_amount_token(no_star[2]) && no_star[3..].iter().all(|t| t.chars().all(|c| c.is_ascii_digit())) && no_star[3..].iter().map(|t| t.len()).sum::<usize>() >= 9 && !check_no(no_star[0]).is_empty() && check_no(no_star[0]).chars().all(|c| c.is_ascii_digit()) && parse_date_token(no_star[1]).is_some() {
            no_star = vec![no_star[0], no_star[1], no_star[2]];
        }
        if no_star.len() == 3 && !st.in_daily && !check_no(no_star[0]).is_empty() && check_no(no_star[0]).chars().all(|c| c.is_ascii_digit()) && parse_date_token(no_star[1]).is_some() && is_amount_token(no_star[2]) {
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(no_star[1], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind: Kind::Debit, amount: parse_amount(no_star[2]).unwrap_or(0.0).abs(), description: format!("Check {}", check_no(no_star[0])), page, table: st.table });
            last_txn = None;
            continue;
        }

        // Relay's export rows under "Name  Date  Status  Amount  Balance" (header taken
        // above): "BUSINESS CHECKING I (4875)  05/18/2026  Settled  -$150.00  —  $0.00", the
        // month name already rewritten. The sign is the kind; pending items are not posted.
        if st.status_table {
            if let Some(d) = tokens.iter().position(|t| parse_date_token(t).is_some()).filter(|d| *d >= 1) {
                let status = tokens.get(d + 1).map(|t| t.to_ascii_lowercase()).unwrap_or_default();
                let amount_tok = tokens.get(d + 2).copied().filter(|t| is_amount_token(t));
                if let (true, Some(a)) = (status == "settled" || status == "completed" || status == "posted", amount_tok) {
                    let negative = a.starts_with('-') || a.starts_with("$-") || a.starts_with('(') || a.ends_with('-');
                    let amount = parse_amount(a).unwrap_or(0.0).abs();
                    // (A scan's margin mark before the name, "g BUSINESS CHECKING", goes.)
                    let desc = tokens[..d].iter().copied().skip_while(|t| t.len() <= 2 && !t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())).collect::<Vec<_>>().join(" ");
                    let id = ledger.transactions.len();
                    let (date, day) = resolve_date(tokens[d], year_hint);
                    ledger.transactions.push(Txn { id, date, day, kind: if negative { Kind::Debit } else { Kind::Credit }, amount, description: desc, page, table: st.table });
                    last_txn = Some(id);
                    continue;
                }
                if status == "pending" {
                    last_txn = None;
                    continue;
                }
            }
        }

        // Transaction line: date first, amount last.
        let starts_with_date = tokens.first().and_then(|t| parse_date_token(t)).is_some();
        let ends_with_amount = tokens.last().map(|t| is_amount_token(t)).unwrap_or(false);

        // National City heads its checks "Check Number  Amount  Description  Date Paid": the
        // number first, then the amount, the date last.
        if !tokens.iter().any(|t| is_amount_token(t)) && tokens.len() <= 8 && lower.starts_with("check number") && lower.contains("amount") && lower.contains("date") {
            st.enter_table(trimmed);
            st.check_table = true;
            st.section = Some(Kind::Debit);
            columns = None;
            last_txn = None;
            continue;
        }
        // (A scan's mark before the number, ": 1031 1,528.00", is skipped.)
        let ct: &[&str] = if tokens.len() >= 3 && tokens[0].chars().count() == 1 && !tokens[0].chars().all(|c| c.is_alphanumeric()) { &tokens[1..] } else { &tokens[..] };
        // (In a scan the number may carry one misread letter, "f040*" for 1040, and the
        // amount may have lost its point, "132850" for 1,328.50: a check row's second cell
        // is its amount, and a run of five to seven digits there ends in the cents.)
        let serial_ok = |n: &str| (3..=7).contains(&n.len()) && n.chars().filter(|c| c.is_ascii_digit()).count() + 1 >= n.len() && n.chars().all(|c| c.is_ascii_alphanumeric()) && n.chars().any(|c| c.is_ascii_digit());
        // (Never a leading zero: "0147240" in a mail-sort line, "48689 0147240 0003-0003
        // TWS380WB123023091540", is a reference, not $1,472.40.)
        let pointless = |t: &str| (5..=7).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit()) && !t.starts_with('0');
        // (Chase prints the date written on the check in the description column when it
        // differs from the date paid, "1979  12/20  12/22  355.40": only dates stand
        // between the number and the amount, and the date paid is the last of them.)
        let dated_check_row = ct.len() >= 3 && is_amount_token(ct[ct.len() - 1]) && ct[1..ct.len() - 1].iter().all(|t| parse_date_token(t).is_some());
        if st.check_table && !starts_with_date && ct.len() >= 2 && (is_amount_token(ct[1]) || pointless(ct[1]) && ct.len() >= 3 || dated_check_row) && serial_ok(check_no(ct[0])) {
            let serial = check_no(ct[0]);
            let amt = if is_amount_token(ct[1]) || pointless(ct[1]) { 1 } else { ct.len() - 1 };
            let date_tok = tokens.iter().rev().find(|t| parse_date_token(t).is_some()).map(|t| t.to_string());
            let prev = ledger.transactions.iter().rev().find(|t| t.table == st.table && t.page == page).map(|t| (t.date.clone(), t.day));
            if let Some((date, day)) = date_tok.map(|d| resolve_date(&d, year_hint)).or(prev) {
                let cell = if pointless(ct[amt]) { format!("{}.{}", &ct[amt][..ct[amt].len() - 2], &ct[amt][ct[amt].len() - 2..]) } else { ct[amt].to_string() };
                let amount = parse_amount(&cell).unwrap_or(0.0).abs();
                let id = ledger.transactions.len();
                ledger.transactions.push(Txn { id, date, day, kind: Kind::Debit, amount, description: format!("Check {serial}"), page, table: st.table });
                last_txn = None;
                continue;
            }
        }
        // A check-table row whose date the scan garbled ("1oys1s 2655 2,014.24"): the check
        // number and amount are clean, so the check is listed, dated like the row before it
        // in the table (the date is the only cell lost).
        if st.check_table && flat && !starts_with_date && tokens.len() == 3 && ends_with_amount {
            let garbled = tokens[0].len() >= 3 && tokens[0].len() <= 8 && tokens[0].chars().any(|c| c.is_ascii_digit()) && tokens[0].chars().any(|c| c.is_ascii_alphabetic()) && tokens[0].chars().all(|c| c.is_ascii_alphanumeric() || c == '/');
            let serial = check_no(tokens[1]);
            if garbled && !serial.is_empty() && serial.len() <= 7 && serial.chars().all(|c| c.is_ascii_digit()) {
                if let Some(prev) = ledger.transactions.iter().rev().find(|t| t.table == st.table && t.page == page) {
                    let (date, day) = (prev.date.clone(), prev.day);
                    let amount = parse_amount(tokens[2]).unwrap_or(0.0).abs();
                    let id = ledger.transactions.len();
                    ledger.transactions.push(Txn { id, date, day, kind: Kind::Debit, amount, description: format!("Check {serial}"), page, table: st.table });
                    last_txn = None;
                    continue;
                }
            }
        }

        // PNC corporate: "06/03  28,273.92  Corporate ACH Txns/Fees  00024155901130577" and
        // "06/21  12490  450.00  017261553": date first, exactly one amount, a reference
        // number last. The reference (nine or more digits) is dropped from the description.
        let amount_positions: Vec<usize> = (1..tokens.len()).filter(|&i| is_amount_token(tokens[i])).collect();
        // (Huntington's period beside its summary, "03/01/23 to 03/31/23   Credits (+)
        // 1,000.00", is a summary line too: two dates joined by "to" open no row.)
        let period_line = tokens.len() >= 3 && parse_date_token(tokens[0]).is_some() && matches!(tokens[1].to_ascii_lowercase().as_str(), "to" | "through" | "thru" | "-") && parse_date_token(tokens[2]).is_some();
        let summary_row = lower.contains("beginning balance") || lower.contains("ending balance") || lower.contains("previous balance") || lower.contains("balance forward") || period_line;
        // A headerless flat list whose rows end in amount then running balance (an OCR
        // page retold as bullets: "10/02 External Withdrawal ... 1,651.07 14,478.08"). The
        // second figure is a balance when it chains from the previous balance or into the
        // next row's; the change's sign then decides the kind.
        let trailing_amounts = tokens.iter().rev().take_while(|t| is_amount_token(t)).count();
        // (A row may have no description at all: "NOV 01   500.00   $130813.77".)
        if columns.is_none() && starts_with_date && trailing_amounts == 2 && tokens.len() >= 3 && !summary_row && !st.in_daily {
            let n = tokens.len();
            let (amount, balance) = (parse_amount(tokens[n - 2]).map(f64::abs), parse_amount(tokens[n - 1]));
            if let (Some(amount), Some(balance)) = (amount, balance) {
                let near = |x: f64, y: f64| (x - y).abs() < 0.005;
                // (Newest first, the previous row is the later one: its balance is this
                // row's balance plus a credit or less a debit.)
                let from_prev = st.last_balance.or(ledger.summary.beginning_balance).filter(|_| !newest_first).map(|p| if near(p + amount, balance) { Some(Kind::Credit) } else if near(p - amount, balance) { Some(Kind::Debit) } else { None });
                // (Newest first, the previous row is the later one: its balance is this
                // row's balance moved by its own amount. That keeps the shape for the last
                // row on a page; its kind comes from the sign or the words.)
                let prev_chains = newest_first && st.last_balance.zip(st.last_amount).map(|(p, a)| near(balance + a, p) || near(balance - a, p)).unwrap_or(false);
                // (amount, balance) of a later row in the same shape, if it has one.
                // (Blank lines between rows are skipped, and so are up to twelve undated
                // lines: the wire details Brookline prints under a row. k counts dated rows.)
                let row_at = |k: usize| -> Option<(f64, f64)> {
                    let mut idx = line_no + 1;
                    let mut seen = 0;
                    let mut skipped = 0;
                    let raw = loop {
                        let l = raw_lines.get(idx)?;
                        idx += 1;
                        if l.trim().is_empty() {
                            continue;
                        }
                        if l.split_whitespace().next().and_then(parse_date_token).is_none() {
                            skipped += 1;
                            if skipped > 12 {
                                return None;
                            }
                            continue;
                        }
                        seen += 1;
                        if seen == k - line_no {
                            break l;
                        }
                    };
                    let next = attach_trailing_sign(&join_split_amounts(&unbullet(&normalize_month_dates(&glue_dollar_sign(&raw.replace('|', " "))))));
                    let nt: Vec<&str> = next.split_whitespace().collect();
                    let m = nt.len();
                    if m >= 3 && parse_date_token(nt[0]).is_some() && is_amount_token(nt[m - 1]) && is_amount_token(nt[m - 2]) {
                        parse_amount(nt[m - 2]).map(f64::abs).zip(parse_amount(nt[m - 1]))
                    } else {
                        None
                    }
                };
                let chains = |b: f64, row: Option<(f64, f64)>| row.map(|(a2, b2)| near(b + a2, b2) || near(b - a2, b2)).unwrap_or(false);
                let into_next = chains(balance, row_at(line_no + 1));
                // An online listing newest first (CommunityAmerica's "Date Description Amount
                // Balance" printout): the next row's balance plus or minus this row's amount
                // is this row's balance, and the direction of the change is the kind.
                let back_kind = row_at(line_no + 1).and_then(|(_, b2)| if near(b2 + amount, balance) { Some(Kind::Credit) } else if near(b2 - amount, balance) { Some(Kind::Debit) } else { None }).filter(|_| !into_next);
                // One misread balance must not break the list: the two rows after this one
                // chaining to each other is enough to keep the shape.
                let shape_holds = row_at(line_no + 1).map(|(_, b2)| chains(b2, row_at(line_no + 2))).unwrap_or(false);
                // A figure that lost its leading digits to the text layer ("0000.00" where the
                // balance fell by 100,000.00, "5000.00" for 75,000.00): between two balances
                // that chain, when the change ends in the digits that survived, the change is
                // the amount, and the row says so.
                let mut from_balance = false;
                let change_cents = format!("{}", ((balance - st.last_balance.unwrap_or(balance)).abs() * 100.0).round() as i64);
                let token_cents = format!("{}", (amount * 100.0).round() as i64);
                let suffix = change_cents.len() > token_cents.len() && change_cents.ends_with(&token_cents);
                // One digit misread in the amount ("7,975.97" where the balances fell by
                // 7,575.97, a scan's 5 read as 9): the change has the same digits but one, and
                // the balance after the row still chains into the next, so the change is the
                // amount. The row says so.
                let one_digit_off = change_cents.len() == token_cents.len() && change_cents.chars().zip(token_cents.chars()).filter(|(a, b)| a != b).count() == 1;
                let (amount, from_prev) = match (from_prev.flatten(), st.last_balance, into_next || shape_holds) {
                    (None, Some(prev), true) if suffix && (balance - prev).abs() > amount + 0.005 => {
                        from_balance = true;
                        (((balance - prev).abs() * 100.0).round() / 100.0, Some(Some(if balance > prev { Kind::Credit } else { Kind::Debit })))
                    }
                    (None, Some(prev), true) if one_digit_off && into_next => {
                        from_balance = true;
                        (((balance - prev).abs() * 100.0).round() / 100.0, Some(Some(if balance > prev { Kind::Credit } else { Kind::Debit })))
                    }
                    _ => (amount, from_prev),
                };
                if from_prev.flatten().is_some() || (from_prev.flatten().is_none() && (into_next || shape_holds || back_kind.is_some() || prev_chains || st.amount_balance)) {
                    let mut desc = tokens[1..n - 2].join(" ");
                    if from_balance {
                        desc = format!("{desc} (amount read from the running balance)").trim().to_string();
                    }
                    // A signed amount ("20.00-", Navy Federal; "($695.00)", a bank verification
                    // report) names its own kind. Where the listing signs its debits, an unsigned
                    // amount is a credit whatever its words say ("... Credit Card  ($695.00)").
                    let t2 = tokens[n - 2];
                    let signed = if t2.ends_with('-') || t2.starts_with('-') || t2.starts_with('(') || t2.starts_with("$(") || t2.starts_with("$-") { Some(Kind::Debit) } else if t2.ends_with('+') || t2.starts_with('+') || signed_balance_page || signed_amounts { Some(Kind::Credit) } else { None };
                    // (Where the listing signs its debits the sign is certain; the balance change
                    // is only a witness, and on a newest-first printout it reads backwards.)
                    let kind = if signed_amounts { signed } else { from_prev.flatten().or(signed) }.or(back_kind).unwrap_or_else(|| st.row_kind(page, &desc).0);
                    let id = ledger.transactions.len();
                    let (date, day) = resolve_date(tokens[0], year_hint);
                    ledger.transactions.push(Txn { id, date: date.clone(), day, kind, amount, description: desc, page, table: st.table });
                    ledger.daily_balances.push(DailyBalance { date, balance });
                    st.last_balance = Some(balance);
                    st.last_amount = Some(amount);
                    last_txn = Some(id);
                    continue;
                }
            }
        }
        // An amount-first row whose figure lost its point to the scan ("03/11  39  8764 Debit
        // Card Purchase Amazon", PNC): a bare one or two digits in the amount slot are the
        // cents of a sub-dollar figure (.39), with the dollar reading (39.00) kept as the
        // alternate for the totals and daily balances to choose.
        if st.amount_first && flat && starts_with_date && amount_positions.is_empty() && tokens.len() >= 4 && !summary_row && tokens[1].len() <= 2 && tokens[1].chars().all(|c| c.is_ascii_digit()) && tokens[2..].iter().any(|t| t.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 3) {
            let figure: f64 = tokens[1].parse::<u32>().unwrap_or(0) as f64;
            if figure > 0.0 {
                let desc = tokens[2..].join(" ");
                let id = ledger.transactions.len();
                let (date, day) = resolve_date(tokens[0], year_hint);
                ledger.transactions.push(Txn { id, date, day, kind: st.row_kind(page, &desc).0, amount: figure / 100.0, description: desc, page, table: st.table });
                ledger.alternates.push((id, figure));
                last_txn = Some(id);
                continue;
            }
        }
        // A report row that ends in the dash it prints for an unrepeated balance may quote other
        // figures in its description ("Cash Svcs Db/Cr Dep Adjust, Org Dep Amt= 85,520.00 ...
        // ($38,150.00)  -"): the figure just before the dash is the row's own amount.
        let dashed_row = !ends_with_amount && tokens.len() >= 4 && matches!(tokens[tokens.len() - 1], "-" | "\u{2014}" | "\u{2013}") && is_amount_token(tokens[tokens.len() - 2]);
        if starts_with_date && !ends_with_amount && tokens.len() >= 3 && (amount_positions.len() == 1 || dashed_row || st.amount_first && amount_positions.first() == Some(&1)) && !summary_row {
            let a = if dashed_row { tokens.len() - 2 } else { amount_positions[0] };
            // (Stray marks, "=" and "*" around a check number in a scan, are not words.)
            let rest: Vec<&str> = tokens[1..].iter().enumerate().filter(|(i, t)| *i + 1 != a && !(t.len() >= 9 && t.chars().all(|c| c.is_ascii_digit())) && (t.chars().count() > 1 || t.chars().all(|c| c.is_alphanumeric()))).map(|(_, t)| *t).collect();
            let desc = if rest.len() == 1 && rest[0].len() <= 7 && rest[0].chars().all(|c| c.is_ascii_digit()) && rest[0].chars().any(|c| c != '0') { format!("Check {}", rest[0]) } else { rest.join(" ") };
            let figure = parse_amount(tokens[a]).unwrap_or(0.0);
            // In a column that signs its debits the sign is the kind, whatever the words say.
            let kind = if signed_amounts { if figure < 0.0 { Kind::Debit } else { Kind::Credit } } else { st.row_kind(page, &desc).0 };
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[0], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind, amount: figure.abs(), description: desc, page, table: st.table });
            last_txn = Some(id);
            continue;
        }
        if starts_with_date && ends_with_amount && tokens.len() >= 2 {
            // Amount-first tables ("12/30 35.00 NSF Return Item Fee for a Transaction Received
            // on 12/29 $23,530.00"): the amount is the one right after the date and the
            // trailing figure belongs to the description.
            let amount_first_row = st.amount_first && tokens.len() >= 4 && is_amount_token(tokens[1]) && !is_amount_token(tokens[2]);
            // On a page whose amount column signs its debits, a row that ends in two figures
            // ends in its running balance; the signed figure before it is the amount. (That
            // is how a bank verification report prints the last row of each day.)
            let signed_pair = signed_amounts && !amount_first_row && tokens.len() >= 4 && is_amount_token(tokens[tokens.len() - 2]) && !is_amount_token(tokens[tokens.len() - 3]);
            let amount_tok = if amount_first_row { tokens[1] } else if signed_pair { tokens[tokens.len() - 2] } else { tokens[tokens.len() - 1] };
            let amount = parse_amount(amount_tok).unwrap_or(0.0).abs();
            let sign_kind = signed_amounts.then(|| if parse_amount(amount_tok).unwrap_or(0.0) < 0.0 { Kind::Debit } else { Kind::Credit });
            // Statement summary rows also start with a date ("11/01/2025 Beginning Balance"); skip them.
            if summary_row {
                last_txn = None;
                continue;
            }
            let desc: String = if amount_first_row { tokens[2..].join(" ") } else if signed_pair { tokens[1..tokens.len() - 2].join(" ") } else { tokens[1..tokens.len() - 1].join(" ") };
            // "03/14 1008 212.26": a single check-table pair is a paid check.
            // (A scan's stray mark between the cells, "8/13 . 2429* 800.00", does not count.)
            let core: Vec<&str> = tokens.iter().copied().filter(|t| t.chars().count() > 1 || t.chars().all(|c| c.is_alphanumeric())).collect();
            let bare_check = core.len() == 3 && !check_no(core[1]).is_empty() && check_no(core[1]).len() <= 7 && check_no(core[1]).chars().all(|c| c.is_ascii_digit());
            // A leading '+' on the amount is a credit whatever the section. KeyBank prints a
            // waived fee as "+3.00" under Fees and charges right after the fee it cancels;
            // the bank nets the two ("Net fees and charges"), so both rows go.
            let plus = tokens[tokens.len() - 1].starts_with('+') || tokens[tokens.len() - 1].ends_with('+');
            if plus && st.section == Some(Kind::Debit) {
                let (date, _) = resolve_date(tokens[0], year_hint);
                if let Some(pos) = ledger.transactions.iter().rposition(|t| t.table == st.table && t.kind == Kind::Debit && (t.amount - amount).abs() < 0.005 && t.date == date) {
                    ledger.transactions.remove(pos);
                    for (i, t) in ledger.transactions.iter_mut().enumerate() {
                        t.id = i;
                    }
                    ledger.weak.retain(|&w| w != pos);
                    for w in &mut ledger.weak {
                        if *w > pos {
                            *w -= 1;
                        }
                    }
                    last_txn = None;
                    continue;
                }
            }
            // On a page that signs its debits ("$-500.00", Bluevine; "-$1,500.00", online
            // exports) the sign is the kind: unsigned rows are credits.
            let last_tok = tokens[tokens.len() - 1];
            let negative = last_tok.starts_with('-') || last_tok.starts_with("$-") || last_tok.starts_with("($") || last_tok.starts_with('(') || last_tok.ends_with('-');
            // An amount-first row ending in its running balance ("04/30  6.66  Accr Earning
            // Pymt Added to Account  45,004.42", First American): the balance change names
            // the kind and the balance leaves the description.
            let mut trailing_balance: Option<(f64, Kind)> = None;
            if let (true, Some(k), Some(balance)) = (signed_pair, sign_kind, parse_amount(tokens[tokens.len() - 1])) {
                trailing_balance = Some((balance, k));
            }
            if amount_first_row && tokens.len() >= 4 && amount_positions.len() == 2 && amount_positions[1] == tokens.len() - 1 {
                if let (Some(balance), Some(prev)) = (parse_amount(last_tok), st.last_balance.or(ledger.summary.beginning_balance)) {
                    if (prev + amount - balance).abs() < 0.005 {
                        trailing_balance = Some((balance, Kind::Credit));
                    } else if (prev - amount - balance).abs() < 0.005 {
                        trailing_balance = Some((balance, Kind::Debit));
                    }
                }
            }
            let desc = if trailing_balance.is_some() && !signed_pair { tokens[2..tokens.len() - 1].join(" ") } else { desc };
            let (desc, kind) = if let Some((_, k)) = trailing_balance {
                (desc, k)
            } else if bare_check {
                (format!("Check {}", check_no(core[1])), Kind::Debit)
            } else if plus {
                (desc, Kind::Credit)
            } else if let Some(k) = sign_kind {
                (desc, k)
            } else if signed_page && !st.amount_first {
                (desc, if negative { Kind::Debit } else { Kind::Credit })
            } else if last_tok.ends_with('-') && !st.amount_first {
                // A trailing minus is the bank's own debit marker ("FORD CREDIT AUTO PYMT
                // 745.26-", Community Bank), whatever the words say.
                (desc, Kind::Debit)
            } else {
                let (kind, strong) = st.row_kind(page, &desc);
                if !strong {
                    ledger.weak.push(ledger.transactions.len());
                }
                (desc, kind)
            };
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[0], year_hint);
            ledger.transactions.push(Txn { id, date: date.clone(), day, kind, amount, description: desc, page, table: st.table });
            if let Some((balance, _)) = trailing_balance {
                ledger.daily_balances.push(DailyBalance { date, balance });
                st.last_balance = Some(balance);
            }
            if bare_check {
                st.check_table = true;
            }
            last_txn = if bare_check { None } else { Some(id) };
            continue;
        }

        // OCR of a wrapped row: "03/04 CCD DEBIT, INTUIT ... BILL_PAY VRA CLEANING SE" then
        // "3,680.00" on the next line. Hold the dated line and complete it when a lone
        // amount follows (text lines in between extend the description).
        // (Aligned pages too, when no column table is open: a doubled text layer breaks
        // "06/04  Online Domestic Wire Transfer Via: ... $25,000.00" over several lines.)
        // On an aligned page only inside a transaction section, and never a statement
        // period line ("02/01/2025 through 02/28/2025").
        // (Or inside a running-balance list: Navy Federal wraps "08-28  Paid To - App Funding
        // Beta 9292549322 Chk 11409434" over "535.72 -   36,293.56".)
        // ("Paid To - App Funding Beta" is not a period line: that needs two dates.)
        let two_dates = tokens.iter().filter(|t| parse_date_token(t).is_some()).count() >= 2;
        let aligned_ok = !flat && columns.is_none() && (st.section.is_some() && st.section_page == Some(page) || st.last_balance.is_some()) && !(two_dates && (lower.contains("through") || lower.contains(" to ")));
        // (Wells' page header "May31,2021 • Page3of4" is dated but never a row.)
        if (flat || aligned_ok) && starts_with_date && !tokens.iter().any(|t| is_amount_token(t)) && tokens.len() >= 2 && !summary_row && !st.in_daily && footer_numbers(trimmed).is_none() {
            pending_flat = Some((tokens[0].to_string(), tokens[1..].join(" ")));
            pending_flat_lines = 0;
            last_txn = None;
            continue;
        }
        if let Some((date_tok, desc)) = pending_flat.take() {
            // A lone amount, or the rest of the description ending with the amount
            // (TD: "RESTAURANT DEPOT ALEXANDRIA * VA 142.29"). A balance label ends the wait.
            // (A section total, "Funds Transfer In   2 transactions for a total of
            // $2,000,000.00", never completes a row.)
            let ends_with_amount = !starts_with_date && tokens.len() <= 12 && tokens.last().map(|t| is_amount_token(t)).unwrap_or(false) && tokens[..tokens.len() - 1].iter().all(|t| !is_amount_token(t)) && !lower.contains("balance") && !lower.contains("total");
            // In an amount-first table (Citizens "Date  Item No.  Amount  Description") the
            // rest of the row is "024225011389664   1,255.00   FlrDecorProPrem ...": at most
            // one reference before the amount, the description after it.
            let amount_at = tokens.iter().position(|t| is_amount_token(t));
            let amount_first_row = st.amount_first && !starts_with_date && !ends_with_amount && amount_at.map(|a| a <= 1 && tokens.len() > a + 1).unwrap_or(false) && tokens.iter().filter(|t| is_amount_token(t)).count() == 1 && !lower.contains("balance");
            if ends_with_amount || amount_first_row {
                let a = if amount_first_row { amount_at.unwrap() } else { tokens.len() - 1 };
                let amount = parse_amount(tokens[a]).unwrap_or(0.0).abs();
                let rest: Vec<&str> = tokens.iter().enumerate().filter(|(i, _)| *i != a).map(|(_, t)| *t).collect();
                let desc = if rest.is_empty() { desc } else { format!("{desc} {}", rest.join(" ")) };
                let id = ledger.transactions.len();
                let (date, day) = resolve_date(&date_tok, year_hint);
                ledger.transactions.push(Txn { id, date, day, kind: st.row_kind(page, &desc).0, amount, description: desc, page, table: st.table });
                last_txn = Some(id);
                continue;
            }
            // Amount then running balance that chains from the last one: the change's sign
            // is the kind ("535.72-   36,293.56" after a balance of 36,829.28).
            let n = tokens.len();
            if !starts_with_date && n >= 2 && n <= 12 && is_amount_token(tokens[n - 1]) && is_amount_token(tokens[n - 2]) && tokens[..n - 2].iter().all(|t| !is_amount_token(t)) {
                if let (Some(prev), Some(amount), Some(balance)) = (st.last_balance, parse_amount(tokens[n - 2]).map(f64::abs), parse_amount(tokens[n - 1])) {
                    let kind = if (prev + amount - balance).abs() < 0.005 { Some(Kind::Credit) } else if (prev - amount - balance).abs() < 0.005 { Some(Kind::Debit) } else { None };
                    if let Some(kind) = kind {
                        let rest: Vec<&str> = tokens[..n - 2].iter().copied().filter(|t| t.chars().any(|c| c.is_alphanumeric())).collect();
                        let desc = if rest.is_empty() { desc } else { format!("{desc} {}", rest.join(" ")) };
                        let id = ledger.transactions.len();
                        let (date, day) = resolve_date(&date_tok, year_hint);
                        ledger.transactions.push(Txn { id, date: date.clone(), day, kind, amount, description: desc, page, table: st.table });
                        ledger.daily_balances.push(DailyBalance { date, balance });
                        st.last_balance = Some(balance);
                        last_txn = Some(id);
                        continue;
                    }
                }
            }
            // (A total's label between, "Total Debits" over "--  101,186.43" in a doubled
            // text layer, ends the wait: the figure below it is the total's.)
            // (And at most three lines of it: a figure four or more lines below the dated
            // line, "--  101,186.43" under a check table's footnote and total labels, is
            // not the row's.)
            if !starts_with_date && !tokens.iter().any(|t| is_amount_token(t)) && tokens.len() <= 12 && !lower.contains("total") && pending_flat_lines < 3 && !trimmed.starts_with('*') {
                pending_flat = Some((date_tok, format!("{desc} {trimmed}")));
                pending_flat_lines += 1;
                continue;
            }
            // Anything else: the dated line was not a transaction after all.
        }

        // Under a column table, text with no date that does not end in an amount may be the
        // description of the next dated row (kept until that row, or replaced).
        if columns.is_some() && !starts_with_date && !ends_with_amount && (3..=14).contains(&tokens.len()) {
            lead_desc = Some(trimmed.to_string());
        } else if starts_with_date {
            lead_desc = None;
        }
        // Continuation line: text right after a transaction with no date and no amount adds
        // to its description. Indentation is not required because OCR output has none.
        // A lone all-caps token with no digits is a page footer artifact, not a description.
        if let Some(id) = last_txn {
            let has_amount = tokens.iter().any(|t| is_amount_token(t));
            let footer_artifact = tokens.len() == 1 && tokens[0].len() >= 6 && tokens[0].chars().all(|c| c.is_ascii_uppercase());
            // ("Navy Federal Credit Union   9 of 36   12/12/2025": a page footer's "9 of 36".)
            let page_count = tokens.windows(3).any(|w| w[1] == "of" && w[0].chars().all(|c| c.is_ascii_digit()) && w[2].chars().all(|c| c.is_ascii_digit()));
            // (Huntington's signed section titles, "Other Debits (-)   Account:---1079", and
            // its "Balance Activity" heading are headers, whatever their indent.)
            let signed_title = (lower.contains(" (-)") || lower.contains(" (+)")) && !has_amount || lower.starts_with("balance activity");
            let boilerplate = lower.contains("member fdic") || lower.contains("page ") && lower.contains(" of ") || lower.starts_with("pg ") || page_count || signed_title;
            if !starts_with_date && !has_amount && tokens.len() <= 12 && !footer_artifact && !boilerplate {
                let t = &mut ledger.transactions[id];
                t.description.push(' ');
                t.description.push_str(trimmed);
                continue;
            }
            // A footer artifact or boilerplate ends the row's description: the lines after
            // it belong to the page, not to the row (Navy Federal prints a change-of-address
            // form under its "Items Paid" recap, and its field labels were being appended).
            if footer_artifact || boilerplate {
                last_txn = None;
            }
            // OCR sometimes drops the dates of the lower rows of a page ("PAYMENT Greystone
            // Power 7904 VitalPharmaceuticals 531.28" under dated rows). Inside a sectioned
            // list, text ending in a single amount right after a complete row is the next
            // row, dated like the one before it.
            let single_trailing_amount = tokens.len() >= 2 && tokens.len() <= 14 && is_amount_token(tokens[tokens.len() - 1]) && tokens[..tokens.len() - 1].iter().all(|t| !is_amount_token(t));
            let summary_like = lower.contains("total") || lower.contains("balance");
            // (A real row has a word in it; "MS 5 02 I 8.45" and "1 1.62" are OCR noise
            // between rows on a scan, not payments.)
            // (A word, not a smear: "OOOoreeOmOOqETH" switches case four times.)
            let gibberish = |t: &str| t.chars().filter(|c| c.is_ascii_alphabetic()).collect::<Vec<_>>().windows(2).filter(|w| w[0].is_ascii_uppercase() != w[1].is_ascii_uppercase()).count() >= 3;
            let letters = |t: &str| t.chars().filter(|c| c.is_ascii_alphabetic()).count();
            let real_words = tokens[..tokens.len() - 1].iter().filter(|t| letters(t) >= 3 && !gibberish(t)).count();
            let junk = tokens[..tokens.len() - 1].iter().filter(|t| t.len() == 1 || gibberish(t)).count();
            let has_word = real_words >= 1 && real_words >= junk;
            if flat && !starts_with_date && single_trailing_amount && has_word && !summary_like && st.section.is_some() && ledger.transactions[id].page == page {
                let (date, day) = (ledger.transactions[id].date.clone(), ledger.transactions[id].day);
                let desc = tokens[..tokens.len() - 1].join(" ");
                let amount = parse_amount(tokens[tokens.len() - 1]).unwrap_or(0.0).abs();
                let new_id = ledger.transactions.len();
                ledger.transactions.push(Txn { id: new_id, date, day, kind: st.row_kind(page, &desc).0, amount, description: desc, page, table: st.table });
                last_txn = Some(new_id);
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

/// End offset and text of a cell that was meant to be an amount but lost a digit to the
/// text layer ("M,000.00"): ends in ".dd", has a thousands comma or a digit run before it,
/// and is not a readable amount. At most one such cell on a line.
fn garbled_amount_span(line: &str) -> Option<(usize, &str)> {
    let mut found = None;
    let mut pos = 0;
    for tok in line.split(' ') {
        let end = pos + tok.len();
        pos = end + 1;
        if tok.len() < 5 || is_amount_token(tok) {
            continue;
        }
        let t = tok.trim_start_matches('$');
        let cents = t.len() >= 3 && t.as_bytes()[t.len() - 3] == b'.' && t[t.len() - 2..].chars().all(|c| c.is_ascii_digit());
        let body = &t[..t.len() - 3];
        let shaped = cents && body.contains(',') && body.split(',').skip(1).all(|g| g.len() == 3 && g.chars().all(|c| c.is_ascii_digit()));
        if shaped {
            if found.is_some() {
                return None;
            }
            found = Some((end, tok));
        }
    }
    found
}

/// "#3214*" -> "3214": check numbers as printed in check tables and image captions.
/// A two-column check table's row ("04/16  2281  22331  04/15  11071  1,199.05", TD in a
/// scan): every amount slot carries cents, so a bare run of digits there lost its point
/// ("22331" is 223.31) and a run split in two lost it to a space ("467 96"). Only on a line
/// of two or more (date, serial, amount) groups where another slot reads as an amount.
fn repair_check_pairs(line: &str) -> String {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let dates: Vec<usize> = toks.iter().enumerate().filter(|(_, t)| parse_date_token(t).is_some()).map(|(i, _)| i).collect();
    if dates.len() < 2 || dates[0] != 0 {
        return line.to_string();
    }
    let digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    let serial = |t: &str| { let n = check_no(t); digits(n) && n.len() <= 7 };
    // Each group: the date, its serial, then the amount slot up to the next date.
    let mut groups: Vec<(usize, usize)> = Vec::new(); // (slot start, slot end)
    for (k, &d) in dates.iter().enumerate() {
        let end = dates.get(k + 1).copied().unwrap_or(toks.len());
        if d + 1 >= end || !serial(toks[d + 1]) {
            return line.to_string();
        }
        groups.push((d + 2, end));
    }
    // (A slot's amount with a semicolon and colon for its comma and point, "1;199:05", is
    // still a witness: it is put right further on.)
    // (Or with a scan's glyph in front of it, "£28.48".)
    let cents_amount = |t: &str| { let fixed = t.replace(';', ",").replace(':', ".").replace(['£', '§'], ""); is_amount_token(&fixed) && fixed.contains('.') };
    let witness = groups.iter().any(|&(a, b)| b == a + 1 && cents_amount(toks[a]));
    if !witness {
        return line.to_string();
    }
    let mut out: Vec<String> = toks.iter().map(|t| t.to_string()).collect();
    let mut repaired = false;
    for &(a, b) in &groups {
        let slot = &toks[a..b];
        match slot {
            [x] if digits(x) && (3..=7).contains(&x.len()) => {
                out[a] = format!("{}.{}", &x[..x.len() - 2], &x[x.len() - 2..]);
                repaired = true;
            }
            [x, y] if digits(x) && x.len() <= 5 && y.len() == 2 && digits(y) => {
                out[a] = format!("{x}.{y}");
                out[a + 1] = String::new();
                repaired = true;
            }
            _ => {}
        }
    }
    if repaired { out.into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ") } else { line.to_string() }
}

fn check_no(t: &str) -> &str {
    // (KeyBank marks a gap in the sequence with a leading star: "*2008"; a scan glues a
    // quote or apostrophe to an image caption's number: "0000\"".)
    t.trim_start_matches(|c| c == '#' || c == '*').trim_end_matches(|c| c == '*' || c == '"' || c == '\'' || c == '\u{201d}' || c == '\u{2019}')
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

/// Court OCR splits words ("Beginni ng Balance"): a key matches when the line without
/// spaces contains the key without spaces.
fn has_key(lower: &str, key: &str) -> bool {
    lower.contains(key) || lower.replace(' ', "").contains(&key.replace(' ', ""))
}

fn capture_summary(lower: &str, line: &str, s: &mut Summary, page: usize) {
    if has_key(lower, "beginning balance") || lower.contains("previous balance") || lower.contains("opening ledger balance") || lower.contains("opening balance") || lower.starts_with("balance forward") || lower.contains("balance last statement") {
        // Sunrise puts the values on the next line; Legends on the same line; Frost says
        // "BALANCE LAST STATEMENT".
        let v = first_amount_after(line, &["beginning balance", "previous balance", "opening ledger balance", "opening balance", "balance forward", "balance last statement"]).or_else(|| if lower.contains("beginning balance") { None } else { last_amount(line) });
        if let Some(v) = v {
            if !s.beginning_balances_seen.iter().any(|b| (b - v).abs() < 0.005) {
                s.beginning_balances_seen.push(v);
            }
        }
        if s.beginning_balance.is_none() {
            s.beginning_balance = v;
        }
        // KeyBank has no "summary" heading: the categories ("1 Addition +7,170.00",
        // "3 Subtractions -7,065.85", "Net fees and charges -5.00") follow the beginning
        // balance line directly, so that line opens the block.
        if v.is_some() && (lower.trim_start().starts_with("beginning balance") || lower.trim_start().starts_with("balance forward") || lower.trim_start().starts_with("previous balance")) && !s.in_summary_block && s.debit_parts.is_empty() && s.debit_parts_unsigned.is_empty() && s.credit_parts.is_empty() {
            s.in_summary_block = true;
            s.summary_lines = 0;
        }
    }
    let ntok = lower.split_whitespace().count();
    // Summary block: debit categories are the negative figures between the "summary"
    // heading and the ending balance.
    // ("Balance Summary:-$10,386.41 (available as of today ...)" on an online printout
    // carries a figure glued to the colon: a heading with cents in it is not the block.)
    let has_cents = line.split_whitespace().any(|t| t.len() >= 4 && t.as_bytes()[t.len() - 3] == b'.' && t[t.len() - 2..].chars().all(|c| c.is_ascii_digit()));
    if lower.contains("summary") && ntok <= 12 && last_amount(line).is_none() && !has_cents && !lower.contains("fee") && !lower.contains("service charge") && !lower.contains("interest") {
        // The first block that captured a category is the account summary; later
        // "summary" headings (card summaries, fee summaries, a credit union's year-to-date
        // "Summary" on the last page) do not replace it.
        if s.debit_parts.is_empty() && s.debit_parts_unsigned.is_empty() && s.credit_parts.is_empty() {
            s.in_summary_block = true;
            s.summary_lines = 0;
        }
    } else if s.in_summary_block {
        let toks: Vec<&str> = line.split_whitespace().collect();
        // The block ends at the first transaction line or section header, or after 25 lines.
        s.summary_lines += 1;
        let first_is_date = toks.first().and_then(|t| parse_date_token(t)).is_some();
        // (The line itself still gets the balance and total checks below: Fifth Third's
        // "06/30 Ending Balance $27,740.38" starts with a date.)
        // (A table header, "Check #  Amount  Date  Check #  Amount  Date   33,439.79" with
        // Citizens' previous balance printed beside it, ends the block as well.)
        let table_header = lower.contains("date") && (lower.contains("amount") || lower.contains("description"));
        // (Community Bank details its fee under "Total Service Charge Breakdown": the block
        // ends there, or the "Item Charges in Service Charge 3.70" would count again.)
        if first_is_date && ntok >= 3 || section_for(line).is_some() || table_header || s.summary_lines > 25 || lower.contains("breakdown") {
            s.in_summary_block = false;
        }
        // The category value is the first amount on the line; U.S. Bank prints unrelated
        // figures to its right ("Other Withdrawals 962.49- Interest Paid this Year $0.62").
        // First State prints the credit marker as its own token ("2,796.37 +").
        // (A zero that lost its point, "+ 0 CREDITS  00  9,507.42", is the category's
        // value; see `first_amount_after`.)
        if let Some(a) = toks.iter().position(|t| is_amount_token(t) || lost_zero(t)).filter(|_| s.in_summary_block) {
            let mut value = if lost_zero(toks[a]) { ".00".to_string() } else { toks[a].to_string() };
            if toks.get(a + 1).map(|t| *t == "+" || *t == "-").unwrap_or(false) {
                value.push_str(toks[a + 1]);
            }
            let last = value.as_str();
            let prev_minus = a >= 1 && toks[a - 1] == "-";
            let label: String = toks[..a].join(" ").to_ascii_lowercase();
            // PNC prints credit and debit categories side by side ("ACH Credits 92
            // 3,199,536.68   ACH Debits 135 3,412,040.00"): such lines are not categories.
            let credit_word = |t: &str| t.contains("deposit") || t.contains("credit") || has_phrase(t, "addition") || has_phrase(t, "additions");
            // ("Commercial Checking 7558 26,937.82" in a consolidated summary is an account
            // line, not a checks category.)
            // ("Payrnents": OCR reads "m" as "rn" in TD's small print.)
            // ("4 Electronic DR 48,619.16", Five Star Bank: DR is a debit category too.)
            let debit_word = |t: &str| t.replace("checking", "").contains("check") || t.contains("payment") || t.contains("payrnent") || t.contains("withdrawal") || t.contains("debit") || has_phrase(t, "dr") || t.contains("charge") || t.contains("fee") || t.contains("card activity") || t.contains("subtraction");
            let two_columns = credit_word(&lower) && debit_word(&lower);
            // Wells' "Summary of accounts" lists each account with its number and ending
            // balance ("Additional Navigate Business Checking  8  2393749219  15,130.18",
            // "Total deposit accounts $21,023.08"): account lines, not categories.
            let account_line = toks.iter().any(|t| t.len() >= 8 && t.chars().all(|c| c.is_ascii_digit())) || label.contains("account");
            // ("Finance Charges Paid Year To Date: $2.57" and "... Last Year: $21.03" on a
            // Citi Checking Plus page are history, not this period's categories.)
            if a >= 1 && a <= 8 && !two_columns && !account_line && !label.contains("balance") && !label.contains("interest") && !label.contains("days") && !label.contains("year") && !label.contains("ytd") {
                let v = parse_amount(last).map(f64::abs);
                if last.starts_with('-') || last.starts_with("-$") || last.ends_with('-') || prev_minus {
                    // Chase: signed categories ("- 483,000.00"); U.S. Bank: trailing "962.49-".
                    if let Some(v) = v {
                        s.debit_parts.push(v);
                    }
                } else if let Some(v) = v {
                    // TD business: unsigned categories told apart by their words
                    // ("Deposits", "Electronic Deposits" vs "Checks Paid", "Electronic Payments");
                    // First State Bank marks credits with a trailing '+' ("12,821.16+"),
                    // KeyBank with a leading one ("+7,170.00").
                    let plus = last.ends_with('+') || last.starts_with('+');
                    let is_credit = plus || credit_word(&label);
                    let is_debit = !plus && debit_word(&label);
                    if is_credit && !is_debit {
                        s.credit_parts.push(v);
                    } else if is_debit && !is_credit {
                        s.debit_parts_unsigned.push(v);
                    }
                }
            }
        }
        if lower.contains("ending balance") || lower.contains("new balance") || lower.contains("closing balance") {
            s.in_summary_block = false;
        }
    }
    // BankNorth: "PREV STATEMENT BALANCE (12/31/23)  6,216.37", "STATEMENT BALANCE (01/31/24)
    // 37,713.20", the statement date as "AS OF: 01/31/24".
    if lower.starts_with("prev") && lower.contains("statement balance") {
        if s.beginning_balance.is_none() {
            s.beginning_balance = last_amount(line);
        }
        if s.period_start.is_none() {
            s.period_start = line.split_whitespace().find_map(|t| { let d = t.trim_matches(|c| c == '(' || c == ')'); parse_date_token(d).map(|_| d.to_string()) });
        }
    } else if lower.starts_with("statement balance (") && s.ending_balance.is_none() {
        s.ending_balance = last_amount(line);
    }
    if s.period_end.is_none() && lower.trim_start().starts_with("as of:") {
        s.period_end = line.split_whitespace().skip(2).find(|t| parse_date_token(t).is_some()).map(str::to_string);
    }
    // TD Bank: "Statement Balance as of 01/18 ... 5,480.39" then "... as of 02/17 ... 50.00".
    if lower.contains("statement balance as of") {
        if s.beginning_balance.is_none() {
            s.beginning_balance = last_amount(line);
        } else if s.ending_balance.is_none() {
            s.ending_balance = last_amount(line);
        }
    }
    const ENDING_KEYS: &[&str] = &["ending balance", "current balance", "new balance", "ending ledger balance", "closing balance", "balance this statement"];
    if s.ending_balance.is_none() && ENDING_KEYS.iter().any(|k| has_key(lower, k)) {
        // The value follows the label; two-column summaries put unrelated figures after
        // it ("Ending Balance $323.02 Interest Paid Year-to-Date $0.11"). Sunrise puts the
        // value on the line alone, which the last amount still covers.
        s.ending_balance = first_amount_after(line, ENDING_KEYS).or_else(|| last_amount(line));
    }
    // Summary totals. Two-column summaries put unrelated figures to the right of the value
    // ("2 Deposits/Credits  53,633.89  Average Ledger  154,454"), so the first amount after
    // the key is the value. Keys, by bank: Legends "Deposits/Other Credits", Sunrise "Total
    // Credits", Wells "Deposits/Additions", Webster "26 Credit(s) this period", Truist
    // "Deposits, credits and interest", Chase "Deposits and Credits", BofA "Deposits and other
    // credits", Mabrey "Deposits/Credits", Pinnacle "Credits + $.00".
    // (Renasant: "ADDITIONS + 2,947,678.33" and "SUBTRACTIONS - 2,994,110.64" beside other
    // figures in a two-column summary.)
    const CREDIT_KEYS: &[&str] = &["deposits/other credits", "total credits", "total deposits", "deposits/additions", "deposits and additions", "credit(s) this period", "deposits, credits and interest", "deposits and credits", "deposits and other credits", "deposits/credits", "deposits & credits", "deposits & credit", "additions +", "credits (+)"];
    const DEBIT_KEYS: &[&str] = &["checks/other debits", "checks & other debits", "checks and other debits", "total debits", "total withdrawals", "withdrawals/subtractions", "withdrawals and subtractions", "debit(s) this period", "other withdrawals, debits and service charges", "withdrawals and debits", "withdrawals and other debits", "checks/debits", "withdrawals/debits", "withdrawals (-)", "subtractions -", "debits (-)"];
    // A total smaller than the categories already captured is a garbled section total
    // ("Total Deposits & Credits  $1 )3,1i 7.18" in a court scan), not the figure.
    let plausible = |v: Option<f64>, parts: &[f64]| v.filter(|v| parts.is_empty() || *v + 0.01 >= parts.iter().sum::<f64>());
    if s.total_credits.is_none() && !lower.contains("---") {
        if CREDIT_KEYS.iter().any(|k| lower.contains(k)) {
            s.total_credits = plausible(first_amount_after(line, CREDIT_KEYS).map(f64::abs), &s.credit_parts);
        } else if lower.starts_with("credits") && ntok <= 5 {
            s.total_credits = plausible(first_amount_after(line, &["credits"]).map(f64::abs), &s.credit_parts);
        }
    }
    if s.total_debits.is_none() && !lower.contains("---") {
        let parts: Vec<f64> = if s.debit_parts.len() >= s.debit_parts_unsigned.len() { s.debit_parts.clone() } else { s.debit_parts_unsigned.clone() };
        if let Some(k) = DEBIT_KEYS.iter().find(|k| lower.contains(*k)) {
            s.total_debits = plausible(first_amount_after(line, DEBIT_KEYS).map(f64::abs), &parts);
            if s.total_debits.is_some() {
                s.debits_key = k;
                s.debits_page = Some(page);
            }
        } else if lower.starts_with("debits") && ntok <= 5 {
            s.total_debits = plausible(first_amount_after(line, &["debits"]).map(f64::abs), &parts);
            if s.total_debits.is_some() {
                s.debits_key = "debits";
                s.debits_page = Some(page);
            }
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
    // (Not a fee schedule's line, "CHECKS, DEP ITEMS/TICKETS, ACH   25   .4500   11.25" in
    // Citi's service charge summary: a unit price with three or four decimals gives it away.)
    let unit_price = line.split_whitespace().any(|t| t.rsplit_once('.').map(|(_, c)| c.len() >= 3 && c.chars().all(|ch| ch.is_ascii_digit())).unwrap_or(false));
    // (": Checks  -$100.00", KeyBank in a scan: a mark before the label does not count.)
    let bare = lower.trim_start_matches(|c: char| !c.is_ascii_alphabetic());
    // ("Checks & Other Debits  8,216.67" is the debit total itself, not a checks figure.)
    if same_block && s.checks_total.is_none() && !unit_price && !bare.contains("other debits") && (bare.starts_with("checks") && !bare.starts_with("checks paid") || bare.starts_with("checks paid") && ntok <= 5) {
        s.checks_total = first_amount_after(line, &["checks"]).map(f64::abs);
    }
    // Fifth Third's "Service Charge withdrawn on 06/10/26 $164.00" is already one of the
    // listed withdrawals, not a figure to add.
    // (KeyBank: "Fees and Charges  -$67.00" under "Withdrawals  -$1,339.68"; the balance
    // equation confirms the fees are outside the withdrawals figure.)
    if same_block && s.fees_total.is_none() && !lower.contains("withdrawn on") && (lower.starts_with("service fees") || lower.starts_with("service charge") || lower.starts_with("- service charge") || lower.starts_with("analysis or maintenance fee") || bare.starts_with("fees and charges") && bare.split_whitespace().count() <= 9) {
        s.fees_total = first_amount_after(line, &["service fees", "service charges", "service charge", "fees for period", "fees and charges"]).map(f64::abs);
    }
    // (Gulf Coast: "Interest Paid  1.58  Annual Percentage Yield Earned  0.05%" beside
    // "Interest Earned  1.58"; the first figure after the label is the period's.)
    if s.interest_total.is_none() && (lower.starts_with("interest earned this period") || lower.starts_with("interest paid this period") || lower.starts_with("interest earned this statement") || lower.starts_with("interest paid this statement")) {
        s.interest_total = first_amount_after(line, &["this period", "this statement"]).map(f64::abs);
    } else if s.interest_total.is_none() && lower.find("interest paid ").map(|p| lower[..p].chars().filter(|c| c.is_ascii_alphabetic()).count() <= 1 && lower[p..].split_whitespace().nth(2).map(|t| is_amount_token(t)).unwrap_or(false)).unwrap_or(false) {
        // ("g + INTEREST PAID 2,369.25": Hancock Whitney's summary signs, and a scan's
        // margin noise, lead the label; "YTD INTEREST PAID" has words before it.)
        s.interest_total = first_amount_after(line, &["interest paid"]).map(f64::abs);
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
        let parts: Vec<&str> = after.split(|c: char| c.is_whitespace() || c == ':').collect();
        let at = parts.iter().position(|t| t.len() >= 4 && t.chars().all(|c| c.is_ascii_digit() || c == 'X' || c == 'x' || c == '*' || c == '-') && t.chars().rev().take(4).all(|c| c.is_ascii_digit()));
        if let Some(i) = at {
            // (Bank of America prints the number in groups, "3250 8165 3203": the groups of
            // four digits that follow belong to it, and the last four are the last group's.)
            let mut c = parts[i].to_string();
            for t in &parts[i + 1..] {
                if t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()) { c.push_str(t) } else { break }
            }
            s.account_last4 = Some(c.chars().rev().take(4).collect::<String>().chars().rev().collect());
        }
    }
    // (Brookline: "Statement Dates 12/11/18 thru 1/10/19".)
    if lower.contains("period") && lower.contains("through") || lower.contains("statement period") || lower.contains("statement dates") && lower.contains("thru") {
        let dates: Vec<&str> = line.split_whitespace().filter(|t| parse_date_token(t).is_some()).collect();
        if dates.len() >= 2 {
            s.period_start = Some(dates[0].to_string());
            s.period_end = Some(dates[1].to_string());
        }
    }
    // "Statement Ending 07/31/2025" (Yampa Valley, Webster), "This statement: 12/31/2021"
    // and "Last statement: 11/30/2021" (Synovus; month names already rewritten).
    let one_date = || line.split_whitespace().find(|t| parse_date_token(t).is_some()).map(str::to_string);
    if s.period_end.is_none() && (lower.starts_with("statement ending") || lower.starts_with("this statement:")) {
        s.period_end = one_date();
    }
    if s.period_start.is_none() && lower.starts_with("last statement:") {
        s.period_start = one_date();
    }
}

/// Summary column labels in left-to-right order, for two-line summaries.
fn column_labels(lower: &str) -> Vec<&'static str> {
    let mut found: Vec<(usize, &'static str)> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (needle, label) in [
        ("previous balance", "beginning"), ("beginning balance", "beginning"), ("balance last statement", "beginning"),
        ("total credits", "credits"), ("total deposits", "credits"), ("deposits and other credits", "credits"), ("deposits/credits", "credits"),
        ("total debits", "debits"), ("total withdrawals", "debits"), ("checks and other debits", "debits"), ("withdrawals/debits", "debits"), ("checks/debits", "debits"),
        ("current balance", "ending"), ("ending balance", "ending"), ("new balance", "ending"), ("balance this statement", "ending"),
    ] {
        if let Some(p) = lower.find(needle) {
            found.push((p, label));
            spans.push((p, p + needle.len()));
        }
    }
    // Bare "Credits" / "Debits" columns next to the balance labels ("Beginning Balance
    // Credits Debits Ending Balance") count as labels too.
    for (needle, label) in [(" credits", "credits"), (" debits", "debits")] {
        let mut from = 0;
        while let Some(p) = lower[from..].find(needle) {
            let at = from + p;
            let end = at + needle.len();
            let whole = lower[end..].chars().next().map(|c| !c.is_alphanumeric() && c != '/').unwrap_or(true);
            let covered = spans.iter().any(|(a, b)| *a <= at + 1 && at + 1 < *b);
            if whole && !covered && !found.is_empty() {
                found.push((at + 1, label));
            }
            from = end;
        }
    }
    // PNC wraps the labels: "Beginning   Deposits and   Checks and   Ending" over
    // "balance   other credits   other debits   balance". Single words carry the order.
    if found.len() < 2 {
        let mut words: Vec<(usize, &'static str)> = Vec::new();
        for (needle, label) in [("beginning", "beginning"), ("previous", "beginning"), ("deposits", "credits"), ("credits", "credits"), ("checks", "debits"), ("withdrawals", "debits"), ("debits", "debits"), ("ending", "ending")] {
            if let Some(p) = lower.find(needle) {
                if !words.iter().any(|(_, l)| *l == label) {
                    words.push((p, label));
                }
            }
        }
        // (A Puerto Rico bank's scan: "SEGHINING BALANGH  DEPOSITS / OTHER CREDITS  CHECKS /
        // OTHER DEBITS  SERVICE CHARGES  ENDING BALANCE" with the balance words garbled but
        // "balance" itself printed at both ends; a service charge column sits before the end.)
        let balances: Vec<usize> = lower.match_indices("balan").map(|(p, _)| p).collect();
        if balances.len() >= 2 && words.iter().any(|(_, l)| *l == "credits") && words.iter().any(|(_, l)| *l == "debits") {
            if !words.iter().any(|(_, l)| *l == "beginning") { words.push((balances[0], "beginning")); }
            if !words.iter().any(|(_, l)| *l == "ending") { words.push((balances[balances.len() - 1], "ending")); }
            if let Some(p) = lower.find("service") { if p > balances[0] && p < balances[balances.len() - 1] { words.push((p, "fees")); } }
        }
        if words.len() >= 3 && words.iter().any(|(_, l)| *l == "beginning") && words.iter().any(|(_, l)| *l == "ending") {
            found = words;
        }
    }
    found.sort();
    // The same label twice ("Deposits & Credits   Total Deposits & Credits" is a section
    // title with its total label) names one column, not a two-column summary.
    let mut labels: Vec<&'static str> = Vec::new();
    for (_, l) in found {
        if !labels.contains(&l) {
            labels.push(l);
        }
    }
    labels
}

fn first_amount_after(line: &str, keys: &[&str]) -> Option<f64> {
    let lower = line.to_ascii_lowercase();
    let pos = keys.iter().filter_map(|k| lower.find(k).map(|p| p + k.len())).min()?;
    // (A zero printed as ".00" loses its point in a scan: "+ 0 CREDITS  00  9,507.42",
    // Hancock Whitney. A bare "00" first after the label is that zero.)
    line[pos..].split_whitespace().find(|t| is_amount_token(t) || lost_zero(t)).and_then(|t| if lost_zero(t) { Some(0.0) } else { parse_amount(t) })
}

/// ".00" that lost its point in a scan: "00", "-00", "+00".
fn lost_zero(t: &str) -> bool {
    t.trim_start_matches(|c| c == '-' || c == '+' || c == '.') == "00"
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
    ledger.parsed_credit_total = ledger.transactions.iter().filter(|t| t.kind == Kind::Credit && !ledger.netted.contains(&t.id)).map(|t| t.amount).sum();
    ledger.parsed_debit_total = ledger.transactions.iter().filter(|t| t.kind == Kind::Debit && !ledger.netted.contains(&t.id)).map(|t| t.amount).sum();

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
    // The court's own stamps ("FILED: MONROE COUNTY CLERK 01/03/2024", "RECEIVED NYSCEF:
    // 01/03/2024", federal "Case ... Filed ... Page") repeat on every page and get no vote.
    let mut votes: BTreeMap<i32, usize> = BTreeMap::new();
    for text in texts {
        for line in text.lines() {
            let lower = line.to_ascii_lowercase();
            if is_court_stamp(&lower) || lower.contains("nyscef") || lower.starts_with("filed:") {
                continue;
            }
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
    for line in texts.iter().flat_map(|t| t.lines()) {
        let lower = line.to_ascii_lowercase();
        // (A notice of what comes next, "As of January 1, 2020, fees will change", SunTrust
        // on a November 2019 statement, names a later year: one vote, not ten.)
        let notice = lower.contains(" will ") || lower.contains("effective") || lower.contains("change");
        // (The year token must be whole: "2061.99" in "NOV 01  2061.99" is an amount.)
        let toks: Vec<&str> = lower.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).collect();
        for w in toks.windows(3) {
            let is_month = MONTHS.iter().any(|m| m.starts_with(w[0]) && w[0].len() >= 3);
            let day_ok = (1..=2).contains(&w[1].len()) && w[1].chars().all(|c| c.is_ascii_digit());
            let year_tok = w[2].trim_end_matches('.');
            if is_month && day_ok && year_tok.chars().all(|c| c.is_ascii_digit()) && (year_tok.len() == 4 || year_tok.len() == 2) {
                if let Ok(y) = year_tok.parse::<i32>() {
                    // "NOV 30 21" (an older commercial statement) is a two-digit year, a weaker vote.
                    let (y, weight) = if year_tok.len() == 2 || notice { (if year_tok.len() == 2 { 2000 + y } else { y }, 1) } else { (y, 10) };
                    if (2000..=2100).contains(&y) {
                        *votes.entry(y).or_default() += weight;
                    }
                }
            }
        }
    }
    if let Some((y, _)) = votes.into_iter().max_by_key(|(_, n)| *n) {
        return Some(y);
    }
    // Last resort: a whole four-digit token that is a plausible year (never part of an amount).
    for text in texts {
        for tok in text.split_whitespace() {
            let tok = tok.trim_matches(|c: char| c == ',' || c == '.' || c == ')' || c == '(');
            if tok.len() == 4 && tok.chars().all(|c| c.is_ascii_digit()) {
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
    // (Dates the scan read without their slash, "1105" for 11/05 at the head of a
    // three-column check row, are repaired first: the columns are cut at the dates.)
    let text = &repair_slashless_dates(text);
    let mut out = String::new();
    // Character offsets where the second, third, ... column start; empty outside a block.
    let mut splits: Vec<usize> = Vec::new();
    let mut cols: Vec<Vec<String>> = Vec::new();
    // Header starts with "Date": every column begins with a date token, which is a
    // safer cut than the whitespace gap when the columns nearly touch.
    let mut date_first = false;
    let mut check_first = false;
    // The block's header names no description column: dates and figures only.
    let mut figures_only = false;
    let flush = |out: &mut String, cols: &mut Vec<Vec<String>>| {
        for l in cols.iter_mut().flat_map(|c| c.drain(..)) {
            out.push_str(&l);
            out.push('\n');
        }
    };
    for line in text.lines() {
        // (And a two-column check row's bare figures get their points back before the
        // columns are cut apart, while the other column can still witness for them.)
        let repaired = repair_check_pairs(&repair_bad_month(line));
        let line = repaired.as_str();
        let lower = line.to_ascii_lowercase();
        let toks: Vec<&str> = line.split_whitespace().collect();
        // A header naming date/amount/description twice or more. Each further column
        // starts at the next occurrence of whichever word repeats ("Description  Date
        // Amount  Description"; First State prints "Date Type Amount" three times).
        // Two of the column words must repeat: "Effective date  Posted date  Amount
        // Transaction detail" repeats "date" alone and is one wide table.
        // "date" alone repeating is not enough (two date columns); another repeated word is.
        let repeated: Vec<&str> = ["date", "description", "amount", "check", "serial"].into_iter().filter(|w| lower.matches(w).count() >= 2).collect();
        let column_groups = repeated.len() >= 2 || repeated.len() == 1 && repeated[0] != "date";
        let is_header = toks.len() <= 12 && !toks.iter().any(|t| is_amount_token(t)) && column_groups && lower.contains("date") && (lower.contains("amount") || lower.contains("serial")) && !lower.contains("balance");
        if is_header {
            flush(&mut out, &mut cols);
            // The further columns start at the second, third, ... "Date"; when only one
            // "Date" is printed (left header partly missing) it is that one, as long as a
            // label precedes it.
            // Each column group starts with the header's first word when that word repeats
            // ("Check  Date  Amount  Check  Date  Amount": the groups start at "Check", the
            // data at the check number); otherwise at the repeated "Date".
            let first_word = toks[0].to_ascii_lowercase();
            let firsts: Vec<usize> = lower.match_indices(first_word.as_str()).map(|(p, _)| p).collect();
            let dates: Vec<usize> = lower.match_indices("date").map(|(p, _)| p).collect();
            splits = if first_word != "date" && firsts.len() >= 2 && firsts.len() == dates.len() {
                firsts[1..].to_vec()
            } else if dates.len() >= 2 {
                dates[1..].to_vec()
            } else if !lower[..dates[0]].trim().is_empty() {
                vec![dates[0]]
            } else {
                Vec::new()
            };
            cols = vec![Vec::new(); splits.len() + 1];
            date_first = lower.trim_start().starts_with("date");
            check_first = lower.trim_start().starts_with("check") || lower.trim_start().starts_with("number");
            figures_only = !lower.contains("description");
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // (In a block of dates and figures alone, a daily balance or check table, a line of
        // three or more words without a digit is the next title, "TRANSACTIONS FOR SERVICE
        // FEE CALCULATION", however it reads; it ends the block before the columns flush.)
        let words_only = !splits.is_empty() && figures_only && toks.len() >= 3 && !toks.iter().any(|t| t.chars().any(|c| c.is_ascii_digit()));
        if words_only {
            flush(&mut out, &mut cols);
            splits.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // A single-column header ("Date  Description  Amount") ends a multi-column block.
        let single_header = !splits.is_empty() && !is_header && toks.len() <= 10 && !toks.iter().any(|t| is_amount_token(t)) && lower.contains("date") && (lower.contains("amount") || lower.contains("description"));
        if single_header {
            flush(&mut out, &mut cols);
            splits.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // A new section title ("• Checks", "Daily Balance", "Withdrawals and Debits") ends the block.
        let has_date_or_amount = toks.iter().any(|t| is_amount_token(t) || parse_date_token(t).is_some());
        // (A sentence of nine words or more is a footer paragraph, not a continuation.)
        let section_title = !has_date_or_amount && !toks.is_empty() && (line.trim_start().starts_with('•') || line.trim_start().starts_with('*') || section_for(line).is_some() || lower.contains("balance") || lower.contains("summary") || toks.len() >= 9);
        if !splits.is_empty() && section_title {
            flush(&mut out, &mut cols);
            splits.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if splits.is_empty() {
            // Two rows printed side by side with no header over them (BankNorth: "01/16
            // 76.65 POINT OF SAL 01/22  178.00 POINT OF SAL").
            if let Some((l, r)) = split_paired_row(line) {
                if l.is_empty() {
                    continue; // a check image caption
                }
                out.push_str(&l);
                out.push('\n');
                if !r.is_empty() {
                    out.push_str(&r);
                    out.push('\n');
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // The court's filing stamp runs through the columns ("Case: 22-10381  Doc# 69-6
        // Filed: 10/24/22"); inside a block it is dropped rather than cut into the columns.
        if !splits.is_empty() && is_court_stamp(&lower) {
            continue;
        }
        // A flat two-column check row whose right date the scan garbled ("07/01 9016 738.54
        // O7/16 5021 606.88"): no date to cut at, but the (date, serial, amount) shape is.
        if splits.len() == 1 && figures_only {
            if let Some((l, r)) = split_garbled_check_pair(line) {
                cols[0].push(l);
                cols[1].push(r);
                continue;
            }
        }
        // Flat OCR of a two-column block keeps no offsets to cut at (Hancock Whitney's
        // "07/01 194.88 Payroll ROMAN CATHOLIC C 07/02 992.08 Payroll SAINT PIUS X CHU"):
        // a line that reads as two amount-first rows is split on its words (two-column
        // blocks only; a three-column block keeps its offsets).
        if splits.len() == 1 {
            if let Some((l, r)) = split_paired_row(line) {
                if l.is_empty() {
                    continue; // a check image caption
                }
                if r.is_empty() {
                    // A lone row: the column it sits under, by where its date starts.
                    let date = l.split_whitespace().next().unwrap_or("");
                    let at = line.find(date).unwrap_or(0);
                    let col = if at + 8 >= splits[0] { 1 } else { 0 };
                    cols[col].push(l);
                } else {
                    cols[0].push(l);
                    cols[1].push(r);
                }
                continue;
            }
        }
        // Text with no date or amount inside a block is a description continuation (or a
        // stray title): it belongs whole to the column it is indented under, never cut.
        if !has_date_or_amount && !toks.is_empty() {
            let indent = line.len() - line.trim_start().len();
            let col = splits.iter().filter(|&&at| indent + 8 >= at).count();
            cols[col].push(line.trim_end().to_string());
            continue;
        }
        if line.len() + 8 <= splits[0] {
            if !line.trim().is_empty() {
                cols[0].push(line.to_string());
            }
            continue;
        }
        // The columns' data start inside the gaps before their headers, not exactly under
        // them, so cut at the whitespace run nearest each header position.
        let mut pieces: Vec<&str> = Vec::new();
        let mut rest = line;
        let mut consumed = 0;
        let mut ok = true;
        for &at in &splits {
            if at <= consumed || rest.len() + 8 <= at - consumed {
                break; // the line ends before this column
            }
            // Columns start with a date, so when the gap is a single space ("445.07
            // 09/12/23") the date token nearest the header position is the cut.
            let cut = if date_first || check_first {
                // No date (or check number) near the column start: the row has no more columns.
                let pred: &dyn Fn(&str) -> bool = if date_first { &|t: &str| parse_date_token(t).is_some() } else { &|t: &str| (3..=7).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit()) };
                match token_near(rest, at - consumed, pred) {
                    Some(c) => Some(c),
                    // Synovus prints the legend in the empty right column ("5562  12/10
                    // 195.00   * Skip in check sequence"): a footnote there is still a cut.
                    None => match gap_near(rest, at - consumed) {
                        Some(c) if rest[c..].trim_start().starts_with('*') => Some(c),
                        _ => break,
                    },
                }
            } else {
                gap_near(rest, at - consumed).or_else(|| date_near(rest, at - consumed))
            };
            match cut {
                Some(cut) => {
                    let (l, r) = rest.split_at(cut);
                    pieces.push(l);
                    rest = r;
                    consumed += cut;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            // Text running through a gap: a title or footer ends the block.
            flush(&mut out, &mut cols);
            splits.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        pieces.push(rest);
        for (i, piece) in pieces.iter().enumerate() {
            if !piece.trim().is_empty() {
                cols[i].push(piece.to_string());
            }
        }
    }
    flush(&mut out, &mut cols);
    out
}

/// A two-column check row read flat, "date serial amount  date serial amount", whose right
/// date the scan garbled: "O7/16" (a letter for the zero), "07124" (the slash read as a 1),
/// or past repair, "OF?". The left group must read whole and the right group's serial and
/// amount too; the right date is put right where one reading does it, and otherwise not
/// guessed.
fn split_garbled_check_pair(line: &str) -> Option<(String, String)> {
    let mut toks: Vec<String> = line.split_whitespace().map(String::from).collect();
    // (An amount whose point the scan read as a space, "247 92", is joined first.)
    let digits_only = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    if toks.len() == 7 && digits_only(&toks[2]) && toks[2].len() <= 5 && toks[3].len() == 2 && digits_only(&toks[3]) {
        toks[2] = format!("{}.{}", toks[2], toks[3]);
        toks.remove(3);
    }
    let toks: Vec<&str> = toks.iter().map(String::as_str).collect();
    let serial = |t: &str| { let n = check_no(t); !n.is_empty() && n.len() <= 7 && n.chars().all(|c| c.is_ascii_digit()) };
    let cents = |t: &str| is_amount_token(t) && t.contains('.');
    if toks.len() != 6 || parse_date_token(toks[0]).is_none() || !serial(toks[1]) || !cents(toks[2]) || !serial(toks[4]) || !cents(toks[5]) {
        return None;
    }
    let slot = toks[3];
    if parse_date_token(slot).is_some() || is_amount_token(slot) || !(2..=6).contains(&slot.chars().count()) {
        return None;
    }
    let zeroed = slot.replace(['O', 'o'], "0");
    let digits = zeroed.chars().all(|c| c.is_ascii_digit());
    let date = if parse_date_token(&zeroed).is_some() {
        Some(zeroed)
    } else if digits && zeroed.len() == 5 && &zeroed[2..3] == "1" && parse_date_token(&format!("{}/{}", &zeroed[..2], &zeroed[3..])).is_some() {
        Some(format!("{}/{}", &zeroed[..2], &zeroed[3..]))
    } else {
        None
    };
    // (A date past repair goes after the figures, where it is not taken for the serial;
    // the row keeps the date of the row above, as any undated check row does.)
    let right = match date {
        Some(d) => format!("{d} {} {}", toks[4], toks[5]),
        None => format!("{} {} {slot}", toks[4], toks[5]),
    };
    Some((toks[..3].join(" "), right))
}

/// A line that is two amount-first rows side by side, each "date [check number] amount
/// description" with exactly one amount and a worded description (BankNorth's "CHECKS /
/// DEBITS" listing: "01/03* 1057 1000.00 CUSTOMER CHE 01/26  1070  6000.00 CUSTOMER CHE").
/// Returns the two rows, footnote stars dropped from the dates.
fn split_paired_row(line: &str) -> Option<(String, String)> {
    // (OCR noise between the words, "Payroll © HOLY FAMILY CATH", "207.13.", is dropped:
    // tokens with no letter or digit, and a point after an amount.)
    let cleaned: Vec<&str> = line.split_whitespace().filter(|t| t.chars().any(|c| c.is_alphanumeric())).map(|t| { let bare = t.trim_end_matches(|c: char| !c.is_alphanumeric()); if bare != t && is_amount_token(bare) { bare } else { t } }).collect();
    // The other column's reference line sometimes lands at either end of a row's line
    // ("08/04 2,221,605.00 INVEST SWEEP DEBIT 025217003765245PPD", "025183004555884PPD
    // 07/31 415,231.00 OPTUMEFT UMRO2", the reference even split in two): runs of such
    // tokens at the ends are dropped first.
    let reference_like = |t: &str| t.chars().filter(|c| c.is_ascii_digit()).count() >= 4 && t.chars().all(|c| c.is_ascii_alphanumeric()) && !is_amount_token(t) && parse_date_token(t).is_none();
    let mut toks: &[&str] = &cleaned;
    while toks.len() > 3 && reference_like(toks[0]) {
        toks = &toks[1..];
    }
    while toks.len() > 3 && reference_like(toks[toks.len() - 1]) && toks.iter().filter(|t| reference_like(t)).count() <= 2 {
        toks = &toks[..toks.len() - 1];
    }
    let toks: Vec<&str> = toks.to_vec();
    if toks.len() < 3 || toks.len() > 16 {
        return None;
    }
    let is_date = |t: &str| parse_date_token(t.trim_end_matches(|c| c == '*' || c == '.')).is_some();
    let is_check = |t: &str| (3..=7).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit());
    // A half: date, optional check number, amount, then one or more words with letters.
    // The descriptions are short fields (BankNorth cuts them at twelve characters, Hancock
    // Whitney at about twenty): a half whose description runs past six words is a
    // sentence quoting a second date and amount ("NSF Return Item Fee for a Transaction
    // Received on 12/29 $23,530.00"), and dollar signs never appear in such paired listings.
    // (A check table's half is "date  serial  amount" with no description at all.)
    let half = |h: &[&str]| -> bool {
        if h.len() < 3 || !is_date(h[0]) {
            return false;
        }
        if h.len() == 3 && is_check(h[1]) && is_amount_token(h[2]) && !h[2].contains('$') {
            return true;
        }
        let a = if is_check(h[1]) && h.len() >= 4 && is_amount_token(h[2]) { 2 } else { 1 };
        let desc = &h[a + 1..];
        is_amount_token(h[a]) && !h[a].contains('$') && (1..=6).contains(&desc.len()) && desc.iter().all(|t| !is_amount_token(t) && !is_date(t)) && desc.iter().any(|t| t.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 3)
    };
    // The account-number box over the first row of the listing clips the month from the
    // right column's date in a scan ("02/01 10.00 AUTOMATIC P /05 144.99 POINT OF SAL"):
    // a "/DD" there takes the left column's month, the same month on a monthly statement.
    let clipped = |t: &str| t.starts_with('/') && (2..=3).contains(&t.len()) && t[1..].chars().all(|c| c.is_ascii_digit());
    // ("42/09 4455950653 3,250.00": a month no calendar has is the row's own month with a
    // digit misread; the other date on the line says which month.)
    let bad_month = |t: &str| t.len() == 5 && t.as_bytes()[2] == b'/' && t[..2].chars().all(|c| c.is_ascii_digit()) && t[3..].chars().all(|c| c.is_ascii_digit()) && t[..2].parse::<u32>().map(|m| m > 12).unwrap_or(false) && parse_date_token(&format!("01/{}", &t[3..])).is_some();
    let month = toks.iter().find_map(|t| parse_date_token(t).map(|_| t.split('/').next().unwrap_or(""))).filter(|m| m.len() == 2).unwrap_or("");
    let repaired: Vec<String> = toks.iter().enumerate().map(|(i, t)| {
        if i >= 3 && clipped(t) && month.len() == 2 && is_amount_token(toks.get(i + 1).unwrap_or(&"")) {
            format!("{month}{t}")
        } else if bad_month(t) && month.len() == 2 && (i == 0 || i >= 3) {
            format!("{month}/{}", &t[3..])
        } else {
            t.to_string()
        }
    }).collect();
    let toks: Vec<&str> = repaired.iter().map(|s| s.as_str()).collect();
    let row = |h: &[&str]| h.iter().map(|t| t.trim_end_matches(|c| c == '*' || c == '.')).collect::<Vec<_>>().join(" ");
    // The same entry twice on one line captions a check image and its back ("03/23/2023
    // 1000 $4,662.00  03/23/2023 1000 $4,662.00", TriState): no row at all.
    let n = toks.len();
    if n >= 6 && n % 2 == 0 && toks[..n / 2] == toks[n / 2..] && is_date(toks[0]) && toks[..n / 2].iter().any(|t| is_amount_token(t)) {
        return Some((String::new(), String::new()));
    }
    let cut = (3..toks.len().saturating_sub(2)).find(|&i| is_date(toks[i]) && half(&toks[..i]) && half(&toks[i..]));
    match cut {
        Some(cut) => Some((row(&toks[..cut]), row(&toks[cut..]))),
        // One row alone once the references are gone (only when something was dropped:
        // a plain single row is not this function's business).
        None if toks.len() < cleaned.len() && half(&toks) => Some((row(&toks), String::new())),
        None => None,
    }
}

/// A court filing stamp: "Case 24-11188-TMH  Doc 226-2  Filed 07/22/24  Page 2 of 10".
fn is_court_stamp(lower: &str) -> bool {
    (lower.contains("case") || lower.contains("pageid")) && (lower.contains("doc") || lower.contains("filed") || lower.contains("entered") || lower.contains(" page "))
}

fn date_near(line: &str, at: usize) -> Option<usize> {
    token_near(line, at, &|t| parse_date_token(t).is_some())
}

/// Start offset of a token satisfying `pred` that begins within 14 characters before `at`
/// or 4 after it, for column data separated by a single space.
fn token_near(line: &str, at: usize, pred: &dyn Fn(&str) -> bool) -> Option<usize> {
    let lo = at.saturating_sub(14);
    let hi = at + 4;
    let mut best: Option<(usize, usize)> = None;
    let mut pos = 0;
    for tok in line.split(' ') {
        if !tok.is_empty() && pos >= lo && pos <= hi && pred(tok) {
            let d = pos.abs_diff(at);
            if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, pos));
            }
        }
        pos += tok.len() + 1;
    }
    best.map(|(_, p)| p)
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
    let repaired: Vec<(usize, String)> = pages.iter().map(|(n, t)| (*n, repair_court_labels(t))).collect();
    let pages: Vec<(usize, &str)> = repaired.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let pages = &pages[..];
    // Bookkeeping reconciliation reports filed between the statements ("Statements &
    // Recs") restate the same month in the bank's words; their pages are left out. A
    // document made only of them is reported as such.
    let mut in_report = false;
    let mut in_card = false;
    let statements: Vec<(usize, &str)> = pages
        .iter()
        .copied()
        .filter(|(_, t)| {
            // Pages of the bankruptcy court's own forms (Monthly Operating Report, Form
            // 425C) and their exhibits carry figures that are not transactions; like a
            // reconciliation report they are left out until a statement page appears.
            if is_reconciliation_page(t) || is_court_form_page(t) || is_email_page(t) {
                in_report = true;
            } else if in_report && statement_words(t) {
                in_report = false; // a statement page again (its letterhead or summary)
            }
            // The member's credit card statement filed behind the deposit accounts (Navy
            // Federal): its payments and purchases are not the business's cash flow. Its
            // own pages mention an "average daily balance" and "APR", so only a deposit
            // statement page without those words ends the block.
            if is_card_page(t) {
                in_card = true;
            } else if in_card && statement_words(t) && !card_words(t) {
                in_card = false;
            }
            !in_report && !in_card
        })
        .collect();
    // A bookkeeper's check register filed in front of the statement ("105102  10/19/2022
    // U S DEPARTMENT OF HOMELAND SECURITY  $1,225.00", outstanding checks of the
    // reconciliation): pages before the first statement page whose rows are all check
    // number, full date, payee and amount are left out too.
    let first_statement = statements.iter().position(|(_, t)| statement_words(t));
    let statements: Vec<(usize, &str)> = statements
        .iter()
        .enumerate()
        // (A register titled "Check Register" is left out wherever it is filed.)
        .filter(|(i, (_, t))| !((first_statement.map(|f| *i < f).unwrap_or(false) || register_titled(t)) && is_check_register_page(t)))
        .map(|(_, p)| *p)
        .collect();
    let kind = document_kind(pages);
    let mut ledger = if statements.is_empty() || statements.len() < pages.len() / 2 && kind.is_some() { parse_statements(pages) } else { parse_statements(&statements) };
    ledger.summary.document_kind = kind;
    ledger
}

/// Court copies lose a letter of the summary labels ("Begi ning balance", "End ng
/// balance", "Ser ice fees") and of the period's month ("for eptember 1, 2022",
/// "for iOctober 1, 2022"). A word (or two fragments of one) one edit away from the label
/// is put back when the word after it says which label it is: "balance", "fees", or a day
/// number after a month. Real words one edit from a label ("pending", "lending") are left.
pub fn repair_court_labels(text: &str) -> String {
    const MONTHS: [&str; 9] = ["january", "february", "march", "april", "august", "september", "october", "november", "december"];
    const LEAVE: [&str; 6] = ["pending", "lending", "sending", "vending", "bending", "mending"];
    let within_one = |a: &str, b: &str| -> bool {
        if a == b {
            return false;
        }
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        if a.len().abs_diff(b.len()) > 1 || a.len() < 4 {
            return false;
        }
        let mut i = 0;
        while i < a.len() && i < b.len() && a[i] == b[i] {
            i += 1;
        }
        if a.len() == b.len() {
            a[i + 1..] == b[i + 1..]
        } else if a.len() > b.len() {
            a[i + 1..] == b[i..]
        } else {
            a[i..] == b[i + 1..]
        }
    };
    let day_number = |t: &str| -> bool {
        let d = t.trim_end_matches(',');
        !d.is_empty() && d.len() <= 2 && d.chars().all(|c| c.is_ascii_digit())
    };
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let lower: Vec<String> = toks.iter().map(|t| t.to_ascii_lowercase()).collect();
        // (start token, tokens covered, replacement)
        let mut fixes: Vec<(usize, usize, &str)> = Vec::new();
        for i in 0..toks.len() {
            for span in [1usize, 2] {
                if i + span >= toks.len() {
                    continue;
                }
                let word: String = lower[i..i + span].concat();
                if span == 2 && (lower[i].len() < 2 || lower[i + 1].len() < 2) {
                    continue; // "J J" and "I 1" are not halves of a label
                }
                if word.chars().any(|c| c.is_ascii_digit()) || LEAVE.contains(&word.as_str()) {
                    continue;
                }
                let next = lower[i + span].as_str();
                let label = if next.starts_with("balance") {
                    ["beginning", "ending"].iter().find(|l| within_one(&word, l)).copied()
                } else if next.starts_with("fee") {
                    within_one(&word, "service").then_some("service")
                } else if day_number(next) {
                    MONTHS.iter().find(|m| within_one(&word, m)).copied()
                } else {
                    None
                };
                if let Some(label) = label {
                    fixes.push((i, span, label));
                    break;
                }
            }
        }
        if fixes.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Rebuild the line in place, keeping the spacing around the repaired span.
        let mut rebuilt = String::with_capacity(line.len());
        let mut rest = line;
        let mut skip = 0;
        for (i, tok) in toks.iter().enumerate() {
            let at = rest.find(tok).unwrap_or(0);
            if skip > 0 {
                skip -= 1;
                rest = &rest[at + tok.len()..];
                continue;
            }
            rebuilt.push_str(&rest[..at]);
            if let Some((_, span, label)) = fixes.iter().find(|(s, _, _)| *s == i) {
                let cap = MONTHS.contains(label) || tok.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
                let mut fixed = label.to_string();
                if cap {
                    fixed = fixed[..1].to_ascii_uppercase() + &fixed[1..];
                }
                rebuilt.push_str(&fixed);
                skip = span - 1;
            } else {
                rebuilt.push_str(tok);
            }
            rest = &rest[at + tok.len()..];
        }
        rebuilt.push_str(rest);
        out.push_str(&rebuilt);
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

/// The first page of a credit card statement: a minimum payment due beside a credit limit
/// or a due date.
fn is_card_page(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    // ("Mnlmum Payment Dua" in a poor text layer: the header's "Minimum Payment" over "Payment
    // Due" and the "Late Payment Warning" still name the card statement.)
    // (American Express: "Minimum Due" with "New Charges", "Pay Over Time" and the
    // "Payment Coupon"; "Closing Date" instead of a statement period.)
    l.contains("minimum payment") && (l.contains("credit limit") || l.contains("payment due") || l.contains("billing cycle") || l.contains("late payment warning"))
        || l.contains("minimum due") && l.contains("new charges") && (l.contains("pay over time") || l.contains("payment coupon") || l.contains("closing date"))
}

/// Words the later pages of a credit card statement carry (interest charge calculation,
/// purchases and cash advances, reward points) and a deposit statement does not.
fn card_words(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    l.contains("cash advance") || l.contains("reward point") || l.contains("interest charge") || l.contains("pay over time") || l.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "apr")
}

/// Words a bank prints on a statement page and a bookkeeping report does not.
fn statement_words(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    // ("For the Period 11/01/2018 to 11/30/2018", "Balance Summary" with the labels over the
    // values: PNC's first page, which prints "Beginning" and "balance" on different lines.)
    // (Not the court form's "for the period ending 01/31/2025": the bank's has two dates.)
    let period_range = l.find("for the period").map(|p| {
        let t: Vec<&str> = l[p + 14..].split_whitespace().take(3).collect();
        t.len() == 3 && parse_date_token(t[0]).is_some() && parse_date_token(t[2]).is_some()
    }).unwrap_or(false);
    // ("Daily Bal ance Information" in a scan: the words are matched with their spaces out.)
    let squashed: String = l.chars().filter(|c| !c.is_whitespace()).collect();
    // ("Statement Ending 05/31/2024" over "Summary of Accounts": Forcht Bank's first page.)
    period_range || ["beginning balance", "previous balance", "balance forward", "statement period", "account summary", "ending balance on", "daily balance", "member fdic", "balance summary", "primary account number", "statement ending", "summary of accounts"].iter().any(|w| l.contains(w))
        || ["dailybalance", "depositsandcredits", "checksanddebits"].iter().any(|w| squashed.contains(w))
}

/// An online-banking page printed to PDF: Bank of America's "Account Activity" with "available
/// as of today", Wells' "Account Detail" with "Pending Transactions" and an available balance.
/// It has no statement totals; its "Pending withdrawals/debits" figures are not ones.
fn is_printout_page(lower: &str) -> bool {
    // (A statement's own disclaimer, "The Ending Daily Balances provided do not reflect
    // pending transactions ... If your available balance wasn't sufficient", SunTrust, is
    // not a printout: the page prints its beginning and ending balance.)
    // (The disclaimer alone, on a last page with the daily balances and no summary, says
    // nothing either.)
    let statement_page = lower.contains("beginning balance") && lower.contains("ending balance") || lower.contains("do not reflect pending transactions");
    // (Chase's online activity export heads its rows "Date  Description  Type  Amount
    // Balance", the type being "Account transfer" or "Check".)
    let chase_export = lower.lines().any(|l| l.split_whitespace().collect::<Vec<_>>().join(" ") == "date description type amount balance");
    lower.contains("account activity") && (lower.contains("available as of today") || lower.contains("all transactions") || lower.contains("view: today") || lower.contains("view:today") || chase_export)
        || lower.contains("pending transactions") && lower.contains("available balance") && !statement_page
        || lower.contains("account detail -") && lower.contains("available balance")
}

/// Rows of "check number, MM/DD/YYYY, payee, $amount" and little else: a check register.
fn is_check_register_page(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let register_row = |l: &str| {
        let t: Vec<&str> = l.split_whitespace().collect();
        t.len() >= 3 && t[0].len() >= 3 && t[0].chars().all(|c| c.is_ascii_digit())
            && parse_date_token(t[1]).map(|(_, _, y)| y.is_some()).unwrap_or(false)
            && t.last().map(|a| a.starts_with('$') && is_amount_token(a)).unwrap_or(false)
    };
    // (Or the date first: "01/02/26  60532  JESUS SANCHEZ JR  $984.41" under a "Check
    // Register" title.)
    let dated_row = |l: &str| {
        let t: Vec<&str> = l.split_whitespace().collect();
        t.len() >= 4 && parse_date_token(t[0]).map(|(_, _, y)| y.is_some()).unwrap_or(false) && t[1].len() >= 3 && t[1].chars().all(|c| c.is_ascii_digit())
            && t.last().map(|a| a.starts_with('$') && is_amount_token(a)).unwrap_or(false)
    };
    let titled = register_titled(text);
    let rows = lines.iter().filter(|l| register_row(l)).count();
    let dated = lines.iter().filter(|l| dated_row(l)).count();
    rows >= 5 && rows * 10 >= lines.len() * 7 || titled && dated >= 5 && dated * 10 >= lines.len() * 6
}

/// A printed e-mail filed among the statements ("On Sep 11, 2018, at 9:55 AM, ... wrote:",
/// "Sent from my iPhone", "From: / Subject:"): its figures are not transactions.
fn is_email_page(text: &str) -> bool {
    let head: String = text.lines().filter(|l| !l.trim().is_empty()).take(15).collect::<Vec<_>>().join("\n").to_ascii_lowercase();
    let mail = head.contains("wrote:") || head.contains("sent from my") || (head.contains("from:") && head.contains("subject:")) || head.contains("sent:") && head.contains("subject:");
    mail && !statement_words(text)
}

/// A bookkeeper's check register by its title ("Check Register") or its header ("DATE
/// CHECK#  TO:  AMOUNT") in the first lines.
fn register_titled(text: &str) -> bool {
    text.lines().filter(|l| !l.trim().is_empty()).take(4).any(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("check register") || (l.contains("check#") || l.contains("check #")) && l.contains("amount") && l.contains("to:") && !l.contains("balance")
    })
}

/// A page of the court's Monthly Operating Report form ("Official Form 425C ... page 2",
/// "Monthly Operating Report for Small Business Under Chapter 11"), of its exhibits, or of
/// another non-deposit report filed with the statements.
fn is_court_form_page(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    if l.contains("official form 425") || l.contains("monthly operating report") && (l.contains("debtor") || l.contains("case number")) {
        return true;
    }
    // A court paper (caption "UNITED STATES BANKRUPTCY COURT ... In Re: ... Case") filed in
    // front of the statements: a trustee's report, a motion, a declaration.
    if l.contains("united states bankruptcy court") && (l.contains("in re") || l.contains("debtor")) {
        return true;
    }
    // A bank's investment sweep statement ("SWEEP REPO  MONTHLY ACTIVITY STATEMENT",
    // Hancock Whitney) lists securities and market values behind the checking statement.
    if l.contains("sweep repo") && l.contains("activity statement") {
        return true;
    }

    // The report's exhibits: the debtor's own receipts and disbursements list ("DATE
    // PURPOSE DESCRIPTION DEBIT CREDIT") and a card processor's settlement report
    // ("Balance To Date  Process Date  Transaction Date  Reason ... Merchant").
    // A bookkeeping general ledger ("Accrual Basis" over "Type  Date  Num  Adj  Name  Debit
    // Credit  Balance") filed behind the statement.
    // A bookkeeper's account register ("Register: Intrust Client Trust Account" over "Date
    // Number Payee Account Memo Payment C Deposit Balance") filed behind the statement.
    l.lines().take(8).any(|line| {
        let sq = line.split_whitespace().collect::<Vec<_>>().join(" ");
        sq.starts_with("date purpose description") || sq.contains("process date") && sq.contains("transaction date") && (sq.contains("merchant") || sq.contains("chain"))
            || sq == "accrual basis" || sq == "cash basis" || sq.starts_with("type date num")
            || sq.ends_with("transaction report") || sq.contains("memo/description") && sq.contains("split")
            || sq.starts_with("register:") || sq.starts_with("date number payee") && sq.contains("memo")
    })
}

/// A page headed "Reconciliation Report" (QuickBooks, Sage: "Cash Account Reconciliation Report").
fn is_reconciliation_page(text: &str) -> bool {
    // (A debtor's own "DIP Accounts - Reconciliation" sheet heads the same way; any
    // reconciliation title counts when the page prints no statement words.)
    let lower = text.to_ascii_lowercase();
    // (A bookkeeper's sheet signs off with "Prepared By:" / "Reviewed By:" and carries the
    // outstanding checks; it prints balances, so the statement words do not clear it.)
    let debtor_sheet = lower.contains("bankruptcy estate of") || lower.contains("dip account") || lower.contains("prepared by") || lower.contains("reviewed by") || lower.contains("outstanding checks");
    // (Not the court's exhibit stamp "Statements and Reconciliations - PART 13 Page 5 of 25".)
    // (A bookkeeper's "General Ledger" export is the same kind of page: the debtor's own
    // books, listed by account, not the bank's statement.)
    // (So are the exhibits of a monthly operating report: "A/P Aging Summary Report",
    // "Balance Sheet", "Profit and Loss", each titled in its first lines.)
    let general_ledger = text.lines().take(6).any(|l| { let l = l.to_ascii_lowercase(); ["general ledger", "generat ledger", "aging summary", "aging report", "balance sheet", "profit and loss", "profit & loss", "transaction summary"].iter().any(|w| l.contains(w)) }) && !statement_words(text);
    // (The court's exhibit stamp says "Statements and Reconciliations - PART 13 Page 5 of
    // 25"; a bookkeeper's "Bank Reconciliation Posting Report  Page 33" is not a stamp.)
    general_ledger || text.lines().take(14).any(|l| { let l = l.to_ascii_lowercase(); let stamp = l.contains("statements") || l.contains(" page ") && (l.contains("case") || l.contains("doc") || l.contains("filed") || l.contains(" of ")); l.contains("reconciliation report") || l.contains("reconciliation detail") || l.contains("reconciliation summary") || l.contains("reconciliation") && !stamp && (debtor_sheet || !statement_words(text)) })
}

/// "RECONCILIATION REPORT" with "Reconciled on" on the first pages is a bookkeeping
/// export, not a bank statement.
fn document_kind(pages: &[(usize, &str)]) -> Option<String> {
    let head: String = pages.iter().take(2).map(|(_, t)| t.to_ascii_lowercase()).collect::<Vec<_>>().join("\n");
    // A bank's ACH activity report (M&T: "TC  AMOUNT  INDIV NAME  INDIV ID NUMBER  SEC
    // COMPANY NAME  CO ID NO  DESCRIPTION" over every page) lists ACH items, not an account.
    let ach_header = |t: &str| { let l = t.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase(); l.contains("indiv name") && l.contains("co id") && l.contains("sec") };
    let ach_pages = pages.iter().filter(|(_, t)| ach_header(t)).count();
    if ach_pages >= 2 && ach_pages * 2 >= pages.len() {
        return Some("ACH activity report".into());
    }
    // A trustee's case-management ledger ("EXPENSE/DISBURSEMENT - TIP ACCOUNT",
    // "DEPOSIT/CREDIT") attached to a final report.
    let ledger_pages = pages.iter().filter(|(_, t)| { let l = t.to_ascii_lowercase(); l.contains("expense/disbursement") && l.contains("deposit/credit") }).count();
    if ledger_pages >= 2 && ledger_pages * 2 >= pages.len() {
        return Some("trustee ledger".into());
    }
    if head.contains("reconciliation report") && (head.contains("reconciled on") || head.contains("cleared transactions")) {
        return Some("reconciliation report".into());
    }
    // A debtor's monthly operating report without statements attached: balance sheet,
    // receipts and disbursements schedules for a "Reporting Period".
    // A bankruptcy petition with its schedules (Official Forms 201, 206, creditor lists).
    if (head.contains("voluntary petition") || head.contains("official form 201") || head.contains("official form 206")) && !pages.iter().any(|(_, t)| statement_words(t) && !is_court_form_page(t)) {
        return Some("bankruptcy petition and schedules".into());
    }
    // A declaration or affidavit ("DECLARATION OF WENDY KWUN IN SUPPORT OF MOTION FOR
    // SUMMARY JUDGMENT") whose exhibits are payment schedules, not statement pages.
    let sq = head.split_whitespace().collect::<Vec<_>>().join(" ");
    // (A plan's "Ending Balance on Effective Date" is one loose statement word; a statement
    // page has an opening balance or two of the words.)
    // (A plan's "Ending Balance on Effective Date $ 87,688" and a decision's prose about
    // "the beginning balance" carry statement words but no dated rows; a statement page
    // behind the court paper, an MOR's attached Bluevine month, has both.)
    let no_statement_page = || !pages.iter().any(|(_, t)| {
        let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
        let dated_rows = t.lines().filter(|line| {
            let toks: Vec<&str> = line.split_whitespace().collect();
            // "04/02", or U.S. Bank's "Apr 2" before the description.
            let dated = toks.first().map(|d| parse_date_token(d).is_some()).unwrap_or(false)
                || toks.len() >= 2 && months.contains(&toks[0].to_ascii_lowercase().trim_end_matches('.')) && toks[1].trim_end_matches(',').parse::<u8>().map(|d| (1..=31).contains(&d)).unwrap_or(false);
            toks.len() >= 3 && dated && toks.iter().any(|tok| is_amount_token(tok))
        }).count();
        // (Not excluding court-form pages: an exhibit's caption "In re: ..." can run on every
        // statement page.)
        // (One dated row is enough under a printed total: a fee page whose summary page is
        // still an unread scan.)
        let total_line = t.lines().any(|line| { let l = line.to_ascii_lowercase(); l.trim_start().starts_with("total") && line.split_whitespace().any(is_amount_token) });
        statement_words(t) && (dated_rows >= 2 || dated_rows == 1 && total_line) && !is_reconciliation_page(t)
    });
    if (sq.contains("declaration of") || sq.contains("affidavit of")) && (sq.contains("in support of") || sq.contains("case no")) && no_statement_page() {
        return Some("court declaration".into());
    }
    // The pre-2015 bankruptcy petition ("B1 (Official Form 1)") with its schedules and the
    // debtor's pay stubs, whose "Leave Balance Summary" is one loose statement word.
    if head.contains("(official form 1)") && no_statement_page() {
        return Some("bankruptcy petition and schedules".into());
    }
    // A debtor's own "DIP Accounts - Reconciliation" sheet, with nothing behind it.
    if pages.first().map(|(_, t)| is_reconciliation_page(t)).unwrap_or(false) && no_statement_page() {
        return Some("reconciliation report".into());
    }
    // Any other paper under the court's caption (an order confirming a plan, a motion)
    // with no statement page behind it; its exhibits' "Total Deposits" are estimates.
    if (sq.contains("united states bankruptcy court") || sq.contains("case no")) && (sq.contains("in re") || sq.contains("debtor")) && no_statement_page() {
        return Some("court filing".into());
    }
    // A debtor's own schedule of "Cash Disbursements" or "Cash Receipts" ("Per Bank
    // Statements", each row tagged with its account and designation) filed as an exhibit.
    let schedule_title = pages.first().map(|(_, t)| t.lines().take(8).any(|l| { let l = l.to_ascii_lowercase(); l.contains("cash disbursements") || l.contains("cash receipts") })).unwrap_or(false);
    // (Its "Statement Period" column is a statement word; a bank's page prints balances.)
    let no_balances = !pages.iter().any(|(_, t)| { let l = t.to_ascii_lowercase(); l.contains("beginning balance") || l.contains("ending balance") || l.contains("previous balance") });
    if schedule_title && (head.contains("per bank statement") || head.contains("account designation")) && no_balances {
        return Some("cash receipts and disbursements schedule".into());
    }
    // A district-court complaint (plaintiffs against funders, with the advances tabled in
    // the pleading) with no statement page behind it.
    if sq.contains("united states district court") && (sq.contains("plaintiff") || sq.contains("complaint")) && no_statement_page() {
        return Some("court filing".into());
    }
    // (An adversary proceeding's cover sheet (Form 1040) opens a complaint; its exhibits,
    // a funder's merchant agreement and the like, are not statements.)
    if sq.contains("adversary proceeding cover sheet") {
        return Some("court filing".into());
    }
    // (A criminal complaint (form AO 91) with an agent's affidavit: the account pictures
    // in it are evidence excerpts, not statements.)
    if sq.contains("criminal complaint") && (sq.contains("ao 91") || sq.contains("united states district court")) {
        return Some("court filing".into());
    }
    // (Or a debtor's own "STATEMENT OF RECEIPTS AND DISBURSEMENTS FOR THE PERIOD ...", a
    // schedule of payments by card, not a bank's statement.)
    let mor_page = |t: &str| is_court_form_page(t) || { let l = t.to_ascii_lowercase(); l.contains("reporting period") && (l.contains("balance sheet") || l.contains("schedule of cash") || l.contains("receipts and disbursements")) || l.contains("statement of receipts and disbursements") };
    // (The court's exhibit cover may come first: the form is on page one or two.)
    if pages.iter().take(2).any(|(_, t)| mor_page(t)) && !pages.iter().any(|(_, t)| statement_words(t) && !is_court_form_page(t) && !mor_page(t)) {
        return Some("monthly operating report".into());
    }
    // A Chapter 7 trustee's Form 2 ledger of the estate account.
    if head.contains("receipts and disbursements record") || head.contains("form 2 - estate cash") {
        return Some("trustee form 2 ledger".into());
    }
    // A property's operating statement (income and expense by account code, year-to-date
    // beside the month): a bookkeeper's report, not a bank's.
    if (head.contains("operating statement") || head.contains("income statement") || head.contains("profit and loss") || head.contains("profit & loss")) && (head.contains("year-to-date") || head.contains("total income") || head.contains("expense")) && !pages.iter().any(|(_, t)| statement_words(t) && !t.to_ascii_lowercase().contains("operating statement")) {
        return Some("operating statement".into());
    }
    // An FX trading account's statement ("Fx Spot & Forwards  USDJPY ...", currency pairs
    // on row after row): a broker's positions, not a bank account.
    let pairs = ["usdjpy", "eurusd", "usdtry", "usdcad", "gbpusd", "usdchf", "gbpchf", "audusd", "eurchf", "eurgbp"];
    let fx_pages = pages.iter().filter(|(_, t)| { let l = t.to_ascii_lowercase(); l.matches("spot").count() >= 3 && pairs.iter().filter(|p| l.contains(*p)).count() >= 2 }).count();
    if fx_pages >= 1 && !pages.iter().any(|(_, t)| { let l = t.to_ascii_lowercase(); l.contains("member fdic") }) {
        return Some("foreign exchange statement".into());
    }
    // A point-of-sale system's sales report ("Sales summary ... Net sales ... Sales by day",
    // a restaurant's card terminal): the merchant's own sales, not a bank's account.
    let pos = |l: &str| (l.contains("sales summary") || l.contains("salessummary")) && (l.contains("net sales") || l.contains("sales by day") || l.contains("revenue summary"));
    if pages.iter().take(2).any(|(_, t)| pos(&t.to_ascii_lowercase())) && no_statement_page() {
        return Some("point of sale sales report".into());
    }
    // A utility bill ("Meter Number ... Prior Read ... Current Read ... Usage"): a bill, not
    // an account statement.
    let utility = |l: &str| l.contains("meter") && (l.contains("read date") || l.contains("prior read") || l.contains("current read")) && (l.contains("usage") || l.contains("ccf") || l.contains("kwh") || l.contains("therms"));
    // (A bill's "Previous Balance" is a statement word; a bill has no dated rows of a ledger.)
    if pages.iter().take(2).any(|(_, t)| utility(&t.to_ascii_lowercase())) && no_statement_page() {
        return Some("utility bill".into());
    }
    // An online store's order receipt ("Order Total: $77.10", "Shipping & Handling: $11.28",
    // "Free Shipping: -$11.28") filed as an exhibit: a purchase, not an account.
    let receipt = |l: &str| (l.contains("order total") || l.contains("grand total")) && (l.contains("shipping & handling") || l.contains("items subtotal") || l.contains("order placed") || l.contains("total before tax"));
    if pages.iter().any(|(_, t)| receipt(&t.to_ascii_lowercase())) && !pages.iter().any(|(_, t)| statement_words(t)) {
        return Some("order receipt".into());
    }
    // Recorded real estate papers (deeds, mortgages, assignments of leases, each with the
    // county's "Recording Fee" cover sheet): land records, not a bank account.
    let recording = pages.iter().filter(|(_, t)| { let l = t.to_ascii_lowercase(); l.contains("recording fee") || l.contains("realty transfer fee") || l.contains("record and return to") || l.contains("recorded inst") }).count();
    if recording >= 2 && !pages.iter().any(|(_, t)| statement_words(t)) {
        return Some("recorded real estate documents".into());
    }
    // A foreign bank's statement in another currency ("Currency: UAE Dirham", Dubai Islamic
    // Bank in a terrorism-litigation exhibit): not a U.S. merchant's account.
    // (Or a translated one, "(Same language in Arabic)" on every label.)
    let foreign = pages.iter().filter(|(_, t)| { let l = t.to_ascii_lowercase(); l.contains("same language in arabic") || l.contains("currency:") && !l.contains("currency: usd") && !l.contains("currency: us dollar") && (l.contains("dirham") || l.contains("euro") || l.contains("pound") || l.contains("riyal") || l.contains("dinar") || l.contains("rupee") || l.contains("peso") || l.contains("yuan") || l.contains("aed") || l.contains("eur ") || l.contains("gbp")) }).count();
    if foreign >= 1 && !pages.iter().any(|(_, t)| { let l = t.to_ascii_lowercase(); l.contains("member fdic") || l.contains("fdic") }) {
        return Some("foreign currency statement".into());
    }
    // A prosecutor's summary charts (loan tables, then "X1441 Account Activity" ledgers with
    // "Payee/Payor" columns, each sourced to a government exhibit "Source: GX #1.s"): the
    // government's retelling of accounts, not the bank's statements.
    let chart_pages = pages.iter().filter(|(_, t)| { let l = t.to_ascii_lowercase(); l.contains("source: gx") || l.contains("payee/payor") && l.contains("account activity") }).count();
    // (Or one such ledger among pages of charts stamped "DRAFT".)
    let draft_pages = pages.iter().filter(|(_, t)| t.lines().take(12).any(|l| l.split_whitespace().any(|w| w == "DRAFT"))).count();
    if (chart_pages >= 2 || chart_pages >= 1 && draft_pages >= 3) && !pages.iter().any(|(_, t)| statement_words(t) && !t.to_ascii_lowercase().contains("source: gx")) {
        return Some("court exhibit charts".into());
    }
    // PayPal merchant activity statements (Gross / Fee / Net columns) are not bank accounts.
    if head.contains("paypal id:") || head.contains("merchant account id:") {
        return Some("PayPal merchant statement".into());
    }
    // Brokerage statements (Fidelity "Investment Report": market values, margin interest).
    if head.contains("investment report") || head.contains("beginning market value") && head.contains("ending market value") {
        return Some("brokerage statement".into());
    }
    // Credit card statements have no deposits; their totals are purchases and payments.
    // (Or a card page opens the document and no page behind it is a deposit statement's:
    // a filing of American Express months, whose detail pages carry no card words.)
    let card_document = pages.first().map(|(_, t)| is_card_page(t)).unwrap_or(false) && !pages.iter().any(|(_, t)| statement_words(t) && !is_card_page(t) && !card_words(t));
    if head.contains("credit card statement") || (head.contains("minimum payment due") && head.contains("credit limit")) || card_document {
        return Some("credit card statement".into());
    }
    None
}

fn parse_statements(pages: &[(usize, &str)]) -> Ledger {
    let unique = drop_duplicate_pages(pages);
    let split = split_sub_accounts(&unique);
    let forced: Vec<bool> = split.iter().map(|(_, _, sub)| *sub).collect();
    let pages: &[(usize, &str)] = &split.iter().map(|(p, t, _)| (*p, t.as_str())).collect::<Vec<_>>();
    let segments = segment_statements(pages, &forced);
    if segments.len() <= 1 {
        let mut ledger = parse_one(pages);
        ledger.summary.missing_pages = pages_missing(pages);
        derive(&mut ledger);
        return ledger;
    }
    let mut combined = Ledger::default();
    for seg in &segments {
        let mut part = parse_one(seg);
        part.summary.pages = seg.first().zip(seg.last()).map(|(a, b)| (a.0, b.0));
        part.summary.missing_pages = pages_missing(seg);
        part.summary.parsed_credits = Some(part.transactions.iter().filter(|t| t.kind == Kind::Credit && !part.netted.contains(&t.id)).map(|t| t.amount).sum());
        part.summary.parsed_debits = Some(part.transactions.iter().filter(|t| t.kind == Kind::Debit && !part.netted.contains(&t.id)).map(|t| t.amount).sum());
        let (id_off, table_off) = (combined.transactions.len(), combined.transactions.iter().map(|t| t.table).max().unwrap_or(0) + 1);
        combined.netted.extend(part.netted.iter().map(|id| id + id_off));
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

/// The bank's own "Page N of M" footers of one statement, one per physical page (the
/// court's stamp, "Page 14 of 63" next to a case number or "NYSCEF", is not one).
fn footer_numbers(line: &str) -> Option<(usize, usize)> {
    footer_numbers_min(line, 2)
}

/// The same with the least page count accepted: 2 for a footer (a "Page 1 of 1" tells
/// nothing about missing pages), 1 for the single-page statement's opening page.
fn footer_numbers_min(line: &str, min_pages: usize) -> Option<(usize, usize)> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("case ") || lower.contains("doc") || lower.contains("nyscef") || lower.contains("exhibit") || lower.contains("statements pg") {
        return None;
    }
    let toks: Vec<&str> = lower.split_whitespace().collect();
    // "3of4" (a squeezed text layer glues the words: "May31,2021 • Page3of4", "Page 2of5")
    // (A bank's statement runs to a few dozen pages; "REDE Page 224 of253" is the court's
    // exhibit stamp, whose count would swallow every statement in the filing.)
    let glued = |s: &str| -> Option<(usize, usize)> {
        let (a, b) = s.split_once("of")?;
        let (n, m) = (a.parse::<usize>().ok()?, b.trim_matches(|c: char| !c.is_ascii_digit()).parse::<usize>().ok()?);
        (n >= 1 && n <= m && m >= min_pages && m <= 120).then_some((n, m))
    };
    for (i, t) in toks.iter().enumerate() {
        let Some(rest) = t.strip_prefix("page") else { continue };
        let rest = rest.trim_start_matches(':');
        // The words after "page", glued back together: "3 of 4", "3of4", "1 of6" (with
        // "page1", the count itself).
        let following: String = toks[i + 1..].iter().take(3).map(|t| *t).collect::<Vec<_>>().join("");
        let found = if rest.is_empty() {
            glued(&following)
        } else {
            glued(rest).or_else(|| rest.chars().all(|c| c.is_ascii_digit()).then(|| glued(&format!("{rest}{following}"))).flatten())
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The line carrying the bank's footer, so an OCR re-read of the page (which drops the
/// footer) can keep it.
/// (A court stamp wrapped onto two lines puts its "Page 16 of 16" on the line under
/// "Case 20-10846 Doc 3775-9 Filed ...": that line is the stamp's, not the bank's.)
pub fn footer_line(text: &str) -> Option<&str> {
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().enumerate().find(|(i, l)| footer_numbers(l).is_some() && !(*i >= 1 && is_court_stamp(&lines[i - 1].to_ascii_lowercase()))).map(|(_, l)| *l)
}

pub fn footer_pages(text: &str) -> Option<(usize, usize)> {
    footer_line(text).and_then(footer_numbers)
}

/// Two of a statement's footers lie further apart in print than in the copy ("Page 3 of
/// 14" and "Page 5 of 14" on neighbouring physical pages): pages were left out of the
/// filing. A copy cut short at the end (the last pages of ads dropped) is not missing
/// anything that carries lines and is not flagged, unless the last page present still
/// lists rows ("Page 5 of 8" ending in transactions: the listing went on).
fn pages_missing(pages: &[(usize, &str)]) -> bool {
    let footers: Vec<(usize, usize, usize)> = pages.iter().enumerate().filter_map(|(i, (_, t))| footer_pages(t).map(|(n, m)| (i, n, m))).collect();
    let Some(&(_, _, total)) = footers.first() else { return false };
    let same: Vec<(usize, usize)> = footers.iter().filter(|(_, _, m)| *m == total).map(|(i, n, _)| (*i, *n)).collect();
    if same.windows(2).any(|w| w[1].1 > w[0].1 && w[1].1 - w[0].1 > w[1].0 - w[0].0) {
        return true;
    }
    let Some((last, text)) = pages.last() else { return false };
    let cut_short = footers.last().map(|&(i, n, m)| pages[i].0 == *last && m == total && n < m).unwrap_or(false);
    let rows = text.lines().filter(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        t.len() >= 3 && parse_date_token(t[0]).is_some() && t.iter().any(|x| is_amount_token(x))
    }).count();
    cut_short && rows >= 3
}

/// A filing sometimes carries the same statement page twice (KeyBank petty cash account,
/// pages 5 and 7 of one exhibit). Pages whose text repeats an earlier page's, apart from
/// the court's own header line, are dropped so nothing counts twice.
fn drop_duplicate_pages<'a>(pages: &[(usize, &'a str)]) -> Vec<(usize, &'a str)> {
    let body = |t: &str| -> String {
        // Court stamps differ between the two copies ("Page 14 of 63", "Statements Pg 16 of 63").
        let stamp = |l: &str| l.contains(" of ") && (l.contains("Page ") || l.contains("Pg ")) && (l.contains("Case ") || l.contains("Doc") || l.contains("NYSCEF") || l.contains("Statements") || l.contains("Exhibit"));
        t.lines()
            .filter(|l| !stamp(l))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    };
    // Two copies from different OCR passes differ in noise; the amounts printed on the
    // page (five or more) are the same, so they are the second signature.
    let amounts = |t: &str| -> Vec<i64> {
        let mut v: Vec<i64> = t.split_whitespace().filter(|w| is_amount_token(w)).filter_map(parse_amount).map(|a| (a.abs() * 100.0).round() as i64).collect();
        v.sort();
        v
    };
    let mut seen: Vec<String> = Vec::new();
    let mut seen_amounts: Vec<Vec<i64>> = Vec::new();
    let mut out = Vec::new();
    for &(page, text) in pages {
        let b = body(text);
        let a = amounts(text);
        // Only pages with rows can double a total; short pages (letterheads) stay.
        // (The amounts must be a listing's: two U.S. Bank first pages share the fee
        // schedule's six figures in their notices and are two statements' pages.)
        let dated_rows = text.lines().filter(|l| { let t: Vec<&str> = l.split_whitespace().collect(); t.len() >= 3 && parse_date_token(t[0]).is_some() && t.iter().any(|x| is_amount_token(x)) }).count();
        if b.split_whitespace().count() >= 40 && (seen.contains(&b) || a.len() >= 5 && dated_rows >= 3 && seen_amounts.contains(&a)) {
            continue;
        }
        seen.push(b);
        seen_amounts.push(a);
        out.push((page, text));
    }
    out
}

/// Credit unions print several sub-accounts on one statement, each under a heading like
/// "KASASA CASH (0008)" with its own summary and rows. Every sub-account becomes a
/// virtual page of its own (same page number), so it segments into its own statement.
fn split_sub_accounts(pages: &[(usize, &str)]) -> Vec<(usize, String, bool)> {
    let heading = |l: &str| {
        let t = l.trim();
        let name_ok = t.len() >= 8 && t.ends_with(')') && t.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 4 && !t.chars().any(|c| c.is_ascii_lowercase());
        name_ok && t.rfind('(').map(|i| t[i + 1..t.len() - 1].chars().all(|c| c.is_ascii_digit()) && t.len() - i - 2 == 4).unwrap_or(false)
    };
    let mut out = Vec::new();
    for &(page, text) in pages {
        let lines: Vec<&str> = text.lines().collect();
        // A heading counts when the summary labels follow within two lines. The page is cut
        // at every heading; the letterhead above the first one stays a chunk of its own.
        // Another credit union prints the heading over three lines: "(ID" / "KASASA CASH
        // BACK" / "0008)" with "Balance Forward" below.
        let split_heading = |i: usize| {
            lines[i].trim() == "(ID" && lines.get(i + 2).map(|l| { let t = l.trim(); t.len() == 5 && t.ends_with(')') && t[..4].chars().all(|c| c.is_ascii_digit()) }).unwrap_or(false)
        };
        // Achieva heads each account "BUSINESS ESSENTIAL CHECKING 0750" / "BUSINESS SAVINGS
        // 0849", the column header between it and "Beginning Balance".
        let caps_heading = |l: &str| {
            let t = l.trim();
            let toks: Vec<&str> = t.split_whitespace().collect();
            let lower = t.to_ascii_lowercase();
            (2..=8).contains(&toks.len()) && toks.last().map(|n| n.len() == 4 && n.chars().all(|c| c.is_ascii_digit())).unwrap_or(false)
                && !t.chars().any(|c| c.is_ascii_lowercase()) && t.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 6
                && ["checking", "savings", "share", "market", "account"].iter().any(|w| lower.contains(w))
        };
        // Navy Federal: "Business Checking - 7125242482" / "Mbr Business Savings - 3150735946".
        let dashed_heading = |l: &str| {
            let t = l.trim();
            let toks: Vec<&str> = t.split_whitespace().collect();
            let lower = t.to_ascii_lowercase();
            (3..=6).contains(&toks.len()) && t.contains(" - ") && toks.last().map(|n| n.len() >= 4 && n.chars().all(|c| c.is_ascii_digit())).unwrap_or(false)
                && ["checking", "savings", "share", "market"].iter().any(|w| lower.contains(w))
        };
        // (Truist: "TRUIST DYNAMIC BUSINESS CHECKING - CORE TIER 4952" over "Account summary"
        // and "Your previous balance as of 06/22/2026".)
        // Blank lines between the heading and its summary are not counted (Truist
        // leaves two before "Account summary" and two more before the balance line;
        // Navy Federal two around the table header before its "Beginning Balance" row).
        let beginning_within = |i: usize, n: usize| lines[i + 1..].iter().filter(|l| !l.trim().is_empty()).take(n).any(|l| { let l = l.to_ascii_lowercase(); l.contains("beginning") || l.contains("previous balance") || l.contains("balance forward") });
        let starts: Vec<usize> = (0..lines.len())
            .filter(|&i| heading(lines[i]) && beginning_within(i, 2) || (caps_heading(lines[i]) || dashed_heading(lines[i])) && beginning_within(i, 4) || split_heading(i))
            .collect();
        if starts.is_empty() {
            out.push((page, text.to_string(), false));
            continue;
        }
        // A category word above the heading ("Savings" over "Mbr Business Savings -
        // 3150735946", Navy Federal) belongs to the new account, not to the last row of the
        // listing before it, which would otherwise take it as a wrapped description.
        let starts: Vec<usize> = starts.iter().map(|&i| {
            let mut j = i;
            while j > 0 && lines[j - 1].trim().is_empty() {
                j -= 1;
            }
            let category = j > 0 && {
                let l = lines[j - 1].trim().to_ascii_lowercase();
                matches!(l.as_str(), "checking" | "savings" | "loans" | "money market" | "certificates")
            };
            if category { j - 1 } else { i }
        }).collect();
        let mut cuts = vec![0];
        cuts.extend(starts.iter().copied());
        cuts.push(lines.len());
        cuts.dedup();
        for w in cuts.windows(2) {
            // Loan sub-accounts ("2015 GMC YUKON (0001)": payment due, percentage rate)
            // are not deposit accounts; their lines are left out.
            let head = lines[w[0]..(w[0] + 4).min(w[1])].join(" ").to_ascii_lowercase();
            if head.contains("percentage rate") || head.contains("past due") || head.contains("payment amount") {
                continue;
            }
            // A chunk that begins at a heading is a sub-account of its own (true); the
            // letterhead chunk before the first heading is not.
            out.push((page, lines[w[0]..w[1]].join("\n"), starts.contains(&w[0])));
        }
    }
    out
}

/// One statement's worth of pages, no derived facts yet.
/// True when the listing keeps one amount column and signs its debits there, "($918.75)"
/// beside "$19,000.00" (a bank verification report, an online printout). The sign is then
/// the kind, whatever the row's words say.
///
/// Read over the whole statement, not one page: a page of a busy account may hold only
/// deposits, and its two signed rows would not show the shape on their own. Three rows of
/// each kind are wanted, so a per-type listing whose amounts all run one way is still read
/// from its heading.
fn signs_the_debits(texts: &[&str]) -> bool {
    let mut signs: Vec<bool> = Vec::new();
    for text in texts {
        for l in text.lines() {
            // (An online printout signs with the minus sign, "\u{2212}$56,000.00".)
            let l = l.replace('\u{2212}', "-");
            let t: Vec<&str> = l.split_whitespace().collect();
            // A row of the listing: it opens with its date, or (where the date is printed
            // once for the day) it ends in its amount and its running balance.
            let row = t.len() >= 3 && (parse_date_token(t[0]).is_some() || is_amount_token(t[t.len() - 1]) && is_amount_token(t[t.len() - 2]));
            // (A line with two dates or more is a daily balance table, "07-31  4,980.65  08-10
            // 343.84  08-22  -7,570.76": an overdrawn day there is a balance, not a debit.)
            if !row || t.iter().filter(|x| parse_date_token(x).is_some()).count() >= 2 {
                continue;
            }
            // Only rows that carry a running balance after the amount, or the dash a report prints
            // for a balance it does not repeat: a listing with one figure to a row is the per-page
            // `signed_page` rule's (Bluevine, an online export), and stays with it.
            let last = t[t.len() - 1];
            let dash = matches!(last, "-" | "\u{2014}" | "\u{2013}");
            let pair = is_amount_token(last) && is_amount_token(t[t.len() - 2]);
            if !dash && !pair {
                continue;
            }
            let amount = t[t.len() - 2];
            if !is_amount_token(amount) {
                continue;
            }
            signs.push(amount.starts_with('(') || amount.starts_with("$(") || amount.starts_with('-') || amount.starts_with("$-") || amount.starts_with("-$"));
        }
    }
    let neg = signs.iter().filter(|s| **s).count();
    // Signed and unsigned rows must take turns, or the signed ones must be the majority:
    // an unsigned "Subtractions" list followed by a short signed fee list (KeyBank) changes
    // sign once, and its unsigned rows are debits, not credits.
    let changes = signs.windows(2).filter(|w| w[0] != w[1]).count();
    // (And a listing that signs its debits signs a real share of its rows: three stray negatives
    // among four hundred plain rows are something else, a misread or a balance.)
    neg >= 3 && signs.len() - neg >= 3 && neg * 10 >= signs.len() && (changes >= 2 || neg * 2 > signs.len())
}

fn parse_one(pages: &[(usize, &str)]) -> Ledger {
    let mut ledger = Ledger::default();
    let texts: Vec<&str> = pages.iter().map(|(_, t)| *t).collect();
    let year = year_hint(&texts);
    ledger.summary.bank = detect_bank(&texts);
    let mut st = State::default();
    st.signed_amounts = signs_the_debits(&texts);
    for (page, text) in pages {
        let unfolded = unfold_two_columns(text);
        if std::env::var("MCA_DUMP_UNFOLDED").is_ok() {
            eprintln!("--- unfolded page {page} ---\n{unfolded}");
        }
        parse_page(&unfolded, *page, year, &mut ledger, &mut st);
        // A page that printed no section header of its own (a continuation the OCR
        // stripped) has one kind of row; a word-decided straggler against a page of ten or
        // more rows of the other kind ("Barclaycard US Creditcard" under withdrawals,
        // "Deposited Item Retn Unpaid") follows the page.
        // (Not on a page with its own column header naming deposits and withdrawals or a
        // balance: there the columns and the balance arithmetic decided every kind, and a
        // lone "DEPOSIT $6,000.00" among thirty card purchases is right as it is.)
        let own_columns = unfolded.lines().any(|l| { let lab = Columns::labels(l); lab.count() >= 2 && !l.split_whitespace().any(|t| is_amount_token(t)) });
        // (Nor where the amount column signs its debits: there every kind came from a sign,
        // and a lone signed row among a page of deposits is right as it is.)
        if st.section.is_some() && st.section_page != Some(*page) && !own_columns && !st.signed_amounts {
            let rows: Vec<usize> = ledger.transactions.iter().enumerate().filter(|(_, t)| t.page == *page).map(|(i, _)| i).collect();
            let credits = rows.iter().filter(|&&i| ledger.transactions[i].kind == Kind::Credit).count();
            let debits = rows.len() - credits;
            let (minority, majority) = if credits < debits { (credits, Kind::Debit) } else { (debits, Kind::Credit) };
            if rows.len() >= 10 && minority >= 1 && minority * 10 <= rows.len() {
                for &i in &rows {
                    ledger.transactions[i].kind = majority;
                }
            }
        }
    }
    // Two or more debit categories in the summary block add up to the debit total; a single
    // signed one ("Checks Paid 2,675.62-", U.S. Bank) is the total when nothing else names it.
    // (One part that is the sum of the others is the total itself, the rest its breakdown:
    // Union Bank's "Subtractions -134.82" over "Purchases -66.82" and "Other Withdrawals
    // -68.00".)
    let whole_of = |parts: &[f64]| -> Option<f64> {
        let total: f64 = parts.iter().sum();
        parts.iter().copied().find(|p| parts.len() >= 3 && (total - 2.0 * p).abs() <= 0.011)
    };
    if ledger.summary.debit_parts.len() >= 2 || ledger.summary.debit_parts.len() == 1 && ledger.summary.total_debits.is_none() && ledger.summary.debit_parts_unsigned.is_empty() {
        ledger.summary.total_debits = Some(whole_of(&ledger.summary.debit_parts).unwrap_or_else(|| ledger.summary.debit_parts.iter().sum()));
        ledger.summary.debits_key = "summary parts (checks and service fees included)";
    }
    // Unsigned categories fill in totals the statement never prints as one figure, and
    // two or more of them outrank a bare "Debits 42 28,151.29" line that is only one category.
    if !ledger.summary.credit_parts.is_empty() && (ledger.summary.total_credits.is_none() || ledger.summary.credit_parts.len() >= 2) {
        ledger.summary.total_credits = Some(whole_of(&ledger.summary.credit_parts).unwrap_or_else(|| ledger.summary.credit_parts.iter().sum()));
    }
    if !ledger.summary.debit_parts_unsigned.is_empty() && (ledger.summary.total_debits.is_none() || ledger.summary.debit_parts_unsigned.len() >= 2) {
        ledger.summary.total_debits = Some(ledger.summary.debit_parts_unsigned.iter().sum());
        ledger.summary.debits_key = "summary parts (checks and service fees included)";
    }
    // One signed and one unsigned debit category (U.S. Bank when the OCR drops a trailing
    // "-": "Other Withdrawals 7,818.26" and "Checks Paid 2,315.99-") are both debits.
    if ledger.summary.debit_parts.len() == 1 && !ledger.summary.debit_parts_unsigned.is_empty() && ledger.summary.debits_key == "summary parts (checks and service fees included)" {
        ledger.summary.total_debits = Some(ledger.summary.debit_parts.iter().sum::<f64>() + ledger.summary.debit_parts_unsigned.iter().sum::<f64>());
    }
    // Signed categories plus an unsigned one whose minus the scan lost ("Withdrawals
    // -$240.83", "Fees and Charges -$3.00", "Checks $438.24", KeyBank): the balance
    // equation says whether the unsigned figures belong to the debits.
    if ledger.summary.debit_parts.len() >= 2 && !ledger.summary.debit_parts_unsigned.is_empty() {
        if let (Some(b), Some(c), Some(e), Some(d)) = (ledger.summary.beginning_balance, ledger.summary.total_credits, ledger.summary.ending_balance, ledger.summary.total_debits) {
            let unsigned: f64 = ledger.summary.debit_parts_unsigned.iter().sum();
            if (b + c - d - e).abs() > 0.011 && (b + c - d - unsigned - e).abs() <= 0.011 {
                ledger.summary.total_debits = Some(d + unsigned);
            }
        }
    }
    // Checks and fees printed as separate figures are added unless the debit key already
    // covers them ("Checks and other debits", "... debits and service charges").
    // When the balances are printed, the balance equation decides: Yampa Valley's "11
    // Debit(s) This Period $62,906.55" already holds the $10.00 service charge listed under
    // it, since beginning + credits - debits is the ending balance as printed.
    if let Some(other) = ledger.summary.total_debits {
        let key = ledger.summary.debits_key;
        let checks = if key.contains("check") { 0.0 } else { ledger.summary.checks_total.unwrap_or(0.0) };
        let fees = if key.contains("service") { 0.0 } else { ledger.summary.fees_total.unwrap_or(0.0) };
        let s = &ledger.summary;
        // (Within a dollar, for a scan that misread a cent of a balance: "435,182.18" for
        // 435,182.13 still says the $10.00 fee is inside the $4,996.53.)
        let already_included = match (s.beginning_balance, s.total_credits, s.ending_balance) {
            (Some(b), Some(c), Some(e)) if checks + fees > 0.0 => {
                let (alone, added) = ((b + c - other - e).abs(), (b + c - other - checks - fees - e).abs());
                alone <= 1.0 && alone < added
            }
            _ => false,
        };
        if !already_included {
            ledger.summary.total_debits = Some(other + checks + fees);
        }
    }
    // Interest printed as its own summary figure is a credit when the printed credits do
    // not reach the ending balance without it (First Citizens: "0 Other Credits 0.00",
    // "Interest Earned This Period 53.12+").
    if let (Some(b), Some(c), Some(d), Some(e), Some(i)) = (ledger.summary.beginning_balance, ledger.summary.total_credits, ledger.summary.total_debits, ledger.summary.ending_balance, ledger.summary.interest_total) {
        if i > 0.0 && (b + c - d - e).abs() > 0.01 && (b + c + i - d - e).abs() <= 0.01 {
            ledger.summary.total_credits = Some(c + i);
        }
    }
    // The same when the summary's interest figure is unreadable ("+ INTEREST PAID 22272373",
    // a scan): the interest rows listed ("07/31  22,272.73  IOD INTEREST PAID") close the
    // balance equation exactly, so they are the missing credit.
    if let (Some(b), Some(c), Some(d), Some(e)) = (ledger.summary.beginning_balance, ledger.summary.total_credits, ledger.summary.total_debits, ledger.summary.ending_balance) {
        let interest: f64 = ledger.transactions.iter().filter(|t| t.kind == Kind::Credit && t.description.to_ascii_lowercase().contains("interest")).map(|t| t.amount).sum();
        if interest > 0.0 && (b + c - d - e).abs() > 0.01 && (b + c + interest - d - e).abs() <= 0.01 {
            ledger.summary.total_credits = Some(c + interest);
        }
    }
    fix_year_crossing(&mut ledger, year);
    merge_check_readings(&mut ledger);
    drop_repeated_copy(&mut ledger);
    dedup_across_tables(&mut ledger);
    net_reversals(&mut ledger);
    settle_verification_deposits(&mut ledger);
    settle_weak_by_section_kinds(&mut ledger);
    meet_totals(&mut ledger);
    use_alternates(&mut ledger);
    meet_section_totals(&mut ledger);
    settle_summary_by_sections(&mut ledger);
    meet_daily_balances(&mut ledger);
    // The interest row itself lost to the scan ("nS LL ee _ 0.18 scvsnemennnnnnannmnd 909-54",
    // Wells): the summary prints "Interest paid this statement $0.18", and the credits fall
    // short of their printed total by exactly that. The row is put back on the period's
    // last day, from the statement's own figure.
    if let (Some(i), Some(tc)) = (ledger.summary.interest_total, ledger.summary.total_credits) {
        let credits: f64 = ledger.transactions.iter().filter(|t| t.kind == Kind::Credit && !ledger.netted.contains(&t.id)).map(|t| t.amount).sum();
        let listed = ledger.transactions.iter().any(|t| t.kind == Kind::Credit && t.description.to_ascii_lowercase().contains("interest"));
        if i > 0.0 && !listed && (tc - credits - i).abs() < 0.005 {
            let date_tok = ledger.summary.period_end.clone().or_else(|| ledger.daily_balances.last().map(|b| b.date.clone()));
            if let Some(date_tok) = date_tok {
                let (date, day) = if date_tok.contains('-') && date_tok.len() == 10 { (date_tok.clone(), date_key(&date_tok).map(|(y, m, d)| days_from_civil(y, m, d))) } else { resolve_date(&date_tok, year) };
                let id = ledger.transactions.len();
                let page = ledger.transactions.last().map(|t| t.page).unwrap_or(pages.last().map(|p| p.0).unwrap_or(1));
                let table = ledger.transactions.last().map(|t| t.table).unwrap_or(0);
                ledger.transactions.push(Txn { id, date, day, kind: Kind::Credit, amount: i, description: "Interest paid (from the statement summary)".into(), page, table });
            }
        }
    }
    // A listing that prints no summary but a running balance on every row (an online
    // printout) still tells where it began and ended: the balance after the oldest row
    // less that row's change, and the balance after the newest. The rows between are
    // then held to the balance equation like any statement's.
    if ledger.summary.beginning_balance.is_none() && ledger.summary.ending_balance.is_none() && ledger.transactions.len() >= 3 && ledger.daily_balances.len() == ledger.transactions.len()
        && ledger.daily_balances.iter().zip(&ledger.transactions).all(|(b, t)| b.date == t.date)
    {
        let (first, last) = (&ledger.transactions[0], &ledger.transactions[ledger.transactions.len() - 1]);
        let newest_first = first.date > last.date;
        let (oldest, oldest_balance, newest_balance) = if newest_first {
            (last, ledger.daily_balances[ledger.daily_balances.len() - 1].balance, ledger.daily_balances[0].balance)
        } else {
            (first, ledger.daily_balances[0].balance, ledger.daily_balances[ledger.daily_balances.len() - 1].balance)
        };
        let change = if oldest.kind == Kind::Credit { oldest.amount } else { -oldest.amount };
        ledger.summary.beginning_balance = Some(((oldest_balance - change) * 100.0).round() / 100.0);
        ledger.summary.ending_balance = Some(newest_balance);
    }
    if let (Some(b), Some(e)) = (ledger.summary.beginning_balance, ledger.summary.ending_balance) {
        let c: f64 = ledger.transactions.iter().filter(|t| t.kind == Kind::Credit && !ledger.netted.contains(&t.id)).map(|t| t.amount).sum();
        let d: f64 = ledger.transactions.iter().filter(|t| t.kind == Kind::Debit && !ledger.netted.contains(&t.id)).map(|t| t.amount).sum();
        ledger.summary.balance_check = Some((b + c - d - e).abs() <= 0.011 && !ledger.transactions.is_empty());
    }
    // An online printout prints pending figures, never statement totals.
    if pages.first().map(|(_, t)| is_printout_page(&t.to_ascii_lowercase())).unwrap_or(false) {
        ledger.summary.total_credits = None;
        ledger.summary.total_debits = None;
    }
    ledger
}

/// The printed totals settle a row whose kind nothing else decided: when the credits fall
/// short of their total by exactly what the debits exceed theirs, and exactly one row of
/// that amount has a defaulted kind, that row is on the wrong side (Wells: "12/17 Agron
/// Inc PC Clear ... 11,615.93" on a page whose day never printed its balance).
fn meet_totals(ledger: &mut Ledger) {
    let (Some(tc), Some(td)) = (ledger.summary.total_credits, ledger.summary.total_debits) else { return };
    let sum = |ledger: &Ledger, k: Kind| ledger.transactions.iter().filter(|t| t.kind == k).map(|t| t.amount).sum::<f64>();
    let (c, d) = (sum(ledger, Kind::Credit), sum(ledger, Kind::Debit));
    let (short_c, over_d) = (tc - c, d - td);
    if short_c.abs() < 0.01 || (short_c - over_d).abs() > 0.01 {
        return;
    }
    // Credits short by X and debits over by X: a debit of X belongs to the credits (and the
    // mirror image).
    let (from, x) = if short_c > 0.0 { (Kind::Debit, short_c) } else { (Kind::Credit, -short_c) };
    let candidates: Vec<usize> = ledger.weak.iter().copied().filter(|id| ledger.transactions.get(*id).map(|t| t.kind == from && (t.amount - x).abs() < 0.005).unwrap_or(false)).collect();
    if candidates.len() == 1 {
        let t = &mut ledger.transactions[candidates[0]];
        t.kind = if from == Kind::Debit { Kind::Credit } else { Kind::Debit };
    }
}

/// A date as (year, month, day) for ordering: ISO from a resolved row, else "MM/DD".
fn date_key(date: &str) -> Option<(i32, u32, u32)> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() == 3 {
        return Some((parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?));
    }
    parse_date_token(date).map(|(m, d, y)| (y.unwrap_or(0), m, d))
}

/// Every repair of one row among `rows` that moves their signed sum by `miss` cents: a digit
/// dropped ("41,500.00" for 1,500.00), a digit read in ("130.00" for 3,130.00), a digit
/// swapped, a kind flipped, or an alternate reading taken. `sign` is +1 for a
/// credit's contribution, -1 for a debit's. Returns (row, new amount in cents, new kind).
fn single_row_repairs(ledger: &Ledger, rows: &[usize], miss: i64, allow_flip: bool) -> Vec<(usize, i64, Kind)> {
    let cents = |v: f64| (v * 100.0).round() as i64;
    let mut fixes: Vec<(usize, i64, Kind)> = Vec::new();
    for &r in rows {
        let t = &ledger.transactions[r];
        let a = cents(t.amount);
        let sign = if t.kind == Kind::Credit { 1 } else { -1 };
        // (A flip is offered for any row: "PODIUM PAYMENTS" reads as a debit by its word
        // and is the day's deposit by its balance; the statement's totals then decide.)
        if allow_flip && -2 * sign * a == miss {
            fixes.push((r, a, if t.kind == Kind::Credit { Kind::Debit } else { Kind::Credit }));
        }
        for &(row, alt) in &ledger.alternates {
            if row == r && sign * (cents(alt) - a) == miss && !fixes.contains(&(r, cents(alt), t.kind)) {
                fixes.push((r, cents(alt), t.kind));
            }
        }
        let digits = a.to_string();
        let mut tries: Vec<String> = Vec::new();
        for pos in 0..=digits.len() {
            if pos < digits.len() {
                if digits.len() > 1 {
                    tries.push(format!("{}{}", &digits[..pos], &digits[pos + 1..]));
                }
                for d in '0'..='9' {
                    tries.push(format!("{}{d}{}", &digits[..pos], &digits[pos + 1..]));
                }
            }
            // (A digit read in: only in front of or inside the dollars, never new cents.)
            if digits.len() >= 3 && pos <= digits.len() - 2 {
                for d in '0'..='9' {
                    tries.push(format!("{}{d}{}", &digits[..pos], &digits[pos..]));
                }
            }
        }
        for t2 in tries {
            let Ok(new) = t2.parse::<i64>() else { continue };
            if new != a && new > 0 && sign * (new - a) == miss && !fixes.contains(&(r, new, t.kind)) {
                fixes.push((r, new, t.kind));
            }
        }
    }
    fixes
}

/// A section's printed total settles a misread figure among its rows ("Total checks =
/// $3,130.00" over one check read as 130.00), when exactly one single-row repair closes
/// it and the printed statement totals are met once it is made: two printed figures
/// agree on the repair, the section's and the statement's.
fn meet_section_totals(ledger: &mut Ledger) {
    if ledger.section_totals.is_empty() || ledger.summary.total_credits.is_none() && ledger.summary.total_debits.is_none() {
        return;
    }
    let cents = |v: f64| (v * 100.0).round() as i64;
    let gap = |l: &Ledger| -> Option<f64> {
        let c: f64 = l.transactions.iter().filter(|t| t.kind == Kind::Credit && !l.netted.contains(&t.id)).map(|t| t.amount).sum();
        let d: f64 = l.transactions.iter().filter(|t| t.kind == Kind::Debit && !l.netted.contains(&t.id)).map(|t| t.amount).sum();
        match (l.summary.total_credits, l.summary.total_debits) {
            (None, None) => None,
            (tc, td) => Some(tc.map(|v| (v - c).abs()).unwrap_or(0.0) + td.map(|v| (v - d).abs()).unwrap_or(0.0)),
        }
    };
    let totals = ledger.section_totals.clone();
    for (table, total) in totals {
        let rows: Vec<usize> = ledger.transactions.iter().enumerate().filter(|(_, t)| t.table == table && !ledger.netted.contains(&t.id)).map(|(i, _)| i).collect();
        if rows.is_empty() || rows.iter().any(|&r| ledger.transactions[r].kind != ledger.transactions[rows[0]].kind) {
            continue;
        }
        let kind = ledger.transactions[rows[0]].kind;
        let sum: i64 = rows.iter().map(|&r| cents(ledger.transactions[r].amount)).sum();
        let miss = cents(total) - sum;
        if miss == 0 {
            continue;
        }
        // (The rows are all one kind, so the signed miss is the plain one for credits and
        // its negative for debits.)
        let fixes = single_row_repairs(ledger, &rows, if kind == Kind::Credit { miss } else { -miss }, false);
        if fixes.len() != 1 {
            continue;
        }
        let (r, new, _) = fixes[0];
        let old_amount = ledger.transactions[r].amount;
        ledger.transactions[r].amount = new as f64 / 100.0;
        if gap(ledger).map(|a| a > 0.011).unwrap_or(true) {
            ledger.transactions[r].amount = old_amount;
        }
    }
}

/// A section whose title the scan lost still names its kind in its total line ("Total
/// Deposits and Additions $74,072.85" under rows headed only "DATE DESCRIPTION AMOUNT"):
/// rows of that table whose kind was only a default take the section's.
fn settle_weak_by_section_kinds(ledger: &mut Ledger) {
    if ledger.section_kinds.is_empty() || ledger.weak.is_empty() {
        return;
    }
    let kinds = ledger.section_kinds.clone();
    for &i in &ledger.weak {
        if let Some(t) = ledger.transactions.get_mut(i) {
            if let Some((_, k)) = kinds.iter().find(|(tb, _)| *tb == t.table) {
                t.kind = *k;
            }
        }
    }
}

/// A bank or payment app proves an account by sending two small deposits and pulling them
/// back in one debit ("INTUIT ACCTVERIFY" 0.15, 0.03 and 0.18 on one day). The words are the
/// same on all three, so they cannot say which is which; the amounts can: in a same-day group of
/// verification entries under a dollar, the one equal to the sum of the others is the debit and
/// the rest are credits.
fn settle_verification_deposits(ledger: &mut Ledger) {
    let verify = |d: &str| { let l = d.to_ascii_lowercase(); l.contains("acctverify") || l.contains("acct verify") || l.contains("verifybank") || l.contains("verify bank") || l.contains("trial deposit") || l.contains("micro deposit") || l.contains("microdeposit") };
    let mut groups: std::collections::BTreeMap<(String, usize), Vec<usize>> = std::collections::BTreeMap::new();
    for (i, t) in ledger.transactions.iter().enumerate() {
        if t.amount < 1.0 && verify(&t.description) {
            groups.entry((t.date.clone(), t.table)).or_default().push(i);
        }
    }
    for ids in groups.values().filter(|g| g.len() >= 3) {
        let total: f64 = ids.iter().map(|&i| ledger.transactions[i].amount).sum();
        let pulls: Vec<usize> = ids.iter().copied().filter(|&i| (ledger.transactions[i].amount * 2.0 - total).abs() < 0.005).collect();
        if pulls.len() == 1 {
            for &i in ids {
                ledger.transactions[i].kind = if i == pulls[0] { Kind::Debit } else { Kind::Credit };
            }
        }
    }
}

/// The section totals settle a misread statement total: when every section of a kind
/// prints its total, the rows of those sections add up to them to the cent, and the
/// statement's own figure for that kind is one digit off their sum (a scan's "75,193.44"
/// for 76,193.44, Chase), the statement figure is the misread one. Two printed witnesses
/// (the section totals and the rows) against one. A misread in one line of the summary
/// ("Electronic Withdrawals -48,108.11" over a section totalling 48,103.11) can move two
/// digits of the statement total; then the balance equation is the third witness: the rows
/// of both kinds carry the beginning balance to the ending one, and the other kind's
/// printed total is met.
fn settle_summary_by_sections(ledger: &mut Ledger) {
    if ledger.section_totals.is_empty() {
        return;
    }
    let cents = |v: f64| (v * 100.0).round() as i64;
    let one_digit_off = |a: i64, b: i64| {
        let (a, b) = (a.to_string(), b.to_string());
        a.len() == b.len() && a.chars().zip(b.chars()).filter(|(x, y)| x != y).count() == 1
    };
    for kind in [Kind::Credit, Kind::Debit] {
        let Some(total) = (if kind == Kind::Credit { ledger.summary.total_credits } else { ledger.summary.total_debits }) else { continue };
        let tables: Vec<usize> = { let mut t: Vec<usize> = ledger.transactions.iter().filter(|t| t.kind == kind && !ledger.netted.contains(&t.id)).map(|t| t.table).collect(); t.sort_unstable(); t.dedup(); t };
        // (A section carried over a page break prints its total once, under its last table:
        // a table with no total of its own is covered by the next one that has one.)
        let totalled = |tb: &usize| ledger.section_totals.iter().any(|(t, _)| t == tb);
        if tables.is_empty() || !tables.iter().all(|tb| totalled(tb) || tables.iter().any(|later| later > tb && totalled(later))) {
            continue;
        }
        let sections: i64 = tables.iter().map(|tb| ledger.section_totals.iter().find(|(t, _)| t == tb).map(|(_, v)| cents(*v)).unwrap_or(0)).sum();
        let rows: i64 = ledger.transactions.iter().filter(|t| t.kind == kind && !ledger.netted.contains(&t.id)).map(|t| cents(t.amount)).sum();
        let other = if kind == Kind::Credit { Kind::Debit } else { Kind::Credit };
        let other_rows: i64 = ledger.transactions.iter().filter(|t| t.kind == other && !ledger.netted.contains(&t.id)).map(|t| cents(t.amount)).sum();
        let other_met = (if other == Kind::Credit { ledger.summary.total_credits } else { ledger.summary.total_debits }).map(|v| cents(v) == other_rows).unwrap_or(false);
        let balance_closes = other_met && match (ledger.summary.beginning_balance, ledger.summary.ending_balance) {
            (Some(b), Some(e)) => {
                let (c, d) = if kind == Kind::Credit { (rows, other_rows) } else { (other_rows, rows) };
                cents(b) + c - d == cents(e)
            }
            _ => false,
        };
        if rows == sections && cents(total) != sections && (one_digit_off(cents(total), sections) || balance_closes) {
            let fixed = sections as f64 / 100.0;
            if kind == Kind::Credit { ledger.summary.total_credits = Some(fixed) } else { ledger.summary.total_debits = Some(fixed) }
        }
    }
}

/// The printed daily balances settle a misread figure. For each balance day the rows up
/// to it must move the balance by exactly the printed change; where one day is off, and
/// exactly one repair of one row closes it (a digit dropped, "41,500.00" for 1,500.00; a
/// digit swapped; a defaulted kind flipped), that repair is made. A day whose miss the
/// next day undoes is a misread balance, not a misread row, and is left alone. Nothing is
/// changed while the printed totals are met, and a repair stays only when it meets them
/// (or, with no totals printed, closes the balance equation): the day and the statement
/// must agree that this one figure was all that was wrong. A statement with rows missing
/// is left as it is rather than have another row bent to cover them.
fn meet_daily_balances(ledger: &mut Ledger) {
    let Some(beginning) = ledger.summary.beginning_balance else { return };
    if ledger.daily_balances.is_empty() {
        return;
    }
    let gap = |l: &Ledger| -> Option<f64> {
        let c: f64 = l.transactions.iter().filter(|t| t.kind == Kind::Credit && !l.netted.contains(&t.id)).map(|t| t.amount).sum();
        let d: f64 = l.transactions.iter().filter(|t| t.kind == Kind::Debit && !l.netted.contains(&t.id)).map(|t| t.amount).sum();
        match (l.summary.total_credits, l.summary.total_debits) {
            (None, None) => None,
            (tc, td) => Some(tc.map(|v| (v - c).abs()).unwrap_or(0.0) + td.map(|v| (v - d).abs()).unwrap_or(0.0)),
        }
    };
    if gap(ledger).map(|g| g <= 0.01).unwrap_or(false) {
        return;
    }
    // The closing balance of every printed day, in date order.
    let mut days: BTreeMap<(i32, u32, u32), f64> = BTreeMap::new();
    for b in &ledger.daily_balances {
        if let Some(k) = date_key(&b.date) {
            days.insert(k, b.balance);
        }
    }
    if days.len() < 2 {
        return;
    }
    let keyed: Vec<Option<(i32, u32, u32)>> = ledger.transactions.iter().map(|t| date_key(&t.date)).collect();
    let signed = |t: &Txn| if t.kind == Kind::Credit { t.amount } else { -t.amount };
    // (printed change minus parsed change, and the rows of the span) per balance day
    let mut misses: Vec<(f64, Vec<usize>)> = Vec::new();
    let mut prev_key: Option<(i32, u32, u32)> = None;
    let mut prev_balance = beginning;
    for (&k, &balance) in &days {
        let rows: Vec<usize> = ledger.transactions.iter().enumerate().filter(|(i, t)| !ledger.netted.contains(&t.id) && keyed[*i].map(|d| d <= k && prev_key.map(|p| d > p).unwrap_or(true)).unwrap_or(false)).map(|(i, _)| i).collect();
        let parsed: f64 = rows.iter().map(|&i| signed(&ledger.transactions[i])).sum();
        misses.push((balance - prev_balance - parsed, rows));
        prev_key = Some(k);
        prev_balance = balance;
    }
    let cents = |v: f64| (v * 100.0).round() as i64;
    // A kind whose printed total the rows already meet is not where the misread is.
    let sum = |l: &Ledger, k: Kind| cents(l.transactions.iter().filter(|t| t.kind == k && !l.netted.contains(&t.id)).map(|t| t.amount).sum::<f64>());
    let settled = |k: Kind| -> bool {
        match k { Kind::Credit => ledger.summary.total_credits, Kind::Debit => ledger.summary.total_debits }.map(|v| cents(v) == sum(ledger, k)).unwrap_or(false)
    };
    let (credits_settled, debits_settled) = (settled(Kind::Credit), settled(Kind::Debit));
    for i in 0..misses.len() {
        let (miss, rows) = (cents(misses[i].0), misses[i].1.clone());
        if miss == 0 || misses.get(i + 1).map(|m| cents(m.0) == -miss).unwrap_or(false) {
            continue;
        }
        // Every single-row repair that closes the day, leaving alone the rows of a kind
        // whose printed total is already met.
        let candidates: Vec<usize> = rows.iter().copied().filter(|&r| { let k = ledger.transactions[r].kind; !(k == Kind::Credit && credits_settled || k == Kind::Debit && debits_settled) }).collect();
        let fixes = single_row_repairs(ledger, &candidates, miss, !credits_settled && !debits_settled);
        if fixes.len() != 1 {
            continue;
        }
        let (r, new, kind) = fixes[0];
        let (old_amount, old_kind) = (ledger.transactions[r].amount, ledger.transactions[r].kind);
        ledger.transactions[r].amount = new as f64 / 100.0;
        ledger.transactions[r].kind = kind;
        let settled = match gap(ledger) {
            Some(g) => g <= 0.011,
            None => match ledger.summary.ending_balance {
                Some(e) => (beginning + ledger.transactions.iter().filter(|t| !ledger.netted.contains(&t.id)).map(signed).sum::<f64>() - e).abs() <= 0.011,
                None => false,
            },
        };
        if !settled {
            ledger.transactions[r].amount = old_amount;
            ledger.transactions[r].kind = old_kind;
        }
    }
}

/// A statement period that crosses New Year ("12/11/18 thru 1/10/19") dates its December
/// rows in the earlier year; the single year hint gave them the later one.
fn fix_year_crossing(ledger: &mut Ledger, year: Option<i32>) {
    let (Some(start), Some(end)) = (ledger.summary.period_start.as_deref().and_then(parse_date_token), ledger.summary.period_end.as_deref().and_then(parse_date_token)) else { return };
    let (Some(end_year), Some(hint)) = (end.2.or(year), year) else { return };
    if start.0 <= end.0 || start.2.map(|y| y + 1 != end_year).unwrap_or(false) || end_year != hint {
        return;
    }
    let redate = |date: &mut String, day: Option<&mut Option<i64>>| {
        let parts: Vec<&str> = date.split('-').collect();
        if parts.len() == 3 && parts[0] == format!("{end_year:04}") {
            if let (Ok(m), Ok(d)) = (parts[1].parse::<u32>(), parts[2].parse::<u32>()) {
                if m >= start.0 {
                    *date = format!("{:04}-{m:02}-{d:02}", end_year - 1);
                    if let Some(day) = day {
                        *day = Some(days_from_civil(end_year - 1, m, d));
                    }
                }
            }
        }
    };
    for t in &mut ledger.transactions {
        redate(&mut t.date, Some(&mut t.day));
    }
    for b in &mut ledger.daily_balances {
        redate(&mut b.date, None);
    }
}

/// Achieva's "Total Credits / Total Debits for this account" leave out reversal pairs: a
/// fee and its "-- Reversed" credit, a card purchase and its "purchase return". When the
/// parsed totals exceed both printed totals by the same amount, and the reversal credits
/// that have a debit of the same amount add up to exactly that, those pairs are netted:
/// still listed, not summed.
fn net_reversals(ledger: &mut Ledger) {
    let (Some(tc), Some(td)) = (ledger.summary.total_credits, ledger.summary.total_debits) else { return };
    let pc: f64 = ledger.transactions.iter().filter(|t| t.kind == Kind::Credit).map(|t| t.amount).sum();
    let pd: f64 = ledger.transactions.iter().filter(|t| t.kind == Kind::Debit).map(|t| t.amount).sum();
    let (over_c, over_d) = (pc - tc, pd - td);
    if over_c < 0.01 || (over_c - over_d).abs() > 0.01 {
        return;
    }
    let reversal = |d: &str| { let l = d.to_ascii_lowercase(); l.contains("reversed") || l.contains("reversal") || l.contains("purchase return") || l.contains("return withdrawal adjustment") };
    let mut netted: Vec<usize> = Vec::new();
    let mut used: Vec<usize> = Vec::new();
    let mut sum = 0.0;
    for c in ledger.transactions.iter().filter(|t| t.kind == Kind::Credit && reversal(&t.description)) {
        let cents = (c.amount * 100.0).round() as i64;
        let debit = ledger.transactions.iter().find(|t| t.kind == Kind::Debit && !used.contains(&t.id) && (t.amount * 100.0).round() as i64 == cents);
        if let Some(d) = debit {
            used.push(d.id);
            netted.push(c.id);
            netted.push(d.id);
            sum += c.amount;
        }
    }
    if !netted.is_empty() && (sum - over_c).abs() <= 0.01 {
        ledger.netted = netted;
    }
}

/// An exhibit sometimes carries one statement twice (Citizens: the statement, then the
/// bank's transaction detail of the same month, different layout, so no two pages match).
/// When the rows split at a page boundary into a part that meets the printed totals to
/// the cent and a remainder whose rows mostly repeat that part, the remainder is the
/// second copy and is dropped. Nothing is dropped unless one side matches the totals.
fn drop_repeated_copy(ledger: &mut Ledger) {
    let (tc, td) = (ledger.summary.total_credits, ledger.summary.total_debits);
    if tc.is_none() && td.is_none() || ledger.transactions.len() < 6 {
        return;
    }
    let totals = |rows: &[&Txn]| -> (f64, f64) {
        (rows.iter().filter(|t| t.kind == Kind::Credit).map(|t| t.amount).sum(), rows.iter().filter(|t| t.kind == Kind::Debit).map(|t| t.amount).sum())
    };
    let meets = |(c, d): (f64, f64)| tc.map_or(true, |t| (t - c).abs() <= 1.0) && td.map_or(true, |t| (t - d).abs() <= 1.0);
    let all: Vec<&Txn> = ledger.transactions.iter().collect();
    if meets(totals(&all)) {
        return;
    }
    let mut pages: Vec<usize> = all.iter().map(|t| t.page).collect();
    pages.sort();
    pages.dedup();
    let key = |t: &Txn| (t.date.clone(), t.kind, (t.amount * 100.0).round() as i64);
    let mut drop: Option<Vec<usize>> = None;
    'search: for &boundary in pages.iter().skip(1) {
        for keep_before in [true, false] {
            let (kept, rest): (Vec<&Txn>, Vec<&Txn>) = all.iter().partition(|t| (t.page < boundary) == keep_before);
            if rest.len() < 3 || !meets(totals(&kept)) {
                continue;
            }
            let keys: Vec<_> = kept.iter().map(|t| key(t)).collect();
            let repeats = rest.iter().filter(|t| keys.contains(&key(t))).count();
            if repeats * 10 >= rest.len() * 6 {
                drop = Some(rest.iter().map(|t| t.id).collect());
                break 'search;
            }
        }
    }
    if let Some(ids) = drop {
        let kept: Vec<bool> = ledger.transactions.iter().map(|t| !ids.contains(&t.id)).collect();
        ledger.transactions.retain(|t| !ids.contains(&t.id));
        renumber(ledger, &kept);
    }
}

/// Renumber the transactions after `kept[i]` decided which of the old rows stay, and
/// carry the `weak` list over to the new ids.
fn renumber(ledger: &mut Ledger, kept: &[bool]) {
    let mut new_id = vec![None; kept.len()];
    let mut next = 0;
    for (old, k) in kept.iter().enumerate() {
        if *k {
            new_id[old] = Some(next);
            next += 1;
        }
    }
    for (id, t) in ledger.transactions.iter_mut().enumerate() {
        t.id = id;
    }
    ledger.weak = ledger.weak.iter().filter_map(|w| new_id.get(*w).copied().flatten()).collect();
    ledger.alternates = ledger.alternates.iter().filter_map(|(r, a)| new_id.get(*r).copied().flatten().map(|n| (n, *a))).collect();
}

/// The same check listed twice with two amounts (Chase's checks-paid table against the
/// caption under its image): same number, same day, cents apart. One row stays and the
/// other reading is kept as its alternate for the totals to choose.
fn merge_check_readings(ledger: &mut Ledger) {
    let number = |t: &Txn| t.description.strip_prefix("Check ").filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).map(|n| n.trim_start_matches('0').to_string());
    let mut keep = vec![true; ledger.transactions.len()];
    let mut alternates: Vec<(usize, f64)> = Vec::new();
    for i in 0..ledger.transactions.len() {
        if !keep[i] {
            continue;
        }
        let Some(n) = number(&ledger.transactions[i]) else { continue };
        for j in i + 1..ledger.transactions.len() {
            let (a, b) = (&ledger.transactions[i], &ledger.transactions[j]);
            if keep[j] && a.kind == Kind::Debit && b.kind == Kind::Debit && a.table != b.table && a.date == b.date && (a.amount - b.amount).abs() >= 0.005 && number(b).as_deref() == Some(n.as_str()) {
                keep[j] = false;
                alternates.push((i, b.amount));
            }
        }
    }
    if alternates.is_empty() {
        return;
    }
    ledger.alternates.extend(alternates);
    let kept = keep.clone();
    let mut k = 0;
    ledger.transactions.retain(|_| { k += 1; kept[k - 1] });
    renumber(ledger, &kept);
}

/// When the printed totals miss by exactly what one alternate reading would change, that
/// reading was the right one.
fn use_alternates(ledger: &mut Ledger) {
    if ledger.alternates.is_empty() {
        return;
    }
    let (Some(tc), Some(td)) = (ledger.summary.total_credits, ledger.summary.total_debits) else { return };
    let cents = |v: f64| (v * 100.0).round() as i64;
    let sum = |l: &Ledger, k: Kind| cents(l.transactions.iter().filter(|t| t.kind == k && !l.netted.contains(&t.id)).map(|t| t.amount).sum::<f64>());
    let (gc, gd) = (cents(tc) - sum(ledger, Kind::Credit), cents(td) - sum(ledger, Kind::Debit));
    if gc == 0 && gd == 0 {
        return;
    }
    let closing: Vec<(usize, f64)> = ledger.alternates.iter().copied().filter(|(r, alt)| {
        let t = &ledger.transactions[*r];
        let delta = cents(*alt) - cents(t.amount);
        match t.kind { Kind::Credit => gc == delta && gd == 0, Kind::Debit => gd == delta && gc == 0 }
    }).collect();
    if closing.len() == 1 {
        ledger.transactions[closing[0].0].amount = closing[0].1;
    }
}

/// Split pages into statements. A page whose own text yields a beginning balance starts a
/// new statement, except for the first such page, which starts the first one along with
/// any cover pages before it. Pages of one statement never repeat its beginning balance
/// with a different value, so a repeated figure (Webster prints it twice) does not split.
/// `forced[i]` marks a page that is a sub-account of its own (see `split_sub_accounts`)
/// and always starts a new statement.
fn segment_statements<'a>(pages: &[(usize, &'a str)], forced: &[bool]) -> Vec<Vec<(usize, &'a str)>> {
    let mut segments: Vec<Vec<(usize, &str)>> = Vec::new();
    let mut current: Vec<(usize, &str)> = Vec::new();
    let mut current_beginning: Option<f64> = None;
    let mut current_ending: Option<f64> = None;
    let mut current_account: Option<String> = None;
    let mut current_bank: Option<String> = None;
    let mut current_period_end: Option<String> = None;
    let mut current_rows = 0usize;
    let mut current_is_printout = false;
    let mut current_summary_page: Option<usize> = None;
    // The bank's own page count ("PAGE: 2 OF 11") of the statement in progress.
    let mut current_footer_total: Option<usize> = None;
    for (i, &(page, text)) in pages.iter().enumerate() {
        let mut probe = Ledger::default();
        let mut st = State::default();
        parse_page(&unfold_two_columns(text), page, None, &mut probe, &mut st);
        let begins = probe.summary.beginning_balance;
        // A different bank named on a summary page is a new statement too (bundles of
        // several banks' statements, even when the beginning balance is garbled). The new
        // name must be mentioned at least twice and the current bank not at all, so a
        // transfer "to Bank of America" in a description does not split a statement.
        // (Votes from the top of the page only: a letterhead sits there, while "Frost Bank
        // Intl Deeproot Tech" in two wire rows lower down is a counterparty, not the bank.)
        let head: String = text.lines().take(25).collect::<Vec<_>>().join("\n");
        let votes = bank_votes(&[head.as_str()]);
        let bank = votes.iter().max_by_key(|(_, n)| *n).map(|(name, _)| name.to_string());
        let lower = text.to_ascii_lowercase();
        // "Balance Summary" alone is the daily balance table, which sits on the last page
        // of a statement (Synovus), so it does not mark a first page.
        let summary_words = has_key(&lower, "beginning balance") || lower.contains("previous balance") || lower.contains("account summary") || lower.contains("opening balance") || lower.contains("balance last statement") || lower.contains("balance forward");
        // (When the current statement never printed a beginning balance, a page that does
        // and names another bank starts a statement even if the old bank is mentioned once
        // on it: Premier Bank's first page lists "INCOMING WIRE CITIBANK" behind a Citi
        // statement that printed no summary.)
        let bank_changes = summary_words && match (&bank, &current_bank) {
            (Some(b), Some(cur)) if b != cur => {
                let new_n = votes.iter().find(|(name, _)| *name == b).map(|(_, n)| *n).unwrap_or(0);
                let cur_n = votes.iter().find(|(name, _)| *name == cur).map(|(_, n)| *n).unwrap_or(0);
                new_n >= 2 && cur_n == 0 || begins.is_some() && current_beginning.is_none() && new_n > cur_n
            }
            _ => false,
        };
        // Zero-balance sweep accounts (Wintrust) all begin at $0.00; there the account
        // number on the page with the beginning balance tells the statements apart.
        let account = probe.summary.account_last4.clone();
        // (Or a summary page naming another account when its beginning balance is lost to
        // the scan: "Account number: 3250 8165 3203" over "Account summary" behind the
        // 3725 account's pages, Bank of America in a complaint's exhibit.)
        let account_changes = (begins.is_some() || summary_words) && matches!((&account, &current_account), (Some(a), Some(cur)) if a != cur);
        // A month with no activity ends where it began (KeyBank, $94.29 to $94.29), so the
        // next statement begins with the same figure: a page that opens with the balance an
        // earlier page closed at is a new statement too.
        let continues = matches!((begins, current_ending), (Some(b), Some(end)) if (b - end).abs() < 0.005) && !current.is_empty();
        // Two months of a swept account both begin at -$10.00 (Yampa Valley): the page's
        // own statement date tells them apart.
        let period_changes = begins.is_some() && matches!((&probe.summary.period_end, &current_period_end), (Some(a), Some(cur)) if a != cur);
        // An online-banking activity printout ("Account Activity", "available as of today",
        // "All Transactions") filed behind a statement is its own document, never a page of it.
        let printout = is_printout_page(&lower);
        // (And a statement page after a printout starts fresh even though the printout
        // never printed a beginning balance to differ from.)
        let after_printout = current_is_printout && begins.is_some() && !printout;
        // A second summary page (beginning and ending balance both printed) with a different
        // ending balance is another statement, even when both accounts began at $0.00
        // (two trustee checking accounts in one filing).
        let summary_repeat = matches!((begins, probe.summary.ending_balance, current_beginning, current_ending), (Some(_), Some(e), Some(_), Some(cur)) if (e - cur).abs() >= 0.005) && !current.is_empty();
        // The bank's own "Page 1 of 10" footer opens a statement even when its summary is
        // printed elsewhere (Mercantile puts the balances at the end); only after rows or
        // balances have been seen, so a cover sheet's own footer does not split.
        // (A one-page statement says "Page: 1 of 1"; it opens one when the page carries
        // statement words, so a bank's one-page notice inside a statement does not split it.)
        let only_page = statement_words(text) && text.lines().enumerate().any(|(i, l)| footer_numbers_min(l, 1) == Some((1, 1)) && !(i >= 1 && is_court_stamp(&text.lines().nth(i - 1).unwrap_or("").to_ascii_lowercase())));
        let first_page = (footer_pages(text).map(|(n, _)| n == 1).unwrap_or(false) || only_page) && (current_rows > 0 || current_beginning.is_some() || current_ending.is_some());
        let tail_rows = current_rows >= 10 && current_beginning.is_none() && current_ending.is_none() && begins.is_some();
        // The bank's footer says this page is inside the statement in progress ("PAGE: 5 OF
        // 11" after "PAGE: 1 OF 11"): its summary is the statement's own, printed at the end
        // (Flushing Bank), not another statement's beginning.
        // (Only while the statement in progress has printed no beginning balance of its
        // own: JPMorgan's commercial statement numbers its pages "8 of 22" straight through
        // two accounts, and the second account's summary page, with its own beginning
        // balance after the first's, is another statement.)
        let inside_footer = matches!((footer_pages(text), current_footer_total), (Some((n, m)), Some(cur)) if n > 1 && m == cur) && current_beginning.is_none();
        let starts_new = forced.get(i).copied().unwrap_or(false) || bank_changes || printout || !inside_footer && (account_changes || continues || period_changes || after_printout || summary_repeat || first_page || tail_rows || match (begins, current_beginning) {
            (Some(b), Some(cur)) if (b - cur).abs() >= 0.005 => true,
            _ => false,
        });
        // (A current segment that has neither balance yet is a letterhead or cover page;
        // it joins the statement that starts here instead of standing alone. So does a
        // summary page with no rows of its own when a sub-account heading follows: Navy
        // Federal's "Summary of your deposit accounts" over "Business Checking - 7125242482".)
        // (Only a summary read from the same page joins that way, with the cover pages
        // before it: a loan page's "Account Balance Summary" on an earlier page is not this
        // account's summary.)
        // (Or a summary that names the heading's account number, when the heading opens the
        // next page: Navy Federal's summary page over "Business Checking - 7125242482".)
        let forced_here = forced.get(i).copied().unwrap_or(false);
        let heading_account = text.lines().filter(|l| !l.trim().is_empty()).take(2).find_map(|l| l.split_whitespace().rev().find(|t| t.len() >= 4 && t.chars().all(|c| c.is_ascii_digit())));
        let summary_names_account = heading_account.map(|a| current.iter().any(|(_, t)| t.contains(a))).unwrap_or(false);
        let same_page_summary = forced_here && current_rows == 0 && (current_summary_page == Some(page) || summary_names_account && current_summary_page.is_some());
        // (Ten or more rows with no summary before a page that opens with a beginning
        // balance are the tail of another statement, not a cover sheet: Gulf Coast's check
        // listing pages filed ahead of a First American statement.)
        // (Or the pages before belong to another bank altogether, whatever they printed.)
        if starts_new && !current.is_empty() && (current_beginning.is_some() || current_ending.is_some() || current_is_printout || first_page || tail_rows || bank_changes) && !same_page_summary {
            segments.push(std::mem::take(&mut current));
            current_beginning = None;
            current_ending = None;
            current_account = None;
            current_period_end = None;
            current_rows = 0;
            current_is_printout = false;
            current_summary_page = None;
            current_footer_total = None;
        }
        if let Some((_, m)) = footer_pages(text) {
            current_footer_total = Some(m);
        }
        current_rows += probe.transactions.len();
        if printout {
            current_is_printout = true;
        }
        if begins.is_some() && current_beginning.is_none() {
            current_beginning = begins;
            current_account = account;
            current_summary_page = Some(page);
        }
        if probe.summary.period_end.is_some() && current_period_end.is_none() {
            current_period_end = probe.summary.period_end.clone();
        }
        if probe.summary.ending_balance.is_some() {
            current_ending = probe.summary.ending_balance;
        }
        if bank.is_some() && (starts_new || current_bank.is_none()) {
            current_bank = bank;
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
        document_kind: None,
        pages: parts.first().and_then(|p| p.pages).zip(parts.last().and_then(|p| p.pages)).map(|(a, b)| (a.0, b.1)),
        // (The set's equation holds when every statement's does.)
        balance_check: if parts.iter().all(|p| p.balance_check == Some(true)) { Some(true) } else if parts.iter().any(|p| p.balance_check == Some(false)) { Some(false) } else { None },
        checks_total: None,
        fees_total: None,
        interest_total: None,
        debits_key: "",
        debits_page: None,
        in_summary_block: false,
        summary_lines: 0,
        debit_parts: Vec::new(),
        credit_parts: Vec::new(),
        debit_parts_unsigned: Vec::new(),
        beginning_balances_seen: parts.iter().flat_map(|p| p.beginning_balances_seen.iter().copied()).collect(),
        parsed_credits: None,
        parsed_debits: None,
        missing_pages: parts.iter().any(|p| p.missing_pages),
    }
}

/// Drop a transaction that repeats (same date, kind and amount) one read from an earlier
/// table. Webster prints a running-balance table and then per-type lists; Pinnacle and
/// Legends print check tables and check-image captions; all of these would double the
/// totals. Repeats inside one table (two identical card charges on one day) are kept:
/// each earlier line can absorb at most one later copy.
fn dedup_across_tables(ledger: &mut Ledger) {
    // Per (date, kind, cents): the original entries, each with the tables that already
    // repeated it, so a check listed three times (history, checks-paid table, image
    // caption) collapses to one while two real same-day items each keep their own repeat.
    let mut available: BTreeMap<(String, Kind, i64), Vec<(usize, usize, Vec<usize>)>> = BTreeMap::new();
    let mut keep = vec![true; ledger.transactions.len()];
    // Same date and amount in another table is a repeat only when the descriptions agree:
    // a shared word of four letters or more, or both are check entries. A $20 fee and a
    // $20 check on the same day are two transactions.
    // Words split on punctuation and on digit/letter boundaries ("24490Check" is a check
    // number glued to its label in some text layers).
    let words = |d: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for c in d.to_ascii_lowercase().chars() {
            let boundary = !c.is_alphanumeric() || cur.chars().last().map(|p| p.is_ascii_digit() != c.is_ascii_digit()).unwrap_or(false);
            if boundary {
                if cur.len() >= 4 {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
            if c.is_alphanumeric() {
                cur.push(c);
            }
        }
        if cur.len() >= 4 {
            out.push(cur);
        }
        out
    };
    // Check numbers (three to seven digits) shared by both descriptions pair them too:
    // "CHEC K# 131" in a smeared text layer against "Check 131" from the check table.
    let numbers = |d: &str| -> Vec<String> {
        d.split(|c: char| !c.is_ascii_digit()).filter(|n| (3..=7).contains(&n.len())).map(str::to_string).collect()
    };
    // (`fuzzy`: a check number one digit off still pairs; only for an original that has
    // no repeat yet, so a lost neighbouring serial is not swallowed by a repeated one.)
    let compatible = |a: &Txn, b: &Txn, fuzzy: bool| -> bool {
        // "#0000  07/03/2025  $6,658.37" captions a withdrawal slip's image (Yampa Valley):
        // no check number, so it repeats whatever it matches.
        let zero_caption = |t: &Txn| t.description.strip_prefix("Check ").map(|n| !n.is_empty() && n.chars().all(|c| c == '0')).unwrap_or(false);
        if zero_caption(a) || zero_caption(b) {
            return true;
        }
        // Two check entries are the same check only when their numbers agree: TD lists
        // dozens of $970.00 checks on one day, each with its own serial.
        // (Or agree once a digit the scan read in is dropped: "10138" in the checks-paid
        // table for the check its image captions "0000001013", same day, same amount.)
        if let (Some(na), Some(nb)) = (a.description.strip_prefix("Check "), b.description.strip_prefix("Check ")) {
            if na.chars().all(|c| c.is_ascii_digit()) && nb.chars().all(|c| c.is_ascii_digit()) {
                let (na, nb) = (na.trim_start_matches('0'), nb.trim_start_matches('0')); // "#0652" captions check 652
                let one_dropped = |long: &str, short: &str| long.len() == short.len() + 1 && (0..long.len()).any(|k| format!("{}{}", &long[..k], &long[k + 1..]) == short);
                // (Or once a digit misread is put back: "4029" in the table for the check
                // its image captions "0000001029", same day, same amount. An exact number
                // is matched first below, so neighbouring serials keep their own repeats.)
                let one_changed = fuzzy && na.len() == nb.len() && na.len() >= 3 && na.chars().zip(nb.chars()).filter(|(x, y)| x != y).count() == 1;
                return na == nb || one_dropped(na, nb) || one_dropped(nb, na) || one_changed;
            }
        }
        let (wa, wb) = (words(&a.description), words(&b.description));
        if wa.is_empty() || wb.is_empty() {
            return true; // a bare caption or check-image line repeats whatever it matches
        }
        // Navy Federal's "Items Paid" recap names each item by a code ("ACH", "ATMO"),
        // sometimes with a smear of the text layer in front ("seis ACH"): one or two short
        // tokens are a code, not a description, and repeat whatever they match.
        // (A form's field labels wrapped onto the recap's last row, "ATMO RANK/RATE
        // NAME(FIRST MI LAST) ACCOUNT", are not description either: after the code every
        // word is a label, printed without a lowercase letter.)
        // (Or the code with a form's field labels wrapped onto it, "ATMO RANK/RATE
        // NAME(FIRST MI LAST) ACCOUNT": labels carry no digit and no lowercase letter, and
        // bracket or slash their parts, which no description of a payment does.)
        let code = |d: &str| {
            let t: Vec<&str> = d.split_whitespace().collect();
            if t.is_empty() || t[0].len() > 4 {
                return false;
            }
            t.len() <= 2 && t.iter().all(|w| w.len() <= 4)
                || t.len() > 2
                    && t[1..].iter().all(|w| !w.chars().any(|c| c.is_ascii_digit() || c.is_ascii_lowercase()))
                    && t[1..].iter().any(|w| w.contains('(') || w.contains(')') || w.contains('/'))
        };
        if code(&a.description) || code(&b.description) {
            return true;
        }
        wa.iter().any(|w| wb.contains(w)) || numbers(&a.description).iter().any(|n| numbers(&b.description).contains(n))
    };
    // The same check number on the same date in another table is the same check even
    // when the amounts disagree: an image caption's OCR ("#361  04/01  $2,000.00" under a
    // $9,000.00 check) is worse than the table's, and the table came first.
    // (Any later listing: a check clears once, so the same number on the same day is one
    // check wherever it is printed again. A placeholder number, "9999*" for every
    // unnumbered check at American River Bank, is no key; and a repeat with a different
    // amount only counts on a later page, where the images are.)
    let check_key = |t: &Txn| t.description.strip_prefix("Check ").filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).map(|n| (n.trim_start_matches('0').to_string(), t.date.clone()));
    let placeholder = |n: &str| n.is_empty() || n.chars().all(|c| c == '9');
    let mut checks_seen: Vec<((String, String), usize, i64)> = Vec::new();
    for i in 0..ledger.transactions.len() {
        let t = &ledger.transactions[i];
        if let Some(k) = check_key(t).filter(|k| !placeholder(&k.0)) {
            let cents = (t.amount * 100.0).round() as i64;
            match checks_seen.iter().find(|(seen, _, _)| *seen == k) {
                Some((_, first_page, first_cents)) if *first_cents == cents || t.page > *first_page => {
                    keep[i] = false;
                    continue;
                }
                Some(_) => {}
                None => checks_seen.push((k, t.page, cents)),
            }
        }
        let key = (t.date.clone(), t.kind, (t.amount * 100.0).round() as i64);
        let candidates = available.entry(key.clone()).or_default().clone();
        // Prefer an original that has not been repeated yet, so two real same-day items
        // each absorb their own repeat; otherwise an original may repeat again.
        let fits = |(table, j, _): &(usize, usize, Vec<usize>), fuzzy: bool| *table != t.table && compatible(&ledger.transactions[*j], t, fuzzy);
        // (Only an image caption's number, "#0000001029" with its leading zeros, is read
        // fuzzily: two serials a digit apart in the checks-paid table are two checks.)
        let caption = |t: &Txn| t.description.strip_prefix("Check ").map(|n| n.len() >= 6 && n.starts_with('0')).unwrap_or(false);
        let same_number = |(_, j, _): &(usize, usize, Vec<usize>)| matches!((check_key(&ledger.transactions[*j]), check_key(t)), (Some(a), Some(b)) if a == b);
        let pos = candidates.iter().position(|c| c.2.is_empty() && same_number(c) && fits(c, false))
            .or_else(|| candidates.iter().position(|c| c.2.is_empty() && fits(c, false)))
            .or_else(|| candidates.iter().position(|c| fits(c, false)))
            .or_else(|| candidates.iter().position(|c| c.2.is_empty() && caption(t) && fits(c, true)));
        let entry = available.get_mut(&key).unwrap();
        if let Some(pos) = pos {
            entry[pos].2.push(t.table);
            keep[i] = false;
        } else {
            entry.push((t.table, i, Vec::new()));
        }
    }
    if keep.iter().any(|k| !k) {
        let mut i = 0;
        ledger.transactions.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
        renumber(ledger, &keep);
    }
}

/// Number of rows under a transaction table header that start with a date but carry no
/// amount. On plain OCR output of a scanned table these are rows whose amount cells were
/// dropped; the caller then asks the OCR model for the table itself.
pub fn rows_missing_amounts(text: &str) -> usize {
    let mut under_header = false;
    let mut balance_column = false;
    let mut missing = 0;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        let toks: Vec<&str> = line.split_whitespace().collect();
        let single_amount_header = lower.contains("date") && lower.contains("amount") && toks.len() <= 8;
        if !toks.iter().any(|t| is_amount_token(t)) && (Columns::labels(line).is_complete(lower.contains("date")) || single_amount_header) {
            under_header = true;
            balance_column = lower.contains("balance");
            continue;
        }
        // Under a header with a balance column, a dated row carrying one figure only has
        // its balance and lost its amount ("04/09/2025  Square Inc SQ250409  [blank]
        // $715,889.24", a text layer that dropped the glyphs); balance rows themselves
        // ("Beginning Balance  $650,051.75") are whole.
        if under_header && balance_column && parse_date_token(toks[0]).is_some() && toks.len() >= 3 && toks.iter().filter(|t| is_amount_token(t)).count() == 1 && toks.last().map(|t| is_amount_token(t)).unwrap_or(false) && !lower.contains("balance") {
            missing += 1;
            continue;
        }
        if under_header && parse_date_token(toks[0]).is_some() && toks.len() >= 3 && !toks.iter().any(|t| is_amount_token(t)) {
            // TD wraps the description and prints the amount at the end of the second line;
            // that row is whole, the parser joins the two lines.
            let next = lines.get(i + 1).map(|l| l.split_whitespace().collect::<Vec<_>>()).unwrap_or_default();
            let wrapped = next.first().and_then(|t| parse_date_token(t)).is_none() && next.last().map(|t| is_amount_token(t)).unwrap_or(false);
            if !wrapped {
                missing += 1;
            }
        }
    }
    missing
}

/// Number of listings whose dated rows add up to less than the subtotal printed under
/// them. The OCR model sometimes drops rows it can see (the right column of TD's two-column
/// "Checks Paid" table; the tail of a long list): the printed "Subtotal: 15,547.77" then
/// exceeds the rows read, and the caller re-reads the page at a higher resolution. Only
/// unambiguous listings count: one amount per dated entry (two-column check tables carry a
/// date before each), and a subtotal line with a single figure. Running-balance tables and
/// two-figure totals give no signal.
pub fn rows_short_of_totals(text: &str) -> usize {
    let mut short = 0;
    let mut sum_cents: i64 = 0;
    let mut rows = 0;
    let mut clean = true; // every row so far had one amount per dated entry
    let mut continued = false; // the listing began on the page before
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let toks: Vec<&str> = line.split_whitespace().filter(|t| *t != "|").collect();
        let lower = line.to_ascii_lowercase();
        let amounts: Vec<i64> = toks.iter().filter(|t| is_amount_token(t)).filter_map(|t| parse_amount(t)).map(|v| (v.abs() * 100.0).round() as i64).collect();
        let total_line = (lower.trim_start().starts_with("subtotal") || lower.trim_start().starts_with("total")) && !lower.contains("balance");
        if total_line {
            if amounts.len() == 1 && rows > 0 && clean && !continued && sum_cents < amounts[0] {
                short += 1;
            }
            sum_cents = 0;
            rows = 0;
            clean = true;
            continued = false;
            continue;
        }
        let dates = toks.iter().filter(|t| parse_date_token(t).is_some()).count();
        if dates == 0 || parse_date_token(toks[0]).is_none() {
            // A header or heading resets the listing; a wrapped description line does not.
            if amounts.is_empty() && toks.len() <= 8 && (lower.contains("date") || lower.ends_with(':')) {
                sum_cents = 0;
                rows = 0;
                clean = true;
            }
            // A listing continued from the page before cannot reach its subtotal here.
            if amounts.is_empty() && lower.contains("continued") {
                continued = true;
            }
            continue;
        }
        if amounts.len() != dates {
            clean = false; // running balance beside the amount, or a cell lost
            continue;
        }
        rows += dates;
        sum_cents += amounts.iter().sum::<i64>();
    }
    short
}

/// Bank named on the statement. Counts mentions on the first pages and picks the most
/// frequent name from a fixed list, so a Wells Fargo statement that mentions Zelle or a
/// wire to Chase still reads "Wells Fargo".
pub fn detect_bank(texts: &[&str]) -> Option<String> {
    bank_votes(texts).into_iter().max_by_key(|(_, n)| *n).map(|(name, _)| name.to_string())
}

/// Mentions per bank name on the given pages.
fn bank_votes(texts: &[&str]) -> Vec<(&'static str, usize)> {
    const BANKS: &[(&str, &str)] = &[
        ("wells fargo", "Wells Fargo"), ("truist", "Truist"), ("jpmorgan chase", "Chase"), ("chase.com", "Chase"),
        ("bank of america", "Bank of America"), ("pnc bank", "PNC"), ("pnc.com", "PNC"), ("td bank", "TD Bank"), ("tdbank.com", "TD Bank"), ("most convenient bank", "TD Bank"), ("u.s. bank", "U.S. Bank"), ("usbank.com", "U.S. Bank"),
        ("capital one", "Capital One"), ("citibank", "Citibank"), ("regions bank", "Regions"), ("fifth third", "Fifth Third"),
        ("huntington", "Huntington"), ("keybank", "KeyBank"), ("citizens bank", "Citizens"), ("citizensbank.com", "Citizens"), ("clearly better business checking", "Citizens"), ("m&t bank", "M&T Bank"), ("bmo", "BMO"),
        ("webster bank", "Webster Bank"), ("yampavalleybank", "Yampa Valley Bank"), ("yampa valley bank", "Yampa Valley Bank"), ("websterbank.com", "Webster Bank"), ("websteronline", "Webster Bank"), ("pinnacle", "Pinnacle Bank"), ("legends bank", "Legends Bank"), ("sunrise bank", "Sunrise Banks"),
        ("ally bank", "Ally"), ("frost bank", "Frost Bank"), ("frostbank", "Frost Bank"), ("box 1600 san antonio", "Frost Bank"), ("first citizens", "First Citizens"), ("comerica", "Comerica"),
        ("zions", "Zions"), ("synovus", "Synovus"), ("santander", "Santander"), ("navy federal", "Navy Federal"), ("bluevine", "Bluevine"), ("suntrust", "SunTrust"), ("home24bank", "Home Bank"), ("flushing bank", "Flushing Bank"),
        ("mercury", "Mercury"), ("novo", "Novo"), ("relayfi", "Relay"), ("relay financial", "Relay"), ("axos", "Axos"), ("live oak", "Live Oak"), ("first horizon", "First Horizon"),
        ("flagstar", "Flagstar"), ("valley national", "Valley National"), ("valleynationalbank", "Valley National"), ("east west bank", "East West Bank"), ("cathay", "Cathay Bank"),
        ("customers bank", "Customers Bank"), ("signature bank", "Signature Bank"), ("silicon valley bank", "Silicon Valley Bank"),
        ("hancock whitney", "Hancock Whitney"), ("hancockwhitney", "Hancock Whitney"), ("mabrey", "Mabrey Bank"), ("wintrust", "Wintrust"),
        ("byline", "Byline Bank"), ("old national", "Old National"), ("associated bank", "Associated Bank"),
        ("first republic", "First Republic"), ("umpqua", "Umpqua"), ("banner bank", "Banner Bank"), ("amerant", "Amerant"), ("city national", "City National"),
        ("first state bank", "First State Bank"), ("bell bank", "Bell Bank"), ("choice bank", "Choice Bank"), ("alerus", "Alerus"), ("bremer", "Bremer Bank"), ("gate city", "Gate City Bank"),
        ("credit union", "Credit Union"),
        ("mercantile bank", "Mercantile Bank"), ("mercantile", "Mercantile Bank"), ("ynovus", "Synovus"),
        ("banknorth", "BankNorth"), ("brookline bank", "Brookline Bank"), ("brooklinebank", "Brookline Bank"), ("tristate capital", "TriState Capital"),
        ("first american bank", "First American Bank"), ("gulf bank", "Gulf Bank"), ("gulfbank", "Gulf Bank"), ("umb bank", "UMB Bank"), ("community bank", "Community Bank"),
        ("national city", "National City"), ("bank one", "Bank One"), ("bankone", "Bank One"), ("chase manhattan", "Chase"), ("first usa bank", "First USA"),
        ("unionbank", "Union Bank"), ("union bank", "Union Bank"), ("first federal savings bank", "First Federal Savings Bank"), ("bankfirstfed", "First Federal Savings Bank"), ("western heritage", "Western Heritage Bank"),
        ("first farmers & merchants", "First Farmers & Merchants"), ("highland bank", "Highland Bank"), ("forcht bank", "Forcht Bank"), ("forchtbank", "Forcht Bank"), ("communityamerica", "CommunityAmerica Credit Union"), ("communityamerica credit", "CommunityAmerica Credit Union"), ("heritage bank", "Heritage Bank"),
        ("trustmark", "Trustmark"), ("renasant", "Renasant Bank"), ("premier bank", "Premier Bank"), ("premierbankne", "Premier Bank"), ("atlantic union", "Atlantic Union Bank"), ("atlanticunion", "Atlantic Union Bank"), ("gulf coast bank", "Gulf Bank"), ("great southern", "Great Southern Bank"), ("greatsouthernbank", "Great Southern Bank"), ("tri counties bank", "Tri Counties Bank"), ("united bank", "United Bank"),
    ];
    // Multi-word names matched with the spaces gone; single words are not here, they hide
    // inside other words ("purchase").
    const SQUEEZED_BANKS: &[(&str, &str)] = &[
        ("jpmorganchase", "Chase"), ("chase.com", "Chase"), ("wellsfargo", "Wells Fargo"), ("bankofamerica", "Bank of America"), ("pncbank", "PNC"), ("tdbank", "TD Bank"),
        ("u.s.bank", "U.S. Bank"), ("capitalone", "Capital One"), ("fifththird", "Fifth Third"), ("citizensbank", "Citizens"), ("m&tbank", "M&T Bank"), ("navyfederal", "Navy Federal"),
        ("firstcitizens", "First Citizens"), ("regionsbank", "Regions"), ("hancockwhitney", "Hancock Whitney"), ("truist", "Truist"), ("synovus", "Synovus"),
    ];
    let mut votes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (i, text) in texts.iter().enumerate() {
        let lower = text.to_ascii_lowercase();
        // The bank's own name sits in the letterhead, the top of the page; other banks
        // show up in transaction descriptions ("Capital One Auto" deposits at a dealer).
        // Transaction rows near the top of a short page are not letterhead (a wire "from
        // Fifth Third" on a Synovus continuation page). Every page's letterhead counts (a
        // court filing puts the statement behind pages of forms); only the first pages'
        // bodies do.
        let head: String = lower.lines().filter(|l| !l.trim().is_empty()).take(30).filter(|l| l.split_whitespace().next().and_then(parse_date_token).is_none()).collect::<Vec<_>>().join("\n");
        // OCR of a letterhead spaces or drops letters ("C H AS E", "YNOVUS" with the logo
        // S): a second look at the head with the spaces squeezed out.
        let squeezed: String = head.chars().filter(|c| !c.is_whitespace()).collect();
        for (needle, name) in BANKS {
            let body = if i < 3 { lower.matches(needle).count() } else { 0 };
            let n = body + 5 * head.matches(needle).count();
            if n > 0 {
                *votes.entry(name).or_default() += n;
            }
        }
        for (needle, name) in SQUEEZED_BANKS {
            let n = squeezed.matches(needle).count();
            if n > 0 {
                *votes.entry(name).or_default() += 5 * n;
            }
        }
    }
    votes.into_iter().collect()
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
    #[test]
    fn one_sided_copy_is_flagged_as_missing_pages() {
        let p1 = "Beginning balance on August 1, 2022 $269.00\nDeposits and other credits 82,480.16\nWithdrawals and other debits -83,749.80\nEnding balance on August 31, 2022 -$1,168.14\nPage 1 of 14\n";
        let p3 = "Deposits and other credits\nDate Description Amount\n08/01/22 FISERV MERCHANT DES:DEPOSIT 3,095.69\nPage 3 of 14\n";
        let p7 = "Withdrawals and other debits\nDate Description Amount\n08/01/22 Zelle Transfer Conf# gfdrmcy9t -1,100.00\nPage 7 of 14\n";
        assert!(super::pages_missing(&[(1, p1), (2, p3), (3, p7)]));
        assert!(super::parse(&[(1, p1), (2, p3), (3, p7)]).summary.missing_pages);
        // The same pages numbered 1, 2, 3 of 4: the copy only lost its last page.
        let q2 = p3.replace("Page 3 of 14", "Page 2 of 4");
        let q3 = p7.replace("Page 7 of 14", "Page 3 of 4");
        assert!(!super::pages_missing(&[(1, &p1.replace("of 14", "of 4")), (2, &q2), (3, &q3)]));
        // The court's own stamp is not a footer.
        assert_eq!(super::footer_pages("Case 8:26-bk-04988 Doc 72-7 Filed 07/21/26 Page 14 of 63\n"), None);
    }

    #[test]
    fn a_dated_beginning_balance_row_under_a_smeared_balance_header_is_not_a_debit() {
        let p1 = "                                                  BEGINNING BALANCE                                         $9,500.00\n                                               DEPOSITS & CREDITS                                           11,300.00\n                                                  LESS CHECKS & DEBITS                                      10,767.86\n                                                                       ACCOUNT ACTIVITY\n        POSTING                                                                      DEPOSITS & OTHER                        WITHDRAWALS &                DAILY\n                                     TRANSACTION DESCRIPTION\n         DATE                                                                           CREDITS l+l                          OTHER DEBITS 1-l            BALANr.E\n        08/01/2025   BEGINNING BALANCE                                                                                                                       $9,500.00\n        08/01/2025   lnstaMed E:XCELLUS B 021000021309996                                                                           $9,025.86                    474.14\n        08/08/2025   SERVICE CHARGE FOR ACCOUNT 000009897315587                                                                         15.15                   458.99\n        08/15/2025   DEPOSIT                                                                  $1,300.00\n        08/15/2025   CHECK NUMBER         1001                                                                                         458.00                  1,300.99\n        08/18/2025   CHECK NUMBER         1002                                                                                         642.00\n        08/18/2025   CHECK NUMBER         1003                                                                                         642.00                     16.99\n        08/29/2025   DEPOSIT                                                                  10,000.00                                                       10,016.99\n                     NUMBER OF DEPOSITS/CHECKS PAID                                                  2                                       3\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 9025.86), (Kind::Debit, 15.15), (Kind::Credit, 1300.0), (Kind::Debit, 458.0), (Kind::Debit, 642.0), (Kind::Debit, 642.0), (Kind::Credit, 10000.0)], "{:?}", l.transactions);
        assert_eq!((l.summary.beginning_balance, l.summary.total_credits), (Some(9500.0), Some(11300.0)));
    }

    #[test]
    fn a_page_that_signs_its_debits_reads_unsigned_rows_as_credits_but_not_a_signed_fee_list() {
        // Bluevine: debits "$-500.00", credits unsigned, under a summary whose
        // "Withdrawals/debits" label would otherwise pass for a section.
        let bluevine = "Deposits/credits                       $10,176.92\n                                               $-\nWithdrawals/debits\n                                        25,064.23\nEnding balance on 08/31/2022           $5,112.69\nTransactions\nDate            Description                                                            Amount\n08/01/22        Interest earned in July 2022                                               $85.44\n08/07/22        CASH APP*ANN1967, gosq.com, CA                                         $-500.00\n08/09/22        ENDICIA, 800-576-3279, CA                                                  $-54.42\n08/09/22        NEW BEGINNING AUTOMOTI, FORT STEWART, GA                               $-971.25\n08/10/22        Transfer to Grants Payable 2566                                      $-6,459.77\n";
        let l = parse(&[(1, bluevine)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 85.44), (Kind::Debit, 500.0), (Kind::Debit, 54.42), (Kind::Debit, 971.25), (Kind::Debit, 6459.77)], "{:?}", l.transactions);
        // KeyBank: an unsigned "Subtractions" list, then a short signed fee list. The signs
        // never mix with the rows above, so those stay debits.
        let keybank = "Subtractions\n(con't)\n            Withdrawals Date     Serial #        Location\n                        10-22                    Direct Withdrawal, Cj Affiliate 8185754753                        1,079.16\n                        10-23                    Goal Zero Llc 8887946250 UT USA                                     569.74\n                        10-26                    In *Flylow Spor 720-2015758 CO USA                                8,145.77\n                        10-27                    Internet Trf To DDA                5204 3720                     18,552.90\n                                                 Total subtractions                                              $28,347.57\nFees and\ncharges                 Date                      Description                                                                          Amount\n                        10-01                     Stop Payment Charge                                                                  -$34.00\n                        10-14                     Fedwire Service Charge                                                               -30.00\n                        10-30                     Fedwire Service Charge                                                               -30.00\n";
        let l = parse(&[(1, keybank)]);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Debit), "{:?}", l.transactions);
        assert_eq!(l.transactions.len(), 7);
        // Wells, overdrawn month: the running balances "-528.63" after the amounts are not
        // signed amounts, and the check summary's numberless row is the check already listed.
        let wells = "      Transaction history\n      Date      Check Number  Description                                                      Deposits/Credits      Withdrawals/Debits     Ending daily balance\n      10/4                    Check                                                                                        369.24               -518.63\n      10/6                    Purchase authorized on 10/05 Spectrum                                                         10.00               -528.63\n      10/16                   Purchase authorized on 10/15 Uber                                                             10.00               -538.63\n      10/31                   Monthly Service Fee                                                                           10.00               -548.63\n      Ending balance on 10/31                                                                                                                    -548.63\n      Totals                                                                             $0.00              $399.24\nSummary of checks written(checks listed are also displayed in the preceding Transaction history)\n     Number            Date                 Amount\n                       10/4                  369.24\n";
        let l = parse(&[(1, wells)]);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Debit), "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 399.24).abs() < 0.001, "{:?}", l.transactions);
    }

    #[test]
    fn interest_left_out_of_the_printed_credits_is_added_by_the_balance_equation() {
        let p1 = "Beginning Balance                             71,393.45+         Statement Period Days                             181\n         0  Deposits                               0.00          Average Collected Balance                   71,438.05+\n         0  Other Credits                          0.00          Interest Rate on Statement Day                   0.15%\nInterest Earned This Period                       53.12+         Total Interest Earned YTD                       53.12+\n         0  Other Debits                           0.00\nEnding   Balance                              71,446.57+\nOther Credits And Interest To Your Account\n Date          Description                                                                                             Amount\n 01/30/26      Interest                                                                                                   9.09\n 02/27/26      Interest                                                                                                  44.03\n           Total                                                                                                    53.12\n";
        let l = parse(&[(1, p1)]);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(53.12), Some(0.0)), "{:?}", l.summary);
        assert!((l.parsed_credit_total - 53.12).abs() < 0.001, "{:?}", l.transactions);
    }

    #[test]
    fn a_bookkeeping_register_behind_the_statement_is_left_out() {
        let p1 = " Previous Balance                87,094.74   Days in the statement period         32\n       Deposits/Credits                .00   Average Ledger                87,094.74\n       Checks/Debits                   .00   Average Collected             87,094.74\n Current Balance                 87,094.74\nDaily Balance Information\n  Date          Balance\n  01/31         87,094.74\n";
        let p2 = "                                                       PE&B Client Trust Account                                        9/1\nRegister: Intrust Client Trust Account:Pioneer Balloon (Bk of Utah)\n\nDate         Number        Payee                    Account                    Memo                   Payment C      Deposit        Balance\n03/17/2025                                          Funds Held In Trust        Pioneer (Bk of ...              X    50,000.00      50,000.00\n03/17/2025                 Intrust Bank, N.A.       Funds Held In Trust        Wire transfer fee         10.00 X                    49,990.00\n";
        let l = parse(&[(1, p1), (2, p2)]);
        assert!(l.transactions.is_empty(), "{:?}", l.transactions);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(0.0), Some(0.0)));
    }

    #[test]
    fn rows_short_of_a_printed_subtotal_are_counted() {
        // TD's two-column check table as GLM-OCR read it at 150 dpi: the right column gone.
        let short = "Subtotal: 161,296.05\nDATE SERIAL NO. AMOUNT\n04/05       2250                                                                  300.00\n04/04       10766*                                                                768.72\n04/15       10782                                                               1,340.29\nSubtotal: 15,547.77\n";
        assert_eq!(rows_short_of_totals(short), 1);
        // The 200 dpi reading, both columns on each line.
        let whole = "DATE SERIAL NO. AMOUNT DATE SERIAL NO. AMOUNT\n04/05 2250 300.00 04/15 10783 966.47\n04/04 10766* 768.72 04/18 10784 548.86\n04/01 10775* 1,214.61 04/15 10785 743.96\n04/01 10776 578.22 04/29 10786 3,907.11\n04/01 10778* 400.25 04/29 10787 1,332.28\n04/01 10779 1,102.13 04/30 10788 946.35\n04/01 10781* 583.87 04/29 10790* 814.65\n04/15 10782 1,340.29\nSubtotal: 15,547.77\n";
        assert_eq!(rows_short_of_totals(whole), 0);
        // A running-balance table gives no signal, nor does a two-figure totals line.
        let balances = "Date Description Deposits Withdrawals Balance\n10/4 Check 369.24 -518.63\n10/6 Purchase 10.00 -528.63\nTotals $0.00 $379.24\n";
        assert_eq!(rows_short_of_totals(balances), 0);
        // A section continued from the page before is short by nature.
        let continued = "Electronic Deposits (continued)\nDate Description Amount\n04/18 CCD DEPOSIT, TOAST 3,176.12\n04/19 CCD DEPOSIT, TOAST 5,225.77\nSubtotal: 161,296.05\n";
        assert_eq!(rows_short_of_totals(continued), 0);
    }

    #[test]
    fn date_amount_pairs_after_a_subtotal_are_daily_balances() {
        // TD savings page as Tesseract reads it: the pale "DAILY BALANCE SUMMARY" heading lost.
        let p1 = "Beginning Balance 25.00 Average Collected Balance 2,541.78\nElectronic Deposits 4,009.23 Interest Earned This Period 0.00\nEnding Balance 4,034.23 Annual Percentage Yield Earned 0.00%\nElectronic Deposits\n08/10 eTransfer Credit, eas 3,074.13\nTransfer from CK 1217\n08/17 eTransfer Credit, i fer 415.52\nTransfer from CK 1217\n08/24 eTransfer Credit, Online Xfer 519.58\nTransfer from CK ma: 217\nSubtotal: 4,009.23\n07/31 25.00 08/17 3,514.65\n08/10 3,099.13 08/24 4,034.23\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<f64> = l.transactions.iter().map(|t| t.amount).collect();
        assert_eq!(rows, vec![3074.13, 415.52, 519.58], "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 4, "{:?}", l.daily_balances);
    }

    #[test]
    fn court_copy_labels_are_repaired() {
        let text = "Begi ning balance on September 1, 2022   -$1,168.14\nEnd ng balance on September 30, 2022\nSer ice fees  -240.00\nfor  eptember 1, 2022 to September 30, 2022\nfor iOctober 1, 2022\nPending balance 5.00\nJ J PREMIUM balance\n";
        let fixed = super::repair_court_labels(text);
        let lines: Vec<&str> = fixed.lines().collect();
        assert!(lines[0].starts_with("Beginning balance on September 1, 2022   -$1,168.14"), "{}", lines[0]);
        assert_eq!(lines[1], "Ending balance on September 30, 2022");
        assert_eq!(lines[2], "Service fees  -240.00");
        assert_eq!(lines[3], "for  September 1, 2022 to September 30, 2022");
        assert_eq!(lines[4], "for October 1, 2022");
        assert_eq!(lines[5], "Pending balance 5.00");
        assert_eq!(lines[6], "J J PREMIUM balance");
    }

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
      1/3                Giggster, Inc. ACH Pmt 260408 Location $2,300.00 Usd, ID: E9F3E6    1,863.00                            5,597.56
      1/3                Everest Business Fundi Everest Bu 220103                                               399.00          5,198.56
      1/4                Everest Business Fundi Everest Bu 220104                                               399.00          4,799.56
      1/4         1021   Check                                                                                  800.00          3,999.56
      Ending balance on 1/4                                                                                                     3,999.56
      Totals                                                                             $4,363.00        $1,743.67
Summary of checks written (checks listed are also displayed in the preceding Transaction history)
      Number              Date                    Amount           Number             Date                  Amount
                          1/4                     800.00           1021 *             1/4                   800.00
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
        assert_eq!(credits.len(), 2, "{:?}", l.transactions);
        assert_eq!(credits[0].amount, 2500.0);
        // The "$2,300.00" inside the description is text; the column holds 1,863.00.
        assert_eq!(credits[1].amount, 1863.0, "{:?}", credits[1]);
        assert!(credits[1].description.contains("$2,300.00 Usd"));
        // The check summary under the table repeats the check; its amounts sit under the
        // old "Credits" offset but are checks (debits), and the repeat is dropped.
        assert_eq!(debits.len(), 4, "{:?}", debits);
        assert_eq!(debits[0].amount, 145.67, "transaction amount, not the running balance");
        assert!(debits[1].description.starts_with("Everest"));
        // Running balances collapse to one ending balance per day.
        assert_eq!(l.daily_balances.len(), 3);
        assert_eq!(l.daily_balances.last().unwrap().balance, 3999.56);
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
        assert!(is_amount_token("($15.00)") && is_amount_token("$(205,309.04)"));
        assert_eq!(parse_amount("($15.00)"), Some(-15.0));
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
7/2 ATM Check Deposit on 07/02 1500 W Lantana Rd Lantana FL 0007768 ATM ID 0836G Card 9798 500.00
7/2 Purchase Return authorized on 07/01 Teach ME to Inc. San Diego CA S385062819308513 Card 9798 258.10
7/2 Recurring Payment authorized on 06/30 Google *Youtube G.CO/Helppay# CA S305181667547988 Card 9798 106.44
7/2 Zelle to Carr Mike on 07/02 Ref #Rp0Yzclgvs 2,000.00 1,908.50
7/15 Lateral Link Gro Payroll 12728400007698x Jowers, Evan P 10,000.00
7/15 Recurring Payment authorized on 07/14 El Car Wash Lantan 305-603-9565 FL S585195372421625 Card 9798 42.78 11,865.72
"#;

    #[test]
    fn flat_ocr_page_uses_words_and_running_balance_arithmetic() {
        let l = parse(&[(1, WELLS_OCR)]);
        assert_eq!(l.summary.total_credits, Some(12758.10));
        assert_eq!(l.transactions.len(), 7, "{:?}", l.transactions);
        let kinds: Vec<Kind> = l.transactions.iter().map(|t| t.kind).collect();
        // "Payroll" carries no credit word; the balance change (+9,957.22) proves it is a credit.
        // "ATM Check Deposit" and "Purchase Return" are credits despite "check" and "purchase".
        assert_eq!(kinds, vec![Kind::Credit, Kind::Credit, Kind::Credit, Kind::Debit, Kind::Debit, Kind::Credit, Kind::Debit]);
        assert_eq!(l.daily_balances.len(), 2);
        assert!((l.parsed_credit_total - 12758.10).abs() < 0.001);
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
ACH Credits                              46           38,775.50            ACH Debits                             10                450.00
Other Credits                             0                    .00            Other Debits                            1                    158.70
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
        // The side-by-side category table ("ACH Credits ... ACH Debits ...") does not
        // replace the total from the balance summary.
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

    #[test]
    fn split_amounts_are_rejoined_keeping_width() {
        assert_eq!(join_split_amounts("FEE      20. 00"), "FEE      20.00 ");
        assert_eq!(join_split_amounts("ACH  1,860. 70"), "ACH  1,860.70 ");
        assert_eq!(join_split_amounts("Ref. 12 items"), "Ref. 12 items");
        assert_eq!(join_split_amounts("v1. 234"), "v1. 234");
        // Court OCR of a Bank of America page: split thousands comma and split date.
        assert_eq!(join_split_amounts("11/23/22   transfer   -1 ,100.00"), "11/23/22   transfer   -1,100.00 ");
        assert_eq!(join_split_amounts("11 /21 /22    1031    -5,000.00"), "11/21/22      1031    -5,000.00");
        assert_eq!(join_split_amounts("Page 1 /2"), "Page 1/2 ");
        assert_eq!(join_split_amounts("Transaction#: 21799506507   3,051 .38"), "Transaction#: 21799506507   3,051.38 ");
    }

    // GLM-OCR text of a TD business statement: categories in the summary, and on later
    // pages the amount printed on the line after the description.
    const TD_BUSINESS_OCR: &str = r#"
ACCOUNT SUMMARY
Beginning Balance 9,125.20
Average Collected Balance 11,431.13
Deposits 200.00
Interest Earned This Period 0.00
Electronic Deposits 6,743.16
Checks Paid 212.26
Days in Period 31
Electronic Payments 3,695.26
Ending Balance 12,160.84

DAILY ACCOUNT ACTIVITY

Deposits

POSTING DATE DESCRIPTION AMOUNT
03/26 DEPOSIT 200.00
Subtotal: 200.00

Electronic Deposits

POSTING DATE DESCRIPTION AMOUNT
03/03 CCD DEPOSIT, TOAST DEP MAR 02 ****395300UYBS7 6,743.16

Checks Paid No. Checks: 1 *Indicates break in serial sequence or check processed electronically and listed under Electronic Payments

DATE SERIAL NO. AMOUNT DATE SERIAL NO. AMOUNT
03/14 1008 212.26

Electronic Payments

POSTING DATE DESCRIPTION AMOUNT
03/03 DBCRD PMT AP, *****04036545477, AUT 030125 VISA DDA PUR AP
GOOGLE GSUITE SMOKECRAFT 650 2530000 * CA
15.26
03/04 CCD DEBIT, INTUIT 36169250 BILL_PAY VRA CLEANING SE
3,680.00
"#;

    #[test]
    fn td_wrapped_ocr_rows_with_the_amount_after_the_second_line() {
        let text = "Electronic Payments\nPOSTING DATE DESCRIPTION AMOUNT\n03/03 CCD DEBIT, MARGINEDGE CO SALE 300.00\n03/03 DEBIT POS AP, *****04036545477, AUT 030125 DDA PURCHASE AP\nRESTAURANT DEPOT ALEXANDRIA * VA 142.29\n03/03 CCD DEBIT, TOAST, INC TOAST, INC ST-Y3N7F9O5N8O8 16.24\n";
        let l = parse(&[(1, text)]);
        let amounts: Vec<f64> = l.transactions.iter().map(|t| t.amount).collect();
        assert_eq!(amounts, vec![300.0, 142.29, 16.24], "{:?}", l.transactions);
        assert!(l.transactions[1].description.ends_with("RESTAURANT DEPOT ALEXANDRIA VA"), "{}", l.transactions[1].description);
    }

    #[test]
    fn td_business_categories_and_split_ocr_rows() {
        let l = parse(&[(1, TD_BUSINESS_OCR)]);
        assert!((l.summary.total_credits.unwrap() - 6943.16).abs() < 0.001);
        assert!((l.summary.total_debits.unwrap() - 3907.52).abs() < 0.001);
        assert!((l.parsed_credit_total - 6943.16).abs() < 0.001, "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 3907.52).abs() < 0.001, "{:?}", l.transactions);
        let check = l.transactions.iter().find(|t| t.description == "Check 1008").expect("check row");
        assert_eq!(check.kind, Kind::Debit);
        let intuit = l.transactions.iter().find(|t| t.description.contains("INTUIT")).expect("split row");
        assert_eq!(intuit.amount, 3680.0);
    }

    // The first '+' credit marker is printed as its own token, the second glued to the amount.
    const FIRST_STATE: &str = r#"
Statement Date: 09/29/2023                                   Account No.:                 7698 Page: 1
        SMALL BUSINESS CHECKING SUMMARY                                 Type :    **REG    Status :   Active
      Category                                              Number                      Amount
      Balance Forward From 08/31/23                                                  10,769.47
      Deposits                                                   2                    1,125.00 +
      Debits                                                     1                      100.00
      Automatic Withdrawals                                      1                       50.00
      Automatic Deposits                                         1                       37.59+
      SERVICE CHARGE                                                                      2.40
      Ending Balance On 09/29/23                                                     11,779.66
        ALL CREDIT ACTIVITY
      Date           Type                     Amount        Date           Type      Amount
      09/01/23       Deposit                    226.00      09/11/23       Deposit     899.00
      Date                        Description                                                     Amount
      09/01/23                    STRIPE TRANSFER                                                  37.59
        ALL DEBIT ACTIVITY
      Date                        Description                                                     Amount
      09/05/23                    CHECK 1001                                                      100.00
      09/06/23                    ACH PAYMENT VENDOR                                               50.00
      09/29/23                    SERVICE CHARGE                                                    2.40
"#;

    #[test]
    fn first_state_bank_plus_marked_categories() {
        let l = parse(&[(1, FIRST_STATE)]);
        assert_eq!(l.summary.beginning_balance, Some(10769.47));
        assert!((l.summary.total_credits.unwrap() - 1162.59).abs() < 0.001);
        assert!((l.summary.total_debits.unwrap() - 152.40).abs() < 0.001);
        assert!((l.parsed_credit_total - 1162.59).abs() < 0.001, "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 152.40).abs() < 0.001, "{:?}", l.transactions);
    }

    // Fifth Third, OCR of a scanned first page: the beginning balance sits under its label,
    // the service charge line in the analysis block is already one of the withdrawals, and
    // the section header carries its own total. Then a Synovus statement follows in the same
    // file with dashed dates and a "Balance Summary" daily table on its second page.
    const FIFTH_THIRD_P1: &str = r#"
FIFTH THIRD
(TAMPA BAY)
Account Summary - 3867

06/01 Beginning Balance Checks
$95,550.90
Number of Days in Period 30
199 Withdrawals / Debits $(205,309.04)
46 Deposits / Credits $137,498.52

06/30 Ending Balance $27,740.38

Analysis Period: 05/01/26 - 05/31/26
Standard Monthly Service Charge $50.00
Service Charge withdrawn on 06/10/26 $164.00

Withdrawals / Debits
199 items totaling $205,309.04
Date Amount Description
06/01 69.00 WEB INITIATED PAYMENT AT PHILA26 L&I GOVSERVICE 1742210 060126
06/10 164.00 SERVICE CHARGE
06/01 85,000.00 OUTGOING WIRE TRANS 060126 TRN 20260601007522
"#;
    const FIFTH_THIRD_P2: &str = r#"
Withdrawals / Debits - continued
Date                     Amount              Description
06/29                    7,275.00            RAD DIVERS CREDITS 8220238670 062926 OFFSET TRANSACTION


Deposits / Credits                                                                                                46 items totaling $137,498.52
Date                     Amount              Description
06/01                      540.00            EARLY PAY: Lead Testing Ser REFUND FRO ST-I7C9G8H8A4O0 RAD DIVERSIFIED REIT I 060226
06/25                      934.00            DEPOSIT

Daily Balance Summary
Date                         Amount         Date                                 Amount Date                         Amount

06/01                        12,010.11      06/10                             53,705.90   06/22                      34,492.94
"#;
    const SYNOVUS_P1: &str = r#"
Statement of Account

Last statement: May 31, 2026
This statement: June 30, 2026

Summary of Account Balance

PINNACLE BANK (TN) DBA SYNOVUS BANK

Beginning balance 249,982.00
Deposits/Credits 127,738.00
Withdrawals/Debits 193,118.75
Ending balance 184,601.25
Average collected balance 317,689.00

Checks
Number Date Amount
0 06/12 2,000.00
* Skip in check sequence

Other Debits
Date Transaction Type Description Amount
06-01 Service Charge DOMESTIC WIRE IN 18.00
06-26 Dom Wire Out Online GGG Partners, LLCS TH STATE BANK GGG Partners, LLC 32,929.50
06-30 Transfer REF 1811408L FUNDS TRANSFER TO DEP XXXXXX4488 FROM ONLINE FUNDS TRANSFER VIA 102,658.00
06-30 Preauthorized Wd QUARTERLY FEE PAYMENT 260630 0000 1,254.25
"#;
    const SYNOVUS_P2: &str = r#"
        P.O. Box 2646-R, Columbus, GA 31902
                                                                           June 30, 2026

Deposits/Other Credits
 Date          Transaction Type               Description                                                       Amount
06-01         Domestic Wire IN                RAD DIVERSIFIED RE INCFIFTH THIRD BA                            85,000.00
                                              NK, NATIONAL ASS RAD DIVERSIFIED RE
06-30         Transfer                        REF 1811409L FUNDS TRANSFER FRM                                 42,055.00
                                              DEP XXXXXX4488 FROM ONLINE

Balance Summary
 Date                               Amount           Date                     Amount         Date                Amount
05-31                          249,982.00           06-12                 332,964.00         06-30            184,601.25
06-01                          334,964.00           06-26                 249,998.50
"#;

    #[test]
    fn fifth_third_ocr_summary_and_section_totals() {
        let l = parse(&[(1, FIFTH_THIRD_P1), (2, FIFTH_THIRD_P2)]);
        assert_eq!(l.summary.beginning_balance, Some(95550.90));
        assert_eq!(l.summary.ending_balance, Some(27740.38));
        assert_eq!(l.summary.total_credits, Some(137498.52));
        // No extra 164.00 for the "withdrawn on" service charge.
        assert_eq!(l.summary.total_debits, Some(205309.04));
        let credits: Vec<f64> = l.transactions.iter().filter(|t| t.kind == Kind::Credit).map(|t| t.amount).collect();
        assert_eq!(credits, vec![540.0, 934.0], "{:?}", l.transactions);
        assert!((l.parsed_debit_total - (69.0 + 164.0 + 85000.0 + 7275.0)).abs() < 0.001, "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 3);
    }

    #[test]
    fn synovus_dashed_dates_follow_fifth_third_in_one_file() {
        let l = parse(&[(1, FIFTH_THIRD_P1), (2, FIFTH_THIRD_P2), (3, SYNOVUS_P1), (4, SYNOVUS_P2)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        let syn = &l.statements[1];
        assert_eq!(syn.bank.as_deref(), Some("Synovus"));
        assert_eq!(syn.beginning_balance, Some(249982.0));
        assert_eq!(syn.total_credits, Some(127738.0));
        assert!((syn.parsed_credits.unwrap() - (85000.0 + 42055.0)).abs() < 0.001, "{:?}", l.transactions);
        assert!((syn.parsed_debits.unwrap() - (2000.0 + 18.0 + 32929.5 + 102658.0 + 1254.25)).abs() < 0.001, "{:?}", l.transactions);
        // The Balance Summary table is daily balances, not deposits.
        assert!(l.daily_balances.iter().any(|b| (b.balance - 332964.0).abs() < 0.001), "{:?}", l.daily_balances);
    }

    // Wells Fargo Initiate Business Checking, flat OCR: the transaction header reads
    // "Deposits/Credits Withdrawals/Debits", which are also summary label words, and
    // "Overdraft Protection" prose sits between the summary and the table.
    const WELLS_INITIATE_OCR: &str = r#"
Statement period activity summary
Beginning balance on 11/1 $931.26
Deposits/Credits 29,886.42
Withdrawals/Debits - 25,737.74
Ending balance on 11/30 $5,079.94

Overdraft Protection
This account is not currently covered by Overdraft Protection.

Transaction history

Date Check Number Description Deposits/Credits Withdrawals/Debits Ending daily balance
11/1 ATM Cash Deposit on 10/31 4156 S Carrier Pkwy Grand Prairie TX 0009227 ATM ID 0172T Card 4719 2,280.00
11/1 ATM Cash Deposit on 10/31 4156 S Carrier Pkwy Grand Prairie TX 0009228 ATM ID 0172T Card 4719 400.00
11/1 Denim.Com Payments 241101 Zgrvqcmd7Eyhcsq Gandy's Transport, LLC 2,861.50
11/1 Online Transfer to Gandy's Transport LLC Business Checking xxxxx5270 Ref #Ib0Q4Frkcb on 11/01/24 50.00
11/1 < Business to Business ACH Debit - Quarterly Fee Payment 241031 0000 Gandys Transport LLC 250.00 6,172.76
11/4 WT S0660413Dcc301 Morgan Stanley A /Org=Msl FBO Julie Beth Kaplan,Tod Subj Srf# S0660413Dcc301 Trn#260210168728 Rfb# 6,000.00 12,172.76
"#;

    #[test]
    fn wells_flat_ocr_with_credit_debit_header_words() {
        let l = parse(&[(1, WELLS_INITIATE_OCR)]);
        assert_eq!(l.summary.beginning_balance, Some(931.26));
        assert_eq!(l.summary.total_credits, Some(29886.42));
        assert_eq!(l.summary.total_debits, Some(25737.74));
        assert_eq!(l.transactions.len(), 6, "{:?}", l.transactions);
        // Denim.Com has no credit word; the running balance proves it is a credit. The
        // Morgan Stanley wire names its originator, so it is incoming.
        let kinds: Vec<Kind> = l.transactions.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, vec![Kind::Credit, Kind::Credit, Kind::Credit, Kind::Debit, Kind::Debit, Kind::Credit], "{:?}", l.transactions);
        assert!((l.parsed_credit_total - 11541.5).abs() < 0.001);
        assert!((l.parsed_debit_total - 300.0).abs() < 0.001);
    }

    #[test]
    fn pnc_scan_artifacts_underscored_dates_and_cent_amounts() {
        let text = "Checks and Other Debits\nFunds Transfers Out                                         3 transactions for a total of $299,103.30\nDate                                                        Transaction                                                     Reference\nposted                                          Amount      description                                                       number\n03/20                                        .15      Int'L Wire Out 233KI21041 Zp 1 Nny                W233KL21041Z P1N NY\n03/20                                 81,158.11       Wire Transfer Out 233KI4459Apq7Soa                W233KL4459APQ7SO A\n03/31_____________                   217,945.04       Wire Transfer Out 233Vk53280K63Efk                W233VK53280K63EFK\n03/02                                     2,500.00          Int'L Wire Out 2332K1637R217Ty5                      W2332K1637R217TY5\n";
        // Next page: the section resumes after the parent header, with another $2,500 wire
        // on the same day. Same listing, so it is not a cross-table repeat.
        let text2 = "Checks and Other Debits continued   -\n\nFunds Transfers Out   -   continued                   37 transactions for a total of $2,870,767.34\nDate                                                  Transaction                                                 Reference\nposted                                    Amount      description                                                   number\n03/02                                  2,500.00       Wire Transfer Out 2332K16370x25Ula                W2332K1637OX25U LA\n";
        let l = parse(&[(1, text), (2, text2)]);
        let amounts: Vec<f64> = l.transactions.iter().map(|t| t.amount).collect();
        assert_eq!(amounts, vec![0.15, 81158.11, 217945.04, 2500.0, 2500.0], "{:?}", l.transactions);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Debit));
        assert_eq!(l.transactions[3].table, l.transactions[4].table);
    }

    #[test]
    fn a_check_listed_three_times_counts_once() {
        // TD: history row, "Checks Paid" table, then the check image caption on a later page.
        let p1 = "Statement Period: Dec 18 2025-Jan 17 2026\nBeginning Balance 187.24\nDaily Account Activity\nElectronic Payments\nPOSTING DATE DESCRIPTION AMOUNT\n12/22 Check #3200 59.00\n12/22 DBCRD PUR AP, AMAZON MKTPL 44.33\n12/22 DBCRD PUR AP, CHICK FIL A 44.33\n\nChecks Paid\nDATE SERIAL NO. AMOUNT\n12/22 3200 59.00\n";
        let p2 = "#3200       12/22       $59.00\n";
        let l = parse(&[(1, p1), (2, p2)]);
        let checks: Vec<&Txn> = l.transactions.iter().filter(|t| t.description.contains("3200")).collect();
        assert_eq!(checks.len(), 1, "{:?}", l.transactions);
        // Two real $44.33 purchases on the same day both survive.
        assert_eq!(l.transactions.iter().filter(|t| (t.amount - 44.33).abs() < 0.001).count(), 2, "{:?}", l.transactions);
    }

    #[test]
    fn bulleted_ocr_rows_read_as_transactions() {
        let text = "Electronic Deposits\n- 04/18: CCD DEPOSIT, TOAST DEP APR 17 0004395300JL33W: 3,176.12\n- 04/19: CCD DEPOSIT, DOORDASH, INC. 1051 NORTH ST-M1O0R4C9Y5C0: 4,055.80\n";
        let l = parse(&[(1, text)]);
        let amounts: Vec<f64> = l.transactions.iter().map(|t| t.amount).collect();
        assert_eq!(amounts, vec![3176.12, 4055.80], "{:?}", l.transactions);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Credit));
        // ("APR 17" after a capitalized word is rewritten like a date column; a month after
        // a lowercase word, "Hcclaimmpt May 4", stays prose.)
        assert_eq!(l.transactions[0].description, "CCD DEPOSIT, TOAST DEP 04/17 0004395300JL33W");
    }

    #[test]
    fn stacked_date_and_amount_cells_are_zipped() {
        let text = "Deposits and Other Credits\nDate     Amount Description\n06/03        8,010.79    EDI PYMNTS EPIC Pharmacy Ne\n         06/04\n         06/05\n                      2,566.85\n                      1,171.23\n                                  EDI PYMNTS EPIC Pharmacy Ne\n06/05        1,422.49    DEPOSIT\n";
        let l = parse(&[(1, text)]);
        let amounts: Vec<f64> = l.transactions.iter().map(|t| t.amount).collect();
        assert_eq!(amounts, vec![8010.79, 2566.85, 1171.23, 1422.49], "{:?}", l.transactions);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Credit));
    }

    #[test]
    fn smeared_court_ocr_section_headers_still_switch_sections() {
        assert_eq!(section_for("!OTHER WITHDRAWALS, FEES & C H A R G E S - I - - - - - - - - -"), Some(Kind::Debit));
        assert_eq!(section_for("IDEPOSITS AND ADDITIONS I--------------"), Some(Kind::Credit));
        assert_eq!(section_for("08/21   Withdrawal   $483,000.00"), None);
        // "IDAILy ENDING BALANCE I" is the daily balance heading, not an ending-balance label.
        let text = "IELECTRONIC WITHDRAWALS!\n DA TE   DESCRIPTION      AMOUNT\n09/26    Online Domestic Wire Transfer      $200,000.00\nIDAILy ENDING BALANCE I\nDATE          AMOUNT\n09/17        $7,251,304.81\n09/18         7,251,311.87\n";
        let l = parse(&[(1, text)]);
        assert_eq!(l.transactions.len(), 1, "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 2, "{:?}", l.daily_balances);
    }

    #[test]
    fn sweep_accounts_starting_at_zero_split_on_the_account_number() {
        let a = "COMMERCIAL CHECKING                      Account Number:  XXXXXX5905\nBalance Summary\nBeginning Balance as of 06/29/24        $0.00\n+ Deposits and Credits (1)             $74.00\n- Withdrawals and Debits (1)           $74.00\nEnding Balance as of 07/31/24           $0.00\nCredits\nDate     Description                   Additions\nJul 17   AUTOMATIC TRANSFER              $74.00\nDebits\nDate     Description                   Subtractions\nJul 17   MAINTENANCE FEE                 -$74.00\n";
        let b = a.replace("XXXXXX5905", "XXXXXX1538").replace("74.00", "295,984.49");
        let l = parse(&[(1, a), (2, &b)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        assert_eq!(l.statements[0].account_last4.as_deref(), Some("5905"));
        assert_eq!(l.statements[1].account_last4.as_deref(), Some("1538"));
        assert_eq!(l.statements[1].total_credits, Some(295984.49));
    }

    // U.S. Bank Uni-Statement, OCR text: trailing '-' debit markers in the summary, a check
    // row with a reference number followed by the right-hand column of the same line.
    const US_BANK: &str = r#"
Account Number: 7243
Statement Period: Nov 6, 2025
through Nov 30, 2025
U.S. Bank National Association
Account Summary
Beginning Balance on Nov 6 $137,352.20 Annual Percentage Yield Earned 0.00486%
Deposits / Credits 10,487.83 Interest Earned this Period $0.47
Other Withdrawals 962.49- Interest Paid this Year $0.62
Checks Paid 1,495.73- Number of Days in Statement Period 25
Ending Balance on Nov 30, 2025 $145,381.81
Deposits / Credits
Date Description of Transaction Ref Number Amount
Nov 14 Mobile Check Deposit 9250600501 $42.59
Nov 28 Interest Paid 2800004304 0.57
Total Deposits / Credits $10,487.83
Other Withdrawals
Date Description of Transaction Ref Number Amount
Nov 10 Electronic Withdrawal REF=253110154517410N00 $594.16-
Total Other Withdrawals $962.49-
Checks Presented Conventionally
Check Date Ref Number Amount Check Date Ref Number Amount
5001 Nov 26 8651583986 113.19 Conventional Checks Paid (2) $1,495.73-
Balance Summary
Date Ending Balance Date Ending Balance Date Ending Balance
Nov 10 136,758.04 Nov 24 147,043.45 Nov 26 146,849.66
"#;

    #[test]
    fn us_bank_single_signed_category_is_the_debit_total() {
        let text = "Account Summary\nBeginning Balance on Nov 6 $ 6,984.00\nDeposits / Credits 3,579.40\nChecks Paid 2,675.62-\nEnding Balance on Nov 30, 2025 $ 7,887.78\nChecks Presented Conventionally\nCheck Date Ref Number Amount\n5001 Nov 25 8351931142 2,675.62\n";
        let l = parse(&[(1, text)]);
        assert_eq!(l.summary.total_debits, Some(2675.62), "{:?}", l.summary);
        assert_eq!(l.summary.total_credits, Some(3579.40));
        assert!((l.parsed_debit_total - 2675.62).abs() < 0.001, "{:?}", l.transactions);
    }

    #[test]
    fn us_bank_trailing_minus_summary_and_check_with_reference() {
        let l = parse(&[(1, US_BANK)]);
        assert_eq!(l.summary.beginning_balance, Some(137352.20));
        assert_eq!(l.summary.total_credits, Some(10487.83));
        assert!((l.summary.total_debits.unwrap() - 2458.22).abs() < 0.001, "{:?}", l.summary);
        let check = l.transactions.iter().find(|t| t.description == "Check 5001").expect("check row");
        assert_eq!(check.amount, 113.19);
        assert_eq!(check.date, "2025-11-26");
        assert!((l.parsed_credit_total - 43.16).abs() < 0.001, "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 707.35).abs() < 0.001, "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 3);
    }

    #[test]
    fn undated_ocr_rows_after_dated_ones_take_the_last_date() {
        let text = "Other withdrawals, debits and service charges\nDATE DESCRIPTION AMOUNT($)\n03/22 ACH CORP DEBIT PLIC-PERIS PRINCIPAL LIFE P TRUIST CUSTOMER ID 8-1567000002584 5.01\nPAYMENT Greystone Power 7904 VitalPharmaceuticals 531.28\nOUTGOING WIRE TRANSFER WIRE REF# 20230322-00020481 2,231,991.05\n03/23 ACH SETTLEMENT 100.00\n";
        let l = parse(&[(1, text)]);
        let rows: Vec<(String, f64)> = l.transactions.iter().map(|t| (t.date.clone(), t.amount)).collect();
        assert_eq!(rows, vec![("03/22".into(), 5.01), ("03/22".into(), 531.28), ("03/22".into(), 2231991.05), ("03/23".into(), 100.0)], "{:?}", l.transactions);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Debit));
    }
    #[test]
    fn summary_labels_read_column_by_column_are_zipped_with_their_amounts() {
        let text = "CHECKING SUMMARY\n\nBeginning Balance\nDeposits and Additions\nATM & Debit Card Withdrawals\nElectronic Withdrawals\nFees\nEnding Balance\n$8.84\n$6,930.00\n$-28.25\n$-6,538.00\n$-43.00\n$329.59\n\nDEPOSITS AND ADDITIONS\n\nDATE DESCRIPTION AMOUNT\n09/17 Deposit 1989894236 3,000.00\n";
        let l = parse(&[(1, text)]);
        let s = &l.summary;
        assert_eq!((s.beginning_balance, s.ending_balance), (Some(8.84), Some(329.59)));
        assert_eq!((s.total_credits, s.total_debits), (Some(6930.0), Some(6609.25)), "{:?}", s);
    }

    #[test]
    fn rows_on_a_page_that_lost_its_header_follow_their_own_words_not_the_inherited_section() {
        // TD: page 4 is "Electronic Deposits", page 5 starts straight into the payments
        // table (the OCR dropped "Electronic Payments"). The debit words decide, and the
        // wordless utility payment follows the page's decided rows.
        let p4 = "Electronic Deposits\nDate Description Debits\n04/01 CCD DEPOSIT, TOAST DEP MAR 31 0004395300IY4ZP 7,213.80\n";
        let p5 = "Date Description Debits\n04/02 CCD DEBIT, CHEF'S WAREHOUSE 3 BILLS e43792119 3,667.27\n04/04 eTransfer Debit, Online Xfer Transfer to CC 4847381245915726 1,500.00\n04/22 ELECTRONIC PMT-WEB, WASHINGTON GAS PAYMENT 310003392126 1,032.23\n";
        let l = parse(&[(1, p4), (2, p5)]);
        let kinds: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(kinds, vec![(Kind::Credit, 7213.80), (Kind::Debit, 3667.27), (Kind::Debit, 1500.0), (Kind::Debit, 1032.23)], "{:?}", l.transactions);
    }

    #[test]
    fn dated_balance_rows_in_an_activity_table_are_not_transactions() {
        let text = "SAVINGS ACCOUNT - XXXXXXX6598\n\nAccount Summary\nDate Description Amount Interest Summary\n11/01/2025 Beginning Balance $323.01 Annual Percentage Yield Earned 0.04%\n1 Credit(s) This Period $0.01 Interest Days 30\n0 Debit(s) This Period $0.00 Interest Earned $0.01\n11/30/2025 Ending Balance $323.02 Interest Paid Year-to-Date $0.11\n\nAccount Activity\nPost Date Description Debits Credits Balance\n11/01/2025 Beginning Balance $323.01\n11/24/2025 INTEREST PAID 11/01 THROUGH 11/30 $0.01 $323.02\n11/30/2025 Ending Balance $323.02\n";
        let l = parse(&[(1, text)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 0.01)], "{:?}", l.transactions);
        assert_eq!((l.summary.beginning_balance, l.summary.ending_balance), (Some(323.01), Some(323.02)));
    }

    #[test]
    fn three_column_tables_unfold_even_when_the_columns_nearly_touch() {
        // First State Bank prints deposits and checks three entries to a line; the amount
        // of one column and the date of the next are one space apart.
        let text = "        ALL CREDIT ACTIVITY\n      Date           Type                     Amount Date                  Type      Amount Date             Type                 Amount\n      09/05/23       Deposit                  1,650.00 09/26/23            Deposit    388.12\n\n        CHECKS AND OTHER DEBITS                                                * indicates a gap in the check numbers\n\n      Date        Check #     Amount Date         Check #     Amount      Date          Check #          Amount\n      09/25/23      1029        445.07 09/12/23    10007        123.50    09/25/23       10018             463.00\n      09/13/23     10006        470.00 09/25/23    10017        516.00\n";
        let l = parse(&[(1, text)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 1650.0), (Kind::Credit, 388.12), (Kind::Debit, 445.07), (Kind::Debit, 470.0), (Kind::Debit, 123.5), (Kind::Debit, 516.0), (Kind::Debit, 463.0)], "{:?}", l.transactions);
    }

    #[test]
    fn markdown_ocr_bullets_with_balances_and_a_pipe_summary_table() {
        // A credit union statement the OCR model retold as Markdown: the summary as a pipe
        // table, the rows as bullets ending in amount then running balance.
        let text = "Account Summary\n\nPrevious Date | Beginning Balance | Deposits | Interest Paid | Withdrawals | Service Charge | Ending Balance\n------- | --- | --- | --- | --- | --- | --- |\n10/01/2023 | 16,129.15 | 3,160.60 | 0.00 | 6,532.41 | 0.00 | 12,757.34\n\n**BEGINNING BALANCE**\n\n- Oct 02: External Withdrawal BK OF AMER MC - ONLINE PMT | 1,651.07 | 14,478.08\n- Oct 10: External Withdrawal AMEX EPAYMENT ER AM - ACH PMT | 959.08 | 13,519.00\n- Oct 12: Check 343 | 500.00 | 13,019.00\n- Oct 18: External Deposit SSA TREAS 310 - XXSOC SEC | 3,160.60 | 16,179.60\n";
        let l = parse(&[(1, text)]);
        let s = &l.summary;
        assert_eq!((s.beginning_balance, s.ending_balance, s.total_credits, s.total_debits), (Some(16129.15), Some(12757.34), Some(3160.6), Some(6532.41)));
        let rows: Vec<(String, Kind, f64)> = l.transactions.iter().map(|t| (t.date.clone(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![("2023-10-02".into(), Kind::Debit, 1651.07), ("2023-10-10".into(), Kind::Debit, 959.08), ("2023-10-12".into(), Kind::Debit, 500.0), ("2023-10-18".into(), Kind::Credit, 3160.6)], "{:?}", l.transactions);
    }

    #[test]
    fn credit_union_sub_accounts_two_dates_and_descriptions_above_the_row() {
        // Texas credit union: several sub-accounts on one statement, each with its own
        // summary; rows carry a transaction date and an effective date; the description
        // sits on the line above and wraps onto the line below.
        let text = "PRIMARY SHARE (0000)\n    Beginning Balance            Deposits/Credits           Withdrawals/Debits           Ending Balance              Dividends YTD\n                  5.00                       0.00                        -5.00                      0.00                       0.00\nTrans   Effective\nDate    Date      Description                                                         Withdrawal           Deposit         Balance\nSep 01            Balance Forward                                                                                            5.00\nSep 16   Sep 16   Withdrawal                                                               -5.00                              0.00\nSep 30            Ending Balance                                                                                             0.00\n\nKASASA CASH (0008)\n    Beginning Balance            Deposits/Credits           Withdrawals/Debits           Ending Balance              Dividends YTD\n             -1,478.50                   2,203.00                      -724.50                      0.00                       0.01\nTrans   Effective\nDate    Date      Description                                                         Withdrawal           Deposit         Balance\nSep 01            Balance Forward                                                                                      -1,478.50\n                  Withdrawal RETURNED ACH FEE In the amount $100.00 Diverse\nSep 03   Sep 03                                                                           -34.50                         -1,513.00\n                  Capital\n                  Withdrawal RETURNED ACH FEE In the amount $101.00 Kash\nSep 03   Sep 03                                                                           -34.50                         -1,547.50\n                  Advance LLC\nSep 16   Sep 16   Deposit ACH KASH ADVANCE                                                                 2,203.00       655.50\n";
        let l = parse(&[(1, text)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        let k = &l.statements[1];
        assert_eq!((k.beginning_balance, k.ending_balance, k.total_credits, k.total_debits), (Some(-1478.5), Some(0.0), Some(2203.0), Some(724.5)));
        let rows: Vec<(Kind, f64, String)> = l.transactions.iter().map(|t| (t.kind, t.amount, t.description.clone())).collect();
        assert_eq!(rows[0], (Kind::Debit, 5.0, "Withdrawal".into()));
        assert_eq!(rows[1], (Kind::Debit, 34.5, "Withdrawal RETURNED ACH FEE In the amount $100.00 Diverse Capital".into()));
        assert_eq!(rows[3], (Kind::Credit, 2203.0, "Deposit ACH KASH ADVANCE".into()));
    }

    #[test]
    fn umb_court_scan_margin_junk_check_image_pages_and_posted_checks() {
        // UMB (court-filed scan): stray margin marks in front of dates, "CHEC K# 131" in the
        // list against a two-column "Checks Posted" table with references, and an "Images"
        // page whose captions repeat the checks.
        let p1 = "          Account Summary\n          Beginning Balance as of 03/01/2024     $12,943.16    Total Days in Statement Period      31\n          + Deposits and Credits (2)             $2 ,850.00\n          - Withdrawals and Debits (3)           $3,151.07\n          - Service Charges and Fees                  $59.38\n          Ending Balance as of 03/31 /2024        $12,582.71\n         Date    Description                                                   Deposits         Withdrawals\n         Mar 04 VENMO         CASHOUT DAN BROWN                                  1,000.00\n         Mar 04 ANALYSIS SERVICE CHARGE(S)                                                              59.38\n         Mar 04 CHECK# 129                                                                           1,500.00\n         Mar 15 CHEC K# 131                                                                            456.11\n0        Mar 20 DEPOSIT                REF 33269043                            1,850.00\nc':,     Mar 25 SUPPORTPDFFILLER.CO 855-750166 MA 03/22 0486                                         1,194.96\n              Checks Posted                                   * Indicates a Skip in Check Number(s)\n               Check No.      Date              Amount Ref No.             Check No.     Date               Amount Ref No.\n                129           Mar04             1,500 .00   00081094018    131           Mar 15                456.11    00035232488\n";
        let p2 = "              Images\n                IUNIES                          CHECKING DEPOSIT\n               03/0 5/2 024                                         #0            $750.00\n               03/01/2024                                      # 130               $694.96\n";
        let l = parse(&[(1, p1), (2, p2)]);
        let s = &l.summary;
        assert_eq!(s.total_credits, Some(2850.0), "{:?}", s);
        assert!((s.total_debits.unwrap() - 3210.45).abs() < 0.005, "{:?}", s);
        assert!((l.parsed_credit_total - 2850.0).abs() < 0.005, "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 3210.45).abs() < 0.005, "{:?}", l.transactions);
    }

    #[test]
    fn credit_union_three_line_headings_balance_forward_summaries_and_a_year_to_date_summary_page() {
        let p1 = "                                    (ID\n   PRIMARY SHARE\n                                    0000)\n\n     Balance Forward                                                     $0.00\n         + 1       Deposit                                              $10.00\n     Ending Balance                                                     $10.00\n    Transaction Detail\n     Date          Description                          Withdrawals             Deposits            Balance\n                   Balance Forward                                                                    $0.00\n     09/06         Deposit Kiosk Transfer                                          $10.00             $10.00\n                   From CTCHGC LLC XXXXXXXXXX Share 0008\n                   Ending Balance                                                                    $10.00\n\n                                    (ID\n   KASASA CASH BACK\n                                    0008)\n\n     Balance Forward                                                     $0.00\n        - 2      Withdrawals                                         $1,065.00\n        + 2      Deposits                                            $1,725.00\n     Ending Balance                                                    $660.00\n    Transaction Detail\n     Date          Description                          Withdrawals             Deposits            Balance\n                   Balance Forward                                                                    $0.00\n     09/06         Deposit Kiosk Transfer                                          $25.00             $25.00\n     09/10         Deposit Kiosk Transfer                                       $1,700.00          $1,725.00\n     09/10         Withdrawal Kiosk Transfer                    -$25.00                            $1,700.00\n     09/11         Withdrawal Kiosk Transfer                 -$1,040.00                               $660.00\n";
        let p2 = "                   Ending Balance                                                                   $660.00\n   Summary\n   Year to Date Totals\n       Dividends Paid YTD                              $0.00\n       PRIMARY SHARE                                  $10.00\n       KASASA CASH BACK                              $660.00\n";
        let l = parse(&[(1, p1), (2, p2)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        let k = &l.statements[1];
        assert_eq!((k.beginning_balance, k.ending_balance, k.total_credits, k.total_debits), (Some(0.0), Some(660.0), Some(1725.0), Some(1065.0)), "{:?}", k);
        assert_eq!((k.parsed_credits, k.parsed_debits), (Some(1725.0), Some(1065.0)));
    }

    #[test]
    fn citizens_commercial_balance_calculation_amount_before_description_and_daily_table_with_a_summary_column() {
        let p1 = "Commercial Checking for                                604-5\n\nBalance Calculation\nPrevious Balance                                         144,072.99\n\nChecks                                       -                     .00\n\nDebits                                       -              1,328.62\n\nDeposits & Credit                            +            27,291.91\n\nCurrent Balance                              =           170,036.28\n\nTRANSACTION DETAILS FOR COMMERCIAL CHECKING ACCOUNT ENDING 604-5\n\nDebits **                                                                                                         Previous Balance\n**May include checks that have been processed electronically by the payee/merchant.\n\nDate                 Amount         Description\nOther Debits\n02/16                1,328.62       SERVICE CHARGE\n";
        let p2 = "Commercial Checking for                    604-5 Continued\n\nDeposits & Credits                                                                       Total Deposits & Credits\n\nDate             Amount      Description                                                 +              27,291.91\n\n02/01             529.77     Worldpay COMB. DEP. 013123 4445082119223\n\n02/02          26,762.14    Worldpay COMB. DEP. 020123 4445082119223\n\nDaily Balance                                                                                     Current Balance\n\nDate               Balance    Date              Balance    Date               Balance    =             170,036.28\n\n02/01           144,602.76    02/10           154,631.39   02/21            167,479.86\n";
        let l = parse(&[(1, p1), (2, p2)]);
        let s = &l.summary;
        assert_eq!((s.beginning_balance, s.ending_balance, s.total_credits, s.total_debits), (Some(144072.99), Some(170036.28), Some(27291.91), Some(1328.62)), "{:?}", s);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 1328.62), (Kind::Credit, 529.77), (Kind::Credit, 26762.14)], "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 3, "{:?}", l.daily_balances);
    }

    #[test]
    fn amount_first_tables_keep_the_amount_after_the_date_when_the_description_quotes_another() {
        // Wells OCR page: "Effective date  Posted date  Amount  Transaction detail"; NSF fee
        // rows quote the returned item's amount and date inside the description.
        let text = "Electronic debits/bank debits (continued)\n\nEffective date Posted date Amount Transaction detail\n12/30 35.00 NSF Return Item Fee for a Transaction Received on 12/29 $23,530.00 Check # 41064\n12/31 35.00 NSF Return Item Fee for a Transaction Received on 12/30 $4,199.00 Yes Capital Grp Mag Auto I 211230\n12/31 2,138.47 Business to Business ACH Debit - Wynwood Capital Direct Pay 122921 21122916\n";
        let l = parse(&[(1, text)]);
        let rows: Vec<(String, Kind, f64)> = l.transactions.iter().map(|t| (t.date.clone(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![("12/30".into(), Kind::Debit, 35.0), ("12/31".into(), Kind::Debit, 35.0), ("12/31".into(), Kind::Debit, 2138.47)], "{:?}", l.transactions);
        assert!(l.transactions[1].description.contains("Yes Capital"));
    }

    #[test]
    fn keybank_dashed_dates_signed_categories_and_a_quiet_month_before_a_busy_one() {
        // Two statements of the same account: November ends where it began, so December
        // opens with the same figure and still has to be its own statement.
        let nov = "KeyBank                          Business Banking Statement\n                                     November 30, 2024\nKeyBank Basic Business Checking\nDNT PROPERTY INVESTMENTS LLC\n            Beginning balance 10-31-24           $94.29\n            Ending balance 11-30-24              $94.29\n";
        let dec = "KeyBank                          Business Banking Statement\n                                     December 31, 2024\nKeyBank Basic Business Checking\nDNT PROPERTY INVESTMENTS LLC\n            Beginning balance 11-30-24           $94.29\n            1 Addition                        +7,170.00\n            3 Subtractions                    -7,065.85\n            Ending balance 12-31-24             $198.44\nAdditions\n     Deposits Date   Serial #   Source\n              12-3              Existing Sec 8 Vendor Pmt                 $7,170.00\n                                Total additions                           $7,170.00\nSubtractions\nPaper Checks       * check missing from sequence\n Check   Date     Amount\n 1391    12-27    $5,646.62\n                                Paper Checks Paid                         $5,646.62\n     Withdrawals Date   Serial #   Location\n              12-13             Fiffik Law Groupj2370 Oofftrn*1*Cz10000B4Uvwc\\R    $1,252.62\n              12-17             Peoplesgas     Peoplesgas                     166.61\n                                Total subtractions                        $7,065.85\n";
        let l = parse(&[(1, nov), (2, dec)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        let d = &l.statements[1];
        assert_eq!((d.beginning_balance, d.ending_balance), (Some(94.29), Some(198.44)));
        assert_eq!((d.total_credits, d.total_debits), (Some(7170.0), Some(7065.85)));
        let rows: Vec<(String, Kind, f64)> = l.transactions.iter().map(|t| (t.date.clone(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![("2024-12-03".into(), Kind::Credit, 7170.0), ("2024-12-27".into(), Kind::Debit, 5646.62), ("2024-12-13".into(), Kind::Debit, 1252.62), ("2024-12-17".into(), Kind::Debit, 166.61)], "{:?}", l.transactions);
        assert_eq!(parse_date_token("1-2"), None);
        assert_eq!(parse_date_token("9-30-24"), Some((9, 30, Some(2024))));
        assert_eq!(parse_date_token("8-8-24"), Some((8, 8, Some(2024))));
    }

    #[test]
    fn keybank_commercial_short_dashed_dates_and_two_column_check_table() {
        // "6-3" is only a date once the statement has shown dashed dates; the check table
        // repeats "Check Date Amount" twice on a line.
        let text = "Commercial Transaction                5934\n            Beginning balance 5-31-24                         $96,234.72\n            3 Additions                                     +9,756.85\n            3 Subtractions                                    -351.50\n            Net fees and charges                               -97.96\n            Ending balance 6-30-24                         $105,541.11\nAdditions\n              Deposits Date       Serial #      Source\n                       6-3                      Hrtland Pmt Sys Txns/Fees 650000012528306                    $9,539.97\n                       6-3                      Osu Health Systeach Pmt 1259                                    154.35\n                       6-10                     Script Care, Ltdach Paymentrn*1*0000579417*1760                  62.53\nSubtractions\nPaper Checks                * check missing from sequence\n Check         Date               Amount        Check        Date         Amount\n 30091         6-18                 $40.25      30092       6-24            311.25\n                                                            Paper Checks Paid                  $351.50\nFees and\ncharges       Date                                    Quantity   Unit Charge\n              8-8-24         Jul Analysis Service Chg   1              97.96    -$97.96\n";
        let l = parse(&[(1, text)]);
        let s = &l.summary;
        assert_eq!((s.total_credits, s.total_debits), (Some(9756.85), Some(449.46)), "{:?}", s);
        let rows: Vec<(String, Kind, f64)> = l.transactions.iter().map(|t| (t.date.clone(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![("2024-06-03".into(), Kind::Credit, 9539.97), ("2024-06-03".into(), Kind::Credit, 154.35), ("2024-06-10".into(), Kind::Credit, 62.53), ("2024-06-18".into(), Kind::Debit, 40.25), ("2024-06-24".into(), Kind::Debit, 311.25), ("2024-08-08".into(), Kind::Debit, 97.96)], "{:?}", l.transactions);
    }

    #[test]
    fn sunrise_deposit_grid_rule_glyphs_and_court_stamp_years() {
        // Table rules come out as "I" glued to the date; the court's stamps carry a later year
        // on every page and must not outvote the statement's own dates.
        let p1 = "FILED: ORANGE COUNTY CLERK 10/24/2025 04:29 PM        INDEX NO. EF007283-2025\nNYSCEF DOC. NO. 42                                    RECEIVED NYSCEF: 10/24/2025\n   Last Statement Previous Balance    Total Credits        Total Debits    This Statement  Current Balance\n       08/30/24           $3,702.38     $200.00 (2)          $50.00 (1)        09/30/24         $3,852.38\n   Total Days In Statement Period 08/31/24 Through 09/30/24:      31\n    DEPOSITS\n     Reference     Date        Amount\n                  I 09/10I            $100.00\n    OTHER CREDITS\n   Date Description                                              Amount\n   09/03 Toast Dep Sep 02 XXXXXX0000OPHBG                       $100.00\n    DEBITS\n   Date Description                                              Amount\n   09/05 Returned Checks NSF Charge                               $50.00\n   Your next statement period will end on October 31, 2024.\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(String, Kind, f64)> = l.transactions.iter().map(|t| (t.date.clone(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![("2024-09-10".into(), Kind::Credit, 100.0), ("2024-09-03".into(), Kind::Credit, 100.0), ("2024-09-05".into(), Kind::Debit, 50.0)], "{:?}", l.transactions);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(200.0), Some(50.0)));
    }

    #[test]
    fn td_checks_with_one_amount_each_are_distinct_and_captions_repeat_by_number() {
        // Dozens of $970.00 checks on one day, each its own serial, listed in a table that
        // continues on the next page under a different title; the image captions "#0361"
        // repeat check 361 by number even when their OCR misreads the amount.
        let p1 = "Checks Paid               No. Checks: 4         *Indicates break in serial sequence\nDATE                   SERIAL NO.                           AMOUNT                                DATE                   SERIAL NO.                            AMOUNT\n09/02                     1774                              970.00                                  09/02                     1830                              970.00\n";
        let p2 = "Checks Paid (continued)\nDATE                   SERIAL NO.                           AMOUNT                                DATE                   SERIAL NO.                            AMOUNT\n09/02                     1860                              970.00                                  09/02                     1897                              970.00\n";
        let p3 = "#01774                      09/02                       $2,000.00                      #1830                      09/02                             $970.00\n#1860                       09/02                         $970.00                      #1897                      09/02                             $970.00\n";
        let l = parse(&[(1, p1), (2, p2), (3, p3)]);
        let mut checks: Vec<&str> = l.transactions.iter().map(|t| t.description.as_str()).collect();
        checks.sort();
        assert_eq!(checks, vec!["Check 1774", "Check 1830", "Check 1860", "Check 1897"], "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 3880.0).abs() < 0.005);
    }

    #[test]
    fn column_table_rows_split_over_lines_and_a_garbled_cell_read_from_the_running_balance() {
        let p1 = "   Account Summary\n   Date           Description                                           Amount\n   03/01/2025     Beginning Balance                                      $15.70\n                  3 Credit(s) This Period                            $4,240.00\n                  2 Debit(s) This Period                             $2,596.58\n   03/31/2025     Ending Balance                                      $1,659.12\n   Account Activity\n    Post Date    Description                                                                     Debits                Credits                 Balance\n   03/01/2025    Beginning Balance                                                                                                               $15.70\n   03/03/2025    RETURNED             BRYAND DA4090589                                                               $3,333.00               $3,348.70\n   03/03/2025                                                                                                        M,000.00               $4,348.70\n   03/04/2025\n                 POSST CROlXCASINOPURCHASE   4384 STATERD70\n                                                                                            $2,500.00                                        $1,848.70\n   03/05/2025    XX2823CHKPURCHSIG SP FRAGRANT\n                                                 JEWE                                          $96.58                                        $1,752.12\n                 CASHOUT 401743570                                                                                    $ 907.00                $2,659.12\n   03/06/2025\n                 ONLINE TRANSFER TO SAVINGS                                                  $1,000.00                                        $1,659.12\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 3333.0), (Kind::Credit, 1000.0), (Kind::Debit, 2500.0), (Kind::Debit, 96.58), (Kind::Credit, 907.0), (Kind::Debit, 1000.0)], "{:?}", l.transactions);
        assert!(l.transactions[1].description.contains("running balance"), "{:?}", l.transactions[1]);
        assert!(l.transactions[2].description.starts_with("POSST"), "{:?}", l.transactions[2]);
        assert!(l.transactions[3].description.ends_with("JEWE"), "{:?}", l.transactions[3]);
    }

    #[test]
    fn a_second_copy_of_the_statement_in_another_layout_is_dropped() {
        let p1 = "Balance Calculation\nPrevious Balance                                  100.00\nChecks                              -              50.00\nDebits                              -              30.00\nDeposits & Credit                   +             200.00\nCurrent Balance                     =             220.00\nTRANSACTION DETAILS FOR BUSINESS CHECKING ACCOUNT ENDING 759-4\nChecks\nCheck #                     Amount                 Date\n1906                         50.00                08/06\nDebits\nDate                 Amount        Description\n08/05                 30.00        0675 DBT PURCHASE\nDeposits & Credits\nDate                 Amount        Description\n08/01                200.00        INCOMING WIRE TRANSFER\n";
        let p2 = "Checks\nCheck #                Amount             Date                Item No.\n1906                    50.00            08/06       000000065362026\nWithdrawals & Debits **\nDate                 Item No.            Amount              Description\n08/05      000000088803863                30.00           0675 DBT PURCHASE\nDeposits & Credits\nDate                 Item No.            Amount              Description\n08/01      024213005227773               200.00           INCOMING WIRE TRANSFER\n";
        let l = parse(&[(1, p1), (2, p2)]);
        assert_eq!(l.transactions.len(), 3, "{:?}", l.transactions);
        assert!((l.parsed_credit_total - 200.0).abs() < 0.005 && (l.parsed_debit_total - 80.0).abs() < 0.005);
    }

    #[test]
    fn newest_first_reports_keep_the_column_kinds() {
        // An account-detail export lists newest first; the balance beside a row relates to
        // the row above, so arithmetic must not flip a $3,000 debit that follows another.
        let p1 = "   TRANSACTION DETAILS\n   DATE         Particulars                                                             Deposits     Withdrawals     Balance\n   03-28-2022   ALFA Advance DES:FAXXX647-P                                                          $3000.00   $153669.08\n   03-25-2022   Zelle Transfer Conf# v7kx67gb7; RAJVINDER                                             $3000.00   $156669.08\n   03-25-2022   Itria Venture H DES:Punj-aab T                                                        $9285.71   $159669.08\n   03-24-2022   ENGLAND CARRIER DES:USBSNGPT                                          $86145.90                  $168954.79\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 3000.0), (Kind::Debit, 3000.0), (Kind::Debit, 9285.71), (Kind::Credit, 86145.9)], "{:?}", l.transactions);
    }

    #[test]
    fn reversal_pairs_the_bank_nets_out_of_its_totals_are_listed_but_not_summed() {
        let p1 = "          BUSINESS ESSENTIAL CHECKING 0750\n          Posted Eff                                                       Withdrawals/                                 Deposits/         New\n          Date   Date Transaction Description                                     Debits                                  Credits         Balance\n          10/01       Beginning Balance                                                                                                 $1,000.00\n          10/09       Fee Withdrawal Courtesy Pay Fee                              -35.00                                                  965.00\n          10/17       POS Card purchase Withdrawal LOWE'S #1935 5750                -30.73                                                  934.27\n          10/18       Card purchase return Withdrawal Adjustment LOWES                                                    30.73             965.00\n          10/22       Fee Withdrawal Courtesy Pay Fee -- Reversed                                                         35.00           1,000.00\n          10/25       Deposit Share ID 0001                                                                              500.00           1,500.00\n          10/31       Ending Balance                                                                                                    $1,500.00\n                       Total Credits for this account: 500.00\n                       Total Debits for this account: 0.00\n";
        let l = parse(&[(1, p1)]);
        assert_eq!(l.transactions.len(), 5, "{:?}", l.transactions);
        assert_eq!(l.netted.len(), 4, "{:?}", l.netted);
        assert!((l.parsed_credit_total - 500.0).abs() < 0.005 && l.parsed_debit_total.abs() < 0.005, "{} {}", l.parsed_credit_total, l.parsed_debit_total);
    }

    #[test]
    fn navy_federal_spaced_signs_wrapped_rows_and_a_summary_with_account_names() {
        let p1 = "Summary of your deposit accounts\n                                           Previous                          Deposits/                          Withdrawals/                             Ending                    YTD\n                                            Balance                             Credits                                  Debits                         Balance                Dividends\nBusiness Checking\n7125242482                                $17,360.42                     $4,486.17                             $11,340.18                           $10,506.41                    $1.81\nChecking\nBusiness Checking - 7125242482\nDate     Transaction Detail                                                                                                                     Amount($)                     Balance($)\n08-01    Beginning Balance                                                                                                                                                   17,360.42\n08-01    Deposit - RTP Paid From Loot                                                                           3,950.45                       21,310.87\n\n08-01    Wire Fee                                                                                                  20.00 -                     21,290.87\n\n08-01    Withdrawal by Wire                                                                                    10,000.00 -                     11,290.87\n08-28    Paid To - App Funding Beta 9292549322 Chk 11409434\n                                                              \"                      535.72 -                    10,755.15\n08-29    Paid To - Loot Loot Chk 27397636                                                                         784.18 -                       9,970.97\n08-29    Dividend                                                                                                 535.72                        10,506.69\n08-31    Ending Balance                                                                                                                       10,506.69\nItems Paid\nDate     Item                    Amount($)     Date     Item                Amount($)\n08-29    ACH                     784.18        08-28    ACH                  535.72\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 3950.45), (Kind::Debit, 20.0), (Kind::Debit, 10000.0), (Kind::Debit, 535.72), (Kind::Debit, 784.18), (Kind::Credit, 535.72)], "{:?}", l.transactions);
        assert_eq!((l.summary.beginning_balance, l.summary.total_credits, l.summary.total_debits, l.summary.ending_balance), (Some(17360.42), Some(4486.17), Some(11340.18), Some(10506.41)), "{:?}", l.summary);
    }

    #[test]
    fn navy_federal_bundle_summary_page_items_paid_recap_savings_and_card_pages() {
        // Summary page (no rows) over a cover page; the checking heading opens the next page
        // under a "Checking" category word; "Items Paid" continues on the page that also
        // opens the savings account; the member's credit card statement follows.
        let cover = "FILED: ONEIDA COUNTY CLERK 01/07/2026\nEXHIBIT A\n";
        let summary = "Summary of your deposit accounts\n                    Previous        Deposits/     Withdrawals/      Ending\n                     Balance          Credits           Debits     Balance\nBusiness Checking\n7125242482        $17,360.42       $4,486.17       $11,875.62   $9,970.97\nMbr Business Savings\n3150735946           $124.42           $0.03            $0.00     $124.45\n";
        let checking = "Checking\n\nBusiness Checking - 7125242482\n\nDate     Transaction Detail                                            Amount($)        Balance($)\n\n08-01    Beginning Balance                                                                17,360.42\n08-01    Deposit - RTP Paid From Loot                                     3,950.45       21,310.87\n08-01    Wire Fee                                                            20.00 -      21,290.87\n08-01    Withdrawal by Wire                                              10,000.00 -      11,290.87\n08-28    Paid To - App Funding Beta 9292549322 Chk 11409434                 535.72 -      10,755.15\n08-29    Paid To - Loot Loot Chk 27397636                                   784.18 -       9,970.97\n08-29    ATM Withdrawal 08-28-25 Comm Ameri Shawnee KS                      535.72 -       9,435.25\n08-29    Dividend                                                          535.72          9,970.97\n08-31    Ending Balance                                                                    9,970.97\n\nItems Paid\n\nDate     Item                    Amount($)     Date     Item                Amount($)\n\n08-28    ACH                     535.72        08-29    seis ACH             784.18\n";
        let rest = "Items Paid                                   (Continued from previous page)\n\nDate     Item                    Amount($)     Date     Item                Amount($)\n\n08-29    ATMO                    535.72\n\n\nSavings\n\nMbr Business Savings - 3150735946\n\nDate     Transaction Detail                                            Amount($)        Balance($)\n\n08-01    Beginning Balance                                                                   124.42\n08-29    Dividend                                                             0.03           124.45\n08-31    Ending Balance                                                                      124.45\n";
        let card1 = "Navy Federal More Rewards Visa\nPAYMENT DUE   Minimum Payment Due  $250.00   Payment Due Date 09/22/2025\nCredit Limit $25,000.00\nSUMMARY OF ACCOUNT ACTIVITY\nPrevious Balance   $24,504.51   New Balance   $24,870.01\nPurchases $1,290.44  Cash Advances $0.00\nREWARD POINT SUMMARY\n";
        let card2 = "TRANSACTIONS\nPAYMENTS AND CREDITS\n08-19  74060955231099171221625 PAYMENT RECEIVED xxxx xxxx xxxx 9563     524.94\nInterest charge calculation: the average daily balance times the APR\n";
        let l = parse(&[(1, cover), (2, summary), (3, checking), (4, rest), (5, card1), (6, card2)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        let checking = &l.statements[0];
        let cents = |v: Option<f64>| v.map(|v| (v * 100.0).round() as i64);
        assert_eq!((checking.pages, cents(checking.total_credits), cents(checking.parsed_credits), cents(checking.total_debits), cents(checking.parsed_debits)), (Some((1, 4)), Some(448617), Some(448617), Some(1187562), Some(1187562)), "{:?}", l.transactions);
        assert_eq!((l.statements[1].beginning_balance, l.statements[1].parsed_credits), (Some(124.42), Some(0.03)));
        assert!(l.transactions.iter().all(|t| t.page <= 4), "card rows kept: {:?}", l.transactions);
    }

    #[test]
    fn two_months_that_begin_at_the_same_balance_split_on_the_statement_date() {
        let p1 = "                                                                                 Statement Ending 07/31/2025\n   Account Summary\n   Date           Description                                         Amount\n   07/01/2025     Beginning Balance                                    -$10.00\n                  1 Credit(s) This Period                              $100.00\n                  1 Debit(s) This Period                               $100.00\n   07/31/2025     Ending Balance                                       -$10.00\n   Electronic Credits\n   Date           Description                                                                                                  Amount\n   07/01/2025     IB Transfer Deposit                                                                                          $100.00\n   Electronic Debits\n   Date           Description                                                                                                  Amount\n   07/10/2025     IB Transfer W/D                                                                                              $100.00\n";
        let p2 = "                                                                                 Statement Ending 08/31/2025\n   Account Summary\n   Date           Description                                         Amount\n   08/01/2025     Beginning Balance                                    -$10.00\n                  1 Credit(s) This Period                              $250.00\n                  1 Debit(s) This Period                               $250.00\n   08/31/2025     Ending Balance                                       -$10.00\n   Electronic Credits\n   Date           Description                                                                                                  Amount\n   08/06/2025     IB Transfer Deposit                                                                                          $250.00\n   Electronic Debits\n   Date           Description                                                                                                  Amount\n   08/07/2025     IB Transfer W/D                                                                                              $250.00\n";
        let l = parse(&[(1, p1), (2, p2)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        assert_eq!(l.statements[0].period_end.as_deref(), Some("07/31/2025"));
        assert_eq!((l.statements[1].total_credits, l.statements[1].parsed_credits), (Some(250.0), Some(250.0)));
    }

    #[test]
    fn comma_decimals_and_stacked_serials_in_a_wide_ocr_layer() {
        assert!(is_amount_token("500,00") && parse_amount("500,00") == Some(500.0));
        assert!(!is_amount_token("500,000"));
        let p1 = "        Checks        Paid          No. Checks: 3\n        DATE                     SERIAL NO.                        AMOUNT\n        04/13                       6                              670.04\n                                    361*\n        04/01                                                    2,000.00\n        04/06                       362                          3,800.00\n        Other Withdrawals\n        POSTING DATE       DESCRIPTION                                            AMOUNT\n        04/12                       DEBIT                                        500,00\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(String, f64)> = l.transactions.iter().map(|t| (t.description.clone(), t.amount)).collect();
        assert_eq!(rows, vec![("Check 6".into(), 670.04), ("Check 361".into(), 2000.0), ("Check 362".into(), 3800.0), ("DEBIT".into(), 500.0)], "{:?}", l.transactions);
    }

    #[test]
    fn spaced_dollar_signs_and_amounts_that_lost_their_leading_digits() {
        // An older commercial statement: "$ 130813.77" with a space, running balance last, no
        // description; a figure the text layer cut short ("0000.00" for 100,000.00) is read
        // from the balance change when the change ends in the digits that survived.
        let p1 = "        BEGINNING BALANCE                 130313.77\n        DEPOSIT AMOUNT               +   100500.00\n        WITHDRAWAL AMOUNT            -   100025.00\n        ENDING BALANCE               =   130788.77\nCOMMERCIAL CHECKING                   22968                                      BALANCE SUMMARY\n                                                WITHDRAWALS       DEPOSITS          $ 130313.77\nNOV 01                                                              500.00          $ 130813.77\nNOV 02                                             25.00                            $ 130788.77\nNOV 02                                             0000.00                          $   30788.77\nNOV 02                                                           100000.00          $   130788.77\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 500.0), (Kind::Debit, 25.0), (Kind::Debit, 100000.0), (Kind::Credit, 100000.0)], "{:?}", l.transactions);
        assert!(l.transactions[2].description.contains("running balance"));
        assert_eq!(l.transactions[0].date, "11/01"); // no year printed anywhere
    }

    #[test]
    fn prose_lead_ins_name_the_section_of_retold_bullets_and_deposit_tickets_are_credits() {
        // The OCR model retold TD's deposits table as bullets after the check table; the
        // sentence before them says what they are. U.S. Bank's "Customer Deposits" table
        // lists deposit ticket numbers like a check table.
        let p1 = "Checks Paid\nDATE SERIAL NO. AMOUNT\n04/05       2250                                                                  300.00\n\nThe daily account activity includes the following electronic deposits:\n\n- 04/18: CCD DEPOSIT, TOAST DEP 0004395300JL33W: 3,176.12\n- 04/19: CCD DEPOSIT, DOORDASH, INC.: 4,055.80\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 300.0), (Kind::Credit, 3176.12), (Kind::Credit, 4055.8)], "{:?}", l.transactions);
        let p2 = "Customer Deposits\nNumber   Date   Ref Number   Amount   Number   Date   Ref Number   Amount\n   Apr 7   8356110329   37,409.66   Apr 22   8056016352   42,913.06\n   Total Customer Deposits   80,322.72\n";
        let l = parse(&[(1, p2)]);
        let rows: Vec<(Kind, f64, String)> = l.transactions.iter().map(|t| (t.kind, t.amount, t.description.clone())).collect();
        assert_eq!(rows, vec![(Kind::Credit, 37409.66, "Deposit 8356110329".into()), (Kind::Credit, 42913.06, "Deposit 8056016352".into())], "{:?}", l.transactions);
    }

    #[test]
    fn court_forms_check_registers_and_printouts_around_a_statement_are_left_out() {
        let form = "Debtor Name   JAR 259 FOOD CORP\n2. Summary of Cash Activity for All Accounts\n19. Total opening balance of all accounts\n449,280.15\n20. Total cash receipts   59,499.51\nOfficial Form 425C   Monthly Operating Report for Small Business Under Chapter 11   page 2\n";
        let register = "105102   10/19/2022 U S DEPARTMENT OF HOMELAND SECURITY   $1,225.00\n105104   10/19/2022 VERGES ROME ARCHITECTS   $103,037.21\n105105   10/19/2022   $156.00\n105106   10/19/2022 WRIGHT NATIONAL FLOOD INS CO   $14,818.00\n105107   10/25/2022 2878 Building Association, LLC   $650.00\n";
        let statement = "ACCOUNT SUMMARY\nBeginning Balance   1,000.00\nElectronic Deposits   500.00\nElectronic Payments   200.00\nEnding Balance   1,300.00\nDAILY ACCOUNT ACTIVITY\nElectronic Deposits\nPOSTING DATE   DESCRIPTION   AMOUNT\n02/01   CCD DEPOSIT, HRTLAND PMT SYS   500.00\nElectronic Payments\nPOSTING DATE   DESCRIPTION   AMOUNT\n02/03   CCD DEBIT, VENDOR   200.00\n";
        let printout = "Business Adv Fundamentals - 1101: Account Activity\nBalance Summary:-$10,386.41 (available as of today 03/03/2022)   Print\nAll Transactions\nDate   Description   Status   Amount Available Balance\n03/02/2022   Wynwood Capital DES:JAR 259 FO   C   -500.00   3,936.51\n";
        let l = parse(&[(1, form), (2, register), (3, statement), (4, printout)]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        let st = &l.statements[0];
        assert_eq!((st.total_credits, st.parsed_credits, st.total_debits, st.parsed_debits), (Some(500.0), Some(500.0), Some(200.0), Some(200.0)), "{:?}", st);
        assert_eq!((l.statements[1].total_credits, l.statements[1].total_debits), (None, None), "{:?}", l.statements[1]);
    }

    #[test]
    fn mercury_day_groups_and_ocr_field_lists_read_as_rows() {
        // Mercury: one date per day, the day's other rows undated, debits with a leading
        // minus (an em dash in the text layer), amounts with spaces, an end-of-day balance.
        let p1 = "  Account activity overview\n  Beginning balance   $26,849.72\n  Total withdrawals   \u{2014}$43,779.85\n  Total deposits   $141,744.37\n  Dat e    De s cript ion                                      T rx T ype                             A mou n t   En d of Day Balan ce\n\n  Feb 01   TabaPay                                                  ACH In                    $123, 614. 97\n\n           TabaPay                                                  ACH In                      $18, 129. 40\n           LIBRE EXPENSE                                            .::t Transfer Out             \u{2014}$30, 000. 00\n           Richard Moore                                            <t1Wire Payment               \u{2014}$13, 779. 85   $124, 814. 24\n";
        let l = parse(&[(1, p1)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 123614.97), (Kind::Credit, 18129.4), (Kind::Debit, 30000.0), (Kind::Debit, 13779.85)], "{:?}", l.transactions);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(141744.37), Some(43779.85)));
        // The OCR model retelling a one-row table as a field list, with and without colons.
        let p2 = "**Trustee Checking**\n\n- **Date:** 11/04\n- **Description:** Electronic transfer\n- **Credits:** $25,000.00\n- **Debits:** $0.00\n- **Ending Balance:** $25,000.00\n";
        let p3 = "Trustee Checking 1211\nDate 10/03\nDescription Bank Service Fee\nCredits $513.01\nDebits\nCHECKS CLEARED\nCheck # Amount Date\n196 147.00 10/26\n";
        let l = parse(&[(1, p2)]);
        assert_eq!(l.transactions.iter().map(|t| (t.kind, t.amount)).collect::<Vec<_>>(), vec![(Kind::Credit, 25000.0)], "{:?}", l.transactions);
        let l = parse(&[(1, p3)]);
        assert_eq!(l.transactions.iter().map(|t| (t.kind, t.amount)).collect::<Vec<_>>(), vec![(Kind::Credit, 513.01), (Kind::Debit, 147.0)], "{:?}", l.transactions);
    }

    #[test]
    fn a_second_summary_page_is_another_statement_and_a_printout_has_no_totals() {
        let p1 = "**Statement Summary**\nDeposit Accounts Beginning Balance Credits Debits Ending Balance\nTrustee Checking $0.00 $500.00 $200.00 $300.00\n**Trustee Checking**\nDate        Description                                  Credits              Debits\n11/03       Wire Transfer Credit                        $500.00\n11/22       Wire Transfer Debit                         $200.00\n";
        let p2 = "**Statement Summary**\nDeposit Accounts Beginning Balance Credits Debits Ending Balance\nTrustee Checking $0.00 $50.00 $0.00 $50.00\n**Trustee Checking**\nDate        Description                                  Credits              Debits\n11/04       Electronic transfer                          $50.00\n";
        let p3 = "Business Adv Fundamentals - 1101: Account Activity\nBalance Summary:-$10,386.41 (available as of today 03/03/2022)   Print\nAll Transactions\nDate   Description   Status   Amount Available Balance\n03/02/2022   Wynwood Capital DES:JAR 259 FO   C   -500.00   3,936.51\n03/01/2022   Kingdom Kapital DES:JAR 259 FO   C   -500.00   4,436.51\n";
        let l = parse(&[(1, p1), (2, p2), (3, p3)]);
        assert_eq!(l.statements.len(), 3, "{:?}", l.statements);
        assert_eq!((l.statements[0].total_credits, l.statements[0].parsed_credits, l.statements[0].total_debits, l.statements[0].parsed_debits), (Some(500.0), Some(500.0), Some(200.0), Some(200.0)));
        assert_eq!((l.statements[1].total_credits, l.statements[1].parsed_credits), (Some(50.0), Some(50.0)));
        assert_eq!((l.statements[2].total_credits, l.statements[2].total_debits), (None, None), "{:?}", l.statements[2]);
    }

    // BankNorth (court scan, flat OCR): "PREV STATEMENT BALANCE (12/31/23)", the checks
    // listed two rows to a line with the amount before the description, the box over the
    // first row clipping the right column's month ("/09"), a starred out-of-sequence check,
    // then "AUTOMATIC TRANSACTIONS", a detailed re-listing of the same debits, and a page
    // of receipt and check pictures with captions.
    const BANKNORTH_P1: &str = "BANKNORTH banknorth.com\nDRAIN SERVICES INC\nAS OF: 01/31/24 PAGE 1\nYOUR ACCOUNT TYPE IS: REGULAR ACCOUNT\nCHECKING SUMMARY ............ ACCOUNT 9319 PIECES 4 BALANCE\nPREV STATEMENT BALANCE (12/31/23) 6,216.37\n2 DEPOSITS / CREDITS ...... 8,000.00\nINTEREST PAID ............\n4 CHECKS / DEBITS ........ 1,114.04\nSTATEMENT BALANCE (01/31/24) 13,102.33\nAVERAGE COLLECTED BALANCE ......... 9,263.47\nDEPOSITS / CREDITS .......... ACCOUNT 9319\n01/02/24 DIRECT DEPOSIT/ACH 7,300.00\n01/10/24 MOBILE CHECK DEPOSIT 700.00\nCHECKS / DEBITS ............. ACCOUNT 02229319\n01/02 37.78 POINT OF S /09 10.26 POINT OF SAL\n01/03* 1057 1000.00 CUSTOMER CHE 01/02 66.00 POINT OF SAL\nDAILY BALANCES .............. ACCOUNT 9319\n12/31 6216.37 01/02 13412.59 01/03 12412.59 01/09 12402.33\n01/10 13102.33\n-------- AUTOMATIC TRANSACTIONS --------------- - DEBITS CREDITS\n01/02/24 Intuit TRANSFER 9002000202 7300.00\n01/02/24 PS4455 FIREHOUSE SUBS 1101 QSR FARGO ND 37.78\n01/02/24 PS2546 USPS PO 3791680913 WEST FARGO ND 66.00\n01/09/24 PSF380 MENARDS MOORHEAD MN MOORHEAD MN 10.26\n";
    const BANKNORTH_P2: &str = "Account 9319 Page 2\nRecord Of Deposit\nInstitution: BankNorth\nDate: 1/10/2024 7:50:54 AMPT\nTotal Transaction Amount: $700.00\n1/10/2024 700.00\nTRAN DATE: 1/02/2024 BankNorth\nDDA DEBIT\nPREPARED BY: Melissa Liebenow\n1057 1/03/2024 Paid 1000.00\n";

    #[test]
    fn a_glued_page_header_is_no_row_and_the_balance_equation_is_checked() {
        // Wells' squeezed text layer: "May31,2021 • Page3of4" heads the page above the check
        // summary; the paired check row below must only be the two checks already listed.
        let p1 = "Transaction history\n       Date                Number Description                                        Credits                Debits            balance\n       5/24                   1187 Check                                                                      587.16\n       5/25                   1186 Check                                                                      665.08              78,086.68\n       5/28                        Online Transfer to Dprt Funds LLC Business Checking                          500.00               1,098.24\n       Ending balance on 5/31                                                                                                     1,098.24\n       Totals                                                                        $0.00          $1,752.24\n";
        let p2 = "May31,2021 \u{2022} Page3of4\n\nSummary of checks written            (checks listed are a/so displayed in the preceding Transaction history)\n\n        Number             Date                 Amount          Number             Date                 Amount\n        1186               5125                  665.08         1187               5124                  587.16\n";
        let l = parse(&[(1, p1), (2, p2)]);
        assert_eq!(l.transactions.len(), 3, "{:?}", l.transactions);
        assert_eq!((l.summary.total_debits, l.parsed_debit_total), (Some(1752.24), 1752.24));
        assert_eq!(footer_numbers("May31,2021 \u{2022} Page3of4"), Some((3, 4)));
        // The equation needs both printed balances: none here, so it is not judged.
        assert_eq!(l.summary.balance_check, None);
        let with_beginning = format!("Beginning balance on 5/1      2,850.48\n{p1}");
        let l = parse(&[(1, with_beginning.as_str()), (2, p2)]);
        assert_eq!(l.summary.balance_check, Some(true), "{:?}", l.summary);
        let off = with_beginning.replace("500.00", "600.00");
        let l = parse(&[(1, off.as_str()), (2, p2)]);
        assert_eq!(l.summary.balance_check, Some(false), "{:?}", l.summary);
    }

    #[test]
    fn community_bank_scan_repairs_and_the_daily_balances_settle_a_dropped_digit() {
        // Tesseract on a court copy: a letter in the cents ("7.1i1-"), a digit read into the
        // month ("110/06"), a letter in the day ("10/a7", 10/07 by the surviving 7), a check
        // row whose date is gone ("1oys1s 2655 2,014.24"), and "41,500.00" for a 1,500.00
        // check that the day's printed balance gives away.
        let p1 = "Previous Balance 1,000.00\n1 Deposits/Credits 5,000.00\n9 Checks/Debits 5,514.24\nCurrent Balance 485.76\nActivity in Date Order\nDate Description Amount\n10/06 \"My Deposit\" Deposit 5,000.00 CR\n10/06 POS DEB 1445 10/02/21 34911601 7.1i1-\n*PPD* TR#HO91000016872376\n110/06 NATIONWIDE EDI PYMNTS 169.25-\n*PPD* TR#HO91000016872376\n10/07 ENTERGY BANK DRAFT 158.30-\n10/a7 ENTERGY BANK DRAFT 857.19-\n10/08 INTUIT FEE 8.15-\n";
        let p2 = "CHECKS IN CHECK NO. ORDER\nDate Check No Amount Date Check No Amount\n10/06 2654 300.00 10/08 2680 41,500.00\n1oys1s 2655 2,014.24 10/08 . 2681* 500.00\n* Denotes missing check numbers\nDaily Balance Information\nDate Balance Date Balance\n10/06 3,509.40 10/08 485.76\n10/07 2,493.91\n";
        let l = parse(&[(1, p1), (2, p2)]);
        let rows: Vec<(&str, Kind, f64, &str)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, t.amount, t.description.as_str())).collect();
        assert_eq!(rows, vec![
            ("2021-10-06", Kind::Credit, 5000.0, "\"My Deposit\" Deposit"), ("2021-10-06", Kind::Debit, 7.11, "POS DEB 1445 10/02/21 34911601 *PPD* TR#HO91000016872376"),
            ("2021-10-06", Kind::Debit, 169.25, "NATIONWIDE EDI PYMNTS *PPD* TR#HO91000016872376"), ("2021-10-07", Kind::Debit, 158.3, "ENTERGY BANK DRAFT"), ("2021-10-07", Kind::Debit, 857.19, "ENTERGY BANK DRAFT"),
            ("2021-10-08", Kind::Debit, 8.15, "INTUIT FEE"), ("2021-10-06", Kind::Debit, 300.0, "Check 2654"), ("2021-10-06", Kind::Debit, 2014.24, "Check 2655"), ("2021-10-08", Kind::Debit, 1500.0, "Check 2680"), ("2021-10-08", Kind::Debit, 500.0, "Check 2681"),
        ], "{:?}", l.transactions);
        assert_eq!((l.summary.total_debits, (l.parsed_debit_total * 100.0).round() as i64, l.summary.balance_check), (Some(5514.24), 551424, Some(true)));
        assert_eq!(footer_numbers("July31,2019 \u{2022} Page1 of6"), Some((1, 6)));
        // "Dale" / "Data" head a date column; "Check   8 368.00" is one figure.
        let wells = "Transaction history\n       Dale                Number Description                                        Credits                Debits            balance\n       7/17                  31156 Check                                                                   8 368.00         89,333.69\n       7/18                        Edeposit IN Branch/Store 07/18/19                        35,341.46                         124,675.15\n";
        let l = parse(&[(1, wells)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 8368.0), (Kind::Credit, 35341.46)], "{:?}", l.transactions);
    }

    #[test]
    fn section_totals_and_second_readings_settle_misread_figures() {
        // Truist scan: the one check under "Total checks = $3,130.00" read as 130.00 with
        // its date as "oo/o9"; the section total puts the 3 back.
        let truist = "Account summary\nYour previous balance as of 08/31/2022 $736.25\nChecks - 3,130.00\nOther withdrawals, debits and service charges - 500.00\nDeposits, credits and interest + 4,000.00\nYour new balance as of 09/30/2022 = $1,106.25\nChecks\nDATE CHECK # AMOUNT (S)\noo/o9 7238 130.00\nTotal checks = $ 3,130.00\nOther withdrawals, debits and service charges\nDATE DESCRIPTION AMOUNT(S)\n09/09 INTERNET PAYMENT ATT 500.00\nTotal other withdrawals, debits and service charges = $500.00\nDeposits, credits and interest\nDATE DESCRIPTION AMOUNT($)\n09/02 REMOTE DEPOSIT 4,000.00\nTotal deposits, credits and interest = $4,000.00\n";
        let l = parse(&[(1, truist)]);
        let rows: Vec<(&str, Kind, f64, &str)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, t.amount, t.description.as_str())).collect();
        assert_eq!(rows, vec![("2022-09-09", Kind::Debit, 3130.0, "Check 7238"), ("2022-09-09", Kind::Debit, 500.0, "INTERNET PAYMENT ATT"), ("2022-09-02", Kind::Credit, 4000.0, "REMOTE DEPOSIT")], "{:?}", l.transactions);
        assert_eq!((l.summary.total_debits, l.summary.balance_check), (Some(3630.0), Some(true)));
        // Chase: the checks-paid table reads check 60544 as 984.44, the caption under its
        // image as $984.41; the printed total picks the caption's figure.
        let table = "CHECKING SUMMARY\nBeginning Balance $1,000.00\nDeposits and Additions 0.00\nChecks Paid -1,184.41\nEnding Balance -$184.41\nCHECKS PAID\nCHECK NO. DESCRIPTION DATE PAID AMOUNT\n1004 ^ 02/02 $200.00\n60544 ^ 02/02 984.44\nTotal Checks Paid $1,184.41\n";
        let images = "001170716100 FEB 02 #0000060544 $984.41 002180191498 FEB 02 #0000001004 $200.00\n";
        let l = parse(&[(1, table), (2, images)]);
        let rows: Vec<(Kind, f64, &str)> = l.transactions.iter().map(|t| (t.kind, t.amount, t.description.as_str())).collect();
        assert_eq!(rows, vec![(Kind::Debit, 200.0, "Check 1004"), (Kind::Debit, 984.41, "Check 60544")], "{:?}", l.transactions);
    }

    #[test]
    fn lifted_amounts_scribbled_check_rows_and_unsigned_summary_parts() {
        // UMB's text layer lifts a row's amount onto the line above, behind a margin mark.
        let umb = "Transaction Detail\n       Date     Description                                                             Deposits       Withdrawals\nN                                                                                                                6.98\n(0     Jun 03   QT 168        KANSAS CIT MO 05/31 0486\nw\nN      Jun 03   QT 168        KANSAS CIT MO 05/31 0486                                                          18.52\n       Jun 04   DEPOSIT REF 35243543                                                     500.00\n";
        let l = parse(&[(1, umb)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 6.98), (Kind::Debit, 18.52), (Kind::Credit, 500.0)], "{:?}", l.transactions);
        // Chase check rows with a rule's tail read after the amount; PNC's three-column check
        // table with the slash dropped from a date and a bracket after the reference.
        let chase = "CHECKS PAID\nCHECK NO. DESCRIPTION DATE PAID AMOUNT\n225 4\u{201c} 02/22 139,667.00 ES\n232 *A 02/22 3,095.00 =\u{2014}\u{2014}s3\n234 4 02/18 5,143.94 \u{2014}rr\n";
        let l = parse(&[(1, chase)]);
        let rows: Vec<(&str, f64)> = l.transactions.iter().map(|t| (t.description.as_str(), t.amount)).collect();
        assert_eq!(rows, vec![("Check 225", 139667.0), ("Check 232", 3095.0), ("Check 234", 5143.94)], "{:?}", l.transactions);
        let pnc = "Checks and Substitute Checks * Gap in check sequence\nDate Check E Date Check Reference Date Check Reference\nposted number Amount number} posted number Amount number] posted number Amount number\n1105 1005 * 800.00 077074442] 11/07 1015 815.29 071793429] 11/08 1020 1,120.55 073059155\n11/05 1006 1,100.00 077074441] 11/09 1016 571.97 074529405] 11/14 1021 1,219.00 071142914\n1108 1012 595.58 073060267] 11/06 1018 547.54 070703065] 11/20 1023 8,000.00 076845328\n11/07 1013 681.16 071761728] 11/13 1019 1,671.26 076411979] 11/07 7001 * 360.00 071805605\n";
        let l = parse(&[(1, pnc)]);
        let rows: Vec<(&str, &str, f64)> = l.transactions.iter().map(|t| (t.date.as_str(), t.description.as_str(), t.amount)).collect();
        assert_eq!(rows, vec![("11/05", "Check 1005", 800.0), ("11/07", "Check 1015", 815.29), ("11/08", "Check 1020", 1120.55), ("11/05", "Check 1006", 1100.0), ("11/09", "Check 1016", 571.97), ("11/14", "Check 1021", 1219.0), ("11/08", "Check 1012", 595.58), ("11/06", "Check 1018", 547.54), ("11/20", "Check 1023", 8000.0), ("11/07", "Check 1013", 681.16), ("11/13", "Check 1019", 1671.26), ("11/07", "Check 7001", 360.0)], "{:?}", l.transactions);
        // KeyBank: the checks category lost its minus and the fees line its label's start;
        // the balance equation puts both into the debits. A smear is not a row.
        let key = "Account Summary\nBeginning Balance on January 14, 2022 $1,568.45\nDeposits 1,500.00\n: Withdrawals -$240.83\n7 Checks $438.24 |g 5 de NO AR a a a a TO\n: Fees and Charges -$3.00\nEnding Balance on February 14, 2022 $2,386.38\nWithdrawals\nDate Description Amount\n01/18 DIRECT WITHDRAWAL, CBN PLGXPRESS DONATION $20.00\n02/11 DIRECT WITHDRAWAL, COMCAST CABLE $220.83\na OOOoreeOmOOqETH OAR A 580.00\nTotal Withdrawals -$240.83\n";
        let l = parse(&[(1, key)]);
        assert_eq!((l.summary.total_debits, l.summary.total_credits), (Some(682.07), Some(1500.0)), "{:?}", l.summary);
        assert_eq!(l.transactions.iter().map(|t| t.amount).collect::<Vec<_>>(), vec![20.0, 220.83], "{:?}", l.transactions);
        // Citi: "Total Debits/Credits" under the columns names both totals; a fee schedule's
        // "CHECKS, DEP ITEMS/TICKETS, ACH  25  .4500  11.25" is not a checks figure.
        let citi = "SERVICE CHARGE SUMMARY FROM FEBRUARY 1, 2024 THRU FEBRUARY 29, 2024\nType of Charge                                                 No./Units            Price/Unit             Amount\n   CHECKS, DEP ITEMS/TICKETS, ACH                                    25                 .4500                 11.25\nCHECKING ACTIVITY\n      8387                                                             Beginning Balance:                  $1,994.28\n                                                                       Ending Balance:                       $880.03\nDate Description                                                           Debits        Credits             Balance\n03/01 ELECTRONIC CREDIT                                                                    38.98             2,033.26\n03/08 SERVICE CHARGE                                                       22.00                             2,011.26\n        Total Debits/Credits                                               22.00         38.98\n";
        let l = parse(&[(1, citi)]);
        assert_eq!((l.summary.total_debits, l.summary.total_credits, l.summary.balance_check), (Some(22.0), Some(38.98), Some(false)), "{:?}", l.summary);
    }

    #[test]
    fn relay_exports_cut_headings_and_wrapped_wire_lines() {
        // Relay's app export: the description before the date, "Settled", the signed amount.
        let relay = "Opening Balance Closing Balance Deposits Withdrawals\n$150.00 $0.00 +$0.00 -$150.00\nName Date Status Amount Balance\ng BUSINESS CHECKING I (4875) May 18, 2026 Settled -$150.00 \u{2014} $0.00\nBUSINESS CHECKING I (4875) May 19, 2026 Pending -$20.00 \u{2014} $0.00\n";
        let l = parse(&[(1, relay)]);
        let rows: Vec<(&str, Kind, f64, &str)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, t.amount, t.description.as_str())).collect();
        assert_eq!(rows, vec![("2026-05-18", Kind::Debit, 150.0, "BUSINESS CHECKING I (4875)")], "{:?}", l.transactions);
        // Synovus: "Balance Summa" (the scan cut the heading) still opens the daily table, and
        // a wire's wrapped detail line "347 '43ELEKTA INC DEPOSIT" opens no deposits section.
        let synovus = "Other Debits\nDate Transaction Type Description Amount\n11-23 Phn/Fax Dom Out Wire =ELEKTA INC DEPOSIT Y ACCOUNTINVOICES 25,698.14\n347 '43ELEKTA INC DEPOSIT\nORY ACCOUNT\n11-23 Service Charge PHN/FAX DOM OUT WI 100.00\n11-28 Preauthorized Wd SPECTRUM SPECTRUM 599.00\nBalance Summa\nDate Amount Date Amount\n11-22 30,000.00 11-28 4,290.61\n11-23 4,201.86\n";
        let l = parse(&[(1, synovus)]);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Debit), "{:?}", l.transactions);
        assert_eq!(l.transactions.len(), 3);
        assert_eq!(l.daily_balances.len(), 3, "{:?}", l.daily_balances);
    }

    #[test]
    fn a_lost_interest_row_and_a_slash_read_as_seven() {
        // Wells scan: the interest row is a smear, but the summary prints "Interest paid this
        // statement $0.18" and the credits fall short by exactly that; "579" heads a row
        // with its running balance beside real May dates, so it is 5/9.
        let text = "Activity summary\nBeginning balance on 5/1 $5,909.36\nDeposits/Credits 5,909.54\nWithdrawals/Debits - 5,919.36\nEnding balance on 5/31 $5,899.54\nInterest paid this statement $0.18\nTransaction history\nDate Number Description Credits Debits balance\n579 Legal Order Debit - Contact Isaac H. Greenfield, Esq. 5,909.36 0.00\n5/18 Legal Order Reversal - Contact Isaac H. Greenfield, Esq. 5,909.36 5,909.36\n5/20 Legal Order Fee 10.00 5,899.36\nEnding balance on 5/31 5,899.54\nTotals $5,909.54 $5,919.36\n";
        let l = parse(&[(1, text)]);
        let rows: Vec<(&str, Kind, f64)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![("05/09", Kind::Debit, 5909.36), ("05/18", Kind::Credit, 5909.36), ("05/20", Kind::Debit, 10.0), ("05/31", Kind::Credit, 0.18)], "{:?}", l.transactions);
        assert_eq!(l.summary.balance_check, Some(true));
    }

    #[test]
    fn huntington_suntrust_and_flushing_summaries() {
        // Huntington: the period beside the summary is no row; "Credits (+)" and "Debits (-)"
        // name the totals; the signed section titles are never a row's continuation.
        let hunt = "Huntington Analyzed Checking                                                         Account:------ 1079\nStatement Activity From:                          Beginning Balance                                $7,500.00\n03/01/23 to 03/31/23                              Credits (+)                                       1,000.00\n                                                     Regular Deposits                                1,000.00\nDays in Statement Period                 31       Debits (-)                                        2,124.32\n                                                     Electronic Withdrawals                          1,510.00\nAverage Ledger Balance*              6,074.40        Service Charges                                   614.32\nAverage Collected Balance*           6,009.88     Ending Balance                                   $6,375.68\nDeposits (+)                                                                                                         Account:-------1079\nDate              Amount           Serial #               Type\n03/28               1,000.00                              Remote\nOther Debits (-)                                                                                                     Account:-------1079\nDate                Amount          Description\n03/07               1,510.00        COMPLI MASTER CORP COLL 230307 2107 202302/202303 FUNDING\n03/15                 614.32        PRIOR MONTH'S SERVICE CHARGES\nBalance Activity                                                                                                     Account:-------1079\n";
        let l = parse(&[(1, hunt)]);
        let rows: Vec<(Kind, f64, &str)> = l.transactions.iter().map(|t| (t.kind, t.amount, t.description.as_str())).collect();
        assert_eq!(rows, vec![(Kind::Credit, 1000.0, "Remote"), (Kind::Debit, 1510.0, "COMPLI MASTER CORP COLL 230307 2107 202302/202303 FUNDING"), (Kind::Debit, 614.32, "PRIOR MONTH'S SERVICE CHARGES")], "{:?}", l.transactions);
        assert_eq!((l.summary.total_credits, l.summary.total_debits, l.summary.balance_check), (Some(1000.0), Some(2124.32), Some(true)), "{:?}", l.summary);
        // SunTrust: the disclaimer's "pending transactions ... available balance" is not a
        // printout; the margin label "Credits" before the first row is stripped.
        let sun = "Get credit for your payment today.\nAccount Account Type Account Number Statement Period\nSummary\nPRIMARY BUSINESS CHECKING 7 6372 04/10/2017 - 04/30/2017\nDescription Amount Description Amount\nBeginning Balance $.00 Average Balance $25,397.14\nDeposits/Credits $36,617.24 Average Collected Balance $25,382.94\nChecks $.00 Number of Days in Statement Period 21\nWithdrawals/Debits $83.17\nEnding Balance $36,534.07\nDeposits/ Date Amount Serial # Description | Date Amount Serial # Description\nCredits 04/18 284.88 DEPOSIT\n04/11 23,282.36 INCOMING FEDWIRE CR TRN #008401\n04/28 13,050.00 INCOMING FEDWIRE CR TRN #009930\nDeposits/Credits: 3 Total Items Deposited: 1\nWithdrawals/ Date Amount Serial # Description\nDebits Paid\n04/11 15.00 INCOMING FEDWIRE TRANSFER FEE TRN #008401\n04/27 53.17 ELECTRONIC/ACH DEBIT\nCITI CARD ONLINE PAYMENT 112312813917308\n04/28 15.00 INCOMING FEDWIRE TRANSFER FEE TRN #009930\nWithdrawals/Debits: 3\nBalance Date Balance Collected Date Balance Collected\n";
        let l = parse(&[(1, sun)]);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(36617.24), Some(83.17)), "{:?}", l.summary);
        assert_eq!(((l.parsed_credit_total * 100.0).round() as i64, (l.parsed_debit_total * 100.0).round() as i64), (3661724, 8317), "{:?}", l.transactions);
        // Flushing Bank: six labels over two lines, a zero without its point in the figures,
        // the summary printed at the end of a statement whose footers keep its pages together,
        // and "Image Statement" picture pages.
        let f1 = "GREGORY M LASPINA CONSERVATOR PAGE: 1 OF 4\nSTATEMENT DATE: 03/31/23\n";
        let f2 = "PAGE: 2 OF 4\nAccount Detail\nDate Description Credits Debits Balance\n02/28 Balance Forward 634,907.25\n03/01 Check Number 1319 16.05- 634,891.20\n03/09 Deposit 34,662.00 669,553.20\n03/17 Deposit 25,362.00 694,915.20\n03/17 Check Number 1539 15.00- 694,900.20\n03/20 Check Number 1325 2,744.75- 692,155.45\n03/22 Check Number 1327 217.75- 691,937.70\n";
        let f3 = "Account Flushing Bank -- Image Statement 03/31/2023 Page 3 of 4\nCheck 1325 Date 03/20 Amount $2,744.75 Check 1327 Date 03/22 Amount $217.75\n";
        let f4 = "PAGE: 4 OF 4\nAccount Summary\nPrevious Statement Date: 02/28/23\nBeginning Interest Service Ending\nBalance + Deposits + Paid - Withdrawals ~ Charge = Balance\n634,907.25 60,024.00 00 2,993.55 .00 691,937.70\nStatement from 03/01/23 Thru 03/31/23\n";
        let l = parse(&[(1, f1), (2, f2), (3, f3), (4, f4)]);
        assert!(l.statements.is_empty(), "one statement, not {:?}", l.statements.iter().map(|s| s.pages).collect::<Vec<_>>());
        assert_eq!((l.summary.beginning_balance, l.summary.total_credits, l.summary.total_debits, l.summary.ending_balance), (Some(634907.25), Some(60024.0), Some(2993.55), Some(691937.7)), "{:?}", l.summary);
        assert_eq!(((l.parsed_credit_total * 100.0).round() as i64, (l.parsed_debit_total * 100.0).round() as i64), (6002400, 299355), "{:?}", l.transactions);
        assert_eq!(footer_numbers("REDE Page 224 of253"), None);
    }

    #[test]
    fn purchases_heading_refunds_stray_marks_in_columns_and_check_captions() {
        // Union Bank: the seven-word "Purchases ATM card and Debit card purchases" heading
        // opens a debit section; the signed summary parts name the totals.
        let union = "Banking By Design Summary                                  Account Number:\nDays in statement period : 30\n                        Balance on 2/28                      $                             17.00\n                        Additions                                                          20.00\n                        Subtractions                                                     -134.82\n                                             Purchases                         -66.82\n                                      Other Withdrawals                        -68.00\n                        Balance on 3/29                 $                                 -97.82\nAdditions\n                                Date        Description/Location                                                   Reference           Amount\n                                3/2         ATM DEPOSIT                                                            79336163    $         20.00\nPurchases ATM card and Debit card purchases\n                                Date        Description/Location                                       Reference                       Amount\n                                2/28        MARKET WOR 2067379149 WA 2067379149 WA                     71672646                $         3.10\n                                2/28        KFC C19100 SAN DIEGO CA SAN DIEGO CA                       71672647                             8.93\n                                3/5         NATIONAL C IT CA NATIONAL CIT CA                           70758030                            54.79\n                                Total                                                                                          $           66.82\nOther Withdrawals including fees and adjustments\n                                Date        Description/Location                                       Reference                       Amount\n                                3/6         TOTAL OVERDRAFT ITEM PAID FEES                             99520095                $        33.00\n                                3/12        CONTINUED OVERDRAFT FEE                                                                      6.00\n                                3/13        CONTINUED OVERDRAFT FEE                                                                     29.00\n";
        let l = parse(&[(1, union)]);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(20.0), Some(134.82)), "{:?}", l.summary);
        assert_eq!(((l.parsed_credit_total * 100.0).round() as i64, (l.parsed_debit_total * 100.0).round() as i64), (2000, 13482), "{:?}", l.transactions);
        // Signature: "DEBIT CARD REFUND" under a deposits section inherited from the page
        // before is a credit, the word "refund" beating "debit".
        let sig1 = "Summary\n Previous Balance as of July      01, 2022                                                       82.96\n       3 Credits                                                                                200.29\n       2 Debits                                                                                 150.00\n Ending Balance as of   July      31, 2022                                                      133.25\nDeposits and Other Credits\n Jul 01 ACH DEPOSIT             ck/ref no.     1567847                                             103.46\n         SHOPIFY             TRANSFER        ST-J4E2V4S5A5H8\n Jul 05 ACH DEPOSIT             ck/ref no.     1887154                                              81.00\n";
        let sig2 = "Statement Period\n Jul 14   DEBIT CARD REFUND                                                                                  15.83\n          ON 07/14 AT GRAINGER                       877 2022594    IL\nWithdrawals and Other Debits\n Jul 05   DEBIT CARD PURCHASE                                                                                 50.00\n Jul 06   ATM WITHDRAWAL CASH WITHDRAWAL                                                                     100.00\n";
        let l = parse(&[(1, sig1), (2, sig2)]);
        assert_eq!(((l.parsed_credit_total * 100.0).round() as i64, (l.parsed_debit_total * 100.0).round() as i64), (20029, 15000), "{:?}", l.transactions);
        // Pinnacle: a stray mark between two amounts is blanked in place, so "$4.99 D
        // $32.00" keeps $32.00 under the Debits column.
        let pinn = "            Check/                                                     Deposits/   Withdrawals/   End of Day\nDate *      Serial #     Description                                     Credits         Debits     Balance\n7/16                     INSUFFICIENT FUNDS-RETURNED ITEM $500.00                       $32.00\n                         DEBIT FOR GIACT SYSTEMS GIACT FEES CO R\n7/16                     INSUFFICIENT FUNDS-RETURNED ITEM $4.99 D                       $32.00\n                         EBIT FOR GO DADDY WEB ORDER CO REF- 1385\n7/19                     CREDIT FOR PAYSAFE IPAYMENT 202106_RP3 CO     $4,434.92\n";
        let l = parse(&[(1, pinn)]);
        let rows: Vec<(Kind, i64)> = l.transactions.iter().map(|t| (t.kind, (t.amount * 100.0).round() as i64)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 3200), (Kind::Debit, 3200), (Kind::Credit, 443492)], "{:?}", l.transactions);
        // Chase: the caption under a check's image, "#0000001029", repeats the table's
        // "4029" (one digit misread) on the same day for the same amount; a caption is
        // read fuzzily only against a table row not repeated yet, and never two table rows
        // a serial apart.
        let chase1 = "CHECKS PAID\nCHECK NO. DESCRIPTION PAID AMOUNT\n1028 4 03/02 20.22\n4029 A 03/04 506.00\n60556 * 03/09 984.41\n";
        let chase2 = "CHECKS PAID (continued)\n60559 A 03/09 984.41\nELECTRONIC WITHDRAWALS\n03/06 Orig CO Name:Nicor Gas Orig ID:8200406241 699.62\n";
        let chase3 = "See both front and back images of cleared checks at Chase.com.\n003580515351 MAR 02 #0000001028 $20.22 109180720961 MAR 04 #0000001029 $506.00\n008080048064 MAR 09 #0000060556 $984.41 009370981191 MAR 09 #0000060559 $984.41\n";
        let l = parse(&[(1, chase1), (2, chase2), (3, chase3)]);
        let mut rows: Vec<(String, i64)> = l.transactions.iter().map(|t| (t.description.clone(), (t.amount * 100.0).round() as i64)).collect();
        rows.sort();
        assert_eq!(rows, vec![("Check 1028".to_string(), 2022), ("Check 4029".to_string(), 50600), ("Check 60556".to_string(), 98441), ("Check 60559".to_string(), 98441), ("Orig CO Name:Nicor Gas Orig ID:8200406241".to_string(), 69962)], "{:?}", l.transactions);
        // Mercantile: a one-page statement's "Page: 1 of 1" opens its own statement behind
        // another statement's pages.
        let merc1 = "Mercantile Page: 1 of 4\nStatement Date: 09/30/2020\nPeriod: 08/31/20 to 09/30/20\nCREDITS\nDate Description Amount\n09/04 Deposit 100.00\nDEBITS\nDate Description Amount\n09/08 Check 40.00\nDAILY BALANCE\nDate Balance\n09/04 100.00\n09/08 60.00\n";
        let merc2 = "Mercantile Page: 1 of 1\nStatement Date: 09/30/2020\nPeriod: 08/31/20 to 09/30/20\nCREDITS\nDate Description Amount\n09/04 Transfer From Coml Analysis Ck Account 27.95\nDEBITS\nDate Description Amount\n09/04 Mthchgs Worldpay Merch Bankcard 27.95\nDAILY BALANCE\nDate Balance\n09/04 0.00\n";
        let l = parse(&[(1, merc1), (2, merc2)]);
        assert_eq!(l.statements.iter().map(|s| s.pages).collect::<Vec<_>>(), vec![Some((1, 1)), Some((2, 2))], "{:?}", l.statements);
    }

    #[test]
    fn check_pairs_newest_first_listings_and_schedules() {
        // TD: a two-column check table in a scan, one figure without its point, one split
        // by a space, one with a glyph in front; the columns are cut apart afterwards.
        let td = "Checks Paid\nDATE SERIAL NO. AMOUNT DATE SERIAL NO. AMOUNT\n04/16 2281 22331 04/15 11071 1;199:05\n04/15 11082 467 96 04/29 11102 £28.48\n04/16 2282 4,308.69 04/14 11072 1,499.78\n";
        let l = parse(&[(1, td)]);
        let mut rows: Vec<(String, i64)> = l.transactions.iter().map(|t| (t.description.clone(), (t.amount * 100.0).round() as i64)).collect();
        rows.sort();
        assert_eq!(rows, vec![("Check 11071".to_string(), 119905), ("Check 11072".to_string(), 149978), ("Check 11082".to_string(), 46796), ("Check 11102".to_string(), 2848), ("Check 2281".to_string(), 22331), ("Check 2282".to_string(), 430869)], "{:?}", l.transactions);
        assert_eq!(repair_check_pairs("04/16 2281 22331 04/15 11071 1,199.05"), "04/16 2281 223.31 04/15 11071 1,199.05");
        assert_eq!(repair_check_pairs("Cust #562791 711 3,783.78"), "Cust #562791 711 3,783.78");
        // CommunityAmerica's online printout: newest first under "Date Description Amount
        // Balance", the balances chaining backwards (and one that does not chain, the
        // header still saying which figure is the amount); the last row on the page keeps
        // its shape from the row above. Where it began and ended comes from the balance
        // column, and the rows between must close the equation.
        let p1 = "CommunityAmerica Credit Union  09/12/2023 11:41 AM\nAug 1, 2023 - Aug 31, 2023 Custom\nDate  Description  Amount  Balance\n08/31/2023  Point Of Sale Withdrawal / WAL-MART #2955  -$28.64  $977.16\n08/31/2023  Point Of Sale Withdrawal / SAMSCLUB.COM  -$39.23  $1,005.80\n08/31/2023  Point Of Sale Deposit / VENMO*Miller David  $541.36  $1,045.03\n08/31/2023  Point Of Sale Withdrawal ROBLOX  -$19.99  $503.67\n08/30/2023  ATM Foreign Transaction Fee  -$1.50  $173.69\n08/30/2023  Point Of Sale Withdrawal CASH / APP*THOMAS  -$100.00  $525.16\n";
        let p2 = "Date  Description  Amount  Balance\n08/29/2023  Point Of Sale Withdrawal / CULVERS  -$20.00  $625.16\n08/29/2023  Point Of Sale Deposit / VENMO  $300.00  $645.16\n08/28/2023  ACH Payment PAYPAL  -$8.72  $345.16\n";
        let l = parse(&[(1, p1), (2, p2)]);
        let rows: Vec<(Kind, i64)> = l.transactions.iter().map(|t| (t.kind, (t.amount * 100.0).round() as i64)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 2864), (Kind::Debit, 3923), (Kind::Credit, 54136), (Kind::Debit, 1999), (Kind::Debit, 150), (Kind::Debit, 10000), (Kind::Debit, 2000), (Kind::Credit, 30000), (Kind::Debit, 872)], "{:?}", l.transactions);
        assert_eq!((l.summary.beginning_balance, l.summary.ending_balance, l.summary.balance_check), (Some(353.88), Some(977.16), Some(true)), "{:?}", l.summary);
        // A debtor's "Cash Disbursements ... Per Bank Statements" schedule is not a statement.
        let sched = "Exhibit B- Cash Disbursements Page 1 of 4\nCash Disbursements\n10/1/2024-10/31/2024\nPer Bank Statements\nDate  Description  Source  Statement Period  Amount  Account Designation\n10/15/2024  ACH WEB  FSTENERGY METED ONLINE PMT  PNC Joint Checking 2001  October Export  $  (428.69) Utilities\n10/11/2024  ACH WEB  WASTE MANAGEMENT ONLINE PMT  PNC Joint Checking 2001  October Export  $  (266.41) Utilities\n";
        let l = parse(&[(1, sched)]);
        assert_eq!(l.summary.document_kind.as_deref(), Some("cash receipts and disbursements schedule"), "{:?}", l.summary);
        // Three scribbles between a check number and its date; the point lost to a space
        // at the end of a dated row with no other figure.
        let chase = "CHECKS PAID\nCHECK NO. DESCRIPTION PAID AMOUNT\n60541 A\u{201d} a \u{2014}_ 01/26 984.41\nELECTRONIC DEPOSITS\n02/21 CCD DEPOSIT, GRUBHUB INC FEB ACTVTY ****2119dKHGk50 424 67\n";
        let l = parse(&[(1, chase)]);
        let rows: Vec<(Kind, i64, &str)> = l.transactions.iter().map(|t| (t.kind, (t.amount * 100.0).round() as i64, t.description.as_str())).collect();
        assert_eq!(rows, vec![(Kind::Debit, 98441, "Check 60541"), (Kind::Credit, 42467, "CCD DEPOSIT, GRUBHUB INC FEB ACTVTY ****2119dKHGk50")], "{:?}", l.transactions);
        // A sweep account's "Other Debits" title at the wrapped lines' indent is the
        // section; a doubled "Check Date Amount" header under the deposits opens the checks.
        let sweep = "Business Account\n07/01/2023 Beginning Balance                                  .00\n  15 Deposits/Other Credits  +  1,553.84\n  4 Checks/Other Debits  -  1,553.84\n07/31/2023 Ending Balance  31 Days in Statement Period  .00\n  Deposits/Other Credits\n07/03/2023 Transfer Deposit  From Loan XXXXXX1727  850.00\n  TECH TOOL SUPPLY, LLC 7342077700 MI #2024\n07/31/2023 Mobile Deposit  703.84\n  Checks listed in numerical order; (*) indicates gap in sequence\nCheck Date Amount Check Date Amount\n4501 07/12 553.84 4503* 07/26 150.00\n  Other Debits\n07/03/2023 Debit Card Debit  800.00\n  LOWES #02231* CEDAR RAPIDS IA #2040\n07/03/2023 Debit Card Debit  50.00\n  MENARDS CEDAR RAPIDS S CEDAR RAPIDS IA #1992\n";
        let l = parse(&[(1, sweep)]);
        assert_eq!(((l.parsed_credit_total * 100.0).round() as i64, (l.parsed_debit_total * 100.0).round() as i64), (155384, 155384), "{:?}", l.transactions);
        assert_eq!(l.summary.balance_check, Some(true), "{:?}", l.summary);
        // Wintrust's second account page in a text layer whose font map lost its figures.
        assert_eq!(unfont("$o.oo").as_deref(), Some("$0.00"));
        assert_eq!(unfont("$431.3s").as_deref(), Some("$431.35"));
        assert_eq!(unfont("O9lO1l24").as_deref(), Some("09/01/24"));
        assert_eq!(unfont("O9l3olz4").as_deref(), Some("09/30/24"));
        assert_eq!(unfont("Balance"), None);
        assert_eq!(unfont("$Total"), None);
        // Chase in a scan: the summary's "75,193.44" is one digit off the deposits section's
        // "Total Deposits and Additions $76,193.44", which the rows meet; the section wins.
        let chase = "CHECKING SUMMARY\nBeginning Balance $0.00\nDeposits and Additions 75,193.44\nElectronic Withdrawals -76,193.44\nEnding Balance $0.00\nDEPOSITS AND ADDITIONS\nDATE DESCRIPTION AMOUNT\n09/12 Deposit 2027225520 $300.00\n09/15 Orig CO Name Sabio Inc Orig ID 9814416124 75,893.44\nTotal Deposits and Additions $76,193.44\nELECTRONIC WITHDRAWALS\nDATE DESCRIPTION AMOUNT\n09/16 Online Transfer To Chk ...8142 Transaction#: 15313724148 $76,193.44\nTotal Electronic Withdrawals $76,193.44\n";
        let l = parse(&[(1, chase)]);
        assert_eq!((l.summary.total_credits, l.summary.total_debits, l.summary.balance_check), (Some(76193.44), Some(76193.44), Some(true)), "{:?}", l.summary);
        // Chase in a scan: a day no month has ("02/41") fitted between the row's own
        // transaction date and the row below; the three-column daily table ends at the
        // "TRANSACTIONS FOR SERVICE FEE CALCULATION" title so its columns are not flushed
        // behind it as debits; the fee calculation block is informational.
        let chase = "ATM & DEBIT CARD WITHDRAWALS\nDATE DESCRIPTION AMOUNT\n02/41 Card Purchase 02/10 Att*Bill Payment 800-288-2020 TX Card 9540 $984.85\n02/17 Card Purchase 02/16 Paypal *Sarah 402-935-7733 CA Card 9540 150.00\nDAILY ENDING BALANCE |_(comlinves)\nDATE AMOUNT DATE __ _ AMOUNT DATE AMOUNT\n03/22 44,659.33 03/25 29,359.39 03/30 78,075.45\n03/23 46,128.94 03/26 30,951.09 03/31 78,903.52\n03/24 27,051.33 03/29 49,079.94\nTRANSACTIONS FOR SERVICE FEE CALCULATION NUMBER OF TRANSACTIONS\nChecks Paid / Debits 27\nSERVICE FEE CALCULATION AMOUNT\nService Fee $12.00\nService Fee Credit -$12.00\nNet Service Fee $0.00\n";
        let l = parse(&[(1, chase)]);
        let rows: Vec<(&str, Kind, i64)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, (t.amount * 100.0).round() as i64)).collect();
        assert_eq!(rows, vec![("02/11", Kind::Debit, 98485), ("02/17", Kind::Debit, 15000)], "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 8, "{:?}", l.daily_balances);
        // Heritage Bank in a scan: a month over 12 in a full date ("42/22/2021"), the
        // three-column daily table headed "Date Amount ~ Date Amount Date Amount" with its
        // title lost, and "INET XFER 12-02 FROM ..." / "TO ..." naming the direction.
        let heritage = "Date Description Amount\n12/22/2021 REGULAR DEPOSIT $300.00\n42/23/2021 ___ INCOMING WIRE FRANCO FACTORING LLC $36,016.17\nNumber of Deposits 2 Total Deposits $36,316.17\nDate Description Amount\n12/02/2021 INET XFER 12-02 TO XXXXXXXX0260 $1,200.00\n12/03/2021 INET XFER 12-03 FROM XXXXXXXX0686 $100.00\nDate Amount ~ Date Amount Date Amount\n12/09/2021 $28,395.91 12/20/2021 $5,643.81 12/27/2021 $16,984.79\n12/10/2021 $3,591.21 12/21/2021 $4,308.81 12/28/2021 $12,473.79\n";
        let l = parse(&[(1, heritage)]);
        let rows: Vec<(Kind, i64)> = l.transactions.iter().map(|t| (t.kind, (t.amount * 100.0).round() as i64)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 30000), (Kind::Credit, 3601617), (Kind::Debit, 120000), (Kind::Credit, 10000)], "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 6, "{:?}", l.daily_balances);
        // Synovus in flat OCR: a wire's wrapped description "PAYMENT ELEKTA INC DEPOSIT"
        // straight under its row is no deposits section.
        let syn = "Other Debits\n09-23 Phn/Fax Dom Out Wire ELEKTAINC DEPOSIT Y ACCOUNTCUREPOINT 24,941.08\nPAYMENT ELEKTA INC DEPOSIT\nORY ACCOUNT\n09-23 Service Charge PHN/FAX DOM OUT WI 100.00\n09-27 Preauthorized Wd CALL EXPERTS CALL EXPER 194.68\n";
        let l = parse(&[(1, syn)]);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Debit) && l.transactions.len() == 3, "{:?}", l.transactions);
        // National City in a scan: a check number with one misread letter ("f040*") and an
        // amount without its point ("132850") in the check table.
        let nc = "Checks\nCheck Number Amount Description Date Paid\n1029 $4,903.00 Paid Check - Image Available Online 10/23\nf040* 435.00 Paid Check - Image Available Online 10/16\n1048 132850 Paid Check - image Available Online 10/27\nTotal: 3 items for $6,666.50\n";
        let l = parse(&[(1, nc)]);
        let rows: Vec<(String, i64)> = l.transactions.iter().map(|t| (t.description.clone(), (t.amount * 100.0).round() as i64)).collect();
        assert_eq!(rows, vec![("Check 1029".to_string(), 490300), ("Check f040".to_string(), 43500), ("Check 1048".to_string(), 132850)], "{:?}", l.transactions);
        // KeyBank in flat OCR: a one-digit day ("12-1") padded to "12-01" must keep its
        // single space; the summary's "11-30-23" had switched the dashed-date padding on.
        let key = "Beginning balance 11-30-23 $9,220.75\nAdditions\nDeposits Date Serial # Source\n12-1 Direct Deposit, Capital Partners6D42 $1,965.00\n12-4 Direct Deposit, Vantage Vantage 1,575.48\n12-12 Direct Deposit, Spartanbusiness Ap 4,400.00\n";
        let l = parse(&[(1, key)]);
        assert_eq!(l.transactions.iter().map(|t| t.date.as_str()).collect::<Vec<_>>(), vec!["2023-12-01", "2023-12-04", "2023-12-12"], "{:?}", l.transactions);
        // Chase in a scan, under "DATE DESCRIPTION AMOUNT BALANCE": figures whose commas
        // and points became spaces are put back together from the right.
        assert_eq!(rejoin_split_figures("03/05 First Foundation Loan Pymt PPD ID: 1320211527 -6 789 84 25 409.88").as_deref(), Some("03/05 First Foundation Loan Pymt PPD ID: 1320211527 -6,789.84 25,409.88"));
        assert_eq!(rejoin_split_figures("03/21 Withdrawal -23 980 38 014").as_deref(), Some("03/21 Withdrawal -23,980.38 0.14"));
        assert_eq!(rejoin_split_figures("03/21 Interest Payment 0.14 23,980.52"), None);
        // Flagstar in a scan: a garbled daily balance row ("gan 31 1,050,488.04 Feb 14
        // 1,3129,847.30") is no check 31; "dan 31 489 36,210.81 Jan 29 508 5,555.41" is a
        // January row when the line prints the month right further on.
        let flag = "Checks by Serial Number\nFeb 14 531 * 58.64\ndan 31 489 36,210.81 Jan 29 508 5,555.41\nDaily Balances\ngan 31 1,050,488.04 Feb 14 1,3129,847.30\nFeb 03 1,079,689.09 Feb 18 1,129,904.80\n";
        let l = parse(&[(1, flag)]);
        let mut rows: Vec<(String, i64)> = l.transactions.iter().map(|t| (format!("{} {}", t.date, t.description), (t.amount * 100.0).round() as i64)).collect();
        rows.sort();
        assert_eq!(rows, vec![("01/29 Check 508".to_string(), 555541), ("01/31 Check 489".to_string(), 3621081), ("02/14 Check 531".to_string(), 5864)], "{:?}", l.transactions);
        // PNC's two summary boxes squashed onto one line by a poor text layer.
        let pnc = "Deposits and Other Additions  Checks and Other Deductions\nDescription  Items  Amount  Description  Items  Amount\nOther Additions  1  400,378.67  Checks  4  380,087.50\n  Service Charges and Fees  1  4.50\n  Other Deductions  1  8,625.00\nTotal  400,378.67  Total  6  388,717.00\nActivity Detail\nDeposits and Other Additions\n07/14  400,378.67  Transfer From Sub Account 0000004257379616\nChecks and Other Deductions\n07/20  380,087.50  Check 1001\n07/29  4.50  Counter Check Fee\n07/29  8,625.00  Withdrawal Tel\n";
        let l = parse(&[(1, pnc)]);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(400378.67), Some(388717.0)), "{:?}", l.summary);
        // Flushing Bank in a scan (a receiver's bundle): margin glyphs before the dates,
        // specks after them and the summary figures, "$60;000.00-", "96/03" under a "05/31
        // Balance Forward" over June's rows, and the six-label header with a lone "i" in it.
        let fl = "All Transactions by Date\nDate Description Withdrawal / Debit Deposit / Credit (+) Balance\n| 05/31. Balance Forward $42,860.59\n96/03 Deposit $5,000.00 $47,860.59\n| 06/10 Deposit $14,500.00 $62,360.59\n| 06/13. TRANSFER TO CK XXXXXXXX8362 $60;000.00- | $2,360.59\n06/15. Deposit $15,000.00 $17,360.59\nAccount Summary\nPrevious: Statement Date; 05/31/2023 |\nBeginning Interest. i Service Ending\nBalance + Deposits + Paid. - Withdrawals: - | Charge = Balance\n$42,860.59. $34,500.00. $0.00 $60,000.00 $0.00 $17,360.59\n";
        let l = parse(&[(1, fl)]);
        assert_eq!((l.summary.beginning_balance, l.summary.total_credits, l.summary.total_debits, l.summary.ending_balance, l.summary.balance_check), (Some(42860.59), Some(34500.0), Some(60000.0), Some(17360.59), Some(true)), "{:?}", l.summary);
        assert_eq!(l.transactions.iter().map(|t| t.date.as_str()).collect::<Vec<_>>(), vec!["2023-06-03", "2023-06-10", "2023-06-13", "2023-06-15"], "{:?}", l.transactions);
        // Check 28's regressions: JPMorgan numbers its pages straight through two accounts,
        // and the second account's summary page (its own beginning balance after the
        // first's) still starts a statement inside the running footer.
        let jp1 = "Page 7 of 22\nDeposits & Credits\nDate Description Amount\n09/30 JPMorgan Access Transfer From Account 9072 600,000.00\nTotal Deposits & Credits $600,000.00\n";
        let jp2 = "Checking Account Summary Instances Amount\nBeginning Balance 221,227.38\nDeposits & Credits 1 414,314.57\nChecks Paid 1 (391,174.19)\nEnding Balance 2 $244,367.76\nDeposits & Credits\nDate Description Amount\n09/01 Remote Online Deposit 1 414,314.57\nChecks Paid\nCheck Number Date Paid Amount\n60405 09/07 391,174.19\nPage 8 of 22\n";
        let jp0 = "Checking Account Summary Instances Amount\nBeginning Balance 100.00\nDeposits & Credits 1 600,000.00\nEnding Balance 1 $600,100.00\nPage 6 of 22\n";
        let l = parse(&[(1, jp0), (2, jp1), (3, jp2)]);
        assert_eq!(l.statements.iter().map(|s| s.pages).collect::<Vec<_>>(), vec![Some((1, 2)), Some((3, 3))], "{:?}", l.statements);
        // A dated wrapped cell under a check table ("06/12 000000075073010") never takes
        // a figure four lines below it ("Total Debits" then "-- 101,186.43").
        let cz = "Checks\nCheck # Amount Date Item No.\n1949 280.00 06/06 000000081035139\n06/12 000000075073010\n*There is a break in sequence\nTotal\nTotal Debits\nDebits\n-- 101,186.43\n";
        let l = parse(&[(1, cz)]);
        assert_eq!(l.transactions.iter().map(|t| (t.amount * 100.0).round() as i64).collect::<Vec<_>>(), vec![28000], "{:?}", l.transactions);
        // Two U.S. Bank first pages sharing the fee schedule's six figures are not one page twice.
        let us = |acct: &str, period: &str, rows: &str| format!("U.S. Bank Uni-Statement\nAccount Number: {acct}\nStatement Period: {period}\nPage 1 of 2\nINFORMATION YOU SHOULD KNOW\nFees: $6.00 $12.00 $35.00 $50.00 $55.00 $55.00 apply to some services described in the Consumer Pricing Information document that you received at account opening and which you can read online at any time.\nAccount Summary\n{rows}");
        let us1 = us("7110", "Dec 1, 2025 through Dec 31, 2025", "Beginning Balance on Dec 1 $ 100.00\nDeposits / Credits 3,579.40\nOther Withdrawals 99.18-\nEnding Balance on Dec 31, 2025 $ 3,580.22\nDeposits / Credits\nDate Description Amount\n12/12 Internet Banking Transfer From Account 1012 3,579.40\nOther Withdrawals\nDate Description Amount\n12/26 Check 5002 99.18\n");
        let us2 = us("9012", "Nov 22, 2025 through Dec 18, 2025", "Beginning Balance on Nov 22 $ 716.99\nDeposits / Credits 3,581.54\nOther Withdrawals 3,579.40-\nEnding Balance on Dec 18, 2025 $ 719.13\nDeposits / Credits\nDate Description Amount\n12/10 Federal Benefit Deposit 3,579.40\n12/18 Interest Paid 2.14\nOther Withdrawals\nDate Description Amount\n12/12 Internet Banking Transfer To Account 7110 3,579.40\n");
        let l = parse(&[(1, us1.as_str()), (2, us2.as_str())]);
        assert_eq!(l.statements.len(), 2, "{:?}", l.statements);
        assert!(l.statements.iter().all(|s| s.balance_check == Some(true)), "{:?}", l.statements);
        // American Express pages: "Minimum Due" with "New Charges" and "Pay Over Time".
        assert!(is_card_page("American Express Gold Card\nClosing Date 03/17/24\nPrevious Balance $852.07\nNew Charges +$2,420.32\nMinimum Due $0.00\nPay Over Time Limit $1,000.00\n"));
        assert!(!is_card_page("Business Checking\nStatement Period 03/01/24 - 03/31/24\nMinimum balance $1,000.00\n"));
        // Chase's online activity export is a printout: its own document, no totals.
        assert!(is_printout_page("total $0.00\naccount activity\ndate description type amount balance\noct 13, 2021 online transfer to chk ...3565 account transfer -$1,500.00 $7.58\n"));
    }

    #[test]
    fn banknorth_paired_rows_relisting_and_picture_pages() {
        let l = parse(&[(1, BANKNORTH_P1), (2, BANKNORTH_P2)]);
        let s = &l.summary;
        assert_eq!(s.bank.as_deref(), Some("BankNorth"));
        assert_eq!((s.beginning_balance, s.ending_balance), (Some(6216.37), Some(13102.33)));
        assert_eq!((s.period_start.as_deref(), s.period_end.as_deref()), (Some("12/31/23"), Some("01/31/24")));
        assert_eq!((s.total_credits, s.total_debits), (Some(8000.0), Some(1114.04)));
        assert!((l.parsed_credit_total - 8000.0).abs() < 0.001, "{:?}", l.transactions);
        assert!((l.parsed_debit_total - 1114.04).abs() < 0.001, "{:?}", l.transactions);
        assert_eq!(l.transactions.len(), 6, "{:?}", l.transactions);
        assert!(l.transactions.iter().any(|t| t.date == "2024-01-09" && (t.amount - 10.26).abs() < 0.001), "{:?}", l.transactions);
        assert!(l.daily_balances.iter().any(|b| (b.balance - 12402.33).abs() < 0.001), "{:?}", l.daily_balances);
    }

    // Brookline Bank court scan (flat OCR): "Statement Dates 12/11/18 thru 1/10/19" crossing
    // New Year, rows "date description amount[-] balance" with the figures split by spaces
    // ("10 ,130.00-", "27 , 893.44", "15 ..00-", "74,,84-"), a tilde in a trailing sign
    // ("4,022.84~-"), a code glued to a signed amount ("22.00-SC"), and wire details of up
    // to ten lines under a row before the next one.
    const BROOKLINE_P1: &str = "BrooklineBank\nDate 1/10/19 Page 1\nPrimary Account 1221063538\nCHECKING ACCOUNTS\nBusiness Gold Checking Number of Checks 1\nAccount Number 1221063538 Statement Dates 12/11/18 thru 1/10/19\nBeginning Balance 4,196.20 Days in the statement period 31\n3 Deposits/credits 16,329.00 Average Balance 23 ,605.00\n7 checks/Debits 11,579.51\nEnding Balance 8,945.69\n";
    const BROOKLINE_P2: &str = "Date 1/10/19 Page 2\nBusiness Gold Checking 1221063538 (Continued)\nActivity in Date Order\nDate Description Amount\n12/11 Incoming Domestic wire 15,000.00 19,196.20\nJETSON LLC\n2130 W NORTH AVE APT 201\nCHICAGO, IL 606476772\n2018121181QGC08C001622\n20181211MMQFMPAO000001\n12110801FTO1L\n12/11 wire Transfer Fee 15 ..00- 19,181.20\n12/11 check 1141 1,267 .72- 17,913.48\n12/13 ACH PMT AMEX EPAYMENT 10 ,130.00- 7,783.48\nPPD LaunchByteio Launch\n12/24 PAYMENT CHRYSLER CAPITAL 1,578.00- 6,205.48\n12/26 Return Item Credit 1,329.00 7,534 .48~-\n12/27 Return Item Credit 456.00 7,990,,48\n1/03 wire Transfer Fee 15.00- 7 , 975.48\n1/04 wire Transfer Fee 15.00- 7,960. 48\n1/10 Monthly Maintenance Fee 22.00-SC 7,938.48\n";

    #[test]
    fn brookline_split_figures_signed_balances_and_a_period_across_new_year() {
        let l = parse(&[(1, BROOKLINE_P1), (2, BROOKLINE_P2)]);
        let s = &l.summary;
        assert_eq!(s.bank.as_deref(), Some("Brookline Bank"));
        assert_eq!((s.period_start.as_deref(), s.period_end.as_deref()), (Some("12/11/18"), Some("1/10/19")));
        assert_eq!((s.total_credits, s.total_debits), (Some(16329.0), Some(11579.51)));
        let rows: Vec<(&str, Kind, f64)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![
            ("2018-12-11", Kind::Credit, 15000.0), ("2018-12-11", Kind::Debit, 15.0), ("2018-12-11", Kind::Debit, 1267.72), ("2018-12-13", Kind::Debit, 10130.0),
            ("2018-12-24", Kind::Debit, 1578.0), ("2018-12-26", Kind::Credit, 1329.0), ("2018-12-27", Kind::Credit, 456.0),
            ("2019-01-03", Kind::Debit, 15.0), ("2019-01-04", Kind::Debit, 15.0), ("2019-01-10", Kind::Debit, 22.0),
        ], "{:?}", l.transactions);
        assert!((l.parsed_credit_total - 16785.0).abs() < 0.001 && (l.parsed_debit_total - 13042.72).abs() < 0.001, "{:?}", l.transactions);
    }

    // First American Bank (flat OCR): "TRANSACTIONS SUMMARY" with the amount before the
    // description and a running balance, then seven-word capitalised recaps ("SUMMARY OF
    // ELECTRONIC DEBITS AND OTHER WITHDRAWALS") re-listing the same rows; an interest row
    // whose words say payment but whose balance change says credit.
    #[test]
    fn first_american_recaps_are_their_own_tables_and_the_balance_change_names_the_kind() {
        let text = "First American Bank\nCHECKING SUMMARY\nCHECKING BALANCE LAST STATEMENT......... 1,845.13\n2 DEPOSITS/OTHER CREDITS + 233,506.66\n4 CHECKS/OTHER DEBITS - 234,532.61\nCHECKING BALANCE THIS STATEMENT......... 819.18\nTRANSACTIONS SUMMARY\nDATE AMOUNT DESCRIPTION Balance\n04/01 Beginning Balance 1,845.13\n04/25 226,500.00 ACH Deposit St Charles Catho Trsfr 228,345.13\n04/29 7,000.00 ACH Deposit St Charles Catho Trsfr 235,345.13\n04/29 -402.71 ACH Payment ST CHARLES HIGH Payroll 234,942.42\n04/29 -25,624.50 ACH Payment ST CHARLES HIGH Payroll 209,317.92\n04/29 -51,582.34 ACH Payment ST CHARLES HIGH Payroll 157,735.58\n04/29 -156,923.06 ACH Payment ST CHARLES HIGH Payroll 812.52\n04/30 6.66 Accr Earning Pymt Added to Account 819.18\nSUMMARY OF ELECTRONIC DEBITS AND OTHER WITHDRAWALS\nDATE AMOUNT DESCRIPTION\n04/29 402.71 ACH Payment ST CHARLES HIGH Payroll\n04/29 25,624.50 ACH Payment ST CHARLES HIGH Payroll\n04/29 51,582.34 ACH Payment ST CHARLES HIGH Payroll\n04/29 156,923.06 ACH Payment ST CHARLES HIGH Payroll\nSUMMARY OF ELECTRONIC CREDITS AND OTHER DEPOSITS\nDATE AMOUNT DESCRIPTION\n04/25 226,500.00 ACH Deposit St Charles Catho Trsfr\n04/29 7,000.00 ACH Deposit St Charles Catho Trsfr\n04/30 6.66 Accr Earning Pymt Added to Account\n";
        let l = parse(&[(1, text)]);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(233506.66), Some(234532.61)));
        assert_eq!(l.transactions.len(), 7, "{:?}", l.transactions);
        assert!((l.parsed_credit_total - 233506.66).abs() < 0.001 && (l.parsed_debit_total - 234532.61).abs() < 0.001, "{:?}", l.transactions);
        assert!(l.transactions.iter().any(|t| (t.amount - 6.66).abs() < 0.001 && t.kind == Kind::Credit && t.description == "Accr Earning Pymt Added to Account"), "{:?}", l.transactions);
    }

    // Gulf Coast Bank (aligned columns): a row whose amount the court blacked out, read
    // from the running balance; Hancock Whitney's summary with zeros that lost their point
    // ("+ 0 CREDITS  -00  8,781,412.21") and an unreadable interest figure that the listed
    // interest row supplies.
    #[test]
    fn a_blacked_out_amount_comes_from_the_balance_and_lost_zeros_are_zero() {
        let gulf = "Account Summary\nDate           Description                                Amount\n04/01/2025     Beginning Balance                       $650,051.75\n               3 Credit(s) This Period                   $1,568.42\n               1 Debit(s) This Period                      $964.18\n04/30/2025     Ending Balance                          $650,655.99\nAccount Activity\nPost Date      Description                                                   Debits            Credits               Balance\n04/01/2025     Beginning Balance                                                                                  $650,051.75\n04/01/2025     Gulf Coast Bank Tuition Addl Purchase                                         $185.00              $650,236.75\n04/03/2025     MERCHANT BANKCD DISCOUNT                                    $964.18                                $649,272.57\n04/09/2025     Square Inc SQ250409                                                                                $650,144.99\n04/10/2025     Gulf Coast Bank Tuition Payment                                               $511.00              $650,655.99\n";
        let l = parse(&[(1, gulf)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 185.0), (Kind::Debit, 964.18), (Kind::Credit, 872.42), (Kind::Credit, 511.0)], "{:?}", l.transactions);
        assert!(l.transactions[2].description.contains("amount read from the running balance"));
        let hancock = "Hancock Whitney\nMoney Market Demand Account Summary\n3 PREVIOUS BALANCE 8,780,600.31 AVERAGE BALANCE\n4 + 0 CREDITS -00 8,781,412.21\nFy 0 DEBITS .00 YTD INTEREST PAID\n8 - SERVICE CHARGES 00 195,380.63\ng + INTEREST PAID 25169O5\n8 ENDING BALANCE 8,805,769.36\ne Deposits and Other Credits\nDate Amount Description Date Amount Description\n08/29 25,169.05 IOD INTEREST PAID\n";
        let l = parse(&[(1, hancock)]);
        assert_eq!((l.summary.beginning_balance, l.summary.ending_balance), (Some(8780600.31), Some(8805769.36)));
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(25169.05), Some(0.0)), "{:?}", l.summary);
        assert!((l.parsed_credit_total - 25169.05).abs() < 0.001, "{:?}", l.transactions);
    }

    // TriState Capital (flat OCR): a table rule read as glyph noise between "Daily Balances"
    // and its header must not end the daily table; a check image captioned twice on one
    // line is no row. Community Bank: "CR" after the amount is a credit and a trailing minus
    // a debit whatever the words say. Tri Counties: stray glyphs between the amount and the
    // balance, an ending balance in the summary that must not open the chain, and the
    // balance arithmetic naming the kind on a flat column page.
    #[test]
    fn glyph_noise_captions_and_markers_in_court_scans() {
        let tristate = "TriState Capital Bank\nBeginning Balance $629,815.94\n- Total Subtractions $9,717.00\nEnding Balance $620,098.94\nChecks\nCheck # Date Amount\n4001 01-23 $9,717.00\nDaily Balances\n\u{2014}\u{2014}\u{2014}\u{2014}E\u{2014}\u{2014}EeEEE~_\u{2014}\u{2014}&\u{2014}_zx\u{2014}\u{2014}>>>>&&>_>>\u{2014}eiiEEiEIEIEIEq*&*_\u{2014}>~>\nDate Amount Date Amount Date Amount\n12-31 $629,815.94 01-23 $620,098.94\n";
        let caption = "TriState Capital Bank\nPeriod Covered:\n01/23/2023 4001 $9,717.00 01/23/2023 4001 $9,717.00\n";
        let l = parse(&[(1, tristate), (2, caption)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Debit, 9717.0)], "{:?}", l.transactions);
        assert_eq!(l.daily_balances.len(), 2, "{:?}", l.daily_balances);

        let community = "Activity in Date Order\nDate Description Amount\n11/01 INTUIT 64168915 DEPOSIT 2,915.71 CR\n11/12 POS CRE 0000 11/10/21 01569111 27.81 CR\n11/15 FREEDOM LIFE INSINS. PREM 279.38-\n11/17 FREEDOM LIFE INSREVERSAL 279.38 CR\n11/17 FORD CREDIT AUTO PYMT 745.26-\n";
        let l = parse(&[(1, community)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 2915.71), (Kind::Credit, 27.81), (Kind::Debit, 279.38), (Kind::Credit, 279.38), (Kind::Debit, 745.26)], "{:?}", l.transactions);

        let tri = "tri counties bank\nAccount Summary\nDate Description Amount\n07/28/2025 Beginning Balance $260.76\n2 Credit(s) This Period $340.00\n2 Debit(s) This Period $236.61\n08/27/2025 Ending Balance $364.15\nAccount Activity\nPost Date Description Withdrawals Deposits Balance\n07/28/2025 Beginning Balance $260.76\n07/28/2025 OLB XFER FR DDA 000452056859 $90.00 $350.76\n07/29/2025 AES STDNT LOAN $188.11 : $162.65\n08/11/2025 DEPOSIT $250.00 $412.65\n08/25/2025 CHECK #1527 $48.50 . $364.15\n";
        let l = parse(&[(1, tri)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 90.0), (Kind::Debit, 188.11), (Kind::Credit, 250.0), (Kind::Debit, 48.5)], "{:?}", l.transactions);
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(340.0), Some(236.61)));
    }

    // Scan repairs from the September 21 loop: a month no calendar has takes the month of
    // the neighbouring rows ("42/22", "42/09" in a two-column check table), an o for a zero
    // ("o9/11"), a section sign for a five ("§,442.13"), a semicolon and colon in a figure
    // ("1;199:05"), an overdrawn balance marked "OD" under garbled column labels, and PNC's
    // count sentence running into a section header and an amount-first table header.
    #[test]
    fn scan_repairs_of_dates_figures_and_headers() {
        let td = "Electronic Deposits\nPOSTING DATE DESCRIPTION AMOUNT\n12/19 ACH DEPOSIT, PAYROLL 1,000.00\n42/22 ACH DEPOSIT, VENMO CASHOUT ****071298269 868.00\n12/23 ACH DEPOSIT, SQUARE 500.00\nChecks Paid No. Checks: 3\nDATE SERIAL NO. AMOUNT DATE SERIAL NO. AMOUNT\n12/04 4455950652 897.00 12/24 4455950677 744.60\n42/09 4455950653 3,250.00 12/24 4455950678 94.39\nElectronic Payments\nPOSTING DATE DESCRIPTION AMOUNT\n12/15 ELECTRONIC PMT-WEB, BARCLAYCARD US CREDITCARD ****519589 §,442.13\n12/16 ELECTRONIC PMT-WEB, DISCOVER 1;199:05\n";
        let l = parse(&[(1, td)]);
        let rows: Vec<(&str, Kind, f64)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![("12/19", Kind::Credit, 1000.0), ("12/22", Kind::Credit, 868.0), ("12/23", Kind::Credit, 500.0), ("12/04", Kind::Debit, 897.0), ("12/09", Kind::Debit, 3250.0), ("12/24", Kind::Debit, 744.6), ("12/24", Kind::Debit, 94.39), ("12/15", Kind::Debit, 5442.13), ("12/16", Kind::Debit, 1199.05)], "{:?}", l.transactions);

        let pr = "STATEMENT OF ACCOUNT\nSEGHINING BALANGH DEPOSITS / OTHER CREDITS| CHECKS / OTHER DEBITS SERVICE Enibinie BALANCE\n3.16-OD 2843.44 2840.87 | 10.00] 10.59-0D\nCHECKS\nDATE. ...CHECK NO......AMOUNT DATE....CHECK NO......AMOUNT\n09/08 205 200.00 09/21 209 200.00\no9/11 206 200.00 09/28 211 200.00\n";
        let l = parse(&[(1, pr)]);
        let s = &l.summary;
        assert_eq!((s.beginning_balance, s.total_credits, s.total_debits, s.ending_balance), (Some(-3.16), Some(2843.44), Some(2850.87), Some(-10.59)), "{:?}", s);
        assert_eq!(l.transactions.len(), 4, "{:?}", l.transactions);
        assert!(l.transactions.iter().all(|t| t.kind == Kind::Debit && (t.amount - 200.0).abs() < 0.001));

        let pnc = "Balance Summary\nBeginning Deposits and Checks and other Ending\nbalance other additions deductions balance\n5,984.40 250.00 110.97 6,123.43\nActivity Detail\nDeposits and Other Additions There were 1 Deposits and Other\nDate Amount Description Additions totaling $250.00.\n03/03 250.00 Online Transfer From 3397\nBanking/Debit Card Withdrawals and Purchases There were 1 Banking Machine\nDate Amount Description withdrawals totaling $110.97.\n02/28 110.97 B768 Debit Card Purchase Tst*Zuzul Coastal PIN POS purchases totaling $89.35.\n";
        let l = parse(&[(1, pnc)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 250.0), (Kind::Debit, 110.97)], "{:?}", l.transactions);
    }

    #[test]
    fn rows_broken_across_a_section_marker_and_checks_dated_in_their_description() {
        // Chase's print stream marks its sections and breaks the last row of a page across
        // the marker: the date prints, then "*end*deposits and additions", then the row's
        // description and amount. Its check table prints the date written on the check in
        // the description column beside the date paid, and names the listing by its first
        // column ("Check No."), with the word "Date" on the line above.
        let chase = "               CHECKING SUMMARY\n                                                    INSTANCES              AMOUNT\n             Beginning Balance                                          $1,000.00\n             Deposits and Additions                      3             11,500.00\n             Checks Paid                                 2             -1,300.00\n             Ending Balance                              5            $11,200.00\n       *end*summary\n       *start*deposits and additions\n             DEPOSITS AND ADDITIONS\n        DATE        DESCRIPTION                                              AMOUNT\n        03/02       Deposit     1254529466                                 5,000.00\n        03/03       Deposit     1254529467                                 4,500.00\n        03/04\n*end*deposits and additions\n                    Online Transfer From Chk ...6372 Transaction#: 28342108456      2,000.00\n\n             CHECKS PAID\n                                                              DATE\n        CHECK NO.              DESCRIPTION                    PAID              AMOUNT\n        1979 ^                 03/01                          03/05             300.00\n        1980 ^                                                03/06           1,000.00\n             Total Checks Paid                                              $1,300.00\n";
        let l = parse(&[(1, chase)]);
        let rows: Vec<(String, Kind, f64)> = l.transactions.iter().map(|t| (t.date.clone(), t.kind, t.amount)).collect();
        assert_eq!(
            rows,
            vec![
                ("03/02".into(), Kind::Credit, 5000.0),
                ("03/03".into(), Kind::Credit, 4500.0),
                ("03/04".into(), Kind::Credit, 2000.0),
                ("03/05".into(), Kind::Debit, 300.0),
                ("03/06".into(), Kind::Debit, 1000.0),
            ],
            "{:?}",
            l.transactions
        );
        assert_eq!((l.summary.total_credits, l.summary.total_debits), (Some(11500.0), Some(1300.0)));
        assert_eq!(l.summary.balance_check, Some(true));
    }

    #[test]
    fn an_online_printout_carries_its_day_and_a_signed_amount_column_names_the_kind() {
        // A printout from an online banking page prints the date once for each day, leaves
        // the cell blank on the rows below it and wraps every description over several
        // lines. The date column is narrow, so the day is written back in its short form.
        // The page signs its debits, so the sign is the kind and the unsigned rows are
        // credits whatever the words around them say.
        let printout = "Printed from Chase for Business\n\nDate   Description           Type                  Amount     Balance\n\n04/20/2026  Online Transfer to    Account transfer   -$5,000.00   $1,000.00\n            CHK ...8781\n            TRANSACTION#:\n            28903665366 04/20\n\n            REAL TIME PAYMENT     Other               $4,000.00    $6,000.00\n            CREDIT RECD FROM\n            ABA/CONTR BNK\n\n            ORIG CO               ACH debit          -$2,000.00    $2,000.00\n            NAME:A SUPPLIER\n\n04/17/2026  Online Transfer to    Account transfer   -$1,500.00    $4,000.00\n            CHK ...8781\n\n            REAL TIME PAYMENT     Other               $3,000.00    $5,500.00\n            CREDIT RECD FROM\n\n            ORIG CO               ACH debit            -$500.00    $2,500.00\n            NAME:A FEE\n\n";
        let l = parse(&[(1, printout)]);
        let rows: Vec<(String, Kind, f64)> = l.transactions.iter().map(|t| (t.date.clone(), t.kind, t.amount)).collect();
        assert_eq!(
            rows,
            vec![
                ("2026-04-20".into(), Kind::Debit, 5000.0),
                ("2026-04-20".into(), Kind::Credit, 4000.0),
                ("2026-04-20".into(), Kind::Debit, 2000.0),
                ("2026-04-17".into(), Kind::Debit, 1500.0),
                ("2026-04-17".into(), Kind::Credit, 3000.0),
                ("2026-04-17".into(), Kind::Debit, 500.0),
            ],
            "{:?}",
            l.transactions
        );

        // What the document-wide sign rule votes on: rows that end in a running balance or in
        // the dash a report prints for one. An overdrawn day in a daily balance table is no
        // debit, and a listing with one figure to a row stays with the per-page rule.
        let report_rows = "9/21/2026  Transfer  ($2,500.00)  $68.57\n9/18/2026  Loan  ($7,479.62)  -\n9/18/2026  Transfer  $10,000.00  -\n9/01/2026  Supplier  ($36.75)  -\n9/01/2026  Deposit  $2,043.32  -\n9/01/2026  Transfer  ($2,000.00)  -\n9/01/2026  Deposit  $1,000.00  -\n";
        assert!(super::signs_the_debits(&[report_rows]));
        let daily = "07-31  4,980.65  08-10  343.84  08-22  -7,570.76\n08-01  -53,970.77  08-11  1,045.93  08-23  -1,580.08\n08-09  1,215.62  08-19  -15,382.86\n";
        assert!(!super::signs_the_debits(&[daily]));
        let one_figure = "08/01/22  Interest  $85.44\n08/07/22  CASH APP  $-500.00\n08/09/22  ENDICIA  $-54.42\n08/09/22  AUTOMOTIVE  $-971.25\n08/10/22  Deposit  $300.00\n08/11/22  Deposit  $120.00\n";
        assert!(!super::signs_the_debits(&[one_figure]));

        // A bank verification report keeps one amount column and signs its debits there.
        // Its header's last cell is stacked over three lines, "EOD" above and "Balance"
        // below, and it prints the running balance only on the last row of each day.
        let report = "                                                                                              EOD\nDate         Codes      Description                              Category        Amount\n                                                                                            Balance\n9/21/2026    bd,tt      Online Transfer to CHK ...1296           Transfers    ($2,500.00)   $68.57\n9/18/2026    ld         ORIG CO NAME:A Lender DESCR:LOAN PYMT    Loan        ($7,479.62)      -\n9/18/2026    bd,ts,dp   Online Transfer from CHK ...1296         Transfers   $10,000.00       -\n9/01/2026    ad         Online ACH Payment To A Supplier         Services        ($36.75)     -\n9/01/2026    dp         Deposit 1254529466                       Deposit      $2,043.32       -\n9/01/2026    bd,tt      Online Transfer to CHK ...1296           Transfers    ($2,000.00)     -\n9/01/2026    dp         Deposit 1254529467                       Deposit      $1,000.00       -\n";
        let l = parse(&[(1, report)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(
            rows,
            vec![
                (Kind::Debit, 2500.0),
                (Kind::Debit, 7479.62),
                (Kind::Credit, 10000.0),
                (Kind::Debit, 36.75),
                (Kind::Credit, 2043.32),
                (Kind::Debit, 2000.0),
                (Kind::Credit, 1000.0),
            ],
            "{:?}",
            l.transactions
        );
    }

    #[test]
    fn report_rows_that_quote_a_figure_and_parenthesised_debits_beside_a_balance() {
        // A bank verification report: a deposit adjustment row quotes the original deposit
        // ("Org Dep Amt= 85,520.00") before its own amount, and its wrapped reason line
        // ("Reason= Missing Deposit Tick") is the row's, not a deposits heading. A debit in
        // parentheses beside the day's running balance is a debit whatever the category
        // column says ("Credit Card").
        let page = "                                                                                                                        EOD\nDate         Codes      Description                                                        Category          Amount\n                                                                                                                       Balance\n6/8/2026     dp         Deposit 1254529466                                                 Deposit       $2,043.32  $40,328.44\n6/5/2026                Cash Svcs Db/Cr Dep Adjust, Org Dep Amt= 85,520.00, Depdate= 06/02/2026  Other     ($38,150.00)       -\n                        Reason= Missing Deposit Tick\n6/5/2026                ORIG CO NAME:PAYCHEX TPS DESCR:TAXES                               Services      ($2,858.35)       -\n6/4/2026     dp         Cash Svcs Db/Cr Dep Adjust, Org Dep Amt= 14,350.00, Depdate= 06/01/2026  Deposit     $4,614.00       -\n6/4/2026     cc         ORIG CO NAME:CAPITAL ONE DESCR:CRCARDPMT SEC:CCD Credit Card       Credit Card     ($695.00)  $36,000.00\n6/3/2026     dp         Deposit 1254529467                                                 Deposit       $1,000.00       -\n6/3/2026                ORIG CO NAME:SUPPLIER                                              Services        ($500.00)       -\n";
        let l = parse(&[(1, page)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(
            rows,
            vec![
                (Kind::Credit, 2043.32),
                (Kind::Debit, 38150.0),
                (Kind::Debit, 2858.35),
                (Kind::Credit, 4614.0),
                (Kind::Debit, 695.0),
                (Kind::Credit, 1000.0),
                (Kind::Debit, 500.0),
            ],
            "{:?}",
            l.transactions
        );
    }

    #[test]
    fn order_receipts_and_recorded_land_papers_are_not_statements() {
        let receipt = "Order Placed: January 3, 2022\nItems Subtotal: $70.98\nShipping & Handling: $11.28\nFree Shipping: -$11.28\nTotal before tax: $70.98\nEstimated tax to be collected: $6.12\nGrand Total: $77.10\n";
        assert_eq!(parse(&[(1, receipt)]).summary.document_kind.as_deref(), Some("order receipt"));
        let deed = "RECORD AND RETURN TO:\nTITLE COMPANY\nRecording Fee (excluding transfer tax) $40.00\nTotal Amount $40.00\n";
        let mortgage = "MORTGAGE\n(J) \"Community Association Dues, Fees, and Assessments\" means all dues\nRECORDING FEES 40.00\n";
        assert_eq!(parse(&[(1, deed), (2, mortgage)]).summary.document_kind.as_deref(), Some("recorded real estate documents"));
    }

    #[test]
    fn a_returned_pull_and_a_transfer_in_are_credits_but_a_returned_item_fee_is_not() {
        assert_eq!(super::kind_and_confidence("RETURNED ACH DEBIT NSF WEB COMCAST RETRY PYMT", None), (Kind::Credit, true));
        assert_eq!(super::kind_and_confidence("TRANSFER IN RECORD NO. P0F023 ZELLE FROM A PERSON", None), (Kind::Credit, true));
        assert_eq!(super::kind_and_confidence("RETURNED ACH DEBIT FEE", None).0, Kind::Debit);
        assert_eq!(super::kind_and_confidence("TRANSFER OUT RECORD NO. P0L0IV ZELLE TO A PERSON", None).0, Kind::Debit);
        assert_eq!(super::kind_and_confidence("VENMO CASHOUT A PERSON", None), (Kind::Credit, true));
        assert_eq!(super::kind_and_confidence("VENMO PAYMENT A PERSON", None).0, Kind::Debit);
        assert_eq!(super::kind_and_confidence("SQUARE INC SQ240101 A COMPANY", None), (Kind::Credit, true));
        assert_eq!(super::kind_and_confidence("SQ *COFFEE SHOP CITY ST", None).0, Kind::Debit);
        assert_eq!(super::kind_and_confidence("PODIUM PAYMENTS PODIUM PAY A COMPANY", None), (Kind::Credit, true));
        assert_eq!(super::kind_and_confidence("WWW.PODIUM.COM HTTPSWWW UT 02/14", None).0, Kind::Debit);
        // Three verification entries on one day: the one equal to the others' sum is the pull.
        let page = "Date Description Deposits Withdrawals\nFeb 13 INTUIT ACCTVERIFY A COMPANY 0.15\nFeb 13 INTUIT ACCTVERIFY A COMPANY 0.03\nFeb 13 INTUIT ACCTVERIFY A COMPANY 0.18\n";
        let l = parse(&[(1, page)]);
        let rows: Vec<(Kind, f64)> = l.transactions.iter().map(|t| (t.kind, t.amount)).collect();
        assert_eq!(rows, vec![(Kind::Credit, 0.15), (Kind::Credit, 0.03), (Kind::Debit, 0.18)], "{:?}", l.transactions);
    }

    #[test]
    fn sales_reports_utility_bills_and_old_form_petitions_are_not_statements() {
        let pos = "SalesSummary_2024-08-01_2024-08-31\n-A Restaurant\nRevenue summary\nNet sales                     184005.6\nGratuity                        1899.08\n";
        assert_eq!(parse(&[(1, pos)]).summary.document_kind.as_deref(), Some("point of sale sales report"));
        let bill = "Account Number: 2035438-7\nMeter Number  Meter Size  Prior Read Date  Current Read Date  Usage (CCF)\n88189524  1\"  11/2/25  11/11/25  37.2\nBill Date 2/3/26\nPrevious Balance  $17,038.31\n";
        assert_eq!(parse(&[(1, bill)]).summary.document_kind.as_deref(), Some("utility bill"));
        let petition = "B1 (Official Form 1) (04/13)\nUnited States Bankruptcy Court\nName of Debtor (if individual, enter Last, First, Middle):\n";
        let stub = "Earnings Statement\nLeave Balance Summary\nLeave Type  Beginning Balance  Earned  Current\nSick and Personal  63.53  3.00\n";
        assert_eq!(parse(&[(1, petition), (2, stub)]).summary.document_kind.as_deref(), Some("bankruptcy petition and schedules"));
    }

    #[test]
    fn a_day_read_with_a_digit_too_many_and_a_misread_summary_line() {
        // Chase scan: "03/117" between 03/17 and 03/22 is 03/17; the summary's electronic
        // line reads 1,508.11 for 1,503.11, which moves two digits of the debit total. The
        // section, carried over a page break, prints its total once; the rows meet it and
        // carry the beginning balance to the ending one, so the summary figure is the misread.
        let p1 = "CHECKING SUMMARY\nBeginning Balance $1,000.00\nDeposits and Additions 2 2,000.00\nElectronic Withdrawals 3 -1,508.11\nEnding Balance $1,496.89\nDEPOSITS AND ADDITIONS\nDATE DESCRIPTION AMOUNT\n03/01 Remote Online Deposit 1 $1,500.00\n03/02 Remote Online Deposit 1 500.00\nTotal Deposits and Additions $2,000.00\nELECTRONIC WITHDRAWALS\nDATE DESCRIPTION AMOUNT\n03/10 Orig CO Name:Utility Co Orig ID:1234 CO Entry $600.00\n";
        let p2 = "ELECTRONIC WITHDRAWALS (continued)\nDATE DESCRIPTION AMOUNT\n03/17 Orig CO Name:Supply Co Orig ID:4321 CO Entry 400.00\n03/117 Orig CO Name:Insurance Co Orig ID:5678 CO Entry 91.62\n03/22 Orig CO Name:Payroll Fees Orig ID:9999 CO Entry 411.49\nTotal Electronic Withdrawals $1,503.11\n";
        let l = parse(&[(1, p1), (2, p2)]);
        let rows: Vec<(&str, Kind, f64)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, t.amount)).collect();
        assert!(rows.contains(&("03/17", Kind::Debit, 91.62)), "{:?}", l.transactions);
        assert_eq!((l.summary.total_debits, l.summary.total_credits), (Some(1503.11), Some(2000.0)), "{:?}", l.summary);
    }

    #[test]
    fn a_flat_check_pair_whose_right_date_the_scan_garbled() {
        // TD scan, two check columns read flat: the right date read "O7/16", "07124", "OF?";
        // one left amount lost its point to a space, "247 92".
        let text = "Checks Paid No. Checks: 6\nDATE SERIAL NO. AMOUNT DATE SERIAL NO. AMOUNT\n07/01 9016 738.54 O7/16 5021 606.88\n07/03 5017 247 92 OF? 5022 247.92\n07/05 5018 745.39 07124 5024 205.95\nSubtotal: 2,791.60\n";
        let l = parse(&[(1, text)]);
        let rows: Vec<(&str, f64, &str)> = l.transactions.iter().map(|t| (t.date.as_str(), t.amount, t.description.as_str())).collect();
        assert_eq!(rows, vec![("07/01", 738.54, "Check 9016"), ("07/03", 247.92, "Check 5017"), ("07/05", 745.39, "Check 5018"), ("07/16", 606.88, "Check 5021"), ("07/16", 247.92, "Check 5022"), ("07/24", 205.95, "Check 5024")], "{:?}", l.transactions);
        assert_eq!(split_garbled_check_pair("07/01 9016 738.54 Paid Check 606.88"), None);
    }

    #[test]
    fn a_section_named_in_the_margin_three_check_groups_and_a_notice_of_next_year() {
        // SunTrust: "Deposits/" beside the column header names the section (a wire's "CR"
        // names nothing); three check groups under one header, the last row alone; "As of
        // January 1, 2020, fees will change" on a November 2019 statement; the last page's
        // disclaimer about pending transactions is no online printout.
        let p1 = "Account Statement\n11/30/2019\nAs of January 1, 2020, fees will change for some treasury services.\nDeposits/    Date                   Amount Serial #          Description\nCredits      11/16                   735.00                  ELECTRONIC/ACH CREDIT\n             11/29              10,000.00                    INCOMING FEDWIRE CR TRN #016901\n             Deposits/Credits: 2\nChecks           Check                      Amount   Date       Check                       Amount   Date       Check                       Amount   Date\n                 Number                              Paid       Number                               Paid       Number                               Paid\n                 19831                       182.05 11/15       19841                        395.00 11/29       19851                      1,000.00 11/29\n                 19832                     1,473.00 11/18\n                 Checks: 4\n";
        let p2 = "11/30/2019\nWithdrawals/   Date                    Amount Serial #                Description\nDebits         Paid\n               11/25                    563.13                        ELECTRONIC/ACH DEBIT\nBalance        Date                        Balance                   Collected            Date                        Balance                   Collected\nActivity                                                               Balance                                                                    Balance\nHistory        11/01                    206,900.28                  206,900.28            11/17                     49,449.75                   49,449.75\nThe Ending Daily Balances provided do not reflect pending transactions or holds. If your available balance wasn't sufficient when transactions posted, fees may have been assessed.\n";
        let l = parse(&[(1, p1), (2, p2)]);
        let rows: Vec<(&str, Kind, f64)> = l.transactions.iter().map(|t| (t.date.as_str(), t.kind, t.amount)).collect();
        assert_eq!(rows, vec![("2019-11-16", Kind::Credit, 735.0), ("2019-11-29", Kind::Credit, 10000.0), ("2019-11-15", Kind::Debit, 182.05), ("2019-11-29", Kind::Debit, 395.0), ("2019-11-29", Kind::Debit, 1000.0), ("2019-11-18", Kind::Debit, 1473.0), ("2019-11-25", Kind::Debit, 563.13)], "{:?}", l.transactions);
        assert_eq!(l.statements.len(), 0, "{:?}", l.statements);
    }
}
