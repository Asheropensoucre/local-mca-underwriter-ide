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

/// Replace every `<table>...</table>` in plain OCR `text` with aligned layout lines; text
/// outside the tables is kept as is. The plain task answers with HTML when the image it
/// is given is a table on its own (a band cut from a page), so a banded reading is mostly
/// tables. A table without a header row, or one the layout rules do not fit, is written
/// as its rows' cells separated by spaces.
pub fn expand_html_tables(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<table") {
        out.push_str(&rest[..start]);
        let Some(end_rel) = rest[start..].find("</table>") else { out.push_str(&rest[start..]); rest = ""; break };
        let block = &rest[start..start + end_rel + "</table>".len()];
        let table_rows = split_side_by_side(rows(block));
        match rows_to_layout(table_rows.clone()) {
            Some(layout) => out.push_str(&layout),
            None => {
                for row in table_rows {
                    let line = row.join(" ");
                    if !line.trim().is_empty() {
                        out.push_str(line.trim());
                        out.push('\n');
                    }
                }
            }
        }
        rest = &rest[start + end_rel + "</table>".len()..];
    }
    out.push_str(rest);
    out
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
            let rows = split_side_by_side(markdown_rows(&lines[start..i]));
            match rows_to_layout(rows.clone()) {
                Some(layout) => out.push_str(&layout),
                None => {
                    // Not a transaction table: keep the cells as words.
                    for row in rows {
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

/// A table printed as two identical column sets side by side (TD's "Checks Paid": DATE,
/// SERIAL NO., AMOUNT twice) becomes one table twice as long: the left half of every row,
/// then the right halves, empty halves dropped. Any other table is returned as it came.
fn split_side_by_side(rows: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let header = rows.iter().find(|r| r.iter().any(|c| c.eq_ignore_ascii_case("date")));
    // Without a header row (GLM-OCR's HTML for a band holding only the table body), the
    // data rows say it themselves: an even number of cells with a date at the start of
    // each half in most rows.
    let n = match header {
        Some(h) => h.len(),
        None => rows.first().map(|r| r.len()).unwrap_or(0),
    };
    if n < 4 || n % 2 != 0 {
        return rows;
    }
    let half = n / 2;
    let same = match header {
        Some(h) => h[..half].iter().zip(&h[half..]).all(|(a, b)| a.eq_ignore_ascii_case(b)),
        None => {
            let dated = rows.iter().filter(|r| r.len() == n && parse_date_token(&r[0]).is_some() && (parse_date_token(&r[half]).is_some() || r[half..].iter().all(|c| c.is_empty()))).count();
            dated >= 2 && dated * 2 >= rows.len()
        }
    };
    if !same {
        return rows;
    }
    let header_idx = header.and_then(|h| rows.iter().position(|r| r == h));
    let mut out: Vec<Vec<String>> = rows[..header_idx.unwrap_or(0)].to_vec();
    if let Some(h) = header {
        out.push(h[..half].to_vec());
    }
    let header_idx = header_idx.map(|i| i + 1).unwrap_or(0);
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for row in &rows[header_idx..] {
        if row.len() != n {
            left.push(row.clone()); // a subtotal or a short row: kept in order
            continue;
        }
        if row[..half].iter().any(|c| !c.is_empty()) {
            left.push(row[..half].to_vec());
        }
        if row[half..].iter().any(|c| !c.is_empty()) {
            right.push(row[half..].to_vec());
        }
    }
    out.extend(left);
    out.extend(right);
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
    fn html_tables_in_a_banded_reading_are_expanded() {
        // A band holding TD's two-column check table: no header row, "Subtotal:" in the last row.
        let html = "<table border=\"1\"><tr><td>04/05</td><td>2250</td><td>300.00</td><td>04/15</td><td>10783</td><td>966.47</td></tr><tr><td>04/15</td><td>10782</td><td>1,340.29</td><td></td><td></td><td></td></tr><tr><td></td><td></td><td></td><td></td><td>Subtotal:</td><td>15,547.77</td></tr></table>";
        let text = expand_html_tables(html);
        let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split_whitespace().collect()).collect();
        assert_eq!(rows, vec![vec!["04/05", "2250", "300.00"], vec!["04/15", "10782", "1,340.29"], vec!["04/15", "10783", "966.47"], vec!["Subtotal:", "15,547.77"]], "{text}");
        // A Wells debits band: heading row, header row as <td>, rows with an empty first cell.
        let html = "Debits\n<table border=\"1\"><tr><td colspan=\"4\">Debits\nElectronic debits/bank debits</td></tr><tr><td>Effective date</td><td>Posted date</td><td>Amount</td><td>Transaction detail</td></tr><tr><td></td><td>05/03</td><td>14750.00</td><td>Online Transfer to Civitas Health Services xxxxx8749\nRef #lb0Bdwzdbr on 05/03/21</td></tr><tr><td></td><td>05/04</td><td>8600.00</td><td>Online Transfer to Civitas Health Services xxxxx8749</td></tr></table>";
        let text = expand_html_tables(html);
        let dated: Vec<&str> = text.lines().filter(|l| l.split_whitespace().next().and_then(parse_date_token).is_some()).collect();
        assert_eq!(dated.len(), 2, "{text}");
        assert!(dated[0].contains("14750.00") || dated[0].contains("14,750.00"), "{text}");
    }

    #[test]
    fn side_by_side_check_tables_are_stacked() {
        let md = "| DATE | SERIAL NO. | AMOUNT | DATE | SERIAL NO. | AMOUNT |\n| :--- | :--- | :--- | :--- | :--- | :--- |\n| 04/05 | 2250 | 300.00 | 04/15 | 10783 | 966.47 |\n| 04/15 | 10782 | 1,340.29 | | | |\n";
        let text = expand_markdown_tables(md);
        let rows: Vec<Vec<&str>> = text.lines().skip(1).map(|l| l.split_whitespace().collect()).collect();
        assert_eq!(rows, vec![vec!["04/05", "2250", "300.00"], vec!["04/15", "10782", "1,340.29"], vec!["04/15", "10783", "966.47"]], "{text}");
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
