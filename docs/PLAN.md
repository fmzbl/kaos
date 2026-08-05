# Everything still open

One plan across both repositories. It exists because five separate threads are
half-built, and each one is cheap to finish and expensive to leave: a harness
with two orchestration languages, a sigil surface that contradicts its own
source, a language blocked on one unmade decision, an agent loop that cannot
say "I don't know", and a leaked credential.

Every item below states its **interface**, its **acceptance criterion**, its
**size**, and what it **risks**. Items with no acceptance criterion are
decisions, and are marked as such — a decision is finished when it is written
down, not when code changes.

The language phases live in `rebis/docs/PLAN_LISP.md` and are referenced, not
restated. This document owns the harness.

---

## 0. The ordering thesis

The work is ordered by one question: **what does the harness claim, and what
currently makes the claim false?**

The claim is cost readability. One written prompt is one firing, countable off
the page, before it is paid for. `rebis/tests/costs.rs` keeps it true *inside a
program*. Chaos mode (shipped) extends it to chats by composing the intent into
a program first. What remains is that the harness itself — the thing that runs
the programs — does not obey it. `myth` orchestrates in a second language whose
cost is not countable. The conclave hand-codes its fan-out in Rust. A run
browser shows a program without showing what it will cost.

So workstream A is first, and everything else is ordered behind whether it
depends on A.

---

## A. Rebis as the orchestration language

**Provenance:** *"could u refactor the whole kaos harness to use mostly rebis
itself as the orchestration language? i mean the chaos mode"* — asked, then
interrupted. Chaos mode as a *stance* shipped; this is the other half.

### A0. The finding

Kaos has **two** orchestration languages.

| | Rebis | myth |
|---|---|---|
| where | `rebis-lang`, a git dependency | `kaos-agent/src/myth.rs`, 518 lines |
| surface | 22 operators, frozen | 5 forms, 4 gates |
| docs | `docs/REBIS.md`, `REFERENCE.md`, `SPEC.md` | `docs/myth.md` |
| cost model | one written prompt is one firing, tested | none |
| tests | 462 in-repo | 12 unit tests |

`myth` already imports `rebis_lang as mirror` to implement its `Mirror` gate —
the two languages share the holonomy code and nothing else. That is the shape
of an accident, not a design.

The mapping is close to exact:

| myth | Rebis | status |
|---|---|---|
| `(spread N X)` + `(gather G X)` | `([M] A B …)` — the mediator square | concurrent already, bounded by `KAOS_REBIS_MAX_CONCURRENCY` |
| `Gate::Mirror(p)` | `[symbol]` mediator | **the same code path** (`mirror::holonomy_reflected`) |
| `Gate::Vote` | `[consensus]` | needs equivalence proof |
| `Gate::First` | `[first]` or `%` | needs checking |
| `Gate::Check(cmd)` | — | **the real gap** |
| `(pipe A B …)` | `(-> A B)` | direct |
| `(ask "role")` | `"prompt"` | direct |

### A1. Prove the mapping before changing anything

**Interface:** a new test file, `kaos-agent/tests/myth_equivalence.rs`. For each
worked example in `docs/myth.md`, the myth graph and its Rebis translation are
run against the same scripted oracle and must produce the same answer and the
same number of firings.

**Acceptance:** every documented myth example has a Rebis translation that is
answer-identical and firing-identical against a scripted oracle. Any example
that cannot be translated is named in the file with the reason.

**Why first:** this is a test, not a refactor. If the mapping does not hold, the
rest of workstream A is wrong and costs nothing to abandon.

**One hazard already known, so it is not rediscovered:** `Gate::Vote` takes the
modal candidate by string equality. Rebis's `[consensus]` is a **symbol
mediator** and picks by `1 − score` against the mediator's own content tokens —
which means it refuses branches that do not share its vocabulary. Earlier in
this work `[consensus]` was observed refusing everything for exactly that
reason. These two are **not** obviously the same gate, and A1 must either show
they agree on the documented examples or record that they do not and say which
one `KAOS_MYTH`'s default should keep.

**Size:** M. **Risk:** low — worst case it produces a list of gaps, which is
itself the deliverable.

### A2. The shell gate

`Gate::Check(cmd)` runs a shell verifier over each candidate and keeps the first
survivor. Rebis has no such thing, and this is the one capability the language
would lose.

**Three options, and the constraint that kills two of them:** the operator set
is frozen at 22 and has been through this whole session unchanged. A new
operator is not on the table.

*(Checked, so the plan does not send anyone down it: the `[M: code]` in
`REFERENCE.md:1349` is the **mandala glyph legend** — a square labelled `M`
containing code — not a mediator parameterised by a shell command. There is no
existing spelling for a shell gate.)*

1. **A host-provided mediator.** Kaos registers `check` as a named mediator the
   way it registers models, and `[check "cargo test"]` resolves to it. No
   language change: to Rebis it is a symbol mediator like any other, and the
   host supplies its behaviour — the same shape `Inlet` already has for
   `(&: src)`.
2. **A symbol mediator over a value the program already obtained.** The program
   runs the command through an existing seam and the mediator judges the
   captured output. Keeps the host out of the mediator table, at the cost of
   making every gated program longer.
3. **Refuse, and keep `Gate::Check` in Rust** as a documented myth-only feature.
   Honest, but leaves the second language alive for one gate.

**Recommendation:** (1). It keeps the operator set frozen, makes verification
first-class, and reuses the seam the host already owns. It is also what D1
(abstention) needs to attach to.

**Interface:**

```rust
/// A mediator the host supplies, resolved by name when a program writes
/// `[name …]`. Refused unless the run holds command authority.
pub trait HostMediator: Sync {
    fn name(&self) -> &str;
    fn pick(&self, branches: &[String]) -> Option<usize>;
}
```

**Acceptance:** a Rebis program can gate candidates on a shell command; the
gate's cost is stated in the program the way every other cost is; and a program
naming `[check …]` **without** command authority is refused at resolution
rather than silently answering.

**Size:** M. **Risk:** medium — a host mediator that runs shell commands is an
authority surface, and must go through the same gate `--allow-tools` guards
(R7).

### A3. `KAOS_MYTH` becomes a Rebis expression

The config default is `(gather vote (spread 5 fire))`. `ValueKind::Expression`
already exists in `config.rs` and already documents itself as "a Rebis
expression evaluated by a Kaos host" — which is currently not true of this key.

**Interface:** `KAOS_MYTH` accepts a Rebis program. A value in the old myth
syntax is translated on read, once, with a one-line notice naming the
replacement, for one release.

**Acceptance:** the shipped default is a Rebis program; an old-syntax value in a
user's config still works and says what to change it to; `CONFIG_DOCS` for the
key stops lying about what it holds.

**Size:** S. **Risk:** low. **Migration:** users' `~/.config/kaos/config`.

### A4. Retire `myth.rs`

Only after A1 and A2 pass. `myth.rs` becomes a translation shim (parse old
syntax → emit Rebis) and then is deleted a release later. `docs/myth.md`
becomes a migration note pointing at `REBIS.md`.

**Acceptance:** `kaos-agent` has one orchestration language. The `Cast` trait
either survives as the model seam or is folded into the Rebis oracle — decide
during A1, when the equivalence tests show which.

**Size:** M. **Risk:** low once A1 holds.

### A5. The conclave — carefully

`solve.rs` hand-codes spread-and-vote in Rust and carries a **measured** result
(+23pts AIME2025 on a mid-band model). Memory: *the edge is verification, not
voting; consensus is mid-band-only.* Carroll independently: *"Scores are not
cumulative"* — N conjurations never exceed the best single one — and moderate
acts move only mid-band probabilities.

**This is the one place where rewriting risks a number that was expensive to
get.**

**Interface:** express the conclave as a Rebis program *beside* the Rust path,
not instead of it. `KAOS_AGENTIC` or a new flag selects which runs.

**Acceptance:** the Rebis conclave matches the Rust conclave on the existing
bench within noise before the Rust path is touched. If it does not match, the
Rust path stays and the reason is written into `docs/EDGE.md`.

**Size:** L. **Risk:** high — this is the item most likely to be dropped, and
dropping it costs nothing that A1–A4 do not already deliver.

### A6. Explicitly out of scope

**The conductor's `<act>` loop stays in Rust.** A tool loop is imperative by
nature: read, edit, run, observe, decide. Rebis's cost model is about firings,
and a tool loop's cost is dominated by steps that are not firings. Expressing it
in Rebis would make the cost *less* legible, not more. Written here so the
question is closed rather than reopened.

---

## B. The sigil operation, faithful to the source

**Provenance:** *"revisit the books at code/ about chaos magick"*, against the
drawn-sigil feature built earlier in the session (`kaos-core/src/ink.rs`,
`Pane::Ink`).

Carroll, *Liber Null*, "Sigils": the operation has three parts — the sigil is
**constructed**, it is **lost to the mind**, it is **charged**. What is built is
faithful on construction-as-drawing and on stacking. It contradicts the source
on the other two.

### B1. The word method

*"I wish to obtain the Necronomicon"* → eliminate repeated letters →
`INSHTOBANECRM` → rearrange into a glyph. Figure 2a.

**Interface:** a pure function in `ink.rs`:

```rust
/// Carroll's word method: the desire, with repeated letters removed.
pub fn letter_skeleton(desire: &str) -> String;
```

The Ink pane offers it as a scaffold — the surviving letters drawn faint on the
paper, to draw over and then discard. The scaffold is never saved; only the
strokes are.

**Acceptance:** `letter_skeleton("I wish to obtain the Necronomicon")` returns
the letters of `INSHTOBANECRM` in first-appearance order, case-folded,
non-letters dropped. The scaffold does not appear in the raster handed to a
model.

**Size:** S. **Risk:** none — additive, and it is the one part of the operation
the source specifies exactly enough to test.

### B2. Losing the sigil — **a mode, not a default**

*"To successfully lose the sigil, both the sigil form and the associated desire
must be banished from normal waking consciousness."*

`Stack.said` keeps the stated desire in readable prose beside the glyph, and the
Wall lists every sigil by name, permanently. That is the opposite of the
operation, by construction.

**This trades a real feature — finding your sigil again — for fidelity, so it
is a mode the operator chooses, not a default.** Making it the default would be
choosing the book over the user.

**Interface:** `Wall::commit(name, sigil, desire, Retention)` where
`Retention::Keep` is today's behaviour and `Retention::Lose` writes the glyph
and discards the desire unwritten. The Ink pane offers "charge and lose"
alongside "save".

**Acceptance:** after a `Lose` commit, the desire appears in no file on disk and
in no run record. Tested by writing a distinctive desire, committing, and
grepping the whole wall directory and the run store for it.

**Size:** M. **Risk:** medium — a user who loses a sigil and wants it back
cannot have it. The button must say so.

### B3. The record

*"A record should be kept of all work with sigils but not in such a way as to
cause conscious deliberation over the sigilized desire."*

Run records echo the prompt. For a run whose program came from a sigil stack
committed under `Retention::Lose`, the record keeps the firing, the glyph
reference, and the answer — not the sentence.

**Acceptance:** a lost-sigil run's retained trace contains the glyph's filename
and the answer, and does not contain the desire. **Size:** S. **Depends on:**
B2. **Risk:** low.

### B4. Charging — documentation only

The run *is* the charge; the operation is already complete in the code. This is
a paragraph in `docs/` mapping the three parts onto compose / commit / run, so
the next reader does not rebuild it. **Size:** XS.

---

## C. The language decisions

**Provenance:** `rebis/docs/PLAN_LISP.md` §6.2 and §9. These are **decisions**,
not features; each is finished when it is written into `SPEC.md`.

### C1. `$`'s evaluation strategy — the map/fold/filter blocker

`$` interpolates its operands rather than running them, and that is
load-bearing: a text constant is a macro whose body is a prompt, and `$` must
read it without firing. So `($ (f head) (map f tail))` cannot see that its
second operand is going to be a list. Lifting a lone scalar was tried and
reverted — it broke `($ "list=" '(1 2 3))`, and writing a list *into* a prompt
is the commoner thing.

**Three options:**

1. **Leave it.** Document that Rebis has no `map`/`fold`/`filter`, and that the
   mediator square is the fan-out. Costs nothing; the library already ships 11
   list macros without them.
2. **Position dispatch.** `$` in list position conses, in text position
   interpolates. Consistent with how the rest of the language dispatches — but
   "list position" must be defined, and this changes what **every text-plane
   program costs**.
3. **A fold mediator over syntax.** `[M]` already folds in the numeric plane.
   Extending it to syntax may cover the real need without touching `$` at all.

**Recommendation:** try (3); if it covers the cases, take (1) and close the
question. (2) is a cost-model change and needs evidence, not preference.

**Acceptance (of the decision):** `SPEC.md` states which, and why, in a
paragraph a reader can disagree with. **Size:** S to decide, M if (3).

### C2. Closures

`PLAN_LISP.md` §Phase 5 already recommends: land anonymous `~` with **dynamic
scope, documented as such**, measure whether anything real wants capture, and
treat closures as a separate proposal with its own evidence. The tension is
that a captured environment makes what a run carries into a firing invisible on
the page — directly against cost readability.

**Action:** record the recommendation as the decision in `SPEC.md`. Do not
build closures. **Size:** XS.

### C3. The empty list's spelling

`'()` requires a position-dispatched exception in a parser that refuses empty
groups everywhere else, and `([count] '())` must answer `0`. **Action:** decide
and write it down. **Size:** S.

---

## D. What would make it the best agentic harness

**Provenance:** *"also improve it make it the best agentic harness"*. Grounded in
what is measurably weak, not in a feature list.

### D1. Abstention — the biggest single gap

There is no abstention anywhere in the harness. `grep -rn "abstain"` returns
nothing. A run that cannot verify its work ships it anyway.

Memory, from the agi-testing work: *the Lean gate's win was **soundness** — 0
false ships, abstains — **not** accuracy.* That is the measured result, and the
harness does not implement it.

**Interface:** a third outcome beside success and failure.

```rust
pub enum Outcome {
    Shipped { answer: String, gate: Verified },
    Failed { error: String },
    /// The work was done and could not be verified. Deliberately NOT a
    /// failure: the distinction is the whole point.
    Abstained { work: String, why: String },
}
```

An abstention is surfaced as an abstention in the TUI, the visual run browser,
and the CLI exit code — not folded into either neighbour.

**Acceptance:** on a corpus of seeded defects where the gate cannot pass,
**zero false ships**. This is the criterion; accuracy is explicitly not.

**Size:** L — it touches the conductor, the run model, and three frontends.
**Risk:** medium — a harness that abstains looks worse on any accuracy metric,
and that is the trade being bought deliberately.

### D2. Restarts on Rebis nodes and chats

`spiral.rs` implements Fibonacci restart scheduling with solar/lunar polarity,
grounded in restart theory against heavy-tailed solve times. It is wired into
the `/code` path (`main.rs:2609, 2747, 2789`) — and **not** into `chat_task`,
and **not** into `RebisOracle`. A Rebis model node gets one session and no
restart; if it wanders, it wanders to the timeout.

**Interface:** `RebisOracle` runs its node through `spiral::budgets` when the
node's first attempt fizzles (`spiral::fizzled` already exists and reads a
session honestly).

**Acceptance:** a node whose first attempt fizzles is retried on a fresh context
at the next Fibonacci rung with the opposite polarity, and the trace shows both
attempts and which one produced the answer. The total step budget does not grow
— the schedule redistributes it, per `spiral::budgets`' own contract.

**Size:** M. **Risk:** low — the mechanism is built and tested; this is wiring.

### D3. Show the cost before it is paid

This is the payoff of the whole chaos-mode design and it is the smallest item
here. A composed program is queued in the run browser. The browser shows the
source. It does not show **the number of firings the program will make**, which
is the one number the language exists to make countable.

**Interface:** a function in `rebis-lang` (or kaos, if it can be done by walking
the parsed tree) returning the static firing count, plus the conditional part
as prose — the same split `tests/costs.rs` already makes between `Cost: nothing`
(exact, checkable) and conditional costs (prose, for a reader).

**Acceptance:** the run browser shows `N firings + conditional` for a queued
program before launch, and `N` agrees with what the run actually fires for every
program in `kaos-conformance/programs/`.

**Size:** M. **Risk:** low. **Value:** this is the feature that makes the claim
visible to a user instead of only to a test.

### D4. Conformance, re-run and recorded

Last run: 75/75 against qwen3-32b, **before** the last several features
(attachments, the sigil wall, agent-to-agent prompting, the ollama host
setting, chaos mode). The number is stale and is quoted as if it is not.

**Acceptance:** `kaos-conformance` re-run against qwen3-32b, the result written
into `kaos-conformance/FINDINGS.md` with the date and the commit, and any
regression fixed or documented.

**Size:** S to run, unknown to fix. **Risk:** this is the item most likely to
find something.

### D5. The gate is the harness's verification story

A2 (the shell gate in Rebis) and D1 (abstention) are the same feature seen from
two sides: a gate that can fail is what makes abstention possible. Build A2
first; D1 has somewhere to attach.

---

## E. Operations

### E1. Rotate the OpenRouter key — **do this first, today**

An OpenRouter API key was pasted in plaintext earlier in this session and is in
the transcript. It must be revoked at the provider and reissued. Nothing else in
this document is urgent; this is.

**Acceptance:** the old key returns 401. **Size:** XS.

### E2. Decide what `cargo fmt` means here

`cargo fmt --all --check` reports diffs in 23 files, most of them untouched by
recent work. So formatting cannot be a CI gate today, and every future diff
carries unrelated reformatting noise.

**Two options:** adopt rustfmt wholesale in one mechanical commit that touches
nothing else, or add a `rustfmt.toml` that matches the house style (the codebase
favours wide lines and dense prose docs, which default rustfmt fights).

**Recommendation:** one mechanical `cargo fmt --all` commit, alone, immediately
after E3 — so it is trivially reviewable as "formatting only".

**Acceptance:** `cargo fmt --all --check` is clean and is in CI. **Size:** S.

### E3. Commit the working tree

The tree carries a large uncommitted change set: 8 new modules, a new crate
(`kaos-conformance`), the sigil wall, chaos mode, and the Rebis language work in
the sibling repository. It should be committed in coherent pieces before any of
the above starts, so that a plan step can be reverted independently.

**Order matters** (from memory: *check git deps before every push*): `rebis` is
pushed first, then kaos's `Cargo.toml` is verified to use the git dependency and
never `path = "../rebis"`.

**Size:** M. **Risk:** low, but doing the work above on top of an uncommitted
tree makes every step irreversible.

---

## F. Sequencing

```
E1 rotate key            ── today, alone
E3 commit the tree       ──┐
E2 cargo fmt             ──┘ before anything else starts

A1 prove myth ≡ Rebis    ──┬── gates all of A
A2 the shell gate        ──┤        └── gates D1
A3 KAOS_MYTH as Rebis    ──┤
A4 retire myth.rs        ──┘

D4 conformance re-run    ── independent, do early, may add work
D3 show the cost         ── independent, small, high visibility
B1 word method           ── independent, small
C1 the $ decision        ── independent, decision first

D2 restarts on nodes     ── independent, wiring
B2 losing the sigil      ──── B3 the record
D1 abstention            ── after A2
C2, C3, B4               ── write down, any time
A5 the Rebis conclave    ── last, and droppable
```

**A first pass that is worth shipping on its own:** E1, E3, E2, A1, D4, D3, B1,
C1. That is one coherent release: the tree is committed and clean, the second
orchestration language is proven redundant or proven necessary, the conformance
number is true again, a queued program shows what it will cost, the sigil
surface gains the one thing the source specifies exactly, and the language's
open blocker is decided rather than deferred.

---

## G. Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | A1 shows the myth↔Rebis mapping does **not** hold | medium | A2–A5 invalid | A1 is a test, not a refactor; a failed A1 costs one file and produces the gap list as its deliverable |
| R2 | A5 loses the measured +23pt conclave edge | high if attempted | a number that was expensive to get | Build beside, not instead; bench must match before the Rust path is touched; A5 is explicitly droppable |
| R3 | D1 makes every accuracy metric look worse | **certain** | reads as a regression | It is the trade being bought. State it in `docs/EDGE.md` before building, with the 0-false-ships criterion as the actual target |
| R4 | B2 loses a user's sigil irretrievably | medium | user data | It is a mode, never a default; the control says what it does before it does it |
| R5 | C1 option (2) changes what every text-plane program costs | high if chosen | every shipped program's price | Recommendation is (3)-then-(1); (2) requires evidence, and `tests/costs.rs` is the instrument |
| R6 | D4 finds regressions in shipped features | medium | unplanned work | Run it early (it is in the first pass) so the work is discovered before the plan is committed to |
| R7 | A2's host mediator becomes an unaudited shell-execution surface | medium | authority escape | It goes through the same gate `--allow-tools` guards; a mediator that runs commands is refused without it |
| R8 | E2's mechanical fmt commit hides a real change | low | review blindness | It is committed alone, and the diff is verified to be whitespace-only |

---

## H. What this deliberately does not do

- **No new Rebis operators.** The set is 22 and has survived this entire body of
  work unchanged. A2 in particular is designed around that constraint rather
  than through it.
- **The `<act>` loop stays imperative.** See A6.
- **No closures.** See C2 — dynamic scope, documented, until something real
  wants capture.
- **No accuracy chase.** D1 is a soundness feature and will cost accuracy. That
  is the measured lesson, not a compromise.
