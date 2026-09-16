#!/usr/bin/env python3
"""Parser accuracy over a folder of statement PDFs, no model involved.

For every PDF, runs the deterministic parser (--headless-ledger) and compares the
credit and debit totals it summed from transaction lines with the totals the bank
printed in the statement summary. A statement "passes" when both match within
one dollar. Statements where the parser found no summary at all are listed
separately: those are layouts the parser does not understand yet.

    python3 scripts/parser_check.py <folder with PDFs> [--verbose]
"""
import json, os, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "src-tauri", "target", "debug", "local-mca-underwriter-ide")


def ledger(pdf):
    p = subprocess.run([BIN, "--headless-ledger", pdf], capture_output=True, text=True, timeout=300)
    body = "\n".join(l for l in p.stdout.splitlines() if not l.startswith("["))
    return json.loads(body) if body.strip() else None


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    folder, verbose = sys.argv[1], "--verbose" in sys.argv
    passed, failed, no_summary, errors = [], [], [], []
    for name in sorted(os.listdir(folder)):
        if not name.lower().endswith(".pdf"):
            continue
        path = os.path.join(folder, name)
        try:
            d = ledger(path)
        except Exception as e:
            errors.append((name, str(e)[:80]))
            continue
        if not d or not d["transactions"]:
            continue  # no transaction lines at all: not a statement (or fully scanned)
        s, p = d["summary"], d["parsed"]
        stated_c, stated_d = s.get("total_credits"), s.get("total_debits")
        if stated_c is None and stated_d is None:
            no_summary.append((name, len(d["transactions"])))
            continue
        ok_c = stated_c is None or abs(stated_c - p["credit_total"]) <= 1.0
        ok_d = stated_d is None or abs(stated_d - p["debit_total"]) <= 1.0
        row = (name, stated_c, round(p["credit_total"], 2), stated_d, round(p["debit_total"], 2), len(d["transactions"]))
        (passed if ok_c and ok_d else failed).append(row)
    print(f"passed {len(passed)}, failed {len(failed)}, no summary found {len(no_summary)}, errors {len(errors)}")
    if failed:
        print("\nFAILED (name, stated credits, parsed credits, stated debits, parsed debits, lines):")
        for r in failed:
            print("  ", r)
    if no_summary:
        print("\nNO SUMMARY FOUND (name, transaction lines parsed):")
        for r in no_summary[:40]:
            print("  ", r)
    if verbose and passed:
        print("\nPASSED:")
        for r in passed:
            print("  ", r)
    for r in errors:
        print("  error", r)


if __name__ == "__main__":
    main()
