# The Further Operations — a researched program for making every model better

Two research passes feed this document: a full mining of the primary texts
(*Liber Null & Psychonaut*, *Liber Kaos*) for mechanisms kaos does not yet
exploit, and a survey of the test-time-compute literature (2021–2026), every
number checked against its primary source. The two converge to a degree that is
frankly eerie — the books name both the mechanisms and their failure modes, and
the papers measure them.

House rule, as always: **a mechanism ships only when a deterministic checker
shows it pays against a cheap control.** Carroll: *"Objective results are the
proof of magic, all else is mysticism"* (Liber KKK).

## Where the evidence converges

Every high-confidence literature result that fits this harness (local model,
shell-command gates, no reward-model training) points at one architecture — and
each piece already has a name in the order:

| The operation | The evidence | Status in kaos |
|---|---|---|
| **Decomposition raises per-step P** (the Working) | Least-to-Most: 99% vs 16% on compositional tasks (arXiv:2205.10625); SWE-agent: the interface, not the model, was the binding constraint (2405.15793) | built — `working.rs`, `/chainbench` |
| **Repeated sampling under a deterministic gate** (the Conclave) | Codex pass@1 28.8% → pass@100 72.3% (2107.03374); Monkeys: SWE-bench Lite 15.9% → 56% at 250 samples, log-linear coverage (2407.21787) | built — `/code xK -- gate`, `agentbench` |
| **Adaptive early stopping** (the adjourned quorum) | Adaptive-Consistency: 3.3× fewer samples at <0.1% accuracy loss (2305.11860); ESC: −80% cost on GSM8K (2401.10480) | built — `scry.rs`, lossless variant |
| **Difficulty-aware allocation** (the second equation / scry) | Snell et al.: compute-optimal per-difficulty allocation ≈ 4× cheaper than fixed best-of-N (2408.03314) | built — mid-band law, two-tier scry |
| **Cascade with a hard escalation signal** (the Ladder) | Mixture-of-Thought cascade: GPT-4-level at 40% of cost, *agreement* as the untrained escalation signal (2310.03094) | built — `ladder.rs` (gate-signal variant) |
| **External-feedback-only refinement** | Reflexion's own ablation: with executed tests 68%, *without* them 52% — worse than baseline (2303.11366); CRITIC without tools: negative (2305.11738) | partially — the conductor feeds gate output back; see Operation 2 |

And the two hard limits the literature proves, which the books also state:

- **Coverage is the ceiling.** No sampling scheme exceeds the base model's
  support (Yue et al. 2504.13837); Carroll: the bottom line of the second
  equation's graph stays at zero for any M < 1. Decomposition is the only lever
  that *moves* the floor, because it changes what is being asked.
- **Small models must not judge themselves.** The generation–verification gap
  *shrinks* with scale (Mind the Gap, 2412.02674) — the smaller the model, the
  more external the verifier must be. Intrinsic self-correction is net-negative
  even for frontier models (GPT-4 GSM8K 95.5 → 89.0 over two self-correction
  rounds; 2310.01798). Carroll: *"The only defense against these pitfalls is to
  adhere to the formal techniques"* — the gate, not the vibe.

## The next operations, ranked

Ranked by expected value per line of code, with the grounding quote, the
literature verdict, and the kill-criterion each must survive.

### Operation 1 — The Insubordinate (grounded critic with veto)

Most organizations act as though mad and stupid, because most organizations
permit only positive feedback from below; the Pact institutionalizes rebellion
in the office of Insubordinate — with five enumerated duties: Fool (demand
clarity), Jester (convey criticism), Chaplain (point out blind spots),
Confessor (receive accounts), Inquisitor (veto).

**Build:** a designated critic wired into `/code`, whose every duty is grounded
in an **external channel** — this is the non-negotiable synthesis of Carroll
with Huang et al. (self-critique without external feedback degrades):
Fool = spec-lint *before* any adept spends tokens (kills the v0-sigil failure
class at the source); Chaplain = audit the winning diff against signals the
gate does not cover (does it delete tests? touch unrelated files? shrink
coverage?) — mechanical checks, not model opinion; Inquisitor = ship-veto,
logged. **Measure:** false-ship rate against a held-out stronger checker — this
attacks the standing EDGE.md caveat ("a weak gate still ships weak work").
**Kill if:** vetoes are mostly false alarms (measured against the held-out
gate) or the spec-lint gains nothing on pass@1.

### Operation 2 — Retroactive Enchantment (rewrite the failed past)

*Liber Kaos*: *"we can change our future by redefining our past… by an effort
of visualization we can write in parallel, enabling memories of what might also
have happened, to neutralize the originals."*

**Build:** a third retry policy between keep-everything (rot, high R) and
banish (lose the map of dead ends): after a failed Weighing, rewrite the
transcript into a short curated memory — *"X was tried; it fails because Y
(gate output); the missed constraint is Z"* — and retry from that. The gate's
stderr is the enabling memory's raw material, which is exactly the
configuration Reflexion's ablation proved (external test output routed into
the next attempt: 68% vs 52% without). **Measure:** three-arm per-retry pass
rate on the agentbench tasks: full transcript vs banish vs enchanted memory.
**Kill if:** it doesn't beat *both* arms.

### Operation 3 — Random Belief (paradigm-rotated conclave)

*Liber Null*: the belief-cycle die — *"Try each or any of them… chosen by the
sacred cube."* *Liber Kaos*: *"beliefs are not seen as ends in themselves, but
as tools."*

**Build:** each conclave member gets a distinct *approach frame* (minimal diff /
rewrite from spec / add the missing guard / distrust the happy path / trace the
data flow), dice-assigned from the master seed. Voting lift depends on error
*independence*; seeds decorrelate token noise, paradigms decorrelate
*assumptions* — the 5–0 wrong-consensus failure lives there. **Caution from
both sources:** realbench already killed persona framing for single-sample
accuracy, and Self-MoA (2502.00674) shows cross-diversity can be a quality
tax. The claim to test is narrower: *ensemble decorrelation*, measured as vote-
split entropy on wrong-vs-right tasks plus end accuracy. **Kill if:** accuracy
does not beat the seed-only conclave at equal k.

### Operation 4 — Take All Ordinary Steps (the mundane pre-pass)

*Liber Kaos*: *"take all possible ordinary steps to increase the probability of
the desired result occurring by chance alone, before and after using magic. To
do otherwise is basically to subconsciously challenge your magic to fail."*

**Build:** deterministic tooling runs before and after every adept — format,
lint, compile-check — so P rises by mundane means and M is spent only on the
residual (the mid-band law then does the allocation). SWE-agent's lint-gated
edits are the measured precedent (13% of malformed edits prevented at zero
model cost). **Measure:** pass@1 with and without the pre/post-pass. Cheap,
likely real, unglamorous. **Kill if:** it somehow doesn't pay (it will).

### Operation 5 — Gnosis Scheduling (hot generation, cold extraction)

*Liber Null*: inhibitory and excitatory gnosis *"can be employed sequentially,
but not simultaneously, in the same operation"* — and the dosage warning: *"a
large dose leads to depression, confusion, and a general loss of control."*

**Build:** phase-scheduled sampling on the existing `backend::Sampling` seam —
high temperature for the reasoning/generation phase, near-greedy for the
answer-extraction/patch-emission phase, never mixed in one sample; plus a
temperature ceiling (the overdose rule). **Measure:** realbench arms at matched
sample counts. **Kill if:** no lift over the fixed 0.7.

### Operation 6 — Servitors (capability-capped background workers)

*Liber Kaos*: *"Evoked entities should never be allowed to exceed the powers
that the magician built into them… you are their master; if you start accepting
advice from them the results can be disastrous. Four entities are usually
sufficient."*

**Build:** a long-lived research servitor per `/code` session — read-only
explorer that indexes the target, caches *negative* results ("no test suite
exists", "symbol absent") so adepts stop re-running dead-end searches. The 1992
safety spec translates verbatim: tool allowlists fixed at evocation; ≤4
concurrent; **servitor output is data, never instructions** (the
prompt-injection rule). **Measure:** duplicate dead-end tool calls avoided +
wall-clock per task. **Kill if:** the cache hit rate doesn't cover its cost.

### Operation 7 — Illumination over the Diary (benchmark-gated self-modification)

*Liber Kaos*: illumination objectives must be *"precisely specified and
measured… Only those forms of illumination that lead to useful behavioral
changes deserve to be known as such."* *Liber Null*: the magical record is
*"the surest guarantor of success."*

**Build:** (a) the diary — a first-class per-rite JSONL record (task, ray,
A/G/L/R, verdict, spend; failures logged, *"no page should be left blank"*);
(b) a periodic illumination rite that mines the diary for ONE small weakness
and proposes ONE config/prompt patch; (c) the patch ships only if it beats the
fixed benchmark. **The Choronzon rule** (see failure table) makes the gate
non-negotiable: self-modification without a deterministic Weighing is the most
dangerous build in this program — *"some magicians attempting to go too fast…
have gone spectacularly insane as a result."* **Kill if:** three consecutive
illumination cycles produce no gate-passing patch — then the diary stays (the
Confessor and the bandit consume it) and the auto-patcher goes.

### Operation 8 — Agreement-gated escalation (the Ladder without a gate)

Mixture-of-Thought (2310.03094): where no shell gate exists, **vote agreement
is the escalation signal** — the weak model samples k answers; unanimity ships,
disagreement summons the Magus. This extends `ladder.rs` beyond verifiable
work at zero training cost, and it is exactly the two-tier scry generalized to
a cascade. **Measure:** accuracy vs cost against all-strong on the realbench
task set. **Kill if:** the leak (confident-but-wrong unanimity) eats the
savings.

## What we refuse to build, and why

- **Multi-agent debate / Mixture-of-Agents.** Compute-matched, debate does not
  reliably beat self-consistency (Smit et al. 2311.17371); single-best-model
  self-sampling beats mixed-model aggregation (Self-MoA). The conclave already
  is the defensible residue. Carroll concurs on committees: *"the effects of a
  number of persons conjuring for a common objective never exceeds the best
  result that any one of them might achieve."*
- **Intrinsic self-critique loops.** Net-negative without external feedback
  (2310.01798; Self-Refine's own GSM8K +0.2; CRITIC-without-tools negative).
  The books call this the omnipotence spiral: *"the final madness begins when
  one starts interpreting even the disasters which befall as expressions of
  what one must really have wanted."* Refinement in kaos is gate-output-driven
  or it does not exist.
- **Soft-verifier over-search.** Best-of-n against any imperfect proxy rises
  then *falls* (Gao 2210.10760; Cobbe's n>400; Snell's beam-search reversal).
  kaos's gates are deterministic precisely to live in the one regime where
  best-of-k is monotone and unhackable. Never substitute an LLM judge for the
  Weighing.

## The failure modes, named in 1987–92, observed in 2023–26

| Carroll names it | The agent pathology | The defense in kaos |
|---|---|---|
| Choronzon — *"unbalanced scraps of ego… bloat into grotesque monsters"* | runaway self-modification | Operation 7's benchmark gate; small patches; go slow |
| The omnipotence spiral | reward hacking; LLM-judge drift | deterministic gates only |
| Obsession — *"frequent recourse to banishing if it threatens to obsess"* | the stuck retry loop | step budgets, banishing, Confessor detection |
| Liber Boomerang — *"that which is denied gains power"* | suppressing reasoning deforms output (realbench v2, measured) | never forbid chain-of-thought; channel it |
| *"A demon is a god acting out of turn"* | privilege escalation between roles | office boundaries in code, not prompts |
| Servitor advice — *"if you start accepting advice from them…"* | prompt injection via tool/subagent output | output is data, never instructions |
| Excessive identification — *"leads inexorably to sterility"* | mode collapse on last-winning persona | exploration pressure in the bandit; Random Belief |
| Gnosis overdose — *"jibbering idiocy or catatonia"* | temperature/effort past the payoff point | sampling ceilings (Operation 5) |
| Leakage — *"no spell is ever totally insulated"* | cross-task context contamination | per-ray egregore partitions; decode-check sigils |

## The frontier question, answered by both traditions

Test-time orchestration closes the gap to frontier models **iff three
conditions hold**: a cheap deterministic verifier exists; the base model has
non-zero coverage on the task class; difficulty is easy-to-intermediate. Then
the numbers are real — 5 samples of a cheap open model beat 1 frontier sample
at 3.6–4.7× lower cost (Monkeys); a 3B with execution-grounded search passes
GPT-4o-mini (S*, 2502.14382). Where any condition fails — no verifier, zero
coverage, hardest bins — pretraining wins, provably, and the honest move is
the Ladder: escalate, don't sample harder. Carroll put the whole program in
one sentence of Principia Magica: work the mid-band, raise P by ordinary means
first, and never repeat a conjuration that has no chance of being done better.
