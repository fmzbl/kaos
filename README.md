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

Kaos has two first-class layers: the Rust workspace and agent runtime, and the
Rebis orchestration language.
Runs execute in the directory where Kaos was started, so relative file reads,
edits, commands, imports, and output paths share one working context.

## Install and start

```bash
git clone https://github.com/fmzbl/kaos.git
cd kaos
cargo install --path .

# Choose a model, then open the terminal app.
export KAOS_MODEL=claude:sonnet
kaos
```

The [visual mandala editor](#visual-mandala-editor) is a separate application.
It draws natively with egui on OpenGL — no webview, and so no system webkit —
and it does not need the terminal app installed:

```bash
cargo install --path kaos-visual
kaos-visual                 # or: kaos-visual program.rebis
```

Installed alongside the terminal app it is also reachable as `kaos visual`,
which is the same code behind a second front door:

```bash
cargo install --path . --features visual
kaos visual program.rebis
```

Both read the same `~/.kaos` — sigils, sessions, and the `/theme` setting are
shared — but neither requires the other to be present.

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
| `(& port body)` | Receive external input as `port`, then run `body`; a host may block until it arrives. |
| `(-> A B C)` | Route each accepted answer into the next stage as `INPUT:`. |
| `(<- A B)` | Backflow: equivalent to `(-> B A)`. |
| `(^ E)` | Purely invert syntax orientation by recursively exchanging `->` and `<-`. |
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

Toggle the embedded Vim mode with `/vim toggle`, enable it with `/vim on`, or
persist it with `/vim always`.
It includes normal, insert, character-, line-, and block-visual modes; counts;
common motions; `d`, `c`, and `y` operators; text objects; undo/redo; linewise
and rectangular registers; and operations such as `cw`, `cc`, `d$`, and `yG`.
`/vim off` and `/vim never` return to direct editing. The visual app's Source
tab runs this same typed editor core: keyboard motions, counts, operators,
selections, grouped insert undo, `Ctrl-R`, `Ctrl-[`, visual put, clipboard
copy, and `:w`, `:e`, `:q`, `:q!`, and `:wq` therefore behave identically.
Pointer click and drag add caret placement and character-visual selection
without creating a second editing model.

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
documentation. This catalog is shared by the terminal and visual Sigils tabs;
visual mode can draw, inspect, or chat about embedded modules. The `std/`
namespace is read-only, has no delete action, and saves edited copies only
under a personal name.

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

The right panel edits the source live: type there and the drawing redraws as
soon as what you have typed parses; change the drawing and the source
regenerates. Neither overwrites the other mid-keystroke, and source that does
not yet parse is left alone rather than discarded.

While a run is in flight each node wears a rotating dashed purple ring, driven
by the run's own thread rather than a timer standing in for one.

The singleton **Runs** tab owns the same canonical `kaos rebis run` lifecycle
as the terminal: immutable source/record snapshots, dry/direct/chaos modes,
authority gates, serial FIFO or isolated parallel lanes, retained streaming
output, timers, pause/resume/retry, cancellation, rerun, copy, and file output.
Dry mode is the safe visual default; live modes are explicit. Changing the
session working directory also changes where later jobs resolve files, imports,
tools, and output paths.

Tabs hold drawings, Rebis source, conversations, the sigil library, Runs,
Actions, and Settings. A source tab checks and formats with the Rebis parser,
saves files and sigils, searches, shows tree/terminal-mandala projections, loads
records, and draws source onto a canvas. It can run the program, a text
selection, or the form at the caret, serially or in parallel; block runs carry
the source's top-level imports and definitions just as terminal `/run block`
does. Drawing source lays the syntax tree out as a left-to-right circuit:
nesting depth is the column, a tidy row packing stacks subtrees, and
connections route as right-angle traces between the shapes (calls are drawn as
a parallelogram, a `(& port …)` input as an inlet). Selecting a node turns its
attached connections purple; holding **Shift** while completing a connection
draws that one as a straight angled line instead of the default 90° routing. A
drawing's `edit as text` goes the other way. The sigil browser supports
draw/edit/chat actions for personal and embedded `std/` entries, with delete
limited to personal sigils.

Chat browses and resumes the same durable sessions `/resume` reads in the
terminal app. The **Actions** tab exposes the remaining terminal capabilities
as typed UI: code, cast, conclave, scry, roster, egregore, models, credential
status/store/forget, help, attachments, tool authority, and serial/parallel task
history. These use one streamed process supervisor, so cancellation and output
retention do not vary from button to button.

The editor is tabbed: `Ctrl-T` opens a drawing, `Ctrl-Tab` and `Ctrl-←` cycle,
`Ctrl-W` closes. Each tab keeps its own canvas, viewport and selection, so
switching back returns you where you were. The tab rules live in
`kaos-core`'s `tabs` module, generic over what a tab holds, so the terminal app
can adopt the same behaviour without a second implementation.

The header's **View** dropdown chooses **2D · Edit** or **3D · Structure**. 2D
remains the editable source of truth. 3D is an orbitable, zoomable structural
reading you can also move through with the arrow keys. It is a cone tree
derived from the syntax rather than the flat drawing extruded: each nesting
layer is its own plane, and every form fans its operands onto a golden ring
around itself in the next one, so structure occupies real volume. Invalid shared
forms stay single so the structural error remains visible instead of being
copied into several expressions. Recursive
back-edges rise above the graph as purple curves and recursive components gain
a small helical separation, so recursion reads as a loop instead of a crossing
line. Flow forms become explicit arrow-glyph nodes in 3D. Camera movement and
mode switching never change Rebis or enter undo history; use 2D to edit.

The whiteboard exposes every Rebis form. Prompts, symbols, compose, and
combining forms use distinct outlines; source sigils are drawn as their own
shapes:

| draw | means | generates |
|---|---|---|
| `o` | prompt terminal | `"label"` |
| `◇` | symbol | `name` |
| oval `( )` | ordered composition | `(A B …)` |
| `[]` | square, call, or program | the matching structural form |
| `→` | answer flow | `(-> A B)` (reverse drawing loads `<-`) |
| `$`, `~`, `#`, `'`, `,` | the corresponding source sigil | its Rebis form |
| `^` | syntax inverter shape | `(^ E)` |

An arrow means "this answer flows into that shape". Drawing a `←` is the same
as drawing a `→` the other way, so there is nothing extra to learn. The
generated source updates live beside the canvas. Exact Rebis trees round-trip
without approximation. The mandala is deliberately one-to-one: every visual
object is one Rebis expression and every structural link is one AST edge.
Incomplete forms, several roots, shared children, cycles, invalid names, and
wrong arities remain visible as errors; Kaos never repairs them with invisible
expressions. The source panel says `exact · 1:1` only when the drawing is a
valid Rebis AST. Its `open in editor` action opens that exact snapshot.
`Ctrl-Z` and `Ctrl-Shift-Z` undo and redo semantic drawing edits per tab; camera
movement is deliberately excluded.

Composition comes only from the two link tools. Blue `arrow` creates an
explicit Rebis flow form; grey `father of` makes the second clicked shape the
next ordered child of the first. Its grey arrow therefore reads father → child.
Position is presentation only: moving, overlapping, or drawing one shape inside
another never links them or changes the program. The same blue/grey distinction
is retained in 2D and 3D.

Rebis's `->` is binary and folds left, so `(-> a b "label")` means "a flows to b
flows to label". A square provides the direct representation of mediated
branches, with its mediator and branches attached as ordered children. The
complete structural contract is in
[Exact visual AST rules](docs/REBIS.md#exact-visual-ast-rules).
The derived depth and recursion rules are in
[Structural 3D projection](docs/REBIS.md#structural-3d-projection).

Right-drag draws a purple marquee and selects every touched form, including a
flow arrow when the marquee crosses its rendered line. `Ctrl`-click toggles
individual forms in that block; a blue arrow can be toggled by clicking
anywhere along its rendered line. Delete removes the whole selected block in
one undoable edit. `Ctrl-C` copies the selection's exact induced subgraph and
`Ctrl-V` pastes it with fresh IDs, retained internal links, a visible cascading
offset, and one undo checkpoint. Valid copied blocks also enter the system
clipboard as Rebis source. **Run selection** executes the exact induced
subgraph as a block. An incomplete selection is reported rather than inferred.
The source panel's **format** button rewrites the written source in canonical
indented form (only when it parses, so a half-typed program is never mangled),
and **format drawing** re-lays the graph out as a circuit, snapping a
hand-dragged mandala back onto the grid in one undoable edit.

Loading is the exact inverse of generating: every parsed form—including macros,
imports, quotes, `$`, and `^`—has a canvas node and returns to Rebis source
without approximation.

The editor paints `kaos-core::visual`, whose exact graph model, marquee
geometry, glyph geometry, and structural 3D layout are plain testable Rust.
Run vocabulary lives in `kaos-core`; source-block selection lives in the
screen-neutral workspace; one visual process supervisor serves runs and
actions. The UI therefore does not grow alternate meanings for the same state.

`visual` is not a default feature, only to keep the ordinary build small. The
window is native egui on OpenGL, so it needs no system webkit — see
[Install and start](#install-and-start). Built without the feature,
`kaos visual` prints the instruction and exits.

## Theme

Kaos is a neutral grey scale with purple and blue semantic accents, in two
modes. Purple marks focus, recursion, running state, and the chaos star. Blue
marks flow, navigation, live data, and source ranges: it colors arrows in the
terminal and the 2D/3D mandala, terminal navigation and parallel state, and
visual source ranges; structural `father of` links remain grey. Both the
terminal app and `kaos visual` read the same palette.

```text
/theme dark     light on dark
/theme light    dark on light
/theme          report the current mode
```

In light mode the app paints its own white page rather than inheriting the
terminal's background, so a light theme is genuinely light in both interfaces.

The choice is persisted in the Kaos config and read by **both** interfaces, so
the terminal app and [`kaos visual`](#visual-mandala-editor) always agree.
Both repaint immediately after the setting changes.

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

The visual **Settings** tab exposes every declared non-secret Kaos key, grouped
as Appearance, Mind, Agent, Conclave, Rebis, and Diagnostics, with search,
save/reload/restore, descriptions, and the actual config path. Persistent
settings are separated from session-only state such as the current working
directory. Theme changes save and repaint immediately.

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

# Plain line interface and shell-out providers only.
cargo build --no-default-features

# The visual editor (native egui; no extra system libraries).
cargo build --features visual
```

Default features enable the Ratatui terminal application and HTTP providers.
Without default features, Kaos builds a plain REPL suitable for pipes and CI.
The `visual` feature is off by default; its model and Rebis code generation
(`src/visual.rs`) are plain Rust and are covered by `cargo test` either way.

The workspace splits along one line: whether a thing needs a screen.

```text
kaos-core/               no terminal, no window — shared by both front-ends
  src/config.rs          persistent non-secret configuration
  src/theme.rs           the monochrome palette and its two modes
  src/sessions.rs        durable chat transcripts
  src/sigils.rs          the personal library of saved Rebis programs
  src/tabs.rs            an ordered set of tabs, generic over their content
  src/visual.rs          the mandala model, Rebis codegen and loading
kaos-workspace/          the Rebis editor — buffer, Vim behaviour, sigils,
                         runs and checkpoints; returns actions instead of
                         drawing, so a terminal and a window can drive one
                         editor
kaos-agent/              the agent runtime — providers, backends, the
                         conductor's tool-using loop (knows no terminal
                         and no window)
kaos-pact/               the Pact — sigils, rays, grades, the equation
                         (offline and deterministic; no model, socket or screen)
kaos-visual/             native egui surfaces, shared job supervisor and drawers
kaos/                    the application
```

`kaos-core` has one dependency (the Rebis parser) and no knowledge of ratatui,
egui, or any I/O beyond its own files. Rules that both front-ends must agree on
— what a drawing means, what a theme is, what a session contains — live there
once and are tested without either interface on screen.

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
```

## License

Kaos is free software licensed under the GNU General Public License v3.0 or
later. See [LICENSE](LICENSE).
