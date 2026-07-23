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
- `(A B C)` is a group. Its executable children run in source order, but their
  answers do not automatically feed one another.
- `(-> A B C)` is forward flow. `A` runs first; each accepted answer becomes an
  `INPUT:` to the next stage; the value is `C`.
- `(<- A B)` is deliberately simple reverse-flow sugar and is exactly
  equivalent to `(-> B A)`. Do not assign adversarial or hidden semantics to it.
- `(^ E)` is the pure syntax inverter. It recursively exchanges `->` and `<-`
  while preserving written operand order. Groups, squares, and quotes retain
  their structure; prompts, symbols, imports, macro definitions, and all of `$`
  are fixed. A macro call expands before its resulting graph is inverted. It makes
  no model call, and `(^ (^ E))` is exactly `E`. This is an orientation dual,
  not a semantic undo operation for natural-language prompts.
- `([M] A B C)` is convergence. The branches run, absent answers and `nothing`
  are dropped, and executable mediator `M` receives the remaining labeled
  results. A prompt mediator calls the model; a symbol mediator judges
  deterministically without another model call.
- A two-branch square can be conditional: when its mediator yields exactly
  `yes` or `no`, only the selected branch expands. This is Rebis's lazy control
  form and is what makes bounded recursive macros possible.
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
`/run block parallel`, `/runs`, `/search [TEXT]`, `/format`, `/tree`, `/mandala`,
`/output`, `/sigil save NAME`, and `/sigil chat`. `/sigil chat` opens a right-panel
God Agent channel with the current source and every live bot's source, input,
state, directive, trace, and checkpoint context. Valid source revisions rebuild
only the bound run from its unchanged completed prompt prefix. Explicit user
requests may pause/resume a named live run or apply/clear guidance for its next
unfinished prompts; the channel cannot cancel or delete runs.
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
  (with-evidence (-> task "Propose the smallest complete design.")))

(~ attack (task)
  (-> task "Find the strongest safety, concurrency, or operability failure."))

(~ repair (task)
  (-> task "Repair the design without hiding the attack or weakening the requirements."))

(best-of-three surviving-verified-design
  (final-only
    (red-team surviving-verified-design
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
  (chaired-panel chair strongest-operational-plan
    reliability security operations task))

(~ implement (task)
  (-> task "Turn the approved plan into ordered implementation steps with verification after each step."))

(~ review (task)
  (-> task "Audit the implementation against every acceptance criterion and list remaining risk."))

(campaign plan implement review
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
  (with-evidence (-> task "Derive a root cause and a minimal corrective patch.")))

(~ critic (task)
  (-> task "Try to disprove the proposed root cause using the observed behavior."))

(best-of-three reproducible-minimal-verified-fix
  (final-only
    (reflexion solve critic
      "Trace and fix the UTF-8 cursor corruption after multiline paste.")))
```

## Example 6: lazy nested routing

Only one specialist branch runs at each conditional. The classifiers must
return exactly `yes` or `no`, so `std/canon` supplies the answer contract.

```rebis
(# std/search)
(# std/canon)

(~ parser-kind (task)
  (yes-no (-> task "Is this primarily a parsing or syntax problem?")))

(~ runtime-kind (task)
  (yes-no (-> task "Is this primarily a runtime state problem rather than an integration problem?")))

(~ parser-specialist (task)
  (-> task "Trace tokens, delimiters, quoting state, and the smallest failing input."))

(~ runtime-specialist (task)
  (-> task "Trace state transitions, ownership, concurrency, and cancellation."))

(~ integration-specialist (task)
  (-> task "Trace process boundaries, environment, filesystem context, and provider behavior."))

(route-three parser-kind runtime-kind
  parser-specialist runtime-specialist integration-specialist
  "A completed background run sometimes leaves its panel in the running state.")
```

## Example 7: bounded recursive refinement

Macros may call themselves. The two-branch conditional expands only the chosen
branch, and runtime expansion/model-call limits prevent an unbounded run. The
stop macro must return exactly `yes` or `no`.

```rebis
(# std/loops)
(# std/canon)

(~ improve (value)
  (-> value "Rewrite the plan so each step is smaller, reversible, and independently testable."))

(~ done (value)
  (yes-no
    (-> value
        "Are all steps independently verifiable, with an explicit rollback?")))

(loop
  "Draft: migrate the billing schema and deploy every dependent service in one step."
  improve
  done)
```

## Debugging checklist

When correcting a Rebis program, check these before changing its design:

1. Every `(` matches `)` and every mediator `<` matches `>`.
2. A mediator is written inside a group: `([M] branch-a branch-b)`.
3. Macro parameters occur as bare symbols, not as words inside quoted prompts.
4. Higher-order macro calls use quote/unquote correctly: `(,worker ,value)`.
5. Imported modules contain definitions/imports only; executable module bodies
   are rejected.
6. A conditional classifier returns exactly `yes` or `no`.
7. Repeated macro parameters intentionally repeat model work.
8. `<-` has only reverse-flow semantics; rewrite it as `->` when direction is
   unclear.
9. Use `/format` or `/tree` to validate structure before spending live model
   calls. `kaos rebis run --dry` also expands and traces model-free shapes, but
   a model-driven `yes`/`no` conditional will intentionally report no decision
   when its dry oracle returns `nothing`.
