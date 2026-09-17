#!/usr/bin/env python3
"""Parser accuracy over a folder of statement PDFs, no reasoning model involved.

For every PDF, runs the deterministic parser (--headless-ledger) and compares the
credit and debit totals it summed from transaction lines with the totals the bank
printed in the statement summary. A statement "passes" when both match within
one dollar. Statements where the parser found no summary at all are listed
separately: those are layouts the parser does not understand yet.

    python3 scripts/parser_check.py <folder with PDFs> [--ocr] [--verbose] [--markdown]

--ocr       read scanned pages with the OCR model through the engine (starts it; cached
            per page, set MCA_OCR_CACHE=<dir> to keep the cache with the corpus). Without
            it, scanned pages are skipped and the statement is tagged "scan pages skipped".
--ocr-cached  use cached OCR pages only, never start the engine (pages not in the cache are
            skipped as with no --ocr).
--markdown  print a per-bank coverage table instead of the plain lists.
--snapshot <file>  compare with the previous run saved in <file> (regressions and new passes
            are listed), then overwrite it with this run.
"""
import json, os, re, subprocess, sys
from collections import defaultdict

# A recurring debit whose payee reads like a funder: the statement shows MCA activity (a
# position). Data-quality signal for the corpus, reported per bank and in total.
LENDER = re.compile(r"capital|funding|fund\b|advance|kabbage|ondeck|on deck|fundbox|bluevine|credibly|kapitus|libertas|forward fin|rapid fin|everest|fora fin|national fund|cfg merch|\bmca\b|merchant|lendio|clearco|yellowstone|itria|pearl|torro|reliant|fox cap|seamless|byzfunder|greenbox|cloudfund|mantis|premium merch|newco|fundkite|lendr|expansion cap|last chance|one park|unique fund|lending|financ|loan", re.I)
NOT_LENDER = re.compile(r"payroll|tax|insur|utilit|transfer|amex|american express|card|mortgage|lease|rent\b|irs\b", re.I)


def lender_activity(d):
    """Recurring debits to funder-like payees in a ledger dump."""
    return [r for r in d.get("recurring_debits", []) if LENDER.search(r["payee"]) and not NOT_LENDER.search(r["payee"])]

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "src-tauri", "target", "debug", "local-mca-underwriter-ide")


def ledger(pdf, ocr):
    cmd = [BIN, "--headless-ledger", pdf] + (["--ocr"] if ocr else [])
    env = dict(os.environ)
    if ocr == "cached":
        env["MCA_OCR_CACHED_ONLY"] = "1"
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=3600 if ocr else 300, env=env)
    body = "\n".join(l for l in p.stdout.splitlines() if not l.startswith("["))
    return json.loads(body) if body.strip() else None


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    folder = sys.argv[1]
    ocr = "cached" if "--ocr-cached" in sys.argv else ("--ocr" in sys.argv)
    verbose, markdown = "--verbose" in sys.argv, "--markdown" in sys.argv
    snapshot = sys.argv[sys.argv.index("--snapshot") + 1] if "--snapshot" in sys.argv else None
    rows = []  # dicts: name, bank, status, stated/parsed totals, lines, scan pages
    for name in sorted(os.listdir(folder)):
        if not name.lower().endswith(".pdf"):
            continue
        path = os.path.join(folder, name)
        try:
            d = ledger(path, ocr)
        except Exception as e:
            rows.append({"name": name, "bank": "?", "status": "error", "note": str(e)[:80]})
            continue
        if not d or len(d["transactions"]) < 4 and d["summary"].get("total_credits") is None and d["summary"].get("total_debits") is None:
            continue  # a page or two of wire confirmations, not a statement (or fully scanned)
        s, p = d["summary"], d["parsed"]
        scans = d.get("pages", {}).get("scan", 0)
        stated_c, stated_d = s.get("total_credits"), s.get("total_debits")
        row = {"name": name, "bank": s.get("bank") or "unknown", "stated_c": stated_c, "parsed_c": round(p["credit_total"], 2),
               "stated_d": stated_d, "parsed_d": round(p["debit_total"], 2), "lines": len(d["transactions"]), "scans": scans,
               "lenders": [r["payee"][:40] for r in lender_activity(d)]}
        statements = d.get("statements") or []
        row["statements"] = len(statements) or 1
        if len(statements) > 1:
            # A bundle passes when every statement that prints a total matches its own lines.
            checked = [st for st in statements if st.get("total_credits") is not None or st.get("total_debits") is not None]
            if not checked:
                row["status"] = "no summary"
            else:
                ok = all((st.get("total_credits") is None or abs(st["total_credits"] - (st.get("parsed_credits") or 0)) <= 1.0)
                         and (st.get("total_debits") is None or abs(st["total_debits"] - (st.get("parsed_debits") or 0)) <= 1.0) for st in checked)
                row["status"] = "pass" if ok else "fail"
        elif stated_c is None and stated_d is None:
            row["status"] = "no summary"
        else:
            ok_c = stated_c is None or abs(stated_c - p["credit_total"]) <= 1.0
            ok_d = stated_d is None or abs(stated_d - p["debit_total"]) <= 1.0
            row["status"] = "pass" if ok_c and ok_d else "fail"
        rows.append(row)

    if snapshot:
        previous = {}
        if os.path.exists(snapshot):
            previous = json.load(open(snapshot))
        current = {r["name"]: r.get("status") for r in rows}
        regressions = sorted(n for n, st in previous.items() if st == "pass" and current.get(n) not in (None, "pass"))
        gains = sorted(n for n, st in current.items() if st == "pass" and previous.get(n, "pass") != "pass" and n in previous)
        if regressions:
            print("REGRESSIONS (passed before, not now):", ", ".join(regressions))
        if gains:
            print("NEW PASSES:", ", ".join(gains))
        json.dump(current, open(snapshot, "w"), indent=0, sort_keys=True)

    if markdown:
        by_bank = defaultdict(list)
        for r in rows:
            by_bank[r["bank"]].append(r)
        print("| Bank | Files | Pass | Fail | No summary | Scan pages skipped | With lender activity |")
        print("|---|---|---|---|---|---|---|")
        for bank in sorted(by_bank, key=lambda b: -len(by_bank[b])):
            rs = by_bank[bank]
            n = lambda st: sum(1 for r in rs if r.get("status") == st)
            scans = sum(1 for r in rs if r.get("scans"))
            lend = sum(1 for r in rs if r.get("lenders"))
            print(f"| {bank} | {len(rs)} | {n('pass')} | {n('fail')} | {n('no summary')} | {scans} | {lend} |")
        total_lend = sum(1 for r in rows if r.get("lenders"))
        print(f"\nStatements with lender activity (funder-like recurring debits): {total_lend} of {len(rows)}")
        return

    passed = [r for r in rows if r["status"] == "pass"]
    failed = [r for r in rows if r["status"] == "fail"]
    no_summary = [r for r in rows if r["status"] == "no summary"]
    errors = [r for r in rows if r["status"] == "error"]
    lend = [r for r in rows if r.get("lenders")]
    print(f"passed {len(passed)}, failed {len(failed)}, no summary found {len(no_summary)}, errors {len(errors)}; with lender activity {len(lend)}")
    if lend and verbose:
        print("\nLENDER ACTIVITY:")
        for r in lend:
            print("  ", r["name"], r["bank"], ", ".join(r["lenders"][:4]))
    fmt = lambda r: f"{r['name']}  {r['bank']:<16} credits {r['stated_c']} / {r['parsed_c']}  debits {r['stated_d']} / {r['parsed_d']}  lines {r['lines']}" + (f"  [{r['scans']} scan pages skipped]" if r["scans"] else "") + (f"  [{r['statements']} statements in file]" if r.get("statements", 1) > 1 else "")
    if failed:
        print("\nFAILED (stated / parsed):")
        for r in failed:
            print("  ", fmt(r))
    if no_summary:
        print("\nNO SUMMARY FOUND:")
        for r in no_summary[:40]:
            print("  ", fmt(r))
    if verbose and passed:
        print("\nPASSED:")
        for r in passed:
            print("  ", fmt(r))
    for r in errors:
        print("  error", r["name"], r["note"])


if __name__ == "__main__":
    main()
