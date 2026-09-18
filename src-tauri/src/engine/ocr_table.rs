//! Turn GLM-OCR "Table Recognition:" output (an HTML table) into the aligned text the
//! ledger parser reads, so scanned transaction tables keep every amount in its column.
//!
//! Plain "Text Recognition:" drops the amount cells of rows whose description wraps onto
//! a second line; the table task keeps them but emits `<table><tr><td>...` with the
//! occasional shifted or split cell. Rows whose cell count matches the header after
//! normalization are written with amounts right-aligned under a canonical header
//! ("Credits", "Debits", "Balance"); other rows are written flat, and the parser falls
//! back to words and running-balance arithmetic for those.

use super::ledger::{is_amount_token, parse_date_token};

const DATE_W: usize = 12;
const AMOUNT_W: usize = 20;

/// Cells of every `<tr>` in the first `<table>`, tags stripped and entities decoded.
fn rows(html: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for tr in html.split("<tr").skip(1) {
        let tr = tr.split("</tr>").next().unwrap_or("");
        let mut cells = Vec::new();
        for cell in tr.split(|c| c == '<').skip(1) {
            // cell looks like "td>text" or "/td>" or "th>text"
            if !(cell.starts_with("td") || cell.starts_with("th")) {
                continue;
            }
            let text = cell.split_once('>').map(|(_, t)| t).unwrap_or("");
            let text = text.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&").replace("&nbsp;", " ");
            cells.push(text.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        if !cells.is_empty() {
            out.push(cells);
        }
    }
    out
}

/// Canonical label for a header cell, or None for date/check/description columns.
fn amount_label(cell: &str) -> Option<&'static str> {
    let l = cell.to_ascii_lowercase();
    if l.contains("balance") {
        Some("Balance")
    } else if l.contains("deposit") || l.contains("credit") || l.contains("addition") {
        Some("Credits")
    } else if l.contains("withdrawal") || l.contains("debit") || l.contains("subtraction") || l.contains("payment") || l.contains("check") && l.contains("amount") {
        Some("Debits")
    } else if l.contains("amount") {
        Some("Debits")
    } else {
        None
    }
}

/// Convert the table to aligned text. None when the HTML has no header row with a date
/// column and an amount column, in which case the caller keeps the plain OCR text.
pub fn table_html_to_layout(html: &str) -> Option<String> {
    rows_to_layout(rows(html))
}

/// Rows of a Markdown pipe table ("| 1/5 | | Description | 68,729.64 | | |"), separator
/// rows dropped. GLM-OCR's plain text task emits these for some statement tables.
fn markdown_rows(block: &[&str]) -> Vec<Vec<String>> {
    block
        .iter()
        .filter(|l| !l.trim().trim_start_matches('|').trim().chars().all(|c| c == ':' || c == '-' || c == '|' || c == ' '))
        .map(|l| {
            let t = l.trim();
            let inner = t.strip_prefix('|').unwrap_or(t);
            let inner = inner.strip_suffix('|').unwrap_or(inner);
            inner.split('|').map(|c| c.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
        })
        .collect()
}

/// Replace every Markdown pipe table in plain OCR `text` with aligned layout lines the
/// ledger parser reads; text outside the tables is kept as is.
pub fn expand_markdown_tables(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_start().starts_with('|') {
            let start = i;
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                i += 1;
            }
            match rows_to_layout(markdown_rows(&lines[start..i])) {
                Some(layout) => out.push_str(&layout),
                None => {
                    // Not a transaction table: keep the cells as words.
                    for row in markdown_rows(&lines[start..i]) {
                        out.push_str(&row.join(" "));
                        out.push('\n');
                    }
                }
            }
            continue;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// Split `text` at the last space before `width` (or at `width` when there is none).
fn wrap_at(text: &str, width: usize) -> (&str, &str) {
    if text.len() <= width {
        return (text, "");
    }
    let cut = text[..width].rfind(' ').filter(|&i| i > 0).unwrap_or(width);
    (&text[..cut], text[cut..].trim_start())
}

fn rows_to_layout(rows: Vec<Vec<String>>) -> Option<String> {
    let header_idx = rows.iter().position(|r| r.iter().any(|c| c.eq_ignore_ascii_case("date") || c.to_ascii_lowercase().ends_with(" date")) && r.iter().any(|c| amount_label(c).is_some()))?;
    let header = &rows[header_idx];
    let labels: Vec<Option<&'static str>> = header.iter().map(|c| amount_label(c)).collect();
    let first_amount_col = labels.iter().position(|l| l.is_some())?;
    let amount_cols = header.len() - first_amount_col;
    let desc_w = rows[header_idx + 1..].iter().flat_map(|r| r.iter().skip(1).filter(|c| !is_amount_token(c)).map(|c| c.len())).max().unwrap_or(30).clamp(30, 90) + 2;

    let mut out = String::new();
    // Header: "Date" left, description placeholder, canonical labels right-aligned.
    let mut line = format!("{:<DATE_W$}{:<desc_w$}", "Date", "Description");
    for l in &labels[first_amount_col..] {
        line.push_str(&format!("{:>AMOUNT_W$}", l.unwrap_or("")));
    }
    out.push_str(line.trim_end());
    out.push('\n');

    for row in &rows[header_idx + 1..] {
        let mut cells = row.clone();
        // Leading empty cells and a split description shift the amounts one column right.
        while cells.len() > header.len() && cells.first().map(|c| c.is_empty()).unwrap_or(false) {
            cells.remove(0);
        }
        while cells.len() > header.len() {
            let Some(i) = (1..cells.len() - 1).find(|&i| !cells[i].is_empty() && !is_amount_token(&cells[i]) && !cells[i + 1].is_empty() && !is_amount_token(&cells[i + 1]) && parse_date_token(&cells[i]).is_none()) else { break };
            let merged = format!("{} {}", cells[i], cells[i + 1]);
            cells[i] = merged;
            cells.remove(i + 1);
        }
        // Text under an amount column is a split description: pull it left and pad the row.
        if cells.len() == header.len() {
            let mut i = first_amount_col;
            while i < cells.len() {
                if !cells[i].is_empty() && !is_amount_token(&cells[i]) {
                    let moved = cells.remove(i);
                    let desc_idx = first_amount_col - 1;
                    cells[desc_idx] = format!("{} {}", cells[desc_idx], moved).trim().to_string();
                    cells.push(String::new());
                } else {
                    i += 1;
                }
            }
        }
        let date = cells.first().map(|c| c.as_str()).unwrap_or("");
        let has_date = parse_date_token(date).is_some();
        let text_cells: Vec<&str> = cells.iter().skip(if has_date { 1 } else { 0 }).filter(|c| !c.is_empty() && !is_amount_token(c)).map(|c| c.as_str()).collect();
        let desc = text_cells.join(" ");
        let amounts: Vec<&String> = cells.iter().filter(|c| is_amount_token(c)).collect();

        if amounts.is_empty() {
            // Wrapped description or a note: continuation text for the previous line.
            if !desc.is_empty() {
                out.push_str(&format!("{:<DATE_W$}{desc}\n", ""));
            }
            continue;
        }
        if cells.len() == header.len() && (has_date || desc.to_ascii_lowercase().starts_with("total")) {
            // Trusted layout: each amount under its own column. A description longer than
            // the column would push its amount under the next label, so the tail wraps onto a
            // continuation line, as the bank prints it.
            let (head, tail) = wrap_at(&desc, desc_w - 2);
            let mut line = format!("{:<DATE_W$}{:<desc_w$}", if has_date { date } else { "" }, head);
            for c in &cells[first_amount_col..first_amount_col + amount_cols] {
                line.push_str(&format!("{:>AMOUNT_W$}", if is_amount_token(c) { c.as_str() } else { "" }));
            }
            out.push_str(line.trim_end());
            out.push('\n');
            if !tail.is_empty() {
                out.push_str(&format!("{:<DATE_W$}{tail}\n", ""));
            }
        } else {
            // Unknown column: flat line, kind from words and balance arithmetic.
            let amts: Vec<&str> = amounts.iter().map(|a| a.as_str()).collect();
            out.push_str(&format!("{} {} {}\n", if has_date { date } else { "" }, desc, amts.join(" ")).trim_start());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<table><thead><tr><th>Date</th><th>Check Number</th><th>Description</th><th>Deposits/Additions</th><th>Withdrawals/Subtractions</th><th>Ending daily balance</th></tr></thead><tbody>\
<tr><td></td><td>8/3</td><td></td><td>Mobile Deposit : Ref Number :521020865093</td><td>1,475.76</td><td></td><td></td></tr>\
<tr><td>8/3</td><td></td><td>Purchase authorized on 07/30 6840 Beverly Cente Los Angeles CA S306211748910223 Card 5292</td><td></td><td>1.00</td><td></td></tr>\
<tr><td>8/3</td><td></td><td>Blueshieldca Bill Pay 260731</td><td>1501 Sue Halevy</td><td></td><td>199.70</td></tr>\
<tr><td>8/3</td><td>279</td><td>Check</td><td></td><td>150.00</td><td>1,167.01</td></tr>\
<tr><td>8/4</td><td></td><td>Zelle From Philipp David on 08/04 Ref # Wfct22Hgtc75</td><td>1,500.00</td><td></td><td>2,667.01</td></tr>\
</tbody></table>";

    #[test]
    fn converts_rows_and_repairs_shifts() {
        let text = table_html_to_layout(SAMPLE).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("Date"));
        assert!(lines[0].contains("Credits") && lines[0].contains("Debits") && lines[0].contains("Balance"));
        // Leading empty cell removed: the deposit sits under Credits.
        let credits_end = lines[0].find("Credits").unwrap() + "Credits".len();
        let dep = lines[1];
        assert!(dep.starts_with("8/3"));
        assert_eq!(dep.find("1,475.76").unwrap() + "1,475.76".len(), credits_end);
        // Split description merged: 199.70 lands under Debits, not Balance.
        let debits_end = lines[0].find("Debits").unwrap() + "Debits".len();
        let bill = lines[3];
        assert!(bill.contains("Blueshieldca Bill Pay 260731 1501 Sue Halevy"));
        assert_eq!(bill.find("199.70").unwrap() + "199.70".len(), debits_end);
        // Full ledger parse: 2 credits, 3 debits, running balances kept.
        let ledger = crate::engine::ledger::parse(&[(1, &text)]);
        let credits: Vec<_> = ledger.transactions.iter().filter(|t| t.kind == crate::engine::ledger::Kind::Credit).collect();
        assert_eq!(credits.len(), 2, "{:?}", ledger.transactions);
        assert_eq!(ledger.transactions.len(), 5);
        assert!((ledger.parsed_debit_total - 350.70).abs() < 0.001, "{}", ledger.parsed_debit_total);
    }

    #[test]
    fn markdown_tables_are_expanded_into_layout() {
        let text = "Transaction history\n\n| Date | Check Number | Description | Deposits/ Credits | Withdrawals/ Debits | Ending daily balance |\n| :--- | :--- | :--- | :--- | :--- | :--- |\n| 1/5 | | Etransfer IN Branch | 68,729.64 | | |\n| 1/5 | | Online Transfer to Ward | | 24,925.00 | 43,804.64 |\n\nEnding balance on 1/31: 34,039.66\n";
        let out = expand_markdown_tables(text);
        assert!(out.contains("Ending balance on 1/31"));
        let l = crate::engine::ledger::parse(&[(1, &out)]);
        assert_eq!(l.transactions.len(), 2, "{out}");
        assert_eq!(l.transactions[0].kind, crate::engine::ledger::Kind::Credit);
        assert_eq!(l.transactions[1].kind, crate::engine::ledger::Kind::Debit);
        assert_eq!(l.daily_balances.len(), 1);
    }

    #[test]
    fn long_descriptions_wrap_instead_of_pushing_amounts_right() {
        let html = "<table><tr><td>Date</td><td>Check Number</td><td>Description</td><td>Deposits/ Credits</td><td>Withdrawals/ Debits</td><td>Ending daily balance</td></tr>\
<tr><td>2/10</td><td></td><td>WT S0660413Dcc301 Morgan Stanley A /Org=Msl FBO Julie Beth Kaplan,Tod Subj Srf# S0660413Dcc301 Trn#260210168728 Rfb#</td><td>6,000.00</td><td></td><td></td></tr>\
<tr><td>2/10</td><td></td><td>Recurring Payment authorized on 02/09 Cci*Constant-Conta 855-2295506 MA S586040308634033 Card 4336</td><td></td><td>201.43</td><td>5,000.00</td></tr></table>";
        let text = table_html_to_layout(html).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let credits_end = lines[0].find("Credits").unwrap() + "Credits".len();
        assert_eq!(lines[1].find("6,000.00").unwrap() + "6,000.00".len(), credits_end, "{text}");
        assert!(lines[2].trim().starts_with("S0660413Dcc301") || lines[2].trim().starts_with("Trn#"), "{text}");
        let l = crate::engine::ledger::parse(&[(1, &text)]);
        assert_eq!(l.transactions.len(), 2, "{text}");
        assert_eq!(l.transactions[0].kind, crate::engine::ledger::Kind::Credit);
        assert!(l.transactions[0].description.contains("Rfb#"), "{:?}", l.transactions[0]);
    }
}
