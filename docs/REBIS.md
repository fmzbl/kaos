# Rebis in Kaos

Kaos hosts Rebis through the currently selected model. Quoted strings are raw
prompts, bare atoms are Lisp-like symbols, `~` defines structural macros, arrows route
answers, and `[M]` contains executable mediator code. `($ ...)` interpolates a
string from the text of its operands (nothing inside it fires); variables are
macro parameters, and a text constant is simply a macro whose body is a prompt.

Kaos compiles the rules and the complex, nested examples in
[`REBIS_CHAT_CONTEXT.md`](REBIS_CHAT_CONTEXT.md) into both `/chat` and every
executing Rebis agent. Chat can therefore explain, debug, and write
Rebis, while executing nodes understand the surrounding language without
depending on the selected model's prior knowledge. That reference is also the
concise example cookbook for higher-order macros, deep mediators,
standard-library strategies, lazy routing, and bounded recursive refinement.

Rebis also supplies the operation basis for the experimental
[Sisyphus model](SISYPHUS.md): quote/retain, arrow/route, group/compose, and
square/mediate become four learned causal candidates in one recursively shared
cell. Rebis remains the executable orchestration language; Sisyphus is a neural
model inspired by its semantics, not a replacement parser or runtime.

Rebis is the default Kaos screen. Press `Ctrl-K` to open the command palette.
The legacy `Ctrl-/` chord is still recognized too (including the `Ctrl-_` and
unit-separator encodings that older terminals emit). The
list filters as you type; Up/Down scroll it, Tab completes, and Enter executes.
A Rebis file may contain multiple top-level forms like a Lisp file. Kaos parses
them as one implicit program scope, so a top-level `~` definition is available
to later forms without a redundant outer group.
A new unnamed workspace initially shows only the transient red Chaos Star in
the left source pane while the normal right-hand panel remains visible. The
first key, paste, click, or wheel event dismisses it; that event is consumed, so
it inserts no text and performs no command, motion, mode change, or other
action. The editor underneath is empty—there is no hidden starter source. The
star is not source text and can never be parsed, executed, parked, or saved.
`/chat` switches to chat mode without saving or discarding anything. The complete
Rebis workspace is suspended in memory while chat is open; `/rebis` restores the
buffer (including unsaved edits), cursor, undo history, panel selection, graph
scroll, and run output. Chat keeps its own conversation state while Rebis keeps
its editor state, so you can move between them without losing either one. An
approved run is owned by the app rather than the visible panel: it keeps
streaming and advances the shared FIFO while chat, the mandala, the tree, the
sigil explorer, or a hidden panel is on screen. From chat, `/runs` restores the
Rebis workspace directly to the active run browser without waiting behind that
run.

Session-level Kaos commands are also available without leaving Rebis. `/model`
shows the current model, `/model MODEL` changes it and remembers the selection
in the Kaos config for later sessions. `/config` opens that complete file in the
editor (`:w` saves and `:q` returns); `/config restore` restores all non-secret
defaults without touching provider credentials. Restart Kaos to apply edits.
`/new` starts a fresh
conversation sigil, `/clear` clears its visible transcript, and `/quit` exits
Kaos. Model
choices use the same filtered Up/Down, Tab, and Enter autocomplete as chat mode.
In the source editor, bare `/` is ordinary source text, so qualified imports
such as `(# std/loops)` need no escape. `Ctrl-V` still inserts the next
character literally. Chat keeps bare `/` as its command prefix.

```rebis
(
  (~ investigate (target)
    (-> target "Write a verified report"))

  (["Combine both reports"]
    (investigate "Inspect the oven")
    (investigate "Analyze the refunds")))
```

Run and visualize programs:

```bash
kaos rebis run program.rebis
kaos rebis run --allow-tools program.rebis
kaos rebis run --chaos --allow-tools program.rebis
kaos rebis run --dry '(["Combine reports"] "Inspect code" "Trace failure")'
kaos rebis tree '(["synthesize"] "Inspect code" "Trace failure")'
kaos rebis mandala '(["synthesize"] "Inspect code" "Trace failure")'
```

The integrated editor uses direct mode by default: each quoted prompt receives
exactly one tool agent that can inspect the launch directory and perform
requested file edits and commands after the run-level authority gate — a native
Claude agent when the selected model is the Claude CLI, a single node-scoped
Conductor agent on every other backend. `/chaos on` gives each prompt a full
Kaos pipeline agent instead; `/chaos off` returns to direct mode. The
non-interactive CLI is completion-only by default: `--allow-tools` enables
direct tool agents, and adding `--chaos` selects the pipeline. `--dry` performs
no model work and needs no permission.

Open the integrated editor directly with `kaos rebis edit <file>`, or enter
`/rebis <file>` at the main Kaos command line. It supports paired `()` and `[]`,
quote highlighting, `%` matching, and Vim visual modes. `/search TEXT` moves to
the next case-sensitive literal source match and wraps at the end; `/search`
repeats the previous query. Press `Ctrl-K` for Kaos commands such as `/search`,
`/format`, `/run`, `/tree`, and `/mandala`. Vim `:` remains
reserved for `:w`, `:e`, `:q`, `:q!`, and `:wq`.
The top bar shows the complete Rebis punctuation set horizontally—symbols only:
`( ) [ ] ~ # ' , -> <- ; "`. Structural operators and delimiters use one shared
operator color in both that legend and source text.
Semicolons begin line comments only outside quoted prompts. Inside `"..."`, a
semicolon is ordinary prompt text, including in multiline strings and after an
escaped quote; the parser and editor highlighter follow the same rule.

The editor also manages a personal sigil library in `~/.kaos/sigils`:

```text
/sigil save repair-loop    save the current valid Rebis program
/sigil save team/reviews   folders work: saved to team/reviews.rebis
/sigils repair             search saved names in the right panel
/sigil open team/reviews   load a saved sigil into the editor
/sigil chat                supervise and revise this sigil in the right panel
```

Saving also writes the last successful returned value to a neighboring
`.output` sidecar. When an unfinished run exists, a `.run` sidecar retains its
record/input, exact execution source, mode, trace, elapsed time and pause reason;
the atomic prompt journal is copied to `.checkpoint`. Opening the sigil restores
that execution as a paused run. Use `/runs`, then `p`, to rebuild the interpreter
from its identical completed prompt prefix and retry the first unfinished prompt.
Manual pauses, automatic timeouts/allowance pauses, and unexpected child exits
refresh the durable snapshot. A successful result clears `.run` and
`.checkpoint`, while `.output` remains the saved returned value. A plain `/run`
still starts fresh (or uses `/record FILE`).

`/sigil chat` is distinct from ordinary `/chat`: it does not suspend the Rebis
workspace. It opens a durable God Agent transcript in the right panel and binds
to the selected unfinished run (or the newest unfinished run when the selection
is complete). Each turn places the entire editor source and a live-refreshed
snapshot of every nonterminal bot into an isolated bridge. Every entry includes
its source, record/input, state, mode, pause reason, current directive, checkpoint
journal, and undropped trace. The supervisor may revise only the bound source
copy. Explicit user requests may additionally produce validated per-run `PAUSE`,
`RESUME`, `APPLY_DIRECTIVE`, and `CLEAR_DIRECTIVE` actions. A directive remains
attached to that bot's unfinished model prompts until replaced or cleared;
checkpoint replays remain immutable. God-channel cancellation and deletion are
not supported. Kaos
parses the proposal and compares it with the editor revision before merging it,
so invalid output or concurrent human edits cannot overwrite the buffer.

When a bound run is live, the turn takes a coherent process pause. An unchanged
source continues in place. A changed valid source retires only the old
interpreter process and immediately reconstructs it from the same prompt journal:
the exact unchanged prompt prefix replays locally, and checkpoint logic truncates
only from the first changed prompt. The record, completed answers, transcript,
timers, and run-tree identity survive. Runs already paused before the conversation
stay paused so the user remains in control; `/runs` followed by `p` resumes them.

Names take `/`-separated folders, the same shape as module paths — a sigil
saved as `team/reviews` is importable as `(# team/reviews)`. Search walks the
folders and lists qualified names. Import the folder itself with `(# team)` to
load every `.rebis` module below it recursively in stable qualified-name order.
Exact `team.rebis` modules take precedence over a same-named folder.

The explorer is the panel's first view when the workspace opens, and it is
interactive. Folders (like `std`) show collapsed; `j`/`k` (or arrow keys) move
the selection, `Tab` expands or collapses the folder under it, and `Enter`
opens the selected sigil. Clicking a folder toggles it; clicking a leaf opens
it (with the mouse captured — see `/mouse`).

`/sigils <QUERY>` also works from the main Kaos screen; a query auto-expands
the folders that contain matches. The source editor stays visible while
results occupy the scrollable visualization panel.

The embedded standard library appears as the `std` folder. Expand it (Tab, or
`/sigils std/`) to see its fourteen modules, then Enter (or
`/sigil open std/spread`) loads one into the editor as a copy — its inline
comments are the documentation. The `std/` name itself stays read-only:
`/sigil save std/...` is refused, so edits are saved under a new name.

Saved sigils are also foundational Rebis modules (hypersigils). `#` imports all
top-level `~` definitions without executing the module:

```rebis
(
  (# repair-tools)
  (repair "Fix the cancellation lifecycle"))
```

Kaos resolves `(# repair-tools)` from
`~/.kaos/sigils/repair-tools.rebis`. Qualified paths such as `std/loops` are
supported by the same mechanism. `(# std)` imports all fourteen embedded
standard-library modules; `(# std/flow)` still imports only that exact module.
Modules may contain only top-level macro definitions and nested `#` imports;
missing modules, cycles, parse failures, and executable module bodies are
reported in the live run diagnostics.

Opening a saved sigil no longer sacrifices an edited buffer. Kaos parks dirty
source in memory and shows it in the sigil panel as, for example,
`temp:1 * untitled (unsaved)`. Restore it with `/sigil open temp:1`. It remains
available across `/chat` until that restored temporary sigil is saved, or until
the Rebis workspace is deliberately discarded/exited.

## Higher-order macros

`~` defines a macro over raw Rebis syntax. A leading `'` quotes its output
template and `,` inserts caller syntax:

```rebis
(
  (~ twice (work)
    '(-> ,work ,work))
  (twice "Inspect and improve this code."))
```

Named macros can be passed as arguments:

```rebis
(
  (~ apply-to-both (worker left right)
    '(["Combine both results"]
      (,worker ,left)
      (,worker ,right)))
  (~ inspect (target)
    '(-> ,target "Write a verified report"))
  (apply-to-both inspect "Inspect parser" "Inspect tokenizer"))
```

Kaos executes the structurally expanded program using the selected model. Since
macros can repeat arguments and worker calls, production configurations
should retain model-call, token, cost, and time limits.

## Macro loops

Macros may call themselves. A two-branch square with a macro call inside its
brackets evaluates that call first as a `yes`/`no` condition and executes only
the selected branch:

```rebis
(
  (~ step (value) (-> value "Improve once."))
  (~ done (value)
    (-> value "Is it finished? Answer exactly yes or no."))
  (~ loop (value work stop)
    ([(stop value)] value (loop (work value) work stop)))
  (loop "Initial implementation" step done))
```

This supplies loops without adding `#`, `$`, or a dedicated loop form. The
runtime bounds recursive macro expansion.

The complete language manual is in the Rebis repository at `docs/GUIDE.md`,
alongside `docs/REFERENCE.md` — a per-symbol dictionary with semantics,
examples, the value path, combination patterns, limits, and gotchas.

## Mandala notation

Kaos keeps functions inside the whiteboard `o-[]-o` visual alphabet:

```text
o "prompt"       prompt or value terminal
[M: code]        executable mediator
~[f(x)]          named macro template
[f]              expanded macro call
→ / ←            answer flow
```

For example, `(inspect "parser")` appears as:

```text
(o "parser") ─[inspect]─o
```

The definition appears as a reusable template:

```text
~[inspect(target)] ≔ (◇ target) ─→─ (o "Write a report")
```

Use `/mandala` to open this projection and `/tree` for the structural AST.

The projection also runs backwards. `kaos visual` opens a canvas where the same
three elements are drawn rather than read, and the Rebis source is generated
from the drawing — see [the README](../README.md#visual-mandala-editor).
`kaos visual FILE` and `/visual` load an existing program onto that canvas, so
the projection round-trips: source to drawing and back to the same source. It
is built on `kaos::visual`, which owns the mapping between the shapes and the
language.

The mandala is scrollable. Enter `/graph`, then use `hjkl`, arrow keys, Page Up,
Page Down, `Home`, or `g`. `Esc` returns to source focus. `/panel hide` removes
the panel, `/panel show` restores it, and `/panel` toggles it.
Vim window motions work too: `Ctrl-W l` focuses the right mandala/result panel,
and `Ctrl-W h` returns to the source editor.
The mouse wheel scrolls whichever pane is under the pointer: source on the left
and mandala/results on the right. Shift-wheel scrolls that pane horizontally.
Source-wheel review stays at the chosen viewport instead of snapping back to
the stationary edit cursor; the next source key or `/search` follows the cursor
again. Vertical wheel scrolling is clamped to the real source, projection, or
run-log bounds, preventing blank overscroll.
Mouse capture is enabled by default. A drag selects and copies only text from
the pane where it began, clipped at that pane's boundaries instead of selecting
the terminal's whole row. `Ctrl-Shift-C` copies the highlighted pane selection
again without cancelling the active run or leaving the editor. `/mouse off`
restores raw terminal selection;
`/mouse on` restores pane-local selection.
With panel focus, `hjkl`, arrow keys, Page Up/Down, `Home`, and `g` provide
keyboard scrolling.
Groups, branches, macro templates, calls, and arrow stages occupy real rows;
the program is not compressed into a single circuit line.

Source editing includes normal, insert, character-visual (`v`), and line-visual
(`V`) modes. Visual selections support motions plus `y`, `d`, `x`, and `c`; `p`
pastes the most recent visual yank.

Direct editing is the default: typing inserts text immediately and the usual
arrow, Home, End, Backspace, and Delete keys work without Vim modes. Bare `/`
inserts source text; `Ctrl-K` opens the Kaos command palette. Enable Vim for the
current workspace with `/vim on` and disable it with `/vim off`. To persist the
preference, use `/vim always` or `/vim never`. It updates the `vim_mode` entry
in the complete startup config at
`~/.config/kaos/config` (or under `$XDG_CONFIG_HOME`):

```text
vim_mode = true
```

The command palette displays required parameters as `<NAME>` and optional
parameters as `[NAME]`; placeholders are documentation and are never inserted
as literal arguments.

The normal-mode editing subset also includes `i`, `a`, `I`, `A`, `o`, `O`,
`hjkl`, arrows, `w`/`W`, `e`/`E`, `b`/`B`, `0`, `^`, `$`, `gg`, `G`, `%`,
`x`, `D`, `C`, `s`, `p`, `P`, `u`, and `Ctrl-R`. Counts compose before either
an operator or its motion (`2dw`, `d2w`, `3dd`, `2e`, `3x`). The `d`, `c`, and
`y` operators accept character, word, line, document, and end-of-line motions,
including `cw`, `cc`, `d$`, `yG`, and the `iw`/`aw`/`iW`/`aW` text objects.
One insert/change session is one undo unit, linewise yanks paste as lines, and
Escape returns the cursor to the final inserted character. This is a deliberate
embedded Vim editing core; Vim plugins, arbitrary Ex commands, named registers,
marks, recorded macros, and search are not emulated.

Multiline terminal paste uses bracketed-paste mode and is inserted as one
Unicode-safe undo step. CRLF and bare CR line endings are normalized to LF. The
cursor follows the pasted text, so earlier lines may scroll above the viewport;
they remain in the buffer and `gg` returns to the top.

## Returned program output

`/run` renders the program's returned value under `RESULT` before the complete
`TRACE`. This follows the structural value path rather than guessing from the
last model call: arrows return their consumer, squares return their mediator,
conditional squares return the selected branch, and macro calls return their
expanded program.

Execution starts appearing in the right panel immediately: prompt starts and
answers, arrow routing, macro expansion, module loads, mediator starts,
conditional selection, and typed diagnostics stream as they occur. Provider
failures are distinct from an intentional `nothing`; a run containing runtime
diagnostics exits unsuccessfully while retaining the trace in the panel.
Plain `/run` and `/run block` capture their source and record input into the
same FIFO used by chat messages whenever any working is active. Use
`/run parallel` for the whole program (or the current visual selection), or
`/run block parallel` for the form at the cursor, to start a separate job immediately
without waiting for that FIFO. Several parallel jobs may execute at once; each
uses an isolated model session and retains its own execution tree, output stream,
timer, and completion status. Parallel headers carry `∥`. The status bar shows
the ordered queue depth as `⧗N`; queued runs start in order and do not clear the
active trace until they reach the head of the queue. Every submitted run appears
in a durable right-panel tree: click it or use `Ctrl-W l`, choose a
run with `j`/`k`, and press `Tab` to expand or collapse its captured text stream.
Up/Down and the mouse wheel scroll through output rows; Page Up/Down move
faster, Shift-Up or Home returns to the start, and Shift-Down or End reaches
the latest retained output.
Stream lines retain the agent's complete text; nothing is shortened with an
ellipsis, and model/code lines wider than the panel wrap onto continuation rows
without changing the retained stream. Finished runs remain available until `u` or
`Delete` removes them. Those keys also unqueue a waiting run while leaving chat
messages and every other run intact. An active run cannot be removed while it
is running.
In the Rebis workspace, `Ctrl-C` exits Kaos, terminating every serial and
parallel working and scattering every queued item first. (The chat screen
treats `Ctrl-C` as cancel-first: it stops in-flight work and only exits when
the chat is idle.)
Every header includes a live `WAIT` duration while queued or permission-gated
and a `TIME` duration after execution starts. Completion and cancellation freeze
that final duration in the retained run history; suspended time is excluded.
Press `p` on a run to suspend or resume it. Failed or empty model prompts,
timeouts, clean step/model-call allowance boundaries, and vanished child
processes all become pauses rather than failed exits. A live child retains the
interpreter stack directly. If it vanished, `p` starts a replacement that
replays atomically checkpointed prompt answers locally, rebuilds the stack, and
retries the first unfinished prompt. `Ctrl-C` remains explicit cancellation and
terminates the child process group.

An expanded run contains a numbered `AGENT` section for every quoted Rebis
prompt. Each agent uses the same activity stream as a chat coding working:
visible model narration, file reads and observations, edits and writes, shell
commands and their results, verification, finish messages, and the final model
value that flows into the next Rebis node. Nested `STEP` sections preserve the
detailed form emitted by chat instead of reducing the run to only its final
answer or structural Rebis trace. A `generating turn` row is flushed before
each blocking provider call, and every returned raw response is retained in
full in a nested `MODEL` branch even when it does not contain a valid tool
action; the surrounding execution tree remains the primary view.

Use `/runs` after switching to `/mandala`, `/tree`, another panel view, or chat.
It reveals the run browser, focuses it, selects the currently running agent
when one exists, and otherwise selects the newest retained run. Panel commands
change only the visible projection; they never clear, pause, cancel, or reclaim
the retained background stream.

Before any direct or chaos run starts, Kaos asks before its agents receive file
and command tools. Parallel requests keep their jobs independent while authority prompts
are presented one at a time. Press `y` to approve one run, `a` to remember the
approval for every later Rebis run in the current sigil (releasing all waiting
parallel requests), or `n`/`Esc` to deny it. The expanded run panel contains the
authority request, choices, and retained decision. A permission-waiting run
remains visible and can be removed with `u` or `Delete`. Approved agents execute
in Kaos's current directory—the same root used by relative source paths,
`/output write`, file edits, and commands.

Kaos defaults to 256 macro expansions, 64 distinct module loads, and 1,024
model calls per run. Override them with `KAOS_REBIS_MAX_EXPANSIONS`,
`KAOS_REBIS_MAX_MODULES`, and `KAOS_REBIS_MAX_CALLS`; a zero value disables
that capability. Each tool-using agent model turn has a 600-second wall-clock
limit; set `KAOS_REBIS_TIMEOUT_S` to accommodate a slower local model.

Use `/output` to show only the final value. `/output copy` places it in the
embedded Vim yank register for `p`, and `/output write FILE` writes the exact
value relative to Kaos's current directory:

```text
/run
/output
/output write docs/design/ad-hoc-span-wrappers.md
```

Save the Rebis source itself with either command style:

```text
:w
:w program.rebis
/save
/save program.rebis
```
