# Kaos

Kaos is a terminal workspace for [Rebis](https://github.com/fmzbl/rebis), a
small Lisp-like language for composing model calls, agents, deterministic
judges, reusable macros, and execution flows.

The fullscreen application combines:

- a Rebis editor with source, tree, mandala, output, and sigil views;
- live normal-mode model calls or tool-using agents;
- serial and parallel background runs with retained execution logs;
- a personal module library plus Rebis's embedded standard library;
- chat and direct coding-agent workflows in the same session;
- Claude CLI, Ollama, OpenAI, Anthropic, and OpenRouter models.

Kaos has three first-class layers: the Rust workspace and agent runtime, the
Rebis orchestration language, and the root-level Sisyphus neural architecture.
Runs execute in the directory where Kaos was started, so relative file reads,
edits, commands, imports, and output paths share one working context.

## Sisyphus architecture

The root [Sisyphus component](sisyphus/README.md) is an experimental causal
language model that reimagines Thoth around Rebis's
quote/route/group/square semantics and Kaos's evidence-gated recursion. In a
frozen five-seed 29.8k-parameter `enwik8` micro-study it beat a matched modern
Transformer 4/5 seeds (3.7491 vs 3.8013 mean held-out bits/byte; paired 95% CI
`[-0.0971, -0.0061]`). Its second recursive pass improved the first in all five
seeds.

That short-context quality result did not transfer. With every setting frozen,
an untouched five-seed `text8` confirmation favored the Transformer 5/5 (3.1493
vs 3.0679 bpb; paired 95% CI `[+0.0439, +0.1248]`). Sisyphus therefore has no
demonstrated general language-quality edge; enwik8 is a retained
dataset-specific positive, not the headline averaged across a kept null.

The edge is bounded: at context 128 the Transformer still trains 5.09× faster.
Sisyphus's `O(n log n)` mixer crosses over in isolated CPU inference among the
measured lengths at context 2,048; at 4,096 it is 5.04× faster with 83.3% lower
peak RSS. The [architecture](sisyphus/ARCHITECTURE.md),
[paper](sisyphus/PAPER.md), [protocol](sisyphus/PROTOCOL.md),
[confirmation](sisyphus/CONFIRMATION.md), and
[Kaos integration note](docs/SISYPHUS.md) state the complete result and
limitations. Sisyphus is part of the root project, but its present research
checkpoint is not the default Kaos provider.

## Install and start

```bash
git clone https://github.com/fmzbl/kaos.git
cd kaos
cargo install --path .

# Choose a model, then open the terminal app.
export KAOS_MODEL=claude:sonnet
kaos
```

The [visual mandala editor](#visual-mandala-editor) is an optional feature. It
draws natively with egui on OpenGL — no webview, and so no system webkit:

```bash
cargo install --path . --features visual
kaos visual
```

It is off by default only to keep the ordinary build small; `kaos visual`
prints this instruction when the feature is absent.

The Sisyphus training and benchmark stack is an optional Python component:

```bash
python -m venv .venv-sisyphus
.venv-sisyphus/bin/pip install -r sisyphus/requirements.txt
PYTHONPATH=. DEV=CPU .venv-sisyphus/bin/python -m sisyphus --help
```

Without arguments, `kaos` opens the Rebis workspace. A new workspace shows a
transient Chaos Star in the left source pane. The first key, paste, click, or
wheel event only dismisses the star; it does not edit the buffer or perform an
action. The source underneath starts empty.

The default configured model is `sim`, which is useful for inspecting the UI
but does not make live model calls. Select and persist a live model with
`/model MODEL`.

## A first Rebis program

This program imports two standard-library modules, defines three reusable
workers, and performs a red-team design review:

```rebis
(# std/debate)
(# std/shape)

(~ build (task)
  (with-evidence (-> task "Propose the design.")))

(~ attack (task)
  (-> task "Find the strongest failure mode."))

(~ repair (task)
  (-> task "Revise the design so it survives the attack."))

(red-team surviving-verified-design
  build attack repair
  "Design a retry queue for a payment service.")
```

Paste it into the editor, press `Ctrl-K`, and run `/run`. Rebis accepts several
top-level forms without an extra outer pair of parentheses. Definitions and
imports share the program scope with the expressions that follow them.

## The language at a glance

Rebis programs are made from a small set of structural forms:

| Form | Meaning |
|---|---|
| `"prompt"` | Fire the selected model. |
| `symbol` | Name a macro, parameter, module, or deterministic judge. |
| `(A B C)` | Execute a group in source order. |
| `($ A B C)` | Compose one string from the inert text of the operands, then fire it. |
| `(-> A B C)` | Route each accepted answer into the next stage as `INPUT:`. |
| `(<- A B)` | Backflow: equivalent to `(-> B A)`. |
| `([M] A B C)` | Run the branches and let executable mediator `M` resolve them. |
| `(~ name (args) body)` | Define a structural macro; its parameters are its variables. |
| `(# module)` | Import definitions without executing the module. |
| `'form` and `,value` | Quote a macro-template program and splice caller syntax. |
| `; comment` | Comment to end of line, except inside a quoted prompt. |

Macro parameters hold Rebis syntax, not pre-evaluated strings. Passing a quoted
prompt therefore keeps it executable after substitution:

```rebis
(~ investigate (topic)
  (-> topic "Investigate this topic in depth and return a sourced explanation."))

(["Combine both investigations into one comparison"]
  (investigate "Fibonacci numbers")
  (investigate "chaos theory"))
```

The two investigations are explicit branches. The mediator receives their
accepted results and writes the comparison. Rebis does not interpolate the word
`topic` inside a quoted prompt; the parameter is substituted structurally where
the bare `topic` symbol appears.

See [docs/REBIS.md](docs/REBIS.md) for Kaos integration details and the
[chat authoring reference](docs/REBIS_CHAT_CONTEXT.md) for complex, nested
programs. Kaos compiles that reference into `/chat`, so the agent can explain,
debug, and write Rebis without relying on a model's prior knowledge. The same
reference is injected into every executing Rebis agent so nodes also
understand the surrounding language. Rebis's own repository contains the
complete language guide, formal semantics, symbol reference, standard-library
manual, and host API.

## Editor and command palette

Direct editing is the default. Type normally and use the arrow, Home, End,
Backspace, and Delete keys. `Ctrl-K` opens the Kaos command palette; bare `/`
remains ordinary Rebis source text, which allows imports such as
`(# std/loops)`.

The palette filters while you type. Use Up/Down to choose a command and Tab or
Enter to complete it. Vim commands remain under `:`:

```text
:w                    save
:w program.rebis      save as
:q                    return or quit the editor
:q!                   discard
:wq                   save and quit
```

Enable the embedded Vim mode with `/vim on`, or persist it with `/vim always`.
It includes normal, insert, visual, and visual-line modes; counts; common
motions; `d`, `c`, and `y` operators; text objects; undo/redo; linewise yanks;
and operations such as `cw`, `cc`, `d$`, and `yG`. `/vim off` and `/vim never`
return to direct editing.

Useful workspace commands:

```text
/format                 format valid Rebis source
/format!                format even when comments will be removed
/search TEXT            find the next literal source match; /search repeats
/tree                   show the structural tree
/mandala                show the o-[]-o flow projection
/graph                  focus the right panel
/source                 return focus to source
/panel hide|show        control the right panel
/chat                   switch to chat without losing editor state
/rebis                  return from chat to the suspended workspace
/quit                   exit Kaos
```

`Ctrl-W h` and `Ctrl-W l` move between the source and right panels. Mouse
selection is clipped to the pane where the drag began. `Ctrl-Shift-C` copies
that pane-local selection; source line numbers are not included. `/mouse off`
releases mouse capture for native terminal selection, and `/mouse on` restores
pane-local selection.

In the chat, `Ctrl-C` stops what is in flight instead of exiting: it cancels
active and queued work first (terminating every serial and parallel job), then
a pending permission question, then a typed draft; only an idle, empty chat
exits on `Ctrl-C` (`Esc` also quits). In the Rebis workspace, `Ctrl-C` exits
Kaos, terminating all active jobs before restoring the terminal.

## Running programs and blocks

Kaos can run a complete program, a visual selection, or the form at the cursor:

| Command | Behavior |
|---|---|
| `/run` | Run the whole buffer, or the captured visual selection. |
| `/run block` | Run the parenthesized or bracketed form at the cursor. |
| `/run parallel` | Start the program or visual selection immediately in an independent job. |
| `/run block parallel` | Start the form at the cursor in an independent job. |
| `/runs` | Open the retained run browser from any view or from chat. |

Block runs include the buffer's top-level `~` definitions and `#` imports, so a
form resolves as it does inside the complete program.

Plain runs share a FIFO with queued chat messages. Parallel runs bypass that
FIFO and can execute beside the serial job and beside other parallel jobs. Each
parallel run receives an isolated model session and keeps its own timer,
execution tree, stream, exit status, and final value. Parallel headers carry
`∥`; the footer shows how many parallel jobs are active.

Runs continue in the background while you edit, open the mandala or sigil tree,
or switch to chat. Navigation never pauses a job.

### Run browser

Every submitted run appears in the right panel and remains there after it
finishes:

- `j`/`k` select a run;
- `Tab` expands or collapses its full stream;
- Up/Down and Page Up/Down browse retained output;
- Shift-Up or Home jumps to the top, and Shift-Down or End jumps to the tail;
- `p` pauses or resumes the selected run, including a replacement child after an interruption;
- `u` or Delete removes a queued, cancelled, or completed run.

Running entries cannot be deleted. Long output wraps instead of being cut at
the right edge, and the complete model response, agent steps, file operations,
commands, observations, diagnostics, and execution tree are retained. `WAIT`
shows queue/permission time; `TIME` shows active execution time and freezes when
the run finishes. Paused time is not charged to `TIME`.

The mouse wheel follows the pane under the pointer. Scrolling the source
temporarily detaches its viewport from the edit cursor so redraws do not snap
back; the next source key or `/search` resumes cursor following. Source,
projection, and run-log scrolling are clamped to their real bounds.

A hosted Rebis run pauses whenever a model prompt fails, returns no answer,
times out, reaches a clean allowance boundary, or its child process disappears.
`p` resumes a suspended child directly. If the child disappeared, `p` launches a
replacement from the captured source and record: completed prompt answers replay
locally from an atomic journal, then the first unfinished prompt is tried again.
Completed model/tool prompts are not repeated. `Ctrl-C` remains explicit
cancellation and is the intentional way to discard a paused run.

## Direct and chaos runs

The integrated Rebis editor starts in direct mode. Every quoted prompt receives
exactly one tool agent that can inspect the launch directory and perform
requested file edits and commands: a native Claude agent when the selected
model is the Claude CLI, and a single node-scoped Conductor agent on every
other backend. Before any agent run starts, Kaos asks for authority:

- `y` approves one run;
- `a` approves and remembers the choice for the current session;
- `n` or Esc denies it without launching an agent or command.

Permission requests and decisions are retained inside the corresponding run.
Parallel runs remain independent, while their authority questions are presented
one at a time. `/chaos on` upgrades every prompt to a full Kaos pipeline agent;
`/chaos off` returns to one direct tool agent per prompt. The non-interactive
CLI stays completion-only by default: `--allow-tools` enables direct tool
agents, and adding `--chaos` selects the pipeline.

## Results, records, and output

`/run` shows the program's returned value under `RESULT` and the structural
execution under `TRACE`. Use `/output` to focus the final value:

```text
/output
/output copy
/output write docs/design.md
```

`/record FILE` supplies a text record to later runs. Rebis uses records as
evidence and input according to the program's structure.

Every successful serial run retains the current sigil's output. A plain `/run`
keeps its normal fresh/explicit-record behavior; continuation of interrupted
execution belongs to `p`, not to a separate run command.

## Sigils, modules, and the standard library

The right panel starts as a sigil explorer. Personal sigils live under
`~/.kaos/sigils` and act as reusable, definition-only Rebis modules.

`/sigil save NAME` writes `NAME.rebis` and, when a successful output exists,
`NAME.output`. If the selected or most recent run is unfinished, it also writes
`NAME.run` plus `NAME.checkpoint`: the captured record/input, run mode, retained
trace and exact completed prompt journal. Opening that sigil recreates the run as
paused; `/runs` followed by `p` reconstructs the interpreter, replays completed
prompts locally, and retries the first unfinished prompt. Manual/automatic pauses
refresh the durable state. Successful completion removes the resumable sidecars
so an obsolete checkpoint cannot shadow the finished result. Runtime data stays
outside importable `.rebis` source.

```text
/sigil save repair-loop
/sigil save team/reviews
/sigils repair
/sigil open team/reviews
```

`/sigil chat` opens a source-bound **God Agent** channel inside the right panel.
The channel receives an isolated copy of the complete current `.rebis` source,
plus a live-refreshed inventory of every running, paused, queued, or
permission-gated bot: captured source and record, state, mode, directive, full
trace, and prompt journal. Enter sends a turn; Esc returns focus to source.
One run remains the bound source-mutation target. When explicitly requested in
the user turn, the God Agent may target any live run with `PAUSE`, `RESUME`,
`APPLY_DIRECTIVE`, or `CLEAR_DIRECTIVE`; directives are attached to that run's
next unfinished prompts. Cancellation and deletion are unavailable through this
channel. If the agent produces a
valid revision and the editor has not changed concurrently, Kaos applies it to the
live buffer. A running bound interpreter is paused for the coherent snapshot and
then reconstructed from its atomic journal: identical completed prompts replay
without model/tool work, while the first changed prompt invalidates only the
divergent tail. The channel transcript, run trace, record input, and checkpoint are
not cleared. Invalid revisions and concurrent editor conflicts are retained for
inspection but never overwrite source.

Names may contain `/`-separated folders. A saved `team/reviews` sigil is
imported with `(# team/reviews)`. Importing `(# team)` recursively loads every
`.rebis` module below that folder in stable order. Opening another sigil parks
dirty source as a restorable `temp:N` entry instead of discarding it.

The embedded `std` folder contains fourteen documented modules and 51 macros:

```text
std/flow         application, composition, and fan-out
std/spread       deterministic best-of-two/three/five
std/map          static map and zip shapes
std/gate         guards, fallback, and independent audit
std/loops        bounded iteration and convergence
std/evolve       judged refinement and hill-climbing
std/debate       panels, cross-examination, and red teams
std/dialectic    synthesis and reconciliation
std/canon        reusable judgment protocols
std/shape        answer contracts
std/search       routing, fallback, and tree search
std/tournament   bracket reduction and consensus
std/reflexion    critique-and-retry structures
std/committee    chaired panels and quorum workflows
```

Import one module with `(# std/spread)` or all of them with `(# std)`. Expand
the `std` folder in the sigil explorer and open any module to read its inline
documentation. The `std/` namespace is read-only; save edited copies under a
personal name.

## Non-interactive Rebis CLI

The same parser, runtime, module resolver, and projections are available from
the shell:

```bash
# Open the integrated editor.
kaos rebis edit program.rebis

# Execute with the selected model.
kaos rebis run program.rebis

# Tool-using execution requires explicit approval outside the TUI:
# one direct tool agent per prompt, on any backend…
kaos rebis run --allow-tools program.rebis

# …or the full Kaos pipeline per prompt.
kaos rebis run --chaos --allow-tools program.rebis

# Parse and execute without model calls.
kaos rebis run --dry program.rebis

# Inspect inline source without running it.
kaos rebis tree '(["synthesize"] "Inspect code" "Trace failure")'
kaos rebis mandala '(-> "Reproduce the bug" "Write the fix")'
```

`rebis run` accepts either a file or inline Rebis source. `tree` and `mandala`
accept inline source. Piped stdin becomes a record for the run.

## Visual mandala editor

`kaos rebis mandala` projects source *into* the `o-[]-o` notation. `kaos visual`
runs it the other way: draw the mandala on a canvas and it writes the Rebis.

```bash
# Start on an empty canvas.
kaos visual

# Load an existing program onto the canvas.
kaos visual program.rebis
kaos visual '(["synthesize"] "Inspect code" "Trace failure")'
```

From inside the Rebis workspace, `/visual` opens the current buffer the same
way. It parses first, so an undrawable buffer reports on the status line
instead of opening an empty window, and the editor runs in its own window while
the terminal app keeps going.

The whiteboard has the same three elements as the notation — the circle, the
square, and the arrow:

| draw        | means                          | generates            |
|-------------|--------------------------------|----------------------|
| `o`         | a prompt terminal              | `"label"`            |
| `o` fed by arrows | a prompt consuming answers | `(-> in… "label")`   |
| `[]`        | a mediator combining its inputs | `(["label"] in…)`   |
| `→`         | answer flow                    | nesting              |

An arrow means "this answer flows into that shape". The one shape with no
outgoing arrow is the program's result, and the source is generated by walking
back from it. Drawing a `←` is the same as drawing a `→` the other way, so there
is nothing extra to learn. The generated source updates live beside the canvas;
loops, disconnected shapes, and empty mediators are reported there instead of
producing invalid code.

A circle takes at most one incoming arrow. Rebis's `->` is binary and folds
left, so `(-> a b "label")` means "a flows to b flows to label" — a chain, not
"a and b both feed label". Joining several answers is exactly what the square
is for, so a circle with two inputs is reported rather than written out as a
chain that does not match the drawing.

Loading is the exact inverse of generating: a program that came from the canvas
returns to the same canvas. Only the three shapes above can be drawn, so a
buffer using macros, imports, symbols, or `($ ...)` interpolation is refused by
name instead of being approximated.

The editor is a thin shell over `kaos::visual`, which holds the model and the
code generation and is plain testable Rust — the notation's rules live there,
not in the UI.

`visual` is not a default feature, only to keep the ordinary build small. The
window is native egui on OpenGL, so it needs no system webkit — see
[Install and start](#install-and-start). Built without the feature,
`kaos visual` prints the instruction and exits.

## Theme

Kaos is monochrome in two modes. The shapes, sigils and rules carry the
meaning, so colour only separates figure from ground.

```text
/theme dark     light on dark
/theme light    dark on light
/theme          report the current mode
```

The choice is persisted in the Kaos config and read by **both** interfaces, so
the terminal app and [`kaos visual`](#visual-mandala-editor) always agree.
`kaos visual` picks it up the next time it opens; the terminal repaints on
restart.

## Models and authentication

Select a model from chat or Rebis. The choice is written to the Kaos config and
becomes the default for later sessions:

```text
/model claude:sonnet
/model claude:opus
/model claude:fable
/model ollama:qwen3:14b
/model openai:gpt-4o
/model openrouter:deepseek/deepseek-chat
```

`claude:*` uses the installed Claude CLI and its `claude login` subscription,
not the Anthropic API. Ollama uses the local server. Hosted providers accept
credentials from `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or
`OPENROUTER_API_KEY`.

Credentials can also be stored in Kaos's owner-only credentials file:

```bash
kaos auth
kaos auth openrouter sk-or-...
kaos auth forget openrouter
```

Environment variables always take precedence over stored configuration.

## Configuration

On first run, Kaos creates `~/.config/kaos/config`, or
`$XDG_CONFIG_HOME/kaos/config` when `XDG_CONFIG_HOME` is set. The generated file
lists every non-secret setting with its effective default. Existing files are
never silently replaced.

Use `/config` from chat or Rebis to edit it. `:w` saves and `:q` returns to the
previous surface. `/config restore` replaces all non-secret settings with their
documented defaults and leaves provider credentials untouched. Restart Kaos to
apply manual config edits.

Important Rebis settings:

```text
KAOS_MODEL                  selected provider and model
KAOS_REBIS_TIMEOUT_S        model-turn timeout for Rebis agents (600)
KAOS_REBIS_MAX_EXPANSIONS   macro expansion limit (256)
KAOS_REBIS_MAX_MODULES      module import limit (64)
KAOS_REBIS_MAX_CALLS        model call limit (1024)
KAOS_REBIS_MAX_CONCURRENCY  runtime branch concurrency (4)
vim_mode                    persistent embedded-Vim preference (false)
```

The runtime limits are per Rebis process. In a hosted TUI run, the model-call
limit is a renewable slice: reaching it pauses the live run and `p` grants the
next slice. Setting a limit to `0` disables that limit. Explicit shell
environment variables override values loaded from the config file.

## Chat and coding-agent workflows

`/chat` opens the conversation surface without discarding the Rebis workspace.
The current model is always visible in the footer. Messages typed while work is
active join the FIFO; local navigation commands still run immediately.

Kaos can also work directly on a repository:

```bash
cd ~/project
kaos code . "Find and fix the failing parser test"
```

The coding agent may inspect and edit files and run project commands in that
directory. When a test command is available, Kaos verifies the resulting change
before finishing. Use a committed worktree so the resulting diff is easy to
review.

Common chat commands:

```text
/rebis <FILE>             open or restore the Rebis workspace
/runs                     return directly to retained background runs
/code PATH INTENT         run the coding agent in PATH
/cast INTENT              make one model request
/attach FILE              add file contents as conversation context
/cd DIR                   change the working directory
/model <MODEL>            show or select the model
/config <restore>         edit or restore configuration
/new                      start a new conversation session
/clear                    clear the visible transcript
/quit                     exit
```

## Build and test

```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings

# Optional Sisyphus architecture checks.
PYTHONPATH=. DEV=CPU .venv-sisyphus/bin/python \
  -m unittest discover -s sisyphus -t . -v

# Plain line interface and shell-out providers only.
cargo build --no-default-features

# The visual editor (native egui; no extra system libraries).
cargo build --features visual
```

Default features enable the Ratatui terminal application and HTTP providers.
Without default features, Kaos builds a plain REPL suitable for pipes and CI.
The `visual` feature is off by default; its model and Rebis code generation
(`src/visual.rs`) are plain Rust and are covered by `cargo test` either way.

The main integration points are:

```text
src/rebis_workspace.rs   editor, Vim behavior, sigils, panels, and commands
src/tui.rs               terminal application, queues, jobs, and run browser
src/main.rs              CLI, Rebis host runtime, and coding-agent commands
src/visual.rs            mandala model and Rebis code generation (no UI)
src/visual_ui.rs         the `kaos visual` egui canvas (feature `visual`)
src/config.rs            persistent non-secret configuration
src/provider.rs          model/provider selection
src/conductor.rs         tool-using coding-agent loop
sisyphus/                neural architecture, training, evidence, and paper
```

## License

Kaos is free software licensed under the GNU General Public License v3.0 or
later. See [LICENSE](LICENSE).
