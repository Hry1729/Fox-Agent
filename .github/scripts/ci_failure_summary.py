#!/usr/bin/env python3
"""Summarize failing tests from CI logs into the job summary and annotations.

CI-only diagnostics. Never changes a job result: always exits 0.

usage: python .github/scripts/ci_failure_summary.py <format> <label> <glob> [<glob> ...]
  format: cargo | tap | bun

Writes a markdown table to $GITHUB_STEP_SUMMARY and emits ::error annotations.
GitHub keeps at most 10 error annotations per step, so the first one is an
aggregate listing every failure (capped), followed by up to 8 individual ones.
"""
import glob
import os
import re
import sys

MAX_FAILURES = 30
MAX_INDIVIDUAL = 8
ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
# Actions timestamps are not in the files we read, but tolerate a leading BOM.


def clean(line):
    return ANSI.sub("", line).rstrip("\r\n").lstrip("\ufeff")


def short(text, limit=300):
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 3] + "..."


def parse_cargo(lines):
    """cargo test output: '---- name stdout ----' sections + 'failures:' name list."""
    details = {}
    order = []
    current = None
    buf = []

    def flush():
        if current is None:
            return
        msg = ""
        for i, l in enumerate(buf):
            if "panicked at" in l:
                where = l.split("panicked at", 1)[1].strip().rstrip(":")
                tail = []
                for nxt in buf[i + 1:]:
                    if not nxt.strip() or nxt.startswith("note: run with"):
                        break
                    tail.append(nxt.strip())
                    if len(tail) >= 3:
                        break
                msg = (" ".join(tail) + "  @ " + where).strip()
                break
        if not msg:
            meaningful = [l.strip() for l in buf if l.strip()]
            msg = meaningful[-1] if meaningful else "(no panic message captured)"
        details[current] = msg

    in_list = False
    for raw in lines:
        l = clean(raw)
        m = re.match(r"^---- (\S+) stdout ----$", l)
        if m:
            flush()
            current, buf = m.group(1), []
            if current not in order:
                order.append(current)
            continue
        if l == "failures:":
            flush()
            current, buf = None, []
            in_list = True
            continue
        if in_list:
            m = re.match(r"^    (\S+)$", l)
            if m:
                if m.group(1) not in order:
                    order.append(m.group(1))
                continue
            if l.strip():
                in_list = False
        m = re.match(r"^test (\S+) \.\.\. FAILED$", l)
        if m and m.group(1) not in order:
            order.append(m.group(1))
        if current is not None:
            buf.append(l)
    flush()
    return [(n, details.get(n, "(see test log)")) for n in order]


def parse_tap(lines):
    """node --test TAP: 'not ok N - name' + YAML diagnostics block."""
    out = []
    i = 0
    lines = [clean(l) for l in lines]
    while i < len(lines):
        m = re.match(r"^(\s*)not ok \d+ - (.*)$", lines[i])
        if not m:
            i += 1
            continue
        indent, name = m.group(1), m.group(2).strip()
        info = {}
        j = i + 1
        while j < len(lines) and not re.match(r"^\s*(not )?ok \d+ - ", lines[j]) and j - i < 80:
            s = lines[j].strip()
            for key in ("location", "failureType", "error", "expected", "actual", "code"):
                if s.startswith(key + ":") and key not in info:
                    val = s.split(":", 1)[1].strip().strip("'")
                    if val in ("|-", "|", ">-", ""):
                        val = lines[j + 1].strip() if j + 1 < len(lines) else ""
                    info[key] = val
            j += 1
        # Suites that fail only because a subtest failed add no information.
        if info.get("failureType") == "subtestsFailed":
            i = j
            continue
        parts = []
        if "expected" in info or "actual" in info:
            parts.append(f"expected={info.get('expected', '?')} actual={info.get('actual', '?')}")
        if info.get("location"):
            loc = info["location"].replace("\\\\", "\\")
            parts.append("@ " + re.sub(r"^.*[\\/](services|apps|crates)[\\/]", r"\1/", loc))
        if info.get("error"):
            parts.append(info["error"][:200])
        out.append((name, " | ".join(parts) or "(see test log)"))
        i = j
    return out


def parse_bun(lines):
    """bun test: error lines printed before '(fail) name [ms]'."""
    out = []
    recent = []
    unhandled = 0
    err_re = re.compile(r"^(error:|\w*Error: )")
    loc_re = re.compile(r"^at .*[\\/](tests|src)[\\/].*\.(tsx?|jsx?|mjs):\d+")
    for raw in lines:
        l = clean(raw).strip()
        if l.startswith("# Unhandled error between tests"):
            unhandled += 1
            recent = []
            out.append((f"(unhandled error between tests #{unhandled})", ""))
            continue
        if out and out[-1][0].startswith("(unhandled") and not out[-1][1] and err_re.match(l):
            out[-1] = (out[-1][0], short(l))
        m = re.match(r"^\(fail\) (.*?)(?: \[[\d.]+m?s\])?$", l)
        if m:
            err = next((r for r in recent if err_re.match(r)), "")
            at = next((r for r in recent if loc_re.match(r)), "")
            at = re.sub(r"^at .*?[\\/](apps|services)[\\/]", r"at \1/", at)
            out.append((m.group(1), short((err + ("  " + at if at else "")) or "(see test log)")))
            recent = []
            continue
        if re.match(r"^\((pass|skip|todo)\) ", l):
            recent = []
            continue
        if l:
            recent.append(l)
            recent = recent[-400:]
    return [(n, m or "(see test log)") for n, m in out]


def esc_data(s):
    return s.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def esc_prop(s):
    return esc_data(s).replace(":", "%3A").replace(",", "%2C")


def main():
    if len(sys.argv) < 4:
        print("usage: ci_failure_summary.py <cargo|tap|bun> <label> <glob>...")
        return
    fmt, label, patterns = sys.argv[1], sys.argv[2], sys.argv[3:]
    files = sorted({f for p in patterns for f in glob.glob(p, recursive=True)})
    parser = {"cargo": parse_cargo, "tap": parse_tap, "bun": parse_bun}[fmt]
    failures = []
    for f in files:
        with open(f, encoding="utf-8", errors="replace") as fh:
            for name, msg in parser(fh.readlines()):
                if name not in [n for n, _ in failures]:
                    failures.append((name, short(msg)))
    total = len(failures)
    shown = failures[:MAX_FAILURES]

    md = [f"### {label}: failing tests", ""]
    if not files:
        md.append(f"No log files matched `{' '.join(patterns)}` (step may not have run).")
    elif not failures:
        md.append(f"No failing tests found in {len(files)} log file(s).")
    else:
        md.append(f"{total} failing test(s)" + (f", first {MAX_FAILURES} shown" if total > MAX_FAILURES else "") + ":")
        md.append("")
        md.append("| # | Test | First error |")
        md.append("|---|---|---|")
        for k, (n, m) in enumerate(shown, 1):
            cell = m.replace("|", "\\|")
            md.append(f"| {k} | `{n}` | {cell} |")
    md.append("")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    text = "\n".join(md) + "\n"
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(text)
    print(text)

    if failures:
        agg = "\n".join(f"{k}. {n} -- {m[:160]}" for k, (n, m) in enumerate(shown, 1))
        print(f"::error title={esc_prop(f'{label}: {total} failing test(s)')}::{esc_data(agg)}")
        for n, m in shown[:MAX_INDIVIDUAL]:
            print(f"::error title={esc_prop(f'{label} FAIL {n}'[:200])}::{esc_data(m)}")


if __name__ == "__main__":
    try:
        # Windows runners default stdout to the ANSI code page; test names and
        # messages are often Chinese, and the runner reads workflow commands as UTF-8.
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        main()
    except Exception as exc:  # diagnostics must never fail the job
        print("ci_failure_summary: parser error ignored: " + ascii(exc))
    sys.exit(0)
