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
    pub summary: Summary,
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

/// Parse "1,234.56", "$1,234.56", "1,234.56-", "-1,234.56", "(1,234.56)".
pub fn parse_amount(raw: &str) -> Option<f64> {
    let t = raw.trim();
    let neg = t.ends_with('-') || t.starts_with('-') || (t.starts_with('(') && t.ends_with(')'));
    let digits: String = t.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    if digits.is_empty() || !digits.contains('.') {
        return None;
    }
    let v: f64 = digits.parse().ok()?;
    Some(if neg { -v } else { v })
}

fn is_amount_token(tok: &str) -> bool {
    let t = tok.trim_start_matches('$').trim_start_matches('-').trim_end_matches('-').trim_matches(|c| c == '(' || c == ')');
    let mut seen_dot = false;
    let mut decimals = 0;
    if t.is_empty() || !t.chars().next().unwrap().is_ascii_digit() {
        return false;
    }
    for c in t.chars() {
        match c {
            '0'..='9' => {
                if seen_dot {
                    decimals += 1
                }
            }
            ',' if !seen_dot => {}
            '.' if !seen_dot => seen_dot = true,
            _ => return false,
        }
    }
    seen_dot && decimals == 2
}

/// (month, day, year) from MM/DD, MM/DD/YY, MM/DD/YYYY.
fn parse_date_token(tok: &str) -> Option<(u32, u32, Option<i32>)> {
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
    let header_like = (l.contains("---") || toks.len() <= 6) && !starts_with_date && !ends_with_amount;
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
    let l = desc.to_ascii_lowercase();
    // Explicit words on the line win over the section, since some banks mix them.
    if l.contains("withdrawal") || l.contains(" debit") || l.starts_with("debit") || l.contains("purchase") || l.contains(" fee") || l.contains("charge") || l.contains("check ") || l.contains("payment to") {
        return Kind::Debit;
    }
    if l.contains("deposit") || l.contains(" credit") || l.starts_with("credit") || l.contains("interest") && section.is_none() {
        return Kind::Credit;
    }
    section.unwrap_or(Kind::Debit)
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

/// Parse one page. `year_hint` fills in years for MM/DD dates.
fn parse_page(text: &str, page: usize, year_hint: Option<i32>, ledger: &mut Ledger, section: &mut Option<Kind>, in_daily: &mut bool) {
    let mut last_txn: Option<usize> = None;
    // Column-style summaries ("Previous Balance  Total Credits  Total Debits  Current Balance")
    // put the labels on one line and the values on the next.
    let mut pending_columns: Vec<&'static str> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();

        if !pending_columns.is_empty() {
            let amounts: Vec<f64> = trimmed.split_whitespace().filter(|t| is_amount_token(t)).filter_map(parse_amount).collect();
            if amounts.len() >= pending_columns.len() {
                for (label, value) in pending_columns.iter().zip(amounts) {
                    match *label {
                        "beginning" => ledger.summary.beginning_balance.get_or_insert(value),
                        "credits" => ledger.summary.total_credits.get_or_insert(value),
                        "debits" => ledger.summary.total_debits.get_or_insert(value),
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
            continue;
        }

        capture_summary(&lower, trimmed, &mut ledger.summary);

        if lower.contains("daily balance") || lower.contains("daily ending balance") {
            *in_daily = true;
            last_txn = None;
            continue;
        }
        if let Some(k) = section_for(trimmed) {
            *section = Some(k);
            *in_daily = false;
            last_txn = None;
            continue;
        }
        if lower.starts_with("total ") || lower.starts_with("minimum balance") || lower.contains("continued on") {
            last_txn = None;
            continue;
        }

        let tokens: Vec<&str> = trimmed.split_whitespace().collect();

        if *in_daily {
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
            // Any other non-date line ends the daily balance block.
            if tokens.first().and_then(|t| parse_date_token(t)).is_none() && !lower.contains("date") {
                *in_daily = false;
            }
        }

        // Multi-column check tables: two or more (date, amount) pairs on one line, in either
        // "date check# amount" or "check# date amount" order.
        let date_idx: Vec<usize> = tokens.iter().enumerate().filter(|(_, t)| parse_date_token(t).is_some()).map(|(i, _)| i).collect();
        let amt_idx: Vec<usize> = tokens.iter().enumerate().filter(|(_, t)| is_amount_token(t)).map(|(i, _)| i).collect();
        if date_idx.len() >= 2 && date_idx.len() == amt_idx.len() && date_idx.iter().zip(&amt_idx).all(|(d, a)| d < a) {
            let mut prev_end = 0usize;
            for (k, (&d, &a)) in date_idx.iter().zip(&amt_idx).enumerate() {
                let mut desc: Vec<&str> = tokens[d + 1..a].to_vec();
                // A check number printed just before the date belongs to this entry.
                if d > prev_end && d >= 1 && k < date_idx.len() && tokens[d - 1].trim_end_matches('*').chars().all(|c| c.is_ascii_digit()) && !tokens[d - 1].is_empty() {
                    desc.insert(0, tokens[d - 1]);
                }
                let desc: Vec<&str> = desc.into_iter().filter(|t| *t != "*").collect();
                let label = if desc.len() == 1 && desc[0].trim_end_matches('*').chars().all(|c| c.is_ascii_digit()) {
                    format!("Check {}", desc[0].trim_end_matches('*'))
                } else {
                    desc.join(" ")
                };
                let id = ledger.transactions.len();
                let (date, day) = resolve_date(tokens[d], year_hint);
                ledger.transactions.push(Txn { id, date, day, kind: section.unwrap_or(Kind::Debit), amount: parse_amount(tokens[a]).unwrap_or(0.0).abs(), description: label, page });
                prev_end = a + 1;
            }
            last_txn = None;
            continue;
        }

        // Lone check entry "365989* 11/17 20,754.66".
        if tokens.len() == 3 && tokens[0].trim_end_matches('*').chars().all(|c| c.is_ascii_digit()) && parse_date_token(tokens[1]).is_some() && is_amount_token(tokens[2]) {
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[1], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind: Kind::Debit, amount: parse_amount(tokens[2]).unwrap_or(0.0).abs(), description: format!("Check {}", tokens[0].trim_end_matches('*')), page });
            last_txn = None;
            continue;
        }

        // Transaction line: date first, amount last.
        let starts_with_date = tokens.first().and_then(|t| parse_date_token(t)).is_some();
        let ends_with_amount = tokens.last().map(|t| is_amount_token(t)).unwrap_or(false);
        if starts_with_date && ends_with_amount && tokens.len() >= 2 {
            let amount = parse_amount(tokens[tokens.len() - 1]).unwrap_or(0.0).abs();
            // Statement summary rows also start with a date ("11/01/2025 Beginning Balance"); skip them.
            if lower.contains("beginning balance") || lower.contains("ending balance") || lower.contains("previous balance") {
                last_txn = None;
                continue;
            }
            let desc: String = tokens[1..tokens.len() - 1].join(" ");
            // Check tables print "check# date amount" pairs without a description; skip those.
            let kind = kind_from_words(&desc, *section);
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[0], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind, amount, description: desc, page });
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

fn capture_summary(lower: &str, line: &str, s: &mut Summary) {
    if s.beginning_balance.is_none() && (lower.contains("beginning balance") || lower.contains("previous balance")) {
        // Sunrise puts the values on the next line; Legends on the same line.
        s.beginning_balance = first_amount_after(line, &["beginning balance", "previous balance"]);
    }
    if lower.contains("ending balance") || lower.contains("current balance") {
        if let Some(v) = last_amount(line) {
            s.ending_balance = Some(v);
        }
    }
    if s.total_credits.is_none() && (lower.contains("deposits/other credits") || lower.contains("total credits") || lower.contains("total deposits")) && !lower.contains("---") {
        s.total_credits = last_amount(line);
    }
    if s.total_debits.is_none() && (lower.contains("checks/other debits") || lower.contains("total debits") || lower.contains("total withdrawals")) && !lower.contains("---") {
        s.total_debits = last_amount(line);
    }
    if s.days_in_period.is_none() && lower.contains("days in") {
        // "30 Days in Statement Period" or "Total Days In Statement Period ...: 31"
        let toks: Vec<&str> = line.split_whitespace().collect();
        let after_colon = line.rsplit(':').next().and_then(|t| t.trim().parse::<u32>().ok());
        let before = toks.iter().position(|t| t.eq_ignore_ascii_case("days")).and_then(|i| i.checked_sub(1)).and_then(|i| toks[i].parse::<u32>().ok());
        s.days_in_period = after_colon.or(before).filter(|d| (1..=366).contains(d));
    }
    if s.average_balance.is_none() && (lower.contains("average balance") || lower.contains("average ledger balance") || lower.contains("avg daily balance")) {
        s.average_balance = last_amount(line);
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
        ("total credits", "credits"), ("total deposits", "credits"),
        ("total debits", "debits"), ("total withdrawals", "debits"),
        ("current balance", "ending"), ("ending balance", "ending"), ("new balance", "ending"),
    ] {
        if let Some(p) = lower.find(needle) {
            found.push((p, label));
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

fn derive(ledger: &mut Ledger) {
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

/// Parse a whole statement set. `pages` are (page number, text) in reading order.
pub fn parse(pages: &[(usize, &str)]) -> Ledger {
    let mut ledger = Ledger::default();
    let texts: Vec<&str> = pages.iter().map(|(_, t)| *t).collect();
    let year = year_hint(&texts);
    let mut section = None;
    let mut in_daily = false;
    for (page, text) in pages {
        parse_page(text, *page, year, &mut ledger, &mut section, &mut in_daily);
    }
    derive(&mut ledger);
    ledger
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

    let days_in_period = ledger.summary.days_in_period.unwrap_or(30);
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
}
