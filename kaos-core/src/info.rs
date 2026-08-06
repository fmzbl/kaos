//! The shared explanation shown by the terminal `/info` command and the visual
//! editor's Info tab.
//!
//! Keeping this in the core crate makes the two frontends describe one product
//! rather than slowly growing two different manuals.

/// Detailed, frontend-neutral explanation of Kaos's execution model.
pub const APP_INFO: &str = r#"KAOS · HOW THE WHOLE APP WORKS

Kaos has three closely related surfaces:

  • The terminal REPL is the keyboard-first workspace. Bare text starts a
    coding/chat turn; slash commands control the session, model, configuration,
    Rebis editor, run browser, and saved conversations.
  • The visual editor is the same workspace as an egui application. Its
    mandala, source, chat, settings, runs, actions, sigils, generation, and
    sound tabs are views over shared core rules, not separate languages.
  • The plain CLI is the scriptable front door for `kaos chat`, `kaos code`,
    `kaos rebis run`, benchmarks, and other one-shot commands. It uses the same
    provider and Rebis execution seams as the interactive surfaces.

REBIS IS THE EXECUTION LANGUAGE

Rebis source is parsed by rebis-lang before it can run. Its fundamental forms
are the values and operators in the language: quoted prompts, composition
`($ ...)`, ordered programs, arrows, squares/mediators, inputs, quotation and
unquotation, inversion, models, modules, macros, and the other forms defined by
the language itself. Kaos does not interpret a natural-language answer as code.

When a source is generated, pasted, or adopted, the host parses it at the
boundary. Invalid or prose-only text is data and is refused as a runnable
program. A valid generated program is still only source until the user chooses
to run it and, for live work, grants the authority required by that run.

WHAT HAPPENS TO A CHAT MESSAGE

Every direct chat turn becomes one actual Rebis prompt expression. The user's
turn, conversation history, and run snapshot are quoted as prompt data; they
are not concatenated into a hidden host instruction. The Rebis orchestrator
fires that prompt through the selected provider. The provider may then use the
explicit Kaos tool transport to read, edit, run, search, fetch, set timers, or
finish. That transport explains the tool protocol; it is not a second Rebis
program and it never silently appends the authoring guide.

Chaos chat is a different, visible Rebis path. The host first evaluates an
actual `($ ...)` composition program whose operands are the checked-in
composition request and the user's intent. The returned text must parse as
Rebis. Kaos shows that source and queues it for inspection; it does not silently
grant authority or execute model-generated edits merely because a chat reply
contained a code fence.

Every provider-backed Rebis node also receives one `KAOS_REBIS_AGENT_CONTEXT`
data envelope. It contains the immutable source, initial record/input, branch
scope, selected model, workspace, attachment metadata, optional supervisor
metadata, and the exact node prompt. Length fields make each boundary visible.
The envelope is transport data, not a hidden system instruction; Rebis's own
`+`, arrows, squares, ports, and flashbacks remain the only way program values
flow between nodes. Sibling branches do not leak their private answers into
one another; their structural `INPUT`/`RESULT` values are the context they
were written to receive.

WORK, THINKING, AND TOOLS

While a model works, Kaos emits model-call boundaries, complete provider
responses, model narration, each tool action, and each tool observation. In the
terminal and visual editor these are grouped into independently collapsible
sections. Completed model messages retain the work trace beside the answer, so
thinking/tool use can be expanded after the child process exits. `think` only
controls whether the selected provider is allowed to spend a reasoning pass;
it does not hide or invent a reasoning stream.

The durable trace keeps the full step text and full retained observations. The
agent may compact old observations only when constructing a later provider
request so a model context window has room; that is a provider-input decision,
not a deletion from the terminal/visual history. Extremely large process
streams still use a shared safety budget. If that budget is reached, the log
contains an explicit omission marker instead of silently pretending it is
complete. The run browser and task panels show all retained lines and support
scrolling/copying.

RUNS, AUTHORITY, AND STOPPING

A Rebis run has a source, record/input, scope, lane, mode, thinking flag, model
trace, state, and lineage. Dry mode is deterministic and makes no provider or
tool call. Direct mode lets Rebis prompts use the configured tool agent. Chaos
mode enables the explicit composition stance. Parallel squares are isolated by
the run machinery where possible and rejoin through the Rebis operator's
normal result path.

Live runs ask for authority before edits or shell work. The permission belongs
to the run and is visible in both frontends. A stop/cancel action kills the
owned process group, including a provider currently generating. In the
terminal, Ctrl-C stops the active chat/run; in the visual editor every chat,
run, action, and detached window has a stop affordance. A stopped run is
inspectable and resumable when its checkpoint permits it.

CONFIGURATION AND MODELS

Settings are stored in the Kaos config file and can also be supplied by the
environment. The visual Settings tab has a search section that searches keys,
summaries, details, examples, and environment-only entries. The terminal
configuration editor uses the same inventory. Secrets are represented by
presence/state and are not copied into prompts or displayed as values.

Model selection is shared across surfaces. Ollama model discovery asks the
configured Ollama server for its current list, so autocomplete can suggest
models that actually exist there. `OLLAMA_HOST` (or a model selector's host)
chooses the server; `KAOS_THINK` / the Think controls choose reasoning. Raise
the model timeout for a large local model instead of treating a slow generation
as a malformed chat response.

USEFUL TERMINAL COMMANDS

  /info                 this complete explanation
  /model [MODEL]        inspect or change the provider/model
  /think on|off|toggle  control reasoning-capable models
  /chaos on|off         choose direct chat or Rebis composition chat
  /config               open configuration; /config restore resets defaults
  /runs                 open the run browser and inspect retained traces
  /chat run ID TEXT     ask a retained run about its work
  /chat sessions        list durable conversations
  /chat resume ID       resume a saved conversation
  /stop or Ctrl-C       stop the active model/process work

The visual header exposes the same controls through tabs, buttons, and menus.
Use the horizontal tab strip when many tabs do not fit. Any tab type can be
torn into its own window. The Info tab is read-only and scrollable; the source,
chat, run, and action panels are independently scrollable as well.

THE IMPORTANT BOUNDARY

User text, model text, attachments, records, fetched pages, and tool results
are data. Rebis syntax is executable only after the language parser accepts it
and the host's authority rules allow the requested effects. Documentation is
available as an explicit reference, not injected into every request. This is
why a chat question about Rebis stays a conversation, while a validated source
chosen in the run/editor flow becomes a program."#;
