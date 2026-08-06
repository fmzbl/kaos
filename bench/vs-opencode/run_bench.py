#!/usr/bin/env python3
"""Head-to-head: the Kaos harness vs opencode, same model, same tasks, same gates.

Math is graded on the final integer in the reply. Programming is graded by the
task's own pytest suite, run after the agent finishes, in a throwaway copy of
the task directory — so neither harness can pass by editing the test.
"""
import json, os, re, shutil, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
KAOS = os.path.join(HERE, "..", "..", "target", "release", "kaos")
RESULTS = os.path.join(HERE, "results")
MODEL = "ollama:qwen3.6:35b"
TIMEOUT = int(os.environ.get("BENCH_TIMEOUT", "900"))

def env_for(extra=None):
    e = dict(os.environ)
    e["KAOS_MODEL"] = MODEL
    # Fairness: opencode reaches ollama through the OpenAI-compatible /v1
    # endpoint, which has no way to set ollama's `think` flag. Leaving
    # KAOS_THINK on would give one harness a reasoning pass the other cannot
    # ask for — measuring the flag rather than the harness.
    e["KAOS_THINK"] = "0"
    e["KAOS_SESSION_DIR"] = os.path.join(HERE, "results", "sessions")
    e.update(extra or {})
    return e

def host_up(timeout=8):
    """Is the model host actually there?

    Checked before every task because opencode does not fail fast: with the
    host down it sat out the full 900s timeout on each problem and reported a
    row that looks exactly like a wrong answer. A benchmark that cannot tell
    "the model was wrong" from "the network was gone" is worse than no
    benchmark."""
    import urllib.request
    try:
        urllib.request.urlopen("http://192.168.10.197:11434/api/tags", timeout=timeout)
        return True
    except Exception:
        return False

def await_host(limit_s=43200):
    waited = 0
    while not host_up():
        if waited >= limit_s:
            sys.exit(f"ABORT: model host still unreachable after {limit_s}s")
        if waited % 300 == 0:
            print(f"  waiting for model host… {waited}s", flush=True)
        time.sleep(30); waited += 30
    return waited

def run(cmd, cwd, timeout=TIMEOUT, env=None):
    t = time.time()
    try:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True,
                           timeout=timeout, env=env or env_for())
        return p.stdout + "\n" + p.stderr, time.time() - t, None
    except subprocess.TimeoutExpired as e:
        out = (e.stdout or b"")
        out = out.decode() if isinstance(out, bytes) else out
        return out, time.time() - t, "timeout"

# Text that means the call failed rather than answered. Scoring these is how a
# port number becomes a benchmark result: an unreachable host produced
# "…192.168.10.197:11434…" and the grader dutifully returned 11434.
FAILURE_MARKS = ("chat error:", "oracle failure:", "no route to host",
                 "connection refused", "timed out reading response",
                 "charge fizzled", "Network Error")

def failed_reply(text):
    return next((m for m in FAILURE_MARKS if m in text), None)

def last_int(text):
    nums = re.findall(r"-?\d[\d,]*", text.replace("*", ""))
    return nums[-1].replace(",", "") if nums else None

def math_round(harness, probs):
    rows = []
    for p in probs:
        if not host_up():
            print("  host went away mid-suite; waiting for it to return", flush=True)
            await_host()
        q = p["q"] + "\n\nAnswer with the final integer only."
        if harness == "kaos":
            out, secs, err = run([KAOS, "chat", q], HERE)
        else:
            out, secs, err = run(["opencode", "run", q], HERE)
        why = failed_reply(out)
        got = None if why else last_int(out)
        ok = got is not None and got == p["answer"]
        err = err or why
        # Keep the raw reply. A failure you cannot read cannot be told apart
        # from a grader bug.
        rawdir = os.path.join(RESULTS, "raw"); os.makedirs(rawdir, exist_ok=True)
        open(os.path.join(rawdir, f"math-{harness}-{p['id']}.txt"), "w").write(out)
        rows.append(dict(id=p["id"], harness=harness, ok=ok, got=got,
                         want=p["answer"], secs=round(secs, 1), err=err))
        if why in ("no route to host", "connection refused"):
            raise SystemExit(f"ABORT: the model host is unreachable ({why}). "
                             f"Nothing after this point would measure anything.")
        print(f"  {harness:9} {p['id']} {'PASS' if ok else 'FAIL'} "
              f"got={got} want={p['answer']} {secs:.0f}s {err or ''}", flush=True)
    return rows

def prog_round(harness, tasks):
    rows = []
    for t in tasks:
        if not host_up():
            print("  host went away mid-suite; waiting for it to return", flush=True)
            await_host()
        src = os.path.join(HERE, "prog", t)
        work = tempfile.mkdtemp(prefix=f"bench-{harness}-{t}-")
        for f in os.listdir(src):
            shutil.copy(os.path.join(src, f), work)
        task_text = open(os.path.join(src, "TASK.md")).read()
        if harness == "kaos":
            out, secs, err = run([KAOS, "code", ".", task_text], work)
        else:
            shutil.copy(os.path.join(HERE, "opencode.json"), work)
            out, secs, err = run(["opencode", "run", task_text], work)
        # The gate is run by US, on a pristine copy of the tests.
        shutil.copy(os.path.join(src, "test_sol.py"), work)
        g = subprocess.run(["python3", "-m", "pytest", "test_sol.py", "-q"],
                           cwd=work, capture_output=True, text=True, timeout=120)
        ok = g.returncode == 0
        rawdir = os.path.join(RESULTS, "raw"); os.makedirs(rawdir, exist_ok=True)
        open(os.path.join(rawdir, f"prog-{harness}-{t}.txt"), "w").write(
            out + "\n===== GATE =====\n" + g.stdout + g.stderr)
        rows.append(dict(id=t, harness=harness, ok=ok, secs=round(secs, 1),
                         err=err, gate=g.stdout.strip().splitlines()[-1] if g.stdout.strip() else ""))
        print(f"  {harness:9} {t:15} {'PASS' if ok else 'FAIL'} {secs:.0f}s {err or ''}", flush=True)
        shutil.rmtree(work, ignore_errors=True)
    return rows

def preflight():
    """Refuse to run a benchmark that would silently measure the wrong model.

    `kaos code` falls back to the Claude CLI whenever the bound model is `sim`
    (src/main.rs: "A simulated session has none — default to the claude CLI").
    That is sensible for a user and a disaster for a benchmark: the programming
    suite would report Claude's scores under Kaos's name, and nothing in the
    output would say so."""
    if "sim" in (MODEL or "").split(":")[0] or not MODEL.startswith("ollama:"):
        sys.exit(f"refusing to run: MODEL={MODEL!r} is not an ollama model, so "
                 f"`kaos code` may silently use the Claude CLI instead")
    if not os.path.exists(KAOS):
        sys.exit(f"refusing to run: no kaos binary at {KAOS} — "
                 f"build it with `cargo build --release`")

def main():
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    preflight()
    waited = await_host()
    if waited:
        print(f"model host came back after {waited}s", flush=True)
    os.makedirs(RESULTS, exist_ok=True)
    probs = [json.loads(l) for l in open(os.path.join(HERE, "math", "problems.jsonl"))]
    tasks = sorted(os.listdir(os.path.join(HERE, "prog")))
    rows = []
    if which in ("all", "math"):
        print("== MATH ==", flush=True)
        for h in ("kaos", "opencode"):
            rows += math_round(h, probs)
    if which in ("all", "prog"):
        print("== PROGRAMMING ==", flush=True)
        for h in ("kaos", "opencode"):
            rows += prog_round(h, tasks)
    out = os.path.join(RESULTS, f"results-{which}.json")
    json.dump(rows, open(out, "w"), indent=1)
    print("\n== SUMMARY ==")
    for h in ("kaos", "opencode"):
        hs = [r for r in rows if r["harness"] == h]
        if hs:
            print(f"{h:9} {sum(r['ok'] for r in hs)}/{len(hs)} "
                  f"median {sorted(r['secs'] for r in hs)[len(hs)//2]:.0f}s")
    print("wrote", out)

if __name__ == "__main__":
    main()
