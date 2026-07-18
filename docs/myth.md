# The myth

A **myth** is a *layer on top of kaos* — not a loop (there is no iteration). It
is a graph you write as an S-expression to compose the base agent's mind into a
structure of diverge and converge.

kaos's composition layer is not a hardcoded pipeline and not a wizard of named
"phases". It is a **graph you write as an S-expression** — a small Lisp.
Everything the agent does (a single shot, a voted conclave, verified best-of-k,
a nested spiral, a propose→critique→write pipeline) is a composition of two
verbs — **diverge** and **converge** — sequenced with **pipe** and given roles
with **ask**.

This is the whole language:

```lisp
(ask "role")         ; a leaf — one model call carrying an instruction (a stage's job)
fire                 ; the same leaf with no instruction — a bare, generic call
(spread N X)         ; diverge — evaluate subgraph X, N ways (in parallel)
(gather G X)         ; converge — collapse X's candidates through gate G
(pipe A B …)         ; sequence — each stage's collapsed answer feeds the next
; G ::= vote | first | (check "shell-cmd")
```

Five node forms, three gates, one evaluator. `ask` is the workhorse — a stage
with a role; `fire` is just `ask` with nothing to say, kept because the plain
conclave (`(gather vote (spread K fire))`) needs no instruction. That is the
entire surface.

## The mental model: a bowtie

Picture data flowing left to right. A `spread` **fans out** one thing into many
(the wide part). A `gather` **collapses** many back into one (the narrow part).
Chain them and you get a bowtie — wide, narrow, wide, narrow — and because a
`gather` branch can itself contain a `spread`, bowties **nest** to any depth.

```
        · · ·                     · · ·
       ╲  |  ╱                   ╲  |  ╱
  in ── spread ── gather ── … ── spread ── gather ── out
                  (a gate)                 (a gate)
```

The two shapes are the only structure there is. Generation is the wide part
(cheap, diverse, a lottery); selection is the narrow part (where reliability
lives).

## Evaluation semantics

A node evaluates to a **list of candidate answers**.

- `(ask "role")` → a one-element list: the model's answer, with the instruction
  prepended to the task. It is how a pipeline stage is given a job ("Propose an
  approach", "Critique it", "Write the final code") without threading prompts
  through Rust. Each call gets a unique index; the index seeds the sample and
  picks a solar/lunar temperature, so repeated calls are **diverse but
  reproducible**.
- `fire` → the same, with no instruction: a bare, generic call. Use it inside a
  plain conclave where every sample has the identical job.
- `(spread N X)` → evaluates `X` `N` times **concurrently** (scoped threads) and
  concatenates the results. It *grows* the candidate list. `N` fires ⇒ `N`
  candidates; `N` copies of a sub-bowtie ⇒ `N` collapsed candidates.
- `(gather G X)` → evaluates `X` (a list), then collapses it to **one** candidate
  via gate `G`. It *shrinks* the list.
- `(pipe A B …)` → evaluates each stage in turn. Each stage's answer is collapsed
  to one (a `vote` if the stage left several) and appended to the task as
  `Work so far:` context for the next stage. The pipe returns the **last** stage's
  candidate list, so the myth ends on whatever that stage produced.

`run(node, task, cast)` evaluates the top node; if it left more than one
candidate, an implicit final `vote` decides. So a well-formed myth ends in a
`gather` or a `pipe` whose last stage converges.

## The gates

A gate is how a `gather` chooses among candidates.

| Gate | Behaviour | Use it when |
|---|---|---|
| `vote` | the modal (most-common) candidate | there's no external checker — self-consistency *is* the signal (math answers, classifications) |
| `first` | the first non-empty candidate | any answer will do; you just want one that didn't fizzle |
| `(check "cmd")` | keep candidates for which the shell `cmd` exits 0; take the first survivor | there's a **sound verifier** (a test suite, a compiler, a proof kernel) |

For `(check "cmd")`, the candidate is handed to the shell on the `$CANDIDATE`
environment variable **and** on stdin, so both of these work:

```lisp
(gather (check "grep -qxF \"$CANDIDATE\" answers.txt") (spread 8 fire))
(gather (check "python3 verify.py") (spread 8 fire))   ; reads the candidate on stdin
```

`vote` and `first` are pure (no process); `check` spends a subprocess per
candidate — reliable, but not free.

## Recipes

**Conclave — reliable answer from a flaky model.** Fire k times, vote.
```lisp
(gather vote (spread 8 fire))
```

**Cheapest good answer.** First one that comes back.
```lisp
(gather first (spread 3 fire))
```

**Verified best-of-k.** Generate many, keep one a real gate accepts.
```lisp
(gather (check "lake env lean Attempt.lean") (spread 8 fire))
```

**The bowtie (from the whiteboard).** Inner: gate-verify small batches; outer:
vote across the verified winners.
```lisp
(gather vote
  (spread 4
    (gather (check "lake env lean Attempt.lean")
      (spread 4 fire))))
```

**A wide net into a strict gate.** Diverge broadly, gate hard.
```lisp
(gather (check "pytest -q tests/") (spread 16 fire))
```

**An agent pipeline (propose → critique → write).** `pipe` sequences stages and
`ask` gives each a role; the collapsed answer of one becomes the next's context.
This is the shape for real AI-engineering work — a draft, a review of the draft,
then a verified final.
```lisp
(pipe
  (gather vote (spread 5 (ask "Propose an approach")))
  (ask "Critique that approach; list its flaws")
  (gather (check "pytest -q") (spread 3 (ask "Write the final, correct code"))))
```

## A big one — a real bug-fix pipeline

The recipes above are one shape each. A real myth composes them. Here is the one
you'd actually run when a bug report lands on a Python library: reproduce it,
root-cause it, fix it several ways behind a test gate, review the survivor, then
harden it against the review.

```lisp
(pipe
  ; 1 ─ reproduce: turn the vague report into a failing test, 3 ways, vote the clearest
  (gather vote
    (spread 3 (ask "Write a minimal pytest that reproduces the reported bug. Test only.")))

  ; 2 ─ root-cause: diagnose against the repro from stage 1
  (ask "Given that failing test, name the root cause and the exact file:line. No fix yet.")

  ; 3 ─ fix: diverge widely, keep only a candidate the suite actually accepts
  (gather (check "pytest -q")
    (spread 8 (ask "Write the minimal patch that fixes the root cause and passes the suite.")))

  ; 4 ─ critique: adversarial review of the surviving patch
  (ask "Critique that patch: edge cases, regressions, style. List concrete flaws.")

  ; 5 ─ harden: revise against the critique, gate on the FULL suite this time
  (gather (check "pytest -q tests/")
    (spread 4 (ask "Apply the critique. Produce the final patch that passes the whole suite."))))
```

Every verb earns its place:

| Stage | Verb | What it buys |
|---|---|---|
| 1 reproduce | `spread 3` → `vote` | a repro is mid-band — good *sometimes*; consensus picks the clearest of three |
| 2 root-cause | `ask` (single) | diagnosis is cheap and only feeds forward; no fan-out needed |
| 3 fix | `spread 8` → `(check "pytest -q")` | the fix is fallible-but-checkable — cast a wide net, let the **suite** (not a vote) keep the one that works |
| 4 critique | `ask` (single) | one adversarial pass surfaces what a green test missed |
| 5 harden | `spread 4` → `(check "pytest -q tests/")` | revise against the critique, gate harder so a local fix can't break the rest |

The `pipe` is the spine: each stage's collapsed answer arrives at the next as
`Work so far:` context, so the root-cause sees the repro, the fix sees the
diagnosis, the critique sees the fix, and the hardening sees the flaws.

**What it costs.** Stages 3 and 5 spend `8 + 4 = 12` gated attempts, each running
`pytest` — 12 subprocesses of test time plus 18 model calls across the whole pipe.
It pays when the bug is genuinely mid-band (the model *can* fix it, just not first
try). On a bug below the floor — where the model never reaches the fix — the
`check` gate returns nothing and you've spent the samples for a null. Writing the
big myth only where the problem sits in the band that rewards it is half the skill.

**Tighten the gate.** A green test the model *wrote* proves nothing if the model
also edited the test. Gate against tampering:
```lisp
(gather (check "pytest -q tests/ && git diff --quiet -- tests/")
  (spread 4 (ask "Apply the critique. Produce the final patch that passes the whole suite.")))
```

## Why it works

A myth is an **amplifier, not a source**. It cannot make a model know
something it doesn't; it can only recover a correct answer that is *present but
unreliable*. Two measured cases:

- **Self-consistency (`vote`)** pays when the model is mid-band — right sometimes,
  with *diverse* wrong answers. The right answer concentrates on one value while
  the wrong ones scatter, so the vote finds it. AIME 2025, mid-band model:
  **37% → 70%**.
- **A sound gate (`check`)** pays even for a strong model, at any difficulty,
  because the model is *fallible but checkable* — it can reach the answer
  sometimes and the gate confirms it without being fooled. miniF2F (Lean),
  frontier model: **58% → 92%**.

Where neither holds — a model already at ceiling (nothing to recover) or a model
at the floor with no gate (correlated failure, nothing to select) — the myth
correctly does nothing. Half the skill is writing a myth only where it pays.

## Two kinds of leaf: answer vs. act

By default an `ask`/`fire` leaf is **one model completion** — it returns text (an
answer, a proposal, a diff-as-text). Set **`KAOS_AGENTIC=1`** and every leaf
becomes a **full `kaos code` tool-session** instead: each `fire` runs the
conductor (read / edit / bash) in its **own isolated copy** of the working tree,
and returns the *diff it produced*. Now the myth doesn't just answer — it acts.

- `(spread 4 (ask "…"))` → four agents editing four private copies, concurrently.
- `(gather (check "cargo test") …)` → re-applies each agent's diff to a fresh copy
  and keeps only the ones whose changes actually pass.
- `vote` still works: two agents that produced the identical diff share a ballot.

The arena is `KAOS_ARENA` (default the current dir); a leaf's `bash` actions are
bounded by `KAOS_BASH_TIMEOUT_S` (default 600s), a `check` gate by
`KAOS_GATE_TIMEOUT_S` (default 300s — widen it for a full build or benchmark gate).
The source tree
is never mutated — only isolated copies are, so a `spread` can't have one agent
clobber another. This is the mode for "run the whole thing yourself": the agents
clone, edit, run the tests, even run a benchmark, and the gate weighs the result.

## Using it

- **`/myth`** — a clean screen with this grammar; write a myth, then a task, and
  it runs on the bound mind.
- **`KAOS_AGENTIC=1`** — make the leaves *act* (agentic sessions) instead of
  *answer* (single completions). `KAOS_ARENA` sets the working tree.
- **`KAOS_MYTH="(gather vote (spread 8 fire))"`** — override the default myth used
  by `conclave`.
- **`kaos conclave <task>`** — runs the default `(gather vote (spread K fire))`
  (K from `KAOS_K`, default 5).

## Extending

The evaluator is generic over one trait — a *chat you can fire*:

```rust
pub trait Cast: Sync {
    fn fire(&self, task: &str, i: usize) -> Option<String>;
    fn check(&self, task: &str, candidate: &str, cmd: &str) -> bool { false }
}
```

Two `Cast`s ship: **`ChatCast`** (each `fire` = one completion; `check` runs a
shell verifier over the candidate on stdin) and **`AgentCast`** (each `fire` = a
full `Conductor` session in an isolated copy; `check` re-applies the diff and runs
the gate there). Any other `Cast` — a local model, a tool, a mock in tests — drops
straight into the same graph; the evaluator never touches a provider. That's the
whole point of the minimal surface: the graph is the myth, the `Cast` is the world.
