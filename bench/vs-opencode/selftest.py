#!/usr/bin/env python3
"""Check the benchmark before trusting it. Needs no model and no network.

A benchmark is a measuring instrument, and an instrument nobody calibrates
reports whatever it likes. Three properties have to hold or a run means nothing:

  * every stated math answer is the answer (one of the original ten was not);
  * a reply that FAILED is not mined for digits (an unreachable host once
    scored 11434 — the port — as the model's answer);
  * the coding gate cannot be talked out of, including by an agent that
    rewrites the test.

Run: python3 selftest.py
"""
import importlib.util, itertools, json, math, os, sys


def _fib(n):
    a, b = 1, 1
    for _ in range(n - 2):
        a, b = b, a + b
    return b


def _divs(n):
    return [d for d in range(1, n) if n % d == 0]

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("rb", os.path.join(HERE, "run_bench.py"))
rb = importlib.util.module_from_spec(spec)
sys.argv = ["rb"]
spec.loader.exec_module(rb)

failures = []


def check(label, got, want):
    ok = got == want
    if not ok:
        failures.append(label)
    print(f"{'OK ' if ok else 'BUG'} {label:44} got={got!r} want={want!r}")


# 1. The answer key is arithmetic, not memory.
CHECKS = {
    "m01": 2 * (16 - 3 - 4),
    "m02": 78 * (2 * 3 + 1 * 6),
    "m03": int(80000 * 2.5) - (80000 + 50000),
    "m04": 3 * 2 * 2 * 52,
    "m05": len([d for d in range(1, 3**4 * 5 + 1) if (3**4 * 5) % d == 0]) * 5,
    "m06": sum((9 - k) ** 2 for k in range(1, 9)),
    "m07": len(set(itertools.permutations("LEVEL"))) * 4,
    "m08": pow(7, 2024, 100),
    "m09": next(n for n in range(1, 20000) if n * (n + 1) // 2 == 500500),
    "m10": len([x for x in range(1, 1000) if x % 11 == 0]),
    "m11": _fib(10),
    "m12": math.comb(10, 3),
    "m13": 6 * 6,
    "m14": 1**3 + 5**3 + 3**3,
    "m15": math.lcm(*range(1, 11)),
    "m16": [n for n in range(2, 10000) if sum(_divs(n)) == n][2],
    "m17": len([x for x in range(2, 100) if all(x % d for d in range(2, int(x**0.5) + 1))]),
    "m18": 70 * 3,
    "m19": 8 * ((28 - 2 * 8) // 2),
    "m20": sum(range(1, 101)),
}
for line in open(os.path.join(HERE, "math", "problems.jsonl")):
    p = json.loads(line)
    check(f"answer key {p['id']}", p["answer"], str(CHECKS[p["id"]]))

# 2. A failed call is never scored as an answer.
for text, label in [
    ("chat error: oracle failure: ollama http: http://192.168.10.197:11434/api/generate: "
     "timed out reading response", "timeout is not an answer"),
    ("charge fizzled (exit 1) — no route to host (os error 113)", "host down is not an answer"),
]:
    why = rb.failed_reply(text)
    check(label, None if why else rb.last_int(text), None)
check("a real answer still scores", rb.last_int("chat    final answer\n936"), "936")

# 3. The coding gate cannot be cheated.
SOLUTION = (
    "def encode(s):\n"
    "    if not s: return ''\n"
    "    out=[];prev=s[0];n=0\n"
    "    for c in s:\n"
    "        if c==prev: n+=1\n"
    "        else: out.append(f'{prev}{n}');prev=c;n=1\n"
    "    out.append(f'{prev}{n}')\n"
    "    return ''.join(out)\n"
)
rb.host_up = lambda timeout=8: True


def stub(mode):
    def go(cmd, cwd, timeout=None, env=None):
        if mode == "solve":
            open(os.path.join(cwd, "sol.py"), "w").write(SOLUTION)
        elif mode == "cheat":
            open(os.path.join(cwd, "test_sol.py"), "w").write("def test_nothing(): pass\n")
        return "stub", 1.0, None
    return go


for mode, expect in [("solve", True), ("nothing", False), ("cheat", False)]:
    rb.run = stub(mode)
    check(f"gate: agent that does '{mode}'", rb.prog_round("kaos", ["p01_rle"])[0]["ok"], expect)

print()
if failures:
    sys.exit(f"{len(failures)} benchmark self-check(s) FAILED: {failures}")
print("benchmark self-check passed — results from this harness can be trusted")
