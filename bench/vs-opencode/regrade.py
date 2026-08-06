#!/usr/bin/env python3
"""Re-grade a math run from the captured raw replies.

The live runner takes the last integer of the whole output, which a reply that
restates the question after answering ("936 dollars for 78 robes") defeats. This
grades the same replies more carefully, identically for both harnesses:

  1. drop the Kaos trace by keeping only what follows the final-answer marker;
  2. prefer a line that is nothing but an integer — that is what was asked for;
  3. fall back to the last integer in the remaining text.
"""
import json, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
RAW = os.path.join(HERE, "results", "raw")

def answer_section(text):
    marker = "final answer"
    at = text.rfind(marker)
    return text[at + len(marker):] if at != -1 else text

def grade(text):
    """Return (value, confident). Confident only when the reply is the bare
    integer that was asked for. Anything else is guessed AND flagged, because a
    reply like "936 dollars for 78 robes" defeats both first- and last-integer
    rules, and silently picking one turns a grader bug into a benchmark result."""
    body = answer_section(text)
    body = re.sub(r"\x1b\[[0-9;]*m", "", body)
    for line in body.splitlines():
        bare = line.strip().strip("*`. ").replace(",", "")
        if re.fullmatch(r"-?\d+", bare):
            return bare, True
    nums = re.findall(r"-?\d[\d,]*", body.replace("*", ""))
    return (nums[-1].replace(",", ""), False) if nums else (None, False)

def main():
    probs = {json.loads(l)["id"]: json.loads(l)["answer"]
             for l in open(os.path.join(HERE, "math", "problems.jsonl"))}
    if not os.path.isdir(RAW):
        print("no raw captures yet:", RAW); return
    tally = {}
    for f in sorted(os.listdir(RAW)):
        if not f.startswith("math-"):
            continue
        _, harness, pid = f[:-4].split("-", 2)
        text = open(os.path.join(RAW, f)).read()
        got, sure = grade(text)
        want = probs[pid]
        ok = got == want
        tally.setdefault(harness, []).append(ok)
        flag = "" if sure else "  <-- REVIEW: no bare-integer answer line"
        print(f"{harness:9} {pid} {'PASS' if ok else 'FAIL'} got={got} want={want}{flag}")
    print()
    for h, oks in tally.items():
        print(f"{h:9} {sum(oks)}/{len(oks)}")

if __name__ == "__main__":
    main()
