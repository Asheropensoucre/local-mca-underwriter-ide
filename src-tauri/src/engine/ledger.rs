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
    let decimals_ok = groups.last().map(|g| g.len() == 2).unwrap_or(false) && (t.contains('.'));
    // Every inner group is a thousands group of exactly three digits.
    let inner_ok = groups.len() < 3 || groups[1..groups.len() - 1].iter().all(|g| g.len() == 3);
    // Period-separated thousands only when no comma is present (otherwise "1.5.00" is noise).
    let periods = t.matches('.').count();
    let period_thousands_ok = periods == 1 || (!t.contains(',') && groups[0].len() <= 3 && inner_ok);
    decimals_ok && inner_ok && period_thousands_ok
}

const MONTHS: &[&str] = &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// pdftotext sometimes breaks an amount at the decimal point ("20. 00", "1,860. 70").
/// Rejoin it and move the space after the cents so column offsets are kept.
fn join_split_amounts(line: &str) -> String {
    if !line.is_ascii() {
        return line.to_string();
    }
    let b = line.as_bytes();
    let numberish = |c: u8| c.is_ascii_digit() || c == b'.' || c == b',' || c == b'/';
    // A space inside a number: "20. 00" (digit '.' ' ' digit digit, then no digit), or
    // "-1 ,100.00" and "11 /21 /22" (digit ' ' [,/] digit).
    let split_at = |i: usize| -> bool {
        let digit = |k: usize| b.get(k).map(|c| c.is_ascii_digit()).unwrap_or(false);
        if b.get(i) != Some(&b' ') || i == 0 {
            return false;
        }
        let decimal = i >= 2 && b[i - 1] == b'.' && b[i - 2].is_ascii_digit() && digit(i + 1) && digit(i + 2) && !digit(i + 3);
        // "3,051 .38": the space before the decimal point.
        let before_point = b[i - 1].is_ascii_digit() && b.get(i + 1) == Some(&b'.') && digit(i + 2) && digit(i + 3) && !digit(i + 4);
        let group = b[i - 1].is_ascii_digit() && matches!(b.get(i + 1), Some(b',') | Some(b'/')) && digit(i + 2);
        // "1, 000.00": the space after the thousands comma, three digits following.
        let after_comma = i >= 2 && b[i - 1] == b',' && b[i - 2].is_ascii_digit() && digit(i + 1) && digit(i + 2) && digit(i + 3) && !digit(i + 4);
        decimal || before_point || group || after_comma
    };
    let mut out = String::with_capacity(line.len());
    let mut owed = 0; // spaces removed from inside a number, re-added after it (keeps the width)
    let mut i = 0;
    while i < b.len() {
        if split_at(i) {
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
        let dates = lines[i..].iter().take_while(|l| lone(l, &|t| parse_date_token(t).is_some())).count();
        if dates >= 1 {
            let amounts = lines[i + dates..].iter().take_while(|l| lone(l, &|t| is_amount_token(t))).count();
            if amounts == dates {
                for k in 0..dates {
                    let date = lines[i + k];
                    out.push(format!("{}{:>12}", date.trim_end(), lines[i + dates + k].trim()));
                }
                i += dates * 2;
                continue;
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
fn strip_margin_junk(line: &str) -> String {
    let mut it = line.split_whitespace();
    let (Some(first), Some(second), Some(_)) = (it.next(), it.next(), it.next()) else { return line.to_string() };
    let all_digits = first.chars().all(|c| c.is_ascii_digit());
    let junk = first.len() <= 4 && (!all_digits || first.len() <= 2) && parse_date_token(first).is_none() && parse_date_token(second).is_some() && !is_amount_token(first);
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
    let trimmed_after = after.strip_prefix(&" ".repeat(extra)).unwrap_or(after);
    format!("{}{}{}", &line[..indent], padded, trimmed_after)
}

/// A summary label that some banks print as a dated row of the activity table.
fn is_balance_label(lower: &str) -> bool {
    ["beginning balance", "ending balance", "opening balance", "closing balance", "balance forward", "previous balance"].iter().any(|k| lower.contains(k))
}

/// Remove lone "^" and "*" tokens (footnote marks on check rows), keeping the spacing of
/// everything else so aligned pages keep their columns.
fn drop_footnote_marks(line: &str) -> String {
    if !line.contains(" ^") && !line.contains(" *") {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    chars.iter().enumerate().map(|(i, &c)| {
        let lone = (c == '^' || c == '*') && (i == 0 || chars[i - 1] == ' ') && (i + 1 == chars.len() || chars[i + 1] == ' ');
        if lone { ' ' } else { c }
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
fn section_for(line: &str) -> Option<Kind> {
    let mut l = line.to_ascii_lowercase();
    // Fifth Third prints the section total on the header line: "Deposits / Credits
    // 46 items totaling $137,498.52". Only the words before the count name the section.
    if let Some(p) = l.find(" items total") {
        let head = l[..p].trim_end();
        let cut = head.rfind(' ').filter(|_| head.split_whitespace().last().map(|t| t.chars().all(|c| c.is_ascii_digit())).unwrap_or(false));
        l = cut.map(|c| head[..c].to_string()).unwrap_or_else(|| head.to_string());
    }
    // Court OCR smears headers ("!OTHER WITHDRAWALS, FEES & C H A R G E S - I - - -"):
    // single-character tokens are noise there.
    let toks: Vec<&str> = l.split_whitespace().filter(|t| t.chars().count() > 1 || t.chars().all(|c| c.is_ascii_digit())).collect();
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
    if l.contains("deposit") || l.contains("credit") || l.contains("additions") {
        return Some(Kind::Credit);
    }
    if l.contains("debit") || l.contains("withdrawal") || l.contains("checks") || l.contains("fees") || l.contains("payments") || l.contains("subtractions") {
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
    const DEBIT_WORDS: &[&str] = &["withdrawal", " debit", "purchase", " fee", "charge", "check ", "payment to", "zelle to", "transfer to", "payment authorized", "pmt to", "bill pay", "wire out", "outgoing wire"];
    const CREDIT_WORDS: &[&str] = &["deposit", " credit", "zelle from", "transfer from", "pmt from", "payment from", "wire in", "incoming wire", "refund", "cashback", "cash back"];
    // Phrases that contain a debit word but are credits: Wells "ATM Check Deposit",
    // "Purchase Return authorized" on a flat OCR page; Wells incoming wires name the
    // originator ("WT ... Morgan Stanley /Org=..."), outgoing ones the beneficiary (/Bnf=).
    const CREDIT_PHRASES: &[&str] = &["check deposit", "purchase return", "/org="];
    if CREDIT_PHRASES.iter().any(|w| l.contains(w)) {
        return (Kind::Credit, true);
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
    open_group: Vec<(usize, bool)>,
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
    }
}

/// Parse one page. `year_hint` fills in years for MM/DD dates.
fn parse_page(text: &str, page: usize, year_hint: Option<i32>, ledger: &mut Ledger, st: &mut State) {
    let text = &zip_stacked_cells(text);
    let mut columns: Option<Columns> = None;
    let flat = is_flat(text);
    let mut pending_header: Option<Columns> = None;
    let mut last_txn: Option<usize> = None;
    // Dated OCR line waiting for its amount on a following line (date token, description).
    let mut pending_flat: Option<(String, String)> = None;
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
        let raw = raw.replace('_', " ").replace(['\u{2013}', '\u{2014}', '\u{2212}'], "-").replace('|', " ");
        let raw = drop_footnote_marks(&raw);
        // Month names first, so a bullet "- Oct 02: ..." reads as "- 10/02: ..." for unbullet.
        let normalized = drop_second_date(&strip_margin_junk(&join_split_amounts(&unbullet(&normalize_month_dates(&raw)))));
        // KeyBank writes "6-3" once its dashed dates are established ("Beginning balance
        // 5-31-24", "6-10"): a one-digit-by-one-digit dash at the start of a line is a date
        // then, never a range. Padded to "06-03" so the token rules apply.
        let normalized = if st.dashed_dates { pad_short_dashed_date(&normalized) } else { normalized };
        let stripped = strip_margin_barcode(normalized.trim_end());
        let line: &str = stripped.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if !st.dashed_dates && trimmed.split_whitespace().any(|t| t.contains('-') && t.len() >= 5 && parse_date_token(t).is_some()) {
            st.dashed_dates = true;
        }

        // A lone "Beginning Balance" label only takes a line that is nothing but the
        // amount; anything else is parsed as usual.
        if pending_columns.len() == 1 && trimmed.split_whitespace().count() != 1 {
            pending_columns.clear();
        }
        if !pending_columns.is_empty() {
            let amounts: Vec<f64> = trimmed.split_whitespace().filter(|t| is_amount_token(t)).filter_map(parse_amount).collect();
            // Second header line ("balance  other credits  other debits  balance"): keep waiting.
            if amounts.is_empty() && trimmed.split_whitespace().count() <= 8 && ["balance", "credits", "debits", "other"].iter().any(|w| lower.contains(w)) {
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

        capture_summary(&lower, trimmed, &mut ledger.summary, page);

        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
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
            last_txn = None;
            continue;
        }
        // (Right after a transaction the same words are a description continuation:
        // TD prints "CREDIT FUNDING," over "OVERDRAFT PROTECTION FROM".)
        // ("Images" / "Check Images" heads UMB's check image pages, whose captions repeat
        // the checks; it counts even right after a transaction.)
        let images_heading = tokens.len() <= 2 && (lower == "images" || lower == "check images" || lower == "deposit images") || lower.starts_with("image number ");
        if images_heading {
            st.images_page = Some(page);
        }
        if images_heading || tokens.len() <= 6 && last_txn.is_none() && INFORMATIONAL_HEADERS.iter().any(|h| lower.starts_with(h)) {
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
        // Daily balance tables: a "Daily Balance" heading, or a header repeating "Date ...
        // balance" for several columns ("Date  Ledger balance  Date  Ledger balance").
        let has_amount = tokens.iter().any(|t| is_amount_token(t));
        let repeated_date_balance_header = !has_amount && lower.matches("date").count() >= 2 && lower.contains("balance") && tokens.len() <= 12;
        // A transaction table header naming a credit or debit column ("... Ending daily balance") is not a daily balance block.
        let hdr = Columns::labels(line);
        let names_txn_columns = hdr.credit.is_some() || hdr.debit.is_some();
        // Synovus heads its daily table "Balance Summary" over "Date Amount Date Amount".
        let balance_summary_heading = lower.starts_with("balance summary") && tokens.len() <= 3;
        // Court OCR breaks the heading's letters apart ("I DAIL y ENDING BALANCE I"): the
        // spaceless form still reads.
        let squashed: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        let smeared_daily = !has_amount && tokens.len() <= 8 && (squashed.contains("dailyendingbalance") || squashed.contains("dailybalance") || squashed.contains("dailyledgerbalance"));
        if !names_txn_columns && (lower.contains("daily balance") || lower.contains("daily ending balance") || lower.contains("daily ledger balance") || repeated_date_balance_header || balance_summary_heading || smeared_daily) {
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
            }
        }

        if let Some(k) = section_for(trimmed) {
            st.enter_table(trimmed);
            st.section = Some(k);
            st.section_page = Some(page);
            st.in_daily = false;
            last_txn = None;
            continue;
        }
        // Long check-table titles ("Summary of checks written (checks listed are also
        // displayed in the preceding Transaction history)") start a new listing too.
        if !has_amount && lower.contains("check") && (lower.starts_with("checks paid") || (lower.contains("summary of") || lower.contains("checks paid") || lower.contains("checks cleared") || lower.contains("checks written") || lower.contains("checks posted")) && tokens.len() <= 16) {
            st.enter_table(trimmed);
            st.section = Some(Kind::Debit);
            st.section_page = Some(page);
            st.in_daily = false;
            columns = None; // check rows are not under the transaction table's columns
            last_txn = None;
            continue;
        }
        if lower.starts_with("total") || lower.starts_with("subtotal") || lower.starts_with("minimum balance") || lower.contains("continued on") {
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
            let aligned = !flat && line.contains("   ");
            // On an aligned line only the trailing run of amounts is in the columns; an amount
            // inside the description ("ACH Pmt ... $2,300.00 Usd, ID: E9F3E6   1,863.00") is text.
            let mut spans: Vec<(usize, &str)> = amount_spans(line);
            if aligned {
                let trailing = tokens.iter().rev().take_while(|t| is_amount_token(t)).count();
                spans = spans.split_off(spans.len().saturating_sub(trailing));
            }
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
                    let (kind, strong) = st.row_kind(page, &desc);
                    let desc_end = line.find(tokens[amount_idx]).unwrap_or(line.len());
                    // "11/01/2025 Beginning Balance $323.01" in the activity table: a
                    // balance row, not a transaction (its running balance still counts).
                    txn = if is_balance_label(&desc.to_ascii_lowercase()) { None } else { parse_amount(tokens[amount_idx]).map(|v| (v.abs(), kind, strong, desc_end)) };
                }
                let (date, day) = resolve_date(tokens[0], year_hint);
                if let Some((amount, kind, strong, desc_end)) = txn {
                    let mut desc: String = line[..desc_end].split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
                    if desc.is_empty() {
                        if let Some(lead) = lead_desc.take() {
                            desc = lead;
                        }
                    }
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
        if number_amount_date && (date_idx.len() >= 2 || st.section == Some(Kind::Debit)) {
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
        if no_star.len() >= 4 && no_star[2].len() >= 9 && no_star[2].chars().all(|c| c.is_ascii_digit()) && is_amount_token(no_star[3]) && !check_no(no_star[0]).is_empty() && check_no(no_star[0]).chars().all(|c| c.is_ascii_digit()) && parse_date_token(no_star[1]).is_some() {
            no_star = vec![no_star[0], no_star[1], no_star[3]];
        }
        // UMB: "129  Mar 04  1,500.00  00081094018", the reference after the amount (one
        // or two digit groups); a further amount would make it a two-column line, left alone.
        if no_star.len() >= 4 && no_star.len() <= 5 && is_amount_token(no_star[2]) && no_star[3..].iter().all(|t| t.chars().all(|c| c.is_ascii_digit())) && no_star[3..].iter().map(|t| t.len()).sum::<usize>() >= 9 && !check_no(no_star[0]).is_empty() && check_no(no_star[0]).chars().all(|c| c.is_ascii_digit()) && parse_date_token(no_star[1]).is_some() {
            no_star = vec![no_star[0], no_star[1], no_star[2]];
        }
        if no_star.len() == 3 && !check_no(no_star[0]).is_empty() && check_no(no_star[0]).chars().all(|c| c.is_ascii_digit()) && parse_date_token(no_star[1]).is_some() && is_amount_token(no_star[2]) {
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(no_star[1], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind: Kind::Debit, amount: parse_amount(no_star[2]).unwrap_or(0.0).abs(), description: format!("Check {}", check_no(no_star[0])), page, table: st.table });
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
        // A headerless flat list whose rows end in amount then running balance (an OCR
        // page retold as bullets: "10/02 External Withdrawal ... 1,651.07 14,478.08"). The
        // second figure is a balance when it chains from the previous balance or into the
        // next row's; the change's sign then decides the kind.
        let trailing_amounts = tokens.iter().rev().take_while(|t| is_amount_token(t)).count();
        if columns.is_none() && starts_with_date && trailing_amounts == 2 && tokens.len() >= 4 && !summary_row && !st.in_daily {
            let n = tokens.len();
            let (amount, balance) = (parse_amount(tokens[n - 2]).map(f64::abs), parse_amount(tokens[n - 1]));
            if let (Some(amount), Some(balance)) = (amount, balance) {
                let near = |x: f64, y: f64| (x - y).abs() < 0.005;
                let from_prev = st.last_balance.map(|p| if near(p + amount, balance) { Some(Kind::Credit) } else if near(p - amount, balance) { Some(Kind::Debit) } else { None });
                // (amount, balance) of a later row in the same shape, if it has one.
                let row_at = |k: usize| -> Option<(f64, f64)> {
                    let next = join_split_amounts(&unbullet(&normalize_month_dates(&raw_lines.get(k)?.replace('|', " "))));
                    let nt: Vec<&str> = next.split_whitespace().collect();
                    let m = nt.len();
                    if m >= 4 && parse_date_token(nt[0]).is_some() && is_amount_token(nt[m - 1]) && is_amount_token(nt[m - 2]) {
                        parse_amount(nt[m - 2]).map(f64::abs).zip(parse_amount(nt[m - 1]))
                    } else {
                        None
                    }
                };
                let chains = |b: f64, row: Option<(f64, f64)>| row.map(|(a2, b2)| near(b + a2, b2) || near(b - a2, b2)).unwrap_or(false);
                let into_next = chains(balance, row_at(line_no + 1));
                // One misread balance must not break the list: the two rows after this one
                // chaining to each other is enough to keep the shape.
                let shape_holds = row_at(line_no + 1).map(|(_, b2)| chains(b2, row_at(line_no + 2))).unwrap_or(false);
                if from_prev.flatten().is_some() || (from_prev.flatten().is_none() && (into_next || shape_holds)) {
                    let desc = tokens[1..n - 2].join(" ");
                    let kind = from_prev.flatten().unwrap_or_else(|| st.row_kind(page, &desc).0);
                    let id = ledger.transactions.len();
                    let (date, day) = resolve_date(tokens[0], year_hint);
                    ledger.transactions.push(Txn { id, date: date.clone(), day, kind, amount, description: desc, page, table: st.table });
                    ledger.daily_balances.push(DailyBalance { date, balance });
                    st.last_balance = Some(balance);
                    last_txn = Some(id);
                    continue;
                }
            }
        }
        if starts_with_date && !ends_with_amount && tokens.len() >= 3 && amount_positions.len() == 1 && !summary_row {
            let a = amount_positions[0];
            let rest: Vec<&str> = tokens[1..].iter().enumerate().filter(|(i, t)| *i + 1 != a && !(t.len() >= 9 && t.chars().all(|c| c.is_ascii_digit()))).map(|(_, t)| *t).collect();
            let desc = if rest.len() == 1 && rest[0].len() <= 7 && rest[0].chars().all(|c| c.is_ascii_digit()) { format!("Check {}", rest[0]) } else { rest.join(" ") };
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[0], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind: st.row_kind(page, &desc).0, amount: parse_amount(tokens[a]).unwrap_or(0.0).abs(), description: desc, page, table: st.table });
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
            // "03/14 1008 212.26": a single check-table pair is a paid check.
            let bare_check = tokens.len() == 3 && !check_no(tokens[1]).is_empty() && check_no(tokens[1]).len() <= 7 && check_no(tokens[1]).chars().all(|c| c.is_ascii_digit());
            let (desc, kind) = if bare_check {
                (format!("Check {}", check_no(tokens[1])), Kind::Debit)
            } else {
                let kind = st.row_kind(page, &desc).0;
                (desc, kind)
            };
            let id = ledger.transactions.len();
            let (date, day) = resolve_date(tokens[0], year_hint);
            ledger.transactions.push(Txn { id, date, day, kind, amount, description: desc, page, table: st.table });
            last_txn = if bare_check { None } else { Some(id) };
            continue;
        }

        // OCR of a wrapped row: "03/04 CCD DEBIT, INTUIT ... BILL_PAY VRA CLEANING SE" then
        // "3,680.00" on the next line. Hold the dated line and complete it when a lone
        // amount follows (text lines in between extend the description).
        if flat && starts_with_date && !tokens.iter().any(|t| is_amount_token(t)) && tokens.len() >= 2 && !summary_row {
            pending_flat = Some((tokens[0].to_string(), tokens[1..].join(" ")));
            last_txn = None;
            continue;
        }
        if let Some((date_tok, desc)) = pending_flat.take() {
            // A lone amount, or the rest of the description ending with the amount
            // (TD: "RESTAURANT DEPOT ALEXANDRIA * VA 142.29").
            let ends_with_amount = !starts_with_date && tokens.len() <= 12 && tokens.last().map(|t| is_amount_token(t)).unwrap_or(false) && tokens[..tokens.len() - 1].iter().all(|t| !is_amount_token(t));
            if ends_with_amount {
                let amount = parse_amount(tokens[tokens.len() - 1]).unwrap_or(0.0).abs();
                let desc = if tokens.len() == 1 { desc } else { format!("{desc} {}", tokens[..tokens.len() - 1].join(" ")) };
                let id = ledger.transactions.len();
                let (date, day) = resolve_date(&date_tok, year_hint);
                ledger.transactions.push(Txn { id, date, day, kind: st.row_kind(page, &desc).0, amount, description: desc, page, table: st.table });
                last_txn = Some(id);
                continue;
            }
            if !starts_with_date && !tokens.iter().any(|t| is_amount_token(t)) && tokens.len() <= 12 {
                pending_flat = Some((date_tok, format!("{desc} {trimmed}")));
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
            let boilerplate = lower.contains("member fdic") || lower.contains("page ") && lower.contains(" of ") || lower.starts_with("pg ");
            if !starts_with_date && !has_amount && tokens.len() <= 12 && !footer_artifact && !boilerplate {
                let t = &mut ledger.transactions[id];
                t.description.push(' ');
                t.description.push_str(trimmed);
                continue;
            }
            // OCR sometimes drops the dates of the lower rows of a page ("PAYMENT Greystone
            // Power 7904 VitalPharmaceuticals 531.28" under dated rows). Inside a sectioned
            // list, text ending in a single amount right after a complete row is the next
            // row, dated like the one before it.
            let single_trailing_amount = tokens.len() >= 2 && tokens.len() <= 14 && is_amount_token(tokens[tokens.len() - 1]) && tokens[..tokens.len() - 1].iter().all(|t| !is_amount_token(t));
            let summary_like = lower.contains("total") || lower.contains("balance");
            if flat && !starts_with_date && single_trailing_amount && !summary_like && st.section.is_some() && ledger.transactions[id].page == page {
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
    if lower.contains("beginning balance") || lower.contains("previous balance") || lower.contains("opening ledger balance") || lower.contains("opening balance") || lower.starts_with("balance forward") || lower.contains("balance last statement") {
        // Sunrise puts the values on the next line; Legends on the same line; Frost says
        // "BALANCE LAST STATEMENT".
        let v = first_amount_after(line, &["beginning balance", "previous balance", "opening ledger balance", "opening balance", "balance forward", "balance last statement"]);
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
        if v.is_some() && (lower.trim_start().starts_with("beginning balance") || lower.trim_start().starts_with("balance forward")) && !s.in_summary_block && s.debit_parts.is_empty() && s.debit_parts_unsigned.is_empty() && s.credit_parts.is_empty() {
            s.in_summary_block = true;
            s.summary_lines = 0;
        }
    }
    let ntok = lower.split_whitespace().count();
    // Summary block: debit categories are the negative figures between the "summary"
    // heading and the ending balance.
    if lower.contains("summary") && ntok <= 12 && last_amount(line).is_none() && !lower.contains("fee") {
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
        if first_is_date && ntok >= 3 || section_for(line).is_some() || s.summary_lines > 25 {
            s.in_summary_block = false;
        }
        // The category value is the first amount on the line; U.S. Bank prints unrelated
        // figures to its right ("Other Withdrawals 962.49- Interest Paid this Year $0.62").
        // First State prints the credit marker as its own token ("2,796.37 +").
        if let Some(a) = toks.iter().position(|t| is_amount_token(t)).filter(|_| s.in_summary_block) {
            let mut value = toks[a].to_string();
            if toks.get(a + 1).map(|t| *t == "+" || *t == "-").unwrap_or(false) {
                value.push_str(toks[a + 1]);
            }
            let last = value.as_str();
            let prev_minus = a >= 1 && toks[a - 1] == "-";
            let label: String = toks[..a].join(" ").to_ascii_lowercase();
            // PNC prints credit and debit categories side by side ("ACH Credits 92
            // 3,199,536.68   ACH Debits 135 3,412,040.00"): such lines are not categories.
            let credit_word = |t: &str| t.contains("deposit") || t.contains("credit") || t.contains("addition");
            let debit_word = |t: &str| t.contains("check") || t.contains("payment") || t.contains("withdrawal") || t.contains("debit") || t.contains("charge") || t.contains("fee") || t.contains("card activity") || t.contains("subtraction");
            let two_columns = credit_word(&lower) && debit_word(&lower);
            if a >= 1 && a <= 8 && !two_columns && !label.contains("balance") && !label.contains("interest") && !label.contains("days") {
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
    // TD Bank: "Statement Balance as of 01/18 ... 5,480.39" then "... as of 02/17 ... 50.00".
    if lower.contains("statement balance as of") {
        if s.beginning_balance.is_none() {
            s.beginning_balance = last_amount(line);
        } else if s.ending_balance.is_none() {
            s.ending_balance = last_amount(line);
        }
    }
    const ENDING_KEYS: &[&str] = &["ending balance", "current balance", "new balance", "ending ledger balance", "closing balance", "balance this statement"];
    if s.ending_balance.is_none() && ENDING_KEYS.iter().any(|k| lower.contains(k)) {
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
    // Fifth Third's "Service Charge withdrawn on 06/10/26 $164.00" is already one of the
    // listed withdrawals, not a figure to add.
    if same_block && s.fees_total.is_none() && !lower.contains("withdrawn on") && (lower.starts_with("service fees") || lower.starts_with("service charge") || lower.starts_with("- service charge") || lower.starts_with("analysis or maintenance fee")) {
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
    // Character offsets where the second, third, ... column start; empty outside a block.
    let mut splits: Vec<usize> = Vec::new();
    let mut cols: Vec<Vec<String>> = Vec::new();
    // Header starts with "Date": every column begins with a date token, which is a
    // safer cut than the whitespace gap when the columns nearly touch.
    let mut date_first = false;
    let mut check_first = false;
    let flush = |out: &mut String, cols: &mut Vec<Vec<String>>| {
        for l in cols.iter_mut().flat_map(|c| c.drain(..)) {
            out.push_str(&l);
            out.push('\n');
        }
    };
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let toks: Vec<&str> = line.split_whitespace().collect();
        // A header naming date/amount/description twice or more. Each further column
        // starts at the next occurrence of whichever word repeats ("Description  Date
        // Amount  Description"; First State prints "Date Type Amount" three times).
        let repeated = ["date", "description", "amount"].into_iter().find(|w| lower.matches(w).count() >= 2);
        let is_header = toks.len() <= 12 && !toks.iter().any(|t| is_amount_token(t)) && repeated.is_some() && lower.contains("date") && (lower.contains("amount") || lower.contains("serial")) && !lower.contains("balance");
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
        let section_title = !has_date_or_amount && !toks.is_empty() && (line.trim_start().starts_with('•') || line.trim_start().starts_with('*') || section_for(line).is_some() || lower.contains("balance") || lower.contains("summary"));
        if !splits.is_empty() && section_title {
            flush(&mut out, &mut cols);
            splits.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if splits.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
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
                    None => break,
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
    let unique = drop_duplicate_pages(pages);
    let split = split_sub_accounts(&unique);
    let forced: Vec<bool> = split.iter().map(|(_, _, sub)| *sub).collect();
    let pages: &[(usize, &str)] = &split.iter().map(|(p, t, _)| (*p, t.as_str())).collect::<Vec<_>>();
    let segments = segment_statements(pages, &forced);
    if segments.len() <= 1 {
        let mut ledger = parse_one(pages);
        derive(&mut ledger);
        return ledger;
    }
    let mut combined = Ledger::default();
    for seg in &segments {
        let mut part = parse_one(seg);
        part.summary.parsed_credits = Some(part.transactions.iter().filter(|t| t.kind == Kind::Credit).map(|t| t.amount).sum());
        part.summary.parsed_debits = Some(part.transactions.iter().filter(|t| t.kind == Kind::Debit).map(|t| t.amount).sum());
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

/// A filing sometimes carries the same statement page twice (KeyBank petty cash account,
/// pages 5 and 7 of one exhibit). Pages whose text repeats an earlier page's, apart from
/// the court's own header line, are dropped so nothing counts twice.
fn drop_duplicate_pages<'a>(pages: &[(usize, &'a str)]) -> Vec<(usize, &'a str)> {
    let body = |t: &str| -> String {
        t.lines()
            .filter(|l| !(l.contains("Page ") && l.contains(" of ") && (l.contains("Case ") || l.contains("Doc"))))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for &(page, text) in pages {
        let b = body(text);
        // Only pages with rows can double a total; short pages (letterheads) stay.
        if b.split_whitespace().count() >= 40 && seen.contains(&b) {
            continue;
        }
        seen.push(b);
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
        let starts: Vec<usize> = (0..lines.len())
            .filter(|&i| heading(lines[i]) && lines[i + 1..(i + 3).min(lines.len())].iter().any(|l| l.to_ascii_lowercase().contains("beginning")) || split_heading(i))
            .collect();
        if starts.is_empty() {
            out.push((page, text.to_string(), false));
            continue;
        }
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
    // Two or more debit categories in the summary block add up to the debit total; a single
    // signed one ("Checks Paid 2,675.62-", U.S. Bank) is the total when nothing else names it.
    if ledger.summary.debit_parts.len() >= 2 || ledger.summary.debit_parts.len() == 1 && ledger.summary.total_debits.is_none() && ledger.summary.debit_parts_unsigned.is_empty() {
        ledger.summary.total_debits = Some(ledger.summary.debit_parts.iter().sum());
        ledger.summary.debits_key = "summary parts (checks and service fees included)";
    }
    // Unsigned categories fill in totals the statement never prints as one figure, and
    // two or more of them outrank a bare "Debits 42 28,151.29" line that is only one category.
    if !ledger.summary.credit_parts.is_empty() && (ledger.summary.total_credits.is_none() || ledger.summary.credit_parts.len() >= 2) {
        ledger.summary.total_credits = Some(ledger.summary.credit_parts.iter().sum());
    }
    if !ledger.summary.debit_parts_unsigned.is_empty() && (ledger.summary.total_debits.is_none() || ledger.summary.debit_parts_unsigned.len() >= 2) {
        ledger.summary.total_debits = Some(ledger.summary.debit_parts_unsigned.iter().sum());
        ledger.summary.debits_key = "summary parts (checks and service fees included)";
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
/// `forced[i]` marks a page that is a sub-account of its own (see `split_sub_accounts`)
/// and always starts a new statement.
fn segment_statements<'a>(pages: &[(usize, &'a str)], forced: &[bool]) -> Vec<Vec<(usize, &'a str)>> {
    let mut segments: Vec<Vec<(usize, &str)>> = Vec::new();
    let mut current: Vec<(usize, &str)> = Vec::new();
    let mut current_beginning: Option<f64> = None;
    let mut current_ending: Option<f64> = None;
    let mut current_account: Option<String> = None;
    let mut current_bank: Option<String> = None;
    for (i, &(page, text)) in pages.iter().enumerate() {
        let mut probe = Ledger::default();
        let mut st = State::default();
        parse_page(&unfold_two_columns(text), page, None, &mut probe, &mut st);
        let begins = probe.summary.beginning_balance;
        // A different bank named on a summary page is a new statement too (bundles of
        // several banks' statements, even when the beginning balance is garbled). The new
        // name must be mentioned at least twice and the current bank not at all, so a
        // transfer "to Bank of America" in a description does not split a statement.
        let votes = bank_votes(&[text]);
        let bank = votes.iter().max_by_key(|(_, n)| *n).map(|(name, _)| name.to_string());
        let lower = text.to_ascii_lowercase();
        // "Balance Summary" alone is the daily balance table, which sits on the last page
        // of a statement (Synovus), so it does not mark a first page.
        let summary_words = lower.contains("beginning balance") || lower.contains("previous balance") || lower.contains("account summary") || lower.contains("opening balance") || lower.contains("balance last statement");
        let bank_changes = summary_words && match (&bank, &current_bank) {
            (Some(b), Some(cur)) if b != cur => {
                let new_n = votes.iter().find(|(name, _)| *name == b).map(|(_, n)| *n).unwrap_or(0);
                let cur_n = votes.iter().find(|(name, _)| *name == cur).map(|(_, n)| *n).unwrap_or(0);
                new_n >= 2 && cur_n == 0
            }
            _ => false,
        };
        // Zero-balance sweep accounts (Wintrust) all begin at $0.00; there the account
        // number on the page with the beginning balance tells the statements apart.
        let account = probe.summary.account_last4.clone();
        let account_changes = begins.is_some() && matches!((&account, &current_account), (Some(a), Some(cur)) if a != cur);
        // A month with no activity ends where it began (KeyBank, $94.29 to $94.29), so the
        // next statement begins with the same figure: a page that opens with the balance an
        // earlier page closed at is a new statement too.
        let continues = matches!((begins, current_ending), (Some(b), Some(end)) if (b - end).abs() < 0.005) && !current.is_empty();
        let starts_new = forced.get(i).copied().unwrap_or(false) || bank_changes || account_changes || continues || match (begins, current_beginning) {
            (Some(b), Some(cur)) if (b - cur).abs() >= 0.005 => true,
            _ => false,
        };
        // (A current segment that has neither balance yet is a letterhead or cover page;
        // it joins the statement that starts here instead of standing alone.)
        if starts_new && !current.is_empty() && (current_beginning.is_some() || current_ending.is_some()) {
            segments.push(std::mem::take(&mut current));
            current_beginning = None;
            current_ending = None;
            current_account = None;
        }
        if begins.is_some() && current_beginning.is_none() {
            current_beginning = begins;
            current_account = account;
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
        checks_total: None,
        fees_total: None,
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
    let compatible = |a: &Txn, b: &Txn| -> bool {
        let (wa, wb) = (words(&a.description), words(&b.description));
        if wa.is_empty() || wb.is_empty() {
            return true; // a bare caption or check-image line repeats whatever it matches
        }
        wa.iter().any(|w| wb.contains(w)) || numbers(&a.description).iter().any(|n| numbers(&b.description).contains(n))
    };
    for i in 0..ledger.transactions.len() {
        let t = &ledger.transactions[i];
        let key = (t.date.clone(), t.kind, (t.amount * 100.0).round() as i64);
        let candidates = available.entry(key.clone()).or_default().clone();
        // Prefer an original that has not been repeated yet, so two real same-day items
        // each absorb their own repeat; otherwise an original may repeat again.
        let fits = |(table, j, _): &(usize, usize, Vec<usize>)| *table != t.table && compatible(&ledger.transactions[*j], t);
        let pos = candidates.iter().position(|c| c.2.is_empty() && fits(c)).or_else(|| candidates.iter().position(fits));
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
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        let toks: Vec<&str> = line.split_whitespace().collect();
        let single_amount_header = lower.contains("date") && lower.contains("amount") && toks.len() <= 8;
        if !toks.iter().any(|t| is_amount_token(t)) && (Columns::labels(line).is_complete(lower.contains("date")) || single_amount_header) {
            under_header = true;
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
        ("bank of america", "Bank of America"), ("pnc bank", "PNC"), ("pnc.com", "PNC"), ("td bank", "TD Bank"), ("u.s. bank", "U.S. Bank"), ("usbank.com", "U.S. Bank"),
        ("capital one", "Capital One"), ("citibank", "Citibank"), ("regions bank", "Regions"), ("fifth third", "Fifth Third"),
        ("huntington", "Huntington"), ("keybank", "KeyBank"), ("citizens bank", "Citizens"), ("m&t bank", "M&T Bank"), ("bmo", "BMO"),
        ("webster", "Webster Bank"), ("pinnacle", "Pinnacle Bank"), ("legends bank", "Legends Bank"), ("sunrise bank", "Sunrise Banks"),
        ("ally bank", "Ally"), ("frost bank", "Frost Bank"), ("frostbank", "Frost Bank"), ("box 1600 san antonio", "Frost Bank"), ("first citizens", "First Citizens"), ("comerica", "Comerica"),
        ("zions", "Zions"), ("synovus", "Synovus"), ("santander", "Santander"), ("navy federal", "Navy Federal"), ("bluevine", "Bluevine"),
        ("mercury", "Mercury"), ("novo", "Novo"), ("relay", "Relay"), ("axos", "Axos"), ("live oak", "Live Oak"), ("first horizon", "First Horizon"),
        ("flagstar", "Flagstar"), ("valley national", "Valley National"), ("valleynationalbank", "Valley National"), ("east west bank", "East West Bank"), ("cathay", "Cathay Bank"),
        ("customers bank", "Customers Bank"), ("signature bank", "Signature Bank"), ("silicon valley bank", "Silicon Valley Bank"),
        ("hancock whitney", "Hancock Whitney"), ("hancockwhitney", "Hancock Whitney"), ("mabrey", "Mabrey Bank"), ("wintrust", "Wintrust"),
        ("byline", "Byline Bank"), ("old national", "Old National"), ("associated bank", "Associated Bank"),
        ("first republic", "First Republic"), ("umpqua", "Umpqua"), ("banner bank", "Banner Bank"), ("amerant", "Amerant"), ("city national", "City National"),
        ("first state bank", "First State Bank"), ("bell bank", "Bell Bank"), ("choice bank", "Choice Bank"), ("alerus", "Alerus"), ("bremer", "Bremer Bank"), ("gate city", "Gate City Bank"),
        ("credit union", "Credit Union"),
    ];
    let mut votes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for text in texts.iter().take(3) {
        let lower = text.to_ascii_lowercase();
        // The bank's own name sits in the letterhead, the top of the page; other banks
        // show up in transaction descriptions ("Capital One Auto" deposits at a dealer).
        // Transaction rows near the top of a short page are not letterhead (a wire "from
        // Fifth Third" on a Synovus continuation page).
        let head: String = lower.lines().filter(|l| !l.trim().is_empty()).take(30).filter(|l| l.split_whitespace().next().and_then(parse_date_token).is_none()).collect::<Vec<_>>().join("\n");
        for (needle, name) in BANKS {
            let n = lower.matches(needle).count() + 5 * head.matches(needle).count();
            if n > 0 {
                *votes.entry(name).or_default() += n;
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
        // ("APR 17" inside the description is rewritten as a date by normalize_month_dates.)
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
}
