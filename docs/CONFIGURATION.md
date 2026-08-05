# Kaos configuration

Kaos has one persistent, non-secret configuration file and a small set of
environment-only values used for credentials, module discovery, and child-run
transport. The persistent file is created on first run at:

```text
$XDG_CONFIG_HOME/kaos/config   # when XDG_CONFIG_HOME is set
~/.config/kaos/config           # otherwise
```

The generated file is intentionally self-documenting. Kaos does not rewrite
an existing file when new settings are added. `/config restore` (or **restore
defaults** in the visual Settings tab) replaces the persistent file with the
current documented defaults.

## Precedence and storage

For uppercase settings, the effective order is:

1. An explicit environment variable already present when Kaos starts.
2. The matching value in `kaos/config`.
3. The documented default.

When the process starts, values read from the file seed missing uppercase
environment variables. This lets the terminal, visual editor, child runs, and
library crates share one behavior without each parsing the file. `theme` and
`vim_mode` are lowercase editor preferences and remain file-backed; they are
not exported as environment variables.

Provider credentials are never written to this file. Use `kaos auth`, the
owner-only credentials file, or an explicit shell export for
`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, and `OPENROUTER_API_KEY`.

The terminal `/config` editor preserves comments and ordering when changing one
key. `:w` saves and `:q` returns. The visual **Settings** tab shows every key
below, its type, default, behavior, example, dirty state, and the actual file
path. It also has search, reload, save, and restore controls. Theme changes
apply immediately; restart Kaos after changing provider, agent, or runtime
environment settings so the current process and its children use the new
values.

## Persistent settings

The defaults below are the values written by a new installation. An empty
default means that the optional override is disabled.

### Appearance and editor

| Key | Default | Type | Meaning |
| --- | --- | --- | --- |
| `theme` | `dark` | choice | Shared terminal and visual palette; accepts `dark` or `light`. |
| `vim_mode` | `false` | boolean | Default Vim-style bindings for newly opened Rebis editors. |

### Stance

| Key | Default | Type | Meaning |
| --- | --- | --- | --- |
| `KAOS_CHAOS` | `false` | boolean | Compose the work as a Rebis program before running it. |

Off, an agent is handed an intent and works it. On, the intent is written as a
Rebis program first and the program is what runs — so its cost can be read off
the page before it is paid, and the result is an artifact you can edit, save to
the sigil wall, and run again.

It is one stance across every surface. A chat turn is composed into a program
instead of answered; a Rebis run gives each model node the whole Kaos pipeline
instead of one node-scoped agent; a sigil stack's generated program runs the
same way. `/chaos [on|off]` toggles it in the terminal app, the chat pane has a
`chaos` checkbox in the visual editor, `kaos rebis run --chaos` turns it on for
one run, and every child process inherits it.

A composed program is **queued, not started**: composing costs one model call,
and whatever the program itself costs stays a separate decision made at the
authority gate. The terminal app registers it in the run browser; the visual
editor opens it as a `composed` source tab.

### Mind and providers

| Key | Default | Type | Meaning |
| --- | --- | --- | --- |
| `KAOS_MODEL` | `sim` | text | Provider/model binding shared by chat, agents, conclaves, and Rebis prompts. |
| `KAOS_TIMEOUT_S` | `120` | seconds | Wall timeout for one-shot completions such as `/cast`. |
| `KAOS_CHAT_TIMEOUT_S` | `600` | seconds | Wall timeout for one model turn in a tool-using chat or coding agent. |
| `KAOS_MAX_TOKENS` | `8192` | integer | `max_tokens` sent to OpenAI-compatible providers. |
| `KAOS_NUM_PREDICT` | *(empty)* | integer | Optional Ollama `num_predict` generation cap. Empty keeps Ollama's default. |
| `KAOS_FABLE_FALLBACK_MODEL` | *(empty)* | text | Claude model used as an optional fallback after a Fable refusal. |
| `KAOS_PROVIDER_SORT` | *(empty)* | text | OpenRouter routing preference: commonly `throughput`, `latency`, or `price`. |
| `KAOS_PROVIDER_ONLY` | *(empty)* | text | Comma-separated OpenRouter provider slugs; disables provider fallbacks. |
| `KAOS_ROUTE_ALLOW` | *(empty)* | text | Comma-separated model prefixes a RUN may route itself to with `(/ ,name …)`. Empty means unrestricted. Selectors written in the source are never checked — only ones a program chose. |
| `OPENAI_BASE_URL` | `https://api.openai.com` | URL | OpenAI-compatible API host root. |
| `OPENROUTER_BASE_URL` | `https://openrouter.ai/api` | URL | OpenRouter API host root. |
| `OLLAMA_HOST` | `http://127.0.0.1:11434` | URL | Ollama server; bare `host:port` values also work. |

Examples:

```ini
KAOS_MODEL = ollama:qwen3:4b
KAOS_TIMEOUT_S = 300
KAOS_NUM_PREDICT = 4096
OPENAI_BASE_URL = https://my-openai-compatible-host.example
```

`KAOS_MODEL` accepts the same provider forms exposed by `/model`, including
`sim`, `ollama:model`, `openai:model`, `openrouter:vendor/model`, and
`claude:sonnet` or another Claude CLI tag. `KAOS_MAX_TOKENS` is for hosted
chat-completions; Ollama's explicit generation cap is `KAOS_NUM_PREDICT`.

The visual Settings editor provides a filtered model dropdown for `KAOS_MODEL`,
including the currently installed Ollama names. The terminal `/model` palette
and Rebis `model` command autocomplete the same names as `ollama:model`. Kaos
gets the live list by running `ollama ls` against `OLLAMA_HOST`; use the visual
editor's **refresh Ollama** button, or `/models` in the terminal, after pulling
a model. Custom provider/model values can still be typed directly.

### Agent working

| Key | Default | Type | Meaning |
| --- | --- | --- | --- |
| `KAOS_MAX_STEPS` | `14` | integer | Maximum read, edit, command, or other action steps in one coding-agent session. |
| `KAOS_HAND` | `0` | boolean | Use native HTTP provider tool calling instead of Kaos's parsed action protocol. |
| `KAOS_PROTECT` | *(empty)* | text | Comma-separated path fragments whose file-tool writes and edits are refused. |
| `KAOS_NO_FORGE` | `0` | boolean | Skip automatic `kaos_repro.py` reproduction-script forging when no gate exists. |
| `KAOS_NO_DREAM` | `0` | boolean | Skip the short hypothesis call between failed coding attempts. |
| `KAOS_NO_PARADIGM` | `0` | boolean | Skip rotating debugging hypotheses on retries. |
| `KAOS_CLAUDE_YOLO` | *(empty)* | tri-state | Empty asks, `1` grants shell authority, and `0` accepts edits without unrestricted shell authority. |

`KAOS_PROTECT` is a mutation guard, not a complete sandbox: reads remain
available and a shell command can bypass it. `KAOS_CLAUDE_YOLO` is deliberately
tri-state because an undecided first coding task should ask the user rather
than silently choose an authority level.

### Conclave and myth

| Key | Default | Type | Meaning |
| --- | --- | --- | --- |
| `KAOS_MYTH` | `(gather vote (spread ${KAOS_K} fire))` | Rebis expression | Orchestration expression used by `/conclave`. `${KAOS_K}` is expanded while loading. |
| `KAOS_K` | `5` | integer | Default width of the myth's `(spread k fire)`; values below one become one. |
| `KAOS_AGENTIC` | `0` | boolean | Turn myth leaves into isolated read/edit/bash agent sessions instead of completions. |
| `KAOS_ARENA` | `.` | text | Working-tree root copied for each agentic leaf. |
| `KAOS_MAX_CONCURRENCY` | `3` | integer | Maximum concurrent agentic conclave leaves; values below one become one. |
| `KAOS_BASH_TIMEOUT_S` | `600` | seconds | Per-shell-action timeout for agentic leaves. |
| `KAOS_GATE_TIMEOUT_S` | `300` | seconds | Wall timeout for one validation gate or test command. |
| `KAOS_QUIET` | `0` | boolean | Suppress live agentic progress while retaining the verdict and diagnostics. |

The myth is its own small S-expression language — kaos's composition layer,
documented in `kaos-agent/src/myth.rs` — not Rebis and not a hard-coded
workflow. Its forms are `fire`, `(ask "role")`, `(spread N X)`, `(gather G X)`,
and `(pipe A B …)`, where a gate `G` is `vote`, `first`, `(check "shell-cmd")`,
or `(mirror P)`. The default is a conclave: fan out five ways, then vote.

```lisp
(gather vote (spread 5 fire))
```

With `KAOS_AGENTIC=1`, each `fire` leaf works in an isolated copy of
`KAOS_ARENA`; the selected result is then checked and returned. Without it,
the leaves are ordinary model answers. `KAOS_MAX_CONCURRENCY` controls these
agentic sessions and is independent of the Rebis branch setting described
below.

### Twin-ladder transcript compaction

| Key | Default | Type | Meaning |
| --- | --- | --- | --- |
| `KAOS_UNIT` | `700` | integer | Character charge granted by one Fibonacci transcript rung. |
| `KAOS_BASE` | `500` | integer | Character floor retained by the middle of a compacted transcript. |
| `KAOS_RUNGS` | `5` | integer | Number of meaningful charge rungs around the first intent and newest observation. |

These settings affect how Kaos preserves context in long agent sessions. A
larger charge retains more text and consumes more model context; the two ends
of the transcript are intentionally favored over its rotting middle.

### Rebis runtime

| Key | Default | Type | Meaning |
| --- | --- | --- | --- |
| `KAOS_REBIS_MAX_EXPANSIONS` | `256` | integer | Structural macro-expansion budget, including recursive calls. `0` disables expansion. |
| `KAOS_REBIS_MAX_MODULES` | `64` | integer | Distinct uncached module-load budget for `(# name)`. `0` disables imports. |
| `KAOS_REBIS_MAX_CALLS` | `1024` | integer | Model-backed prompt budget for one process. `0` makes execution model-silent. |
| `KAOS_REBIS_MAX_CONCURRENCY` | `4` | integer | Parallel branch bound for Rebis square evaluation. `1` or `0` means sequential. |
| `KAOS_REBIS_GIT_WORKTREES` | `0` | boolean | Opt into detached Git worktrees for parallel, live tool-using `[]` children. |
| `KAOS_REBIS_TIMEOUT_S` | `600` | seconds | Timeout for one model-backed Rebis turn, not the whole recursive process. |

These limits are applied by the host when it constructs Rebis's
`RuntimeLimits`. They are independent counters: a program can exhaust its
macro, module, or model-call budget while the other counters remain available.
The structural budgets are shared across nested calls and parallel branches.

Hosted TUI runs treat `KAOS_REBIS_MAX_CALLS` as a renewable slice: reaching the
slice pauses the live child at a continuation-safe boundary and the user can
grant another slice. A direct terminal run reports the model-call diagnostic
when its allowance is exhausted. Unlike the three zero-disable budgets,
`KAOS_REBIS_MAX_CONCURRENCY=0` is normalized to sequential execution and
`KAOS_REBIS_TIMEOUT_S=0` falls back to its safe default.

`KAOS_REBIS_GIT_WORKTREES=1` makes filesystem-writing square children eligible
for parallel execution. Kaos snapshots the parent working tree without
changing its branch or index, creates one detached worktree per child, and
reconciles edits in source order before the mediator. The later child wins an
overlapping hunk; the combined changes are left unstaged. Git and a repository
are optional capabilities: if `git` is missing, too old for `git worktree`, or
the current directory is not in a repository, Kaos reports the reason (and
advises installing/upgrading Git when applicable) and runs the square
sequentially.

### Diagnostics

| Key | Default | Type | Meaning |
| --- | --- | --- | --- |
| `KAOS_DEBUG` | `0` | boolean | Adds diagnostics in selected provider, conductor, and shared-library paths. |

## Environment-only values

These values are intentionally not persistent Settings entries. The visual
Settings tab displays them in **Environment Only & Secrets** as a read-only
boundary reference.

### Credentials

| Variable | Use |
| --- | --- |
| `OPENAI_API_KEY` | OpenAI or OpenAI-compatible provider credential. |
| `ANTHROPIC_API_KEY` | Anthropic API credential; Claude CLI subscription mode uses `claude login`. |
| `OPENROUTER_API_KEY` | OpenRouter provider credential. |
| `KAOS_CONFORMANCE_MODEL` | Overrides the model the conformance suite runs against; a test-harness setting rather than a preference. |

Use `kaos auth`, which stores credentials separately with owner-only file
permissions, or export a key in the shell that launches Kaos.

### Discovery and frontend overrides

| Variable | Use |
| --- | --- |
| `REBIS_COLLECTION_PATH` | Optional Rebis collection root or `modules` directory for source-only imports. |
| `KAOS_BIN` | Visual-editor override for the Kaos executable used to launch terminal children. |
| `XDG_CONFIG_HOME` | Standard config-root override; determines the persistent config and credentials paths. |
| `HOME` | Standard home directory fallback used when `XDG_CONFIG_HOME` is absent. |

### Private child-run transport

These are set by Kaos itself when the terminal or visual frontend launches a
child. They are not user configuration knobs:

| Variable | Use |
| --- | --- |
| `KAOS_FOLD` | Requests foldable child progress rendering. |
| `KAOS_SESSION` | Carries the durable conversation identifier. |
| `KAOS_RESUME` | Selects resume versus create for that conversation. |
| `KAOS_REBIS_CONTEXT` | Adds Rebis authoring and validation guidance to a coding chat. |
| `KAOS_RAW_CHAT_TASK_STDIN` | Marks a raw task supplied through standard input. |
| `KAOS_CHAT_OUTPUT` | Requests an assistant-only child response. |
| `KAOS_CHAT_TRACE` | Requests visible model/tool work alongside the assistant response; terminal and visual front ends render it as collapsible sections. |
| `KAOS_PAUSE_ON_TRANSIENT` | Enables continuation-safe pauses after retryable model failures. |
| `KAOS_RUN_PROCESS_GROUP` | Extends hosted pause/cancel signals to command descendants. |
| `KAOS_REBIS_CHECKPOINT` | Selects the prompt journal used to resume a hosted Rebis run. |
| `KAOS_REBIS_DIRECTIVE` | Selects the supervisor directive file for a live Rebis child. |
| `KAOS_REBIS_INLET` | Selects the input file for user messages delivered to a live Rebis child. |

Do not add these transport variables to the persistent config: they describe
one child process and become stale when copied into later runs.

## Rebis examples

The configuration reference is especially useful when testing a program with
the same limits that the terminal and visual hosts use:

```bash
# Parse and expand without model calls.
KAOS_REBIS_MAX_CALLS=0 kaos rebis run --dry path/to/program.rebis

# Permit imports while keeping the run deterministic.
KAOS_REBIS_MAX_MODULES=64 KAOS_REBIS_MAX_EXPANSIONS=512 \
  kaos rebis run --dry path/to/program.rebis

# Point a source-only collection module at another checkout.
REBIS_COLLECTION_PATH=../rebis-collection/modules \
  kaos rebis run --dry path/to/importing-program.rebis
```

Use `rebis tree` or Kaos `/tree` to inspect the expanded structure. A dry run
can validate imports, macro expansion, arrows, ordinary prompts, and limits;
`%` gates still need a scripted `0` or `1` oracle to exercise the selected
branch.
