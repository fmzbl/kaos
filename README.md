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

Kaos is written in Rust. Runs execute in the directory where Kaos was started,
so relative file reads, edits, commands, imports, and output paths share one
working context.

## Install and start

```bash
git clone https://github.com/fmzbl/kaos.git
cd kaos
cargo install --path .

# Choose a model, then open the terminal app.
export KAOS_MODEL=claude:sonnet
kaos
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
- Shift-Down or End jumps to the tail, and Home returns to the start;
- `u` or Delete removes a queued, cancelled, or completed run.

Running entries cannot be deleted. Long output wraps instead of being cut at
the right edge, and the complete model response, agent steps, file operations,
commands, observations, diagnostics, and execution tree are retained. `WAIT`
shows queue/permission time; `TIME` shows execution time and freezes when the
run finishes.

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

## Sigils, modules, and the standard library

The right panel starts as a sigil explorer. Personal sigils live under
`~/.kaos/sigils` and act as reusable, definition-only Rebis modules.

```text
/sigil save repair-loop
/sigil save team/reviews
/sigils repair
/sigil open team/reviews
```

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

The runtime limits are per Rebis process. Setting a limit to `0` disables that
limit. Explicit shell environment variables override values loaded from the
config file.

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
```

Default features enable the Ratatui terminal application and HTTP providers.
Without default features, Kaos builds a plain REPL suitable for pipes and CI.

The main integration points are:

```text
src/rebis_workspace.rs   editor, Vim behavior, sigils, panels, and commands
src/tui.rs               terminal application, queues, jobs, and run browser
src/main.rs              CLI, Rebis host runtime, and coding-agent commands
src/config.rs            persistent non-secret configuration
src/provider.rs          model/provider selection
src/conductor.rs         tool-using coding-agent loop
```

## License

Kaos is free software licensed under the GNU General Public License v3.0 or
later. See [LICENSE](LICENSE).
