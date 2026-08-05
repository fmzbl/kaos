# Rebis context used by Kaos chat and agents

Kaos injects this reference into the `/chat` coding agent and every executing
Rebis node. When authoring, use it when a user asks for a Rebis program, an
explanation of Rebis code, or help correcting a program. Prefer valid structural
Rebis over invented Lisp forms. Preserve the user's intended number of model
calls and point out when a macro duplicates an argument and therefore repeats
work. When executing a node, use the reference to understand the surrounding
language but follow the node prompt and return only its flow value.

## Core rules

- A file may contain several top-level forms. They share one implicit program
  scope; an extra outer group is optional.
- A quoted string is a raw model prompt and is the primitive that fires a model.
  Rebis never interpolates variable names inside quoted strings.
- A bare atom is a symbol. Symbols name macros, macro parameters, modules, and
  deterministic judges; a symbol alone does not call a model.
- `E/provider:model` optionally routes every model call in expression `E`
  through that Kaos model instead of the session default. The suffix is lexical:
  nested bindings override their parent only for their subtree, while unbound
  forms inherit. It changes no value and makes no call itself. Write it adjacent
  after the `(` that heads it, for example `(/ ollama:qwen4:4b "draft")`,
  `(/ claude:opus5 (-> "a" "b"))`, or
  `(/ openrouter:anthropic/claude-opus-4 (["judge"] "a" "b"))`. A slash inside
  a symbol such as `std/loops` remains part of the symbol. Routing is a FORM,
  not a suffix — a trailing `expr/model` is no longer a route and parses as a
  stray symbol.
- `|A B ...|` is the **numeric plane** — the other pole, where values are
  quantities. NOTHING inside fires, so the whole block costs zero model calls.
  `$` is the monoid operation and so ADDITION; `[M]` is a FOLD (`[sum]`,
  `[product]`, `[max]`, `[min]`); `^` is the INVERSE, so `($ 10 (^ 3))` is 7 and
  subtraction needs no glyph; `/` is the REGIME, a modulus, so `(/ 9 38)` is 2;
  `%` is unchanged, and a comparison answers 0 or 1, which is what it already
  reads. `+` FRAMES, and a framing has never applied to a result — it reaches
  every quantity WRITTEN inside, so `|(+ 10 ([sum] 1 2 3))|` is 36, not 16. A
  name is not written and so is not shifted: `|(+ 10 (= x 5 ($ x x)))|` is 30.
  Nested framings accumulate without compounding. Quantities are integers and
  rationals, never floats. `-` is available ONLY inside this boundary.
- Text becomes quantity through a **crossing**, which is a mediator:
  `([gematria] "text")`, `([length] "abc")`, `([lines] (? topic))`,
  `([calls] <>)` for how many calls the run has made, `([atoms] <>)` for how
  large the program is, `([count] a b c)` for how many branches there are
  without reading them, `([abs] n)`, and `([round] n limit)` for the nearest
  quantity whose denominator fits `limit`. A bare prompt inside `| |` is
  REFUSED — pick a reading.
- The crossing back OUT is interpolation: a `| |` block written inside a `$`
  contributes its figures, so `($ "keep it under " |([product] 2 3)| " lines")`
  asks for six. Nothing fires, so it costs no call.
  A quantity crossing out is its decimal form and a numeral crossing back in
  reads as itself, so `(= n |($ 2 3)| |($ n 1)|)` is 6.
- `(* topic A)` is **supersede** — the one operation that corrects the record.
  `A` runs and answers as usual; between the two, every line the topic reaches
  DIRECTLY and that is older than this form is buried, so a later `?` on the
  subject returns the correction rather than what it replaced. Without it a run
  that changes its mind holds both beliefs, because only ties break toward the
  newer line. It is a tombstone not a deletion (ids stay stable, the text stays
  in the trace); it never buries its own correction; the topic is NOT broadened,
  so it cannot evict a neighbour for co-occurring; a `nothing` buries nothing;
  and what it buried is reported.
- `(@ check A)` is an **invariant** over every arrow in `A`. After each stage
  `check` receives both sides as `RESULT 1` and `RESULT 2` — the same transport
  a square uses — and must answer exactly 0 or 1. It scopes like `+` — the whole subtree, any depth — but `+` rewords what
  is asked and is free, while this ADDS A CALL at every arrow and can stop the
  run. A
  refusal STOPS the scope — the rest is skipped and it answers nothing — and
  names the edge. Invariants do not apply inside their own check. Cost is one
  check per arrow: a call each with a model judge, free through `std/seam`'s
  port, nothing at all if the scope has no arrows.
- `{A B ...}` is an **imaginary space** — a group in every respect but one:
  what happens inside it does not become evidence. Operands run in source
  order, the last one's answer is the space's answer, and only that answer is
  appended to the record. Everything else the body produced is discarded when
  the boundary closes. The boundary fires nothing, so `{A}` costs exactly what
  `A` costs. Use it when a program must EXPLORE without the exploration
  polluting what it remembers: six candidate routes leave six answers in the
  record, and a later `?` on the subject returns the five dead ends ranked
  against the one finding. Inside a space, memory works normally — a flashback
  reaches what that space's own earlier stages answered — so multi-stage work
  lives in one happily. Nesting is relative: an inner space's answer becomes
  real to the space around it, and only the outermost crossing reaches the run.
  `^` recurses through a space without touching the boundary. `{}` with nothing
  in it is a syntax error.
- `(! A)` **inside** a space keeps for the PLANE, not the run: it never reaches
  the host's durable memory, but every space opened afterwards can recall it.
  That is how a search remembers its own dead ends without recording them —
  `{(! A) nothing}` learns something and leaves the record untouched. To keep
  past the run, put the dream outside: `(! {...})` marks what crossed.
- `<>` is **source** — the program itself, as syntax rather than as a string.
  Rebis is homoiconic, so it needs no rules of its own: as a `$` or `?` operand
  it contributes its canonical TEXT (like a macro's expanded text, without
  firing); in an executable position it RUNS, which is genuine self-
  application; inside a quote it is syntax. `($ "Improve this: " <>)` reads the
  program; `("work" <>)` re-enters it. Reading costs nothing. Running has no
  base case and spends the macro-expansion budget, exactly as a recursive macro
  does — bound it with a gate. `<>` is punctuation, so it terminates a bare
  word and cannot be bound away. A program that never writes `<>` is unchanged.
- `(>< A B ...)` is **meta** — a prompt whose answer is a PROGRAM. The operands
  interpolate to one prompt exactly as under `$`, it fires once, and the answer
  is parsed as Rebis and RUNS — generating and running are one act, so under
  `$` it contributes nothing and does not fire, like every executable form. To
  review a program before paying for it, ask for source with an ordinary prompt
  (that is text) and hand it out through `&`. It runs in the live definition
  scope, so a
  generated program may call the macros this one imported. Text that does not
  parse is a diagnostic carrying what was written, never a silent skip. Charged
  to the macro-expansion budget. Two warnings worth stating: what a generated
  program costs cannot be read off the page, and a model's answer stops being
  only data — under `><` it is executed. Execution is POSITIONAL, so wrap it to
  control it: `{(>< ...)}` leaves no evidence, `(% gate (>< ...) fallback)`
  runs it only if something accepted it. There is no destructuring, so macro
  work means wrapping the generator, not transforming its output.
- `($ A B ...)` is string composition — the one operator over the string value.
  It **interpolates** its operands into one string and yields that string;
  nothing inside `$` fires or runs. An operand contributes its text: a prompt its
  characters, a symbol its bound value, a macro its expanded text (NOT fired), a
  nested `$` its assembled text; any other (a program) contributes nothing. The
  assembled string is a prompt in the position the `$` sits, so it fires there,
  once. It never looks inside a quoted string, so a `$` in a prompt is literal
  text. Interpolation adds no model calls, so reusing a value is free. Variables
  are macro parameters: bind names with `(~ f (a b) ...)` and weave them in with
  `$`. A text constant is just a macro whose body is a prompt — `(~ topic ()
  "the fall of Rome")`, used as `($ "Write on " (topic))`, weaves its text in
  without firing. To carry a model-*computed* value into a prompt, use `->`.
- `(? A B ...)` is a **flashback** — the language's one read of memory, and its
  one free form. It builds a topic exactly as `$` builds a string, then answers
  with what the run's record already holds on that topic. **No model fires**, in
  it or beneath it, and the recalled text is never appended back to the record.
  Recall is TOPICAL, not addressed: the topic's content words are broadened one
  hop through the record's co-occurrence graph, and the evidence lines they reach
  come back strongest first, newest breaking a tie, at most eight. So `?` answers
  "what has this run learned about X" — for the previous stage's exact value use
  `->`, and for several values at once use `[M]`. A recall resolves where it
  SITS, so two identical recalls answer differently once a stage between them has
  added evidence: memory accumulates during a run. A topic the record holds
  nothing on answers with `nothing`, which does not flow and adds no characters —
  so a program degrades to its memoryless form rather than asserting a false
  memory. Because recall is pure it interpolates inside `$`, which is how a
  prompt is written around its own memory:
  `($ "Given what we found: " (? "retry queue") " — now propose a fix.")`.
  `(^ (? A))` is `(? A)`. See `std/memory` for the shapes built on it.
- `(A B C)` is a group. Its executable children run in source order, but their
  answers do not automatically feed one another.
- `(-> A B C)` is forward flow, and needs AT LEAST TWO operands — an arrow is a
  route between stages, so `(-> A)` is an error, not a wrapper. Write the bare
  form when there is only one thing to run. `A` runs first; each accepted answer becomes an
  `INPUT:` to the next stage; the value is `C`.
- `(<- A B)` is deliberately simple reverse-flow sugar and is exactly
  equivalent to `(-> B A)`. Do not assign adversarial or hidden semantics to it.
- `(^ E)` is the pure syntax inverter. It recursively exchanges `->` and `<-`
  while preserving written operand order. Groups, squares, and quotes retain
  their structure; prompts, symbols, imports, macro definitions, and all of `$`
  or `?` are fixed. A macro call expands before its resulting graph is inverted. It makes
  no model call, and `(^ (^ E))` is exactly `E`. This is an orientation dual,
  not a semantic undo operation for natural-language prompts.
- `([M] A B C)` is convergence. The branches run, absent answers and `nothing`
  are dropped, and executable mediator `M` receives the remaining labeled
  results. A prompt mediator calls the model; a symbol mediator judges
  deterministically without another model call.
- `(% condition when-one when-zero)` is the explicit lazy binary gate. It runs
  the condition first, accepts only exact `0` or `1` (surrounding whitespace
  ignored), and expands only the selected continuation.
  `[]` is never conditional; it always evaluates all branches before its
  mediator. The `std/control` wrappers route conditions through `std-binary`,
  whose `$`-assembled instruction asks for one exact decision token.
- `(~ name (parameters) body)` defines a structural macro. Arguments are raw
  Rebis syntax, not pre-evaluated string values. A quoted prompt passed as an
  argument remains an executable prompt wherever its parameter symbol appears.
- A quoted *program* is a macro output template: `'(... )` is returned by
  expansion instead of running during it, and caller syntax is inserted with
  `,` — use forms such as `(,worker ,value)` when a parameter is itself a macro
  name. That is `'`'s only role; there is no separate "data string".
- `(# module)` imports top-level definitions without executing the module.
  Personal modules live under `~/.kaos/sigils`; qualified paths and folder
  imports work. `(# std)` imports the complete embedded standard library.
- `;` starts a line comment only outside a quoted prompt. A semicolon inside
  `"..."` is ordinary prompt text.
- `nothing` is the intentional absence/refusal value. Do not quote it when the
  language value is intended.
- Nested source multiplies calls when an argument is structurally repeated.
  Runtime macro, module, model-call, concurrency, and timeout limits are
  backstops; in the hosted TUI, failed prompts, timeouts, clean allowances, and
  vanished children pause. `p` resumes and retries the first unfinished prompt.
  Recursive/search examples should still mention their cost.

Useful editor commands are `/run`, `/run block`, `/run parallel`,
`/run block parallel`, `/runs`, `/chat run ID QUESTION`, `/search [TEXT]`,
`/format`, `/tree`, `/mandala`, `/output`, `/sigil save NAME`, and
`/sigil chat`. In the visual editor, every source or mandala `run` button opens
one settings modal for scope, dry/direct/chaos mode, and serial/parallel lane;
there are no competing run buttons. `/sigil chat` opens a right-panel God Agent
channel with the current source and every live bot's source, input, state,
directive, trace, and checkpoint context. A run-browser `chat` action can ask
about a running, paused, queued, or completed run; each answer receives the
complete captured source, input, state, and retained output, not only the
currently visible scroll window. Valid source revisions rebuild only the bound
run from its unchanged completed prompt prefix. Explicit user requests may
pause/resume a named live run or apply/clear guidance for its next unfinished
prompts; the channel cannot cancel or delete runs.
`/search TEXT` finds the next literal source
match with wraparound; `/search` repeats the previous query. Saving a sigil also
retains its last successful output and any unfinished run's record, trace, and
atomic prompt checkpoint; reopening restores a paused run that `p` can resume.
A visual selection
followed by `/run` evaluates only that selection while carrying top-level
definitions and imports with it.

## Example 1: deeply nested evidence synthesis

This uses only core forms. Each inner branch is independent until its enclosing
mediator resolves it; the final arrow then forwards the selected evidence into
the report writer.

```rebis
(->
  (["Select the strongest falsifiable root-cause hypothesis"]
    (->
      "Inspect the parser and identify the first corrupted value."
      (["Reconcile the parser evidence"]
        "Trace tokenization across quoted strings."
        "Check delimiter matching around nested forms."
        "Find a minimal input that reproduces the corruption."))

    (["Select the strongest independent counter-hypothesis"]
      (<-
        "Write the counter-hypothesis and its distinguishing prediction."
        "Inspect runtime state without assuming the parser is at fault.")
      (->
        "Inspect module expansion order."
        "Explain how it could mimic parser corruption.")))

  (["Challenge the selected hypothesis before accepting it"]
    "Find one observation that would falsify it."
    "Find the most likely confounding variable."
    "Check whether the reproduction distinguishes cause from correlation.")

  "Write a root-cause report with reproduction, evidence, rejected alternatives, and one decisive verification command.")
```

## Example 2: higher-order macros and nested review

`worker` is syntax naming another macro. The template must splice both the
worker and its argument. The worker is expanded twice, so this program performs
two independent investigations before mediation.

```rebis
(~ investigate (topic)
  (-> topic
      "Investigate this topic in depth. Return claims, evidence, and unresolved uncertainty."))

(~ adversarial-review (worker topic)
  '(["Select the report that remains most useful after criticism"]
    (,worker ,topic)
    (->
      (,worker ,topic)
      "Attack the report: identify unsupported claims and missing counterexamples."
      "Rewrite only the conclusions that survive the attack.")))

(adversarial-review investigate
  "Determine why the queue occasionally delivers the same payment twice.")
```

## Example 3: standard-library red team inside best-of-three

The symbol `surviving-verified-design` mediates without a judge prompt.
`best-of-three` repeats its work three times, and each `red-team` repeats its
builder, so this compact program intentionally has substantial call exposure.

```rebis
(# std/debate)
(# std/shape)
(# std/spread)

(~ build (task)
  (std-with-evidence (-> task "Propose the smallest complete design.")))

(~ attack (task)
  (-> task "Find the strongest safety, concurrency, or operability failure."))

(~ repair (task)
  (-> task "Repair the design without hiding the attack or weakening the requirements."))

(std-best-of-three surviving-verified-design
  (std-final-only
    (std-red-team surviving-verified-design
      build attack repair
      "Design an idempotent retry queue for payment processing.")))
```

## Example 4: a chaired committee inside a plan-execute-review campaign

The chair's criteria flow into every panelist. The resulting plan then flows
through implementation and review because `campaign` expands to an arrow.

```rebis
(# std/committee)

(~ chair (task)
  (-> task "Define non-negotiable acceptance criteria and explicit tradeoffs."))

(~ reliability (task)
  (-> task "Design for retries, partial failure, recovery, and observability."))

(~ security (task)
  (-> task "Threat-model trust boundaries, replay, privilege, and secret handling."))

(~ operations (task)
  (-> task "Design rollout, rollback, alerts, and incident response."))

(~ plan (task)
  (std-chaired-panel chair strongest-operational-plan
    reliability security operations task))

(~ implement (task)
  (-> task "Turn the approved plan into ordered implementation steps with verification after each step."))

(~ review (task)
  (-> task "Audit the implementation against every acceptance criterion and list remaining risk."))

(std-campaign plan implement review
  "Replace synchronous webhook delivery with a durable asynchronous pipeline.")
```

## Example 5: reflexion nested inside deterministic best-of-three

`reflexion` attempts the task, critiques the attempt, and retries with the
critique as input. `best-of-three` independently repeats that complete shape and
uses a symbol judge to choose the result that best matches the desired terms.

```rebis
(# std/reflexion)
(# std/spread)
(# std/shape)

(~ solve (task)
  (std-with-evidence (-> task "Derive a root cause and a minimal corrective patch.")))

(~ critic (task)
  (-> task "Try to disprove the proposed root cause using the observed behavior."))

(std-best-of-three reproducible-minimal-verified-fix
  (std-final-only
    (std-reflexion solve critic
      "Trace and fix the UTF-8 cursor corruption after multiline paste.")))
```

## Example 6: lazy nested routing

Only one specialist branch runs at each `%` gate. The classifiers must return
exactly `0` or `1`; `std/control` supplies the lazy wrapper and `std-binary`
supplies the answer contract.

```rebis
(# std/search)
(# std/control)

(~ parser-kind (task)
  (std-binary (-> task "Is this primarily a parsing or syntax problem?")))

(~ runtime-kind (task)
  (std-binary (-> task "Is this primarily a runtime state problem rather than an integration problem?")))

(~ parser-specialist (task)
  (-> task "Trace tokens, delimiters, quoting state, and the smallest failing input."))

(~ runtime-specialist (task)
  (-> task "Trace state transitions, ownership, concurrency, and cancellation."))

(~ integration-specialist (task)
  (-> task "Trace process boundaries, environment, filesystem context, and provider behavior."))

(std-route-three parser-kind runtime-kind
  parser-specialist runtime-specialist integration-specialist
  "A completed background run sometimes leaves its panel in the running state.")
```

## Example 7: bounded recursive refinement

Macros may call themselves. The `%` gate expands only the chosen branch, and
runtime expansion/model-call limits prevent an unbounded run. The stop macro
must return exactly `0` or `1`.

```rebis
(# std/loops)
(# std/control)

(~ improve (value)
  (-> value "Rewrite the plan so each step is smaller, reversible, and independently testable."))

(~ done (value)
  (std-binary
    (-> value
        "Are all steps independently verifiable, with an explicit rollback?")))

(std-loop
  "Draft: migrate the billing schema and deploy every dependent service in one step."
  improve
  done)
```

## Example 8: a long program that does not forget its own beginning

An arrow chain gives each stage only its predecessor's answer, so `(-> a b c)`
leaves `a`'s finding two steps back and unreachable. `?` fixes that without a
single extra model call: every stage recalls the whole record on the subject, and
because a recall resolves where it sits, each one sees what the earlier stages
added. The arrows here carry sequencing only.

```rebis
(# std/memory)

(~ reproduce (subject) ($ "Reproduce the failure in " subject ". Report what you observed."))
(~ diagnose  (subject) ($ "Give the mechanism behind " subject "."))
(~ repair    (subject) ($ "Propose the smallest correct fix for " subject "."))

(std-arc "the retry queue" reproduce diagnose repair)
```

Three calls — one per stage. `std-grounded` inside `std-arc` prefaces each
prompt with the accumulated evidence, so `repair` reasons from what `reproduce`
observed rather than re-deriving it. When a topic may have no memory yet, gate on
it with `(std-recalled subject remembered forgotten)`: that costs one call for
the gate and expands exactly one branch.

## Debugging checklist

When correcting a Rebis program, check these before changing its design:

1. Every `->` and `<-` has at least two operands. A single-operand arrow is the
   most common mistake: an arrow ROUTES between stages, so one stage is not an
   arrow at all — write the form on its own.
2. Every `(` matches `)`, and a mediator is written `[M]` — square brackets, never
   angle brackets. `(<M> A B)` is NOT a syntax error: `<` is an ordinary symbol
   character, so it silently parses as a call to a macro named `<` and fails later
   with something unrelated.
3. A mediator is written inside a group: `([M] branch-a branch-b)`.
4. Macro parameters occur as bare symbols, not as words inside quoted prompts.
5. Higher-order macro calls use quote/unquote correctly: `(,worker ,value)`.
6. Imported modules contain definitions/imports only; executable module bodies
   are rejected.
7. A `%` classifier returns exactly `0` or `1`.
8. Repeated macro parameters intentionally repeat model work.
9. `<-` has only reverse-flow semantics; rewrite it as `->` when direction is
   unclear.
10. A model suffix is adjacent to a closing quote or `)` and names a Kaos model;
   whitespace before `/` does not bind it.
11. Use `/format` or `/tree` to validate structure before spending live model
   calls. `kaos rebis run --dry` also expands and traces model-free shapes, but
   a model-driven `%` gate will intentionally report no decision when its dry
   oracle returns `nothing`.

## Parser and runtime checks

Kaos delegates syntax authority to `rebis-lang`; there is no second, subtly
different Kaos parser. Check a complete file without model calls from the Kaos
checkout with:

```sh
cargo run --manifest-path ../rebis/Cargo.toml -- check path/to/program.rebis
kaos rebis run --dry path/to/program.rebis
```

For a source-only collection module, point the host at the collection and run a
small importing program:

```sh
REBIS_COLLECTION_PATH=../rebis-collection/modules \
  kaos rebis run --dry '((# git/workflow) (git-intent "judge" "task"))'
```

Use `rebis tree` or Kaos `/tree` to inspect the expanded structure. A dry run
can validate imports, macro expansion, arrows, and ordinary prompts without
answering them; `%` gates still need a scripted `0` or `1` oracle to
exercise their selected branch.
