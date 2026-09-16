#!/usr/bin/env python3
"""Known-answer evaluation of the analysis pipeline.

Runs the app headless on each statement listed in eval_expected.json and scores the
report against the expectations. Usage:

    python3 scripts/eval.py "<folder with the PDFs>" [--model qwen3.5-9b-q4] [--only tets.pdf]

Prints one line per check and a summary. Exit code 1 if any check fails.
"""
import json, os, re, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "src-tauri", "target", "debug", "local-mca-underwriter-ide")
CONFIG = os.path.expanduser("~/.local/share/com.local-mca-underwriter.ide/engine/engine_config.json")


def set_model(model_id):
    cfg = {"backend": "gpu", "underwriter_model": model_id}
    if os.path.exists(CONFIG):
        try:
            cfg.update(json.load(open(CONFIG)))
        except Exception:
            pass
    cfg["underwriter_model"] = model_id
    os.makedirs(os.path.dirname(CONFIG), exist_ok=True)
    json.dump(cfg, open(CONFIG, "w"), indent=2)


def run(pdf):
    t = time.time()
    p = subprocess.run([BIN, "--headless-analyze", pdf], capture_output=True, text=True, timeout=1800)
    secs = time.time() - t
    body = "\n".join(l for l in p.stdout.splitlines() if not l.startswith("["))
    if p.returncode != 0 or not body.strip():
        err = [l for l in p.stderr.splitlines() if "FAILED" in l]
        raise RuntimeError(f"headless run failed ({p.returncode}): {' '.join(err)[:300]}")
    return json.loads(body), secs


def check(results, name, ok, detail=""):
    results.append(ok)
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{': ' + detail if detail else ''}")


def near(a, b, tol=0.01):
    return a is not None and b is not None and abs(float(a) - float(b)) <= tol


def evaluate(report, exp):
    r = []
    biz, bm, ver = report.get("business", {}), report.get("bank_metrics", {}), report.get("verification", {})
    if "name_contains_any" in exp:
        name = (biz.get("name") or "").upper()
        check(r, "merchant name", any(k.upper() in name for k in exp["name_contains_any"]), biz.get("name"))
    if "account" in exp:
        check(r, "account", biz.get("account") == exp["account"], str(biz.get("account")))
    if "period_contains" in exp:
        check(r, "period", exp["period_contains"] in (biz.get("period") or ""), str(biz.get("period")))
    for key in ("total_credits", "negative_days", "days_in_period", "avg_daily_balance"):
        if key in exp:
            check(r, key, near(bm.get(key), exp[key]), f"{bm.get(key)} vs {exp[key]}")
    if "parsed_total_debits" in exp:
        check(r, "parsed debits", near(ver.get("parsed_total_debits"), exp["parsed_total_debits"]), str(ver.get("parsed_total_debits")))
    if "nsf_count_range" in exp:
        lo, hi = exp["nsf_count_range"]
        check(r, "nsf count", lo <= bm.get("nsf_count", -1) <= hi, str(bm.get("nsf_count")))
    positions = report.get("positions", [])
    for req in exp.get("positions_required", []):
        hit = [p for p in positions if re.search(req["lender_regex"], (p.get("lender") or "") + " " + str(p.get("evidence") or ""), re.I) and near(p.get("payment"), req["payment"])]
        ok = bool(hit) and ("frequency" not in req or hit[0].get("frequency") == req["frequency"])
        check(r, f"position {req['lender_regex']} {req['payment']}", ok, hit[0].get("frequency") if hit else "missing")
    if "positions_forbidden_regex" in exp:
        bad = [p.get("lender") for p in positions if re.search(exp["positions_forbidden_regex"], (p.get("lender") or "") + " " + str(p.get("evidence") or ""), re.I)]
        check(r, "no false positions", not bad, ", ".join(map(str, bad)) if bad else f"{len(positions)} positions")
    funding = [f["amount"] for f in ver.get("funding_deposits", [])]
    for amt in exp.get("funding_required_amounts", []):
        check(r, f"funding includes {amt}", any(near(a, amt) for a in funding))
    bad = [amt for amt in exp.get("funding_forbidden_amounts", []) if any(near(a, amt) for a in funding)]
    check(r, "no revenue counted as funding", not bad, str(bad) if bad else f"{len(funding)} funding deposits")
    return r


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(2)
    folder = args[0]
    model = args[args.index("--model") + 1] if "--model" in args else None
    only = args[args.index("--only") + 1] if "--only" in args else None
    if model:
        set_model(model)
    expected = json.load(open(os.path.join(os.path.dirname(__file__), "eval_expected.json")))
    all_results, timings = [], {}
    for pdf, exp in expected.items():
        if pdf.startswith("_") or (only and pdf != only):
            continue
        path = os.path.join(folder, pdf)
        print(f"\n== {pdf}" + (f" [{model}]" if model else ""))
        try:
            report, secs = run(path)
        except Exception as e:
            print(f"  FAIL run: {e}")
            all_results.append(False)
            continue
        timings[pdf] = secs
        res = evaluate(report, exp)
        all_results.extend(res)
        risk = report.get("risk", {})
        print(f"  info risk {risk.get('score')} (baseline {risk.get('baseline')} {risk.get('adjustment'):+}) {report.get('recommendation')}, {secs:.0f}s")
        print(f"  info notes: {report.get('notes')}")
        if report.get("verification", {}).get("notes_dropped"):
            print(f"  info dropped from notes: {report['verification']['notes_dropped']}")
        if report.get("verification", {}).get("rejected_positions"):
            print(f"  info rejected positions: {[p['lender'] for p in report['verification']['rejected_positions']]}")
    passed = sum(all_results)
    print(f"\n{passed}/{len(all_results)} checks passed; " + ", ".join(f"{k} {v:.0f}s" for k, v in timings.items()))
    sys.exit(0 if passed == len(all_results) else 1)


if __name__ == "__main__":
    main()
