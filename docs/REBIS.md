# Rebis in Kaos

Kaos hosts Rebis through the currently selected model. Quoted strings are raw
prompts, bare atoms are Lisp-like symbols, `~` defines structural macros, arrows route
answers, `[M]` contains executable mediator code, and `(% C A B)` is the lazy
binary gate that evaluates only `A` or `B` from an exact `1`/`0` decision.
`($ ...)` interpolates a string from
the text of its operands (nothing inside it fires); standard control macros use
that form to tell a decision prompt to reply with exactly one `0` or `1`
token. Variables are macro parameters, and a text constant is simply a macro
whose body is a prompt.
`(^ E)` purely dualizes syntax orientation by recursively exchanging `->` and
`<-`; it makes no model call and applying it twice returns `E`.

Kaos compiles the rules and the complex, nested examples in
[`REBIS_CHAT_CONTEXT.md`](REBIS_CHAT_CONTEXT.md) into both `/chat` and every
executing Rebis agent. Chat can therefore explain, debug, and write
Rebis, while executing nodes understand the surrounding language without
depending on the selected model's prior knowledge. That reference is also the
concise example cookbook for higher-order macros, deep mediators,
standard-library strategies, `%`-based lazy routing, and bounded recursive
refinement.

Rebis is the default Kaos screen. Press `Ctrl-K` to open the command palette.
The legacy `Ctrl-/` chord is still recognized too (including the `Ctrl-_` and
unit-separator encodings that older terminals emit). The
list filters as you type; Up/Down scroll it, Tab completes, and Enter executes.
A Rebis file may contain multiple top-level forms like a Lisp file. Kaos parses
them as one implicit program scope, so a top-level `~` definition is available
to later forms without a redundant outer group.
A new unnamed workspace initially shows only the transient green Chaos Star in
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
in the Kaos config for later sessions. A program can optionally override that
default for one expression subtree with an adjacent postfix selector:

```rebis
"Draft locally"/ollama:qwen4:4b
(-> "Investigate" "Write the report")/claude:opus5
(["Judge both"] "one" "two")/openrouter:anthropic/claude-opus-4
```

Nested suffixes override the surrounding selector only until their subtree
finishes; unbound forms use the session model. Kaos resolves the selector with
the same provider registry and credentials as `/model`. It changes routing
only—no extra model call or prompt is introduced. Provider readiness is checked
per effective call, so an unavailable session default does not prevent a
program whose model-calling subtrees all have usable bindings.

`/config` opens that complete file in the editor (`:w` saves and `:q` returns);
`/config restore` restores all non-secret
defaults without touching provider credentials. Restart Kaos to apply edits.
`/new` starts a fresh
conversation sigil, `/clear` clears its visible transcript, and `/quit` exits
Kaos. Model
choices use the same filtered Up/Down, Tab, and Enter autocomplete as chat mode.
In the source editor, `/` is ordinary source input rather than a palette
command. The parser recognizes `/provider:model` only when it is adjacent to a
closing quote or `)`; slashes inside qualified imports such as
`(# std/loops)` remain part of the module name. `Ctrl-V` still inserts the next
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
Conductor agent on every other backend. A postfix selector chooses that same
execution path with its local model for every prompt beneath it. `/chaos on`
gives each prompt a full
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
`( ) [ ] ~ # ' , $ % & ^ / -> <- ; "`. Structural operators and delimiters use one shared
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

A run that reaches a `(& port …)` input with no value stops in an **awaiting
input** state — a healthy pause, not a failure, like a program waiting for a
message. In `/runs`, select the awaiting run and press `Enter` to open an input
line; type the value and `Enter` delivers it to the port and resumes the run
from exactly where it stopped (`Esc` leaves it paused). This is how one agent's
output can feed another, and how a program waits for a supervisor's direction
before continuing.

The visual Chat tab shows the model's work as it arrives rather than only its
finished answer: a turn is a child process streaming into a retained log, so the
reply is on screen while it is still being written, with a running timer beside
it. You can keep typing while that is happening. A message written mid-answer does
not stop the turn in flight and does not open a second one beside it: it waits,
shown under the stream as queued, and goes out as the next turn the moment the
current one lands. Several of them go out together as one turn, in the order
they were written. Nothing queued is in the transcript until it is actually
asked.

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
`/sigils std/`) to see its twenty-two modules, then Enter (or
`/sigil open std/spread`) loads one into the editor as a copy — its inline
comments are the documentation, and they carry a contract for every parameter,
a cost note, and an example per macro. The `std/` name itself stays read-only:
`/sigil save std/...` is refused, so edits are saved under a new name.
The visual Sigils tab reads the same catalog: `std/` modules can be drawn,
opened as read-only source copies, searched, and attached to chat, but never
deleted.

Saved sigils are also foundational Rebis modules (hypersigils). `#` imports all
top-level `~` definitions without executing the module:

```rebis
(
  (# repair-tools)
  (repair "Fix the cancellation lifecycle"))
```

Kaos resolves `(# repair-tools)` from
`~/.kaos/sigils/repair-tools.rebis`. Qualified paths such as `std/loops` are
supported by the same mechanism. `(# std)` imports all twenty-two embedded
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
should retain model-call, token, cost, and time limits. The complete setting
reference, including provider, agent, transcript, and visual-editor options,
is in [`CONFIGURATION.md`](CONFIGURATION.md).

## Macro loops

Macros may call themselves. A `%` gate evaluates its condition first and
executes only the selected branch:

```rebis
(
  (~ step (value) (-> value "Improve once."))
  (~ done (value)
    (-> value "Is it finished? Answer exactly 0 or 1."))
  (~ loop (value work stop)
    (% (stop value) value (loop (work value) work stop)))
  (loop "Initial implementation" step done))
```

This supplies loops without adding a dedicated loop form. The runtime bounds
recursive macro expansion.

The complete language manual is in the Rebis repository at `docs/GUIDE.md`,
alongside `docs/REFERENCE.md` — a per-symbol dictionary with semantics,
examples, the value path, combination patterns, limits, and gotchas.

## Mandala notation

Kaos keeps functions inside the whiteboard `o-[]-o` visual alphabet:

```text
⬡ "prompt"       prompt terminal with fitted text inside
[M: code]        executable mediator
( )              one indentation — every ( ) in the source is one circle
(~ f (x) …)      named macro template — a circle marked `~ f (x)`
(f …)            expanded macro call — a circle marked with the callee's name
(& port …)       input port — a circle marked `& port`
(^ …)            syntax inverter — a circle marked `^`
(-> A B)         answer flow — a plain untitled circle around the two forms it
                 routes between, with the arrow drawn between them inside it
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

The projection also runs backwards. `kaos visual` opens a canvas where every
Rebis form is drawn rather than read, and the Rebis source is generated
from the drawing — see [the README](../README.md#visual-mandala-editor).
`kaos visual FILE` and `/visual` load an existing program onto that canvas, so
the projection round-trips: source to drawing and back to the same source. It
is built on `kaos::visual`, which owns the mapping between the shapes and the
language.

### Exact visual AST rules

The mandala is a one-to-one visual abstraction of Rebis forms. Every parsed
base expression becomes exactly one selectable visual node. A postfix model
binding is metadata on that node rather than a second shape, so it preserves
the source/visual correspondence without inventing executable geometry.
Select any form to edit its optional **MODEL OVERRIDE**; blank inherits the run
default. Blue `/provider:model` badges remain visible on forms, and on the
rendered edge for a complete `->` or `<-` flow. Every visual node generates
exactly one base expression plus its optional suffix. Kaos does not
insert invisible `nothing` operands, quoted cycle markers, groups, calls, or
program roots.

A circle or square is an indentation boundary, and crossing that boundary is
the compositional gesture. Dropping `B` into `A` makes `B` the next ordered
operand of `A`; moving it within the same boundary changes only coordinates;
pulling it out detaches it; moving it into another boundary reparents its whole
subtree. Children receive one-based positions `1..=n` per block. Drop order
supplies the default, while the selected form's `CHILD ORDER` controls can
change the numbers without moving cells. Operand order is never inferred from
screen position.

There is no separate `father of` tool and no gray parent line. The boundary
already expresses nesting, so another edge would duplicate the same fact.
Source auto-drawing follows delimiters literally: every compose operand starts
inside its circle, while a square contains its complete mediator expression
and every branch. Nested boundaries retain their nearer content.

The only link tool is blue `connect flow`. Click one circle/square block and
then another to create one real `Forward(A, B)` expression node; reversing the
direction loads and writes one explicit `Backflow` node. A loose triangle or
hexagon/sigil is not a flow endpoint, so drawn arrows occur only between
indentation blocks. Coordinates, scale, movement within one boundary, panning,
marquee bounds, camera projection, and 3D piece offsets remain presentation.

A mediator square is the one form drawn as a box. It grows around its mediator
and branches. Circle and square boundaries carry no interior `( )` or `[ ]`
caption; their outlines already express indentation. Every standalone symbol
can also be scaled by hand: hovering reveals a green dashed scale outline, and
dragging that outline changes its stored half-extents. Holding **space** turns the pointer into a hand: drag anywhere, over a form or
not, and the view moves instead of the drawing. Dragging with the **middle
button** does the same without touching the keyboard.

The grab band is a fixed number of pixels wide, so a wall is equally reachable
at every zoom, and the centre stays put. Only the square sizes its two walls
independently — a side changes one axis and a corner changes both — because a
box is the one outline whose proportions carry no meaning. Every other form
scales whole: one factor, taken from whichever axis was dragged further, applied
to both, so a circle stays round and a hexagon stays a hexagon at any size.
Resizing a form changes that form alone; what a boundary holds keeps its own
position and its own size. The wall still stops at its contents, which cannot be
pushed outside their box, so it widens freely and closes only as far as they
allow. Dragging elsewhere on the box moves the box and its content together, and
a block dropped across a wall is still grabbed as a block. While a piece inside
is being dragged, the boundary holds the size it had, so it neither chases the
piece nor collapses under it.

Placing a form on top of a boundary draws it INSIDE, as that boundary's newest
content, in the next place on its spiral rather than on top of whatever is
already there — a click inside a circle is how you fill one in. Any form works,
not only another boundary. Holding **Shift** while placing a circle or square
draws it AROUND the boundary under the pointer instead, keeping that boundary's
contents and taking its exact place in its parent's operand order. Both are one
edit.

A compose form is the only *unmarked* circle: visually, each circle is one
indentation.
It has the same nesting behaviour as the square, but dragging anywhere along
its border changes one shared radius so it never becomes an oval. Its content
is the minimum radius. Resizing scales the
actual glyph, hit target, caption, edge clearance, 2D layout footprint, and 3D
projection together. Hand-set size and positions are presentation; crossing a
boundary is the deliberate structural edit. An indentation carries the sigil
that opened it on its ring, keeping its interior clear for what it holds.
`'` and `,` open no parentheses and take no place of their own: they are drawn
on the front of the form they mark, so `,worker` is one symbol reading `,worker`
rather than a loose comma somewhere near a loose diamond. Stacked prefixes read
outermost first, as written. A form shows one token on the canvas — the operator that opened it, or the
first word of its text — and the panel beside the canvas holds the whole of it,
editable. Nothing is lost by that: it is a smaller view of text that is still
there, one click away. Sizing a form's complete payload to its own outline is
what made its text grow with the form until it covered what was nested inside.
Labels and outlines are therefore sized by the view, not by how much a boundary
encloses. The implicit
`program` form is a quiet triangle whose `program` label appears only while it
is selected. `$`, `~`, `#`, `'`, `,`, `^`, and `%` appear as their own marks
rather than circles.

Drawing parsed source lays the syntax tree out as a left-to-right circuit:
nesting depth is the column, so a form sits one column left of its operands,
and a size-aware row packing centres each form on the rows of the operands it
drives. Column indentation includes the largest resized half-width on both
sides, and row bands include resized heights, so formatted forms cannot overlap.
Nested contents are packed from the innermost boundary outward along one plain
spiral: item `n` sits at radius `c·n` and angle `n` steps around, with the
smallest `c` that separates every resized bound. Formatting sets every form to the
least size it may honestly be drawn at — its own outline, or whatever its
contents occupy — dropping any size set by hand, and closes each boundary onto
what that needs. Formatting twice leaves the drawing exactly where formatting
once put it. Connections route as
right-angle traces only for flow operators between circle/square blocks, like a
board wired stage to stage. This layout changes coordinates only.

Selecting a node thickens every attached flow. Flow arrows keep their blue
under selection because that color carries direction. Connections default to the 90°
routing; holding **Shift** while you complete a connection draws that one as a
straight angled line instead (a per-edge, presentation-only choice). A flow form
can be selected from any point along its rendered line, not only its midpoint
handle. Copying a visual selection retains exactly its nodes and internal links.
Pasting assigns fresh node IDs and offsets the copied block without inventing
links to unselected forms; the paste is one undoable edit.

Two format controls sit above the source panel. **format** reparses what is
written and rewrites it in canonical indented form; it acts only on source that
parses, so a half-typed program is left alone. **format drawing** re-lays the
mandala out with the standard circuit layout — deriving each node's column from
its own structural depth and repacking circle/square contents on their compact
golden spirals — so a hand-dragged graph returns to the size-aware drawing. It
changes coordinates only, never structure or generated source, and is one
undoable edit.

A drawing is executable and can open as source only when it is one exact AST:

1. It has one result/root. Multiple disconnected roots require an explicit
   `program` or compose node.
2. Every form has exactly the arity declared by Rebis.
3. Every non-root node has one parent. A shared visual child is rejected because
   emitting it twice would break the one-node/one-expression rule.
4. The graph is finite and acyclic, and a `program` node occurs only at the
   top level.
5. Source-bearing names, payloads, and optional model selectors produce syntax
   accepted by the Rebis parser.

The side panel reports `exact · 1:1` only when all five invariants hold.
Otherwise it shows the structural or parser error and preserves the drawing so
the links or payload can be corrected.

Right-drag draws a marquee in world space and replaces the current selection.
It selects every touched node; crossing a rendered flow line selects that
arrow's actual `Forward` or `Backflow` node. Hold `Ctrl` while dragging to
add to the set, or `Ctrl`-click a form/arrow handle to toggle it. Delete removes
the complete set as one undoable edit. **Run selection** builds the induced
subgraph containing exactly the selected nodes and internal links, then runs it
as a block only if that subgraph is itself an exact Rebis AST.

The faint green eight-rayed chaos star in the lower-right canvas is chrome
only. It is not a node, cannot be selected, never appears in generated Rebis,
and has no runtime effect.

### PDF export

The visual header's **export PDF** action opens a native save dialog and writes
the complete 2D mandala to one A4 page. Export uses vector paths and embedded
text rather than a screenshot, fits the whole drawing independently of the
current pan/zoom, and preserves the active theme, resized geometry, nesting
paint order, full fitted captions, model bindings, and each operator's straight
or right-angle route. The static program triangle remains anonymous, matching
its unselected canvas state.

### Structural 3D projection

Choose `3D · STRUCTURE` from the header's `VIEW` dropdown. It is a deterministic
projection of the same `Mandala`; it is not another graph and cannot diverge
from the `2D · EDIT` drawing.

1. Position is derived from the syntax, not from the 2D drawing: the 3D reading
   is a **cone tree**, not the flat mandala extruded. Each nesting layer is its
   own plane, and every form fans its operands onto a ring around itself in the
   next plane. A child's angular share of that ring is proportional to the
   subtree it carries, and each layer draws its cone a golden step tighter, so a
   subtree nests inside its parent's cone and the figure occupies real volume.
2. Results—nodes with no structural parent—start at depth 0. A lone program
   sits on the axis; several independent roots share a ring.
3. Each ordered operand is one structural layer deeper, and the layer supplies
   Z. An invalid shared form remains one diagnostic node at its deepest reached
   layer; exact source generation rejects it.
4. An invalid closed recursive component has no ordinary result, so its
   earliest-created node is a stable synthetic depth-0 inspection entry.
5. Reaching an ancestor marks the actual ordered-child relation as a recursive back-edge.
   Participating nodes receive a stable helical offset based on creation order;
   renderers draw those back-edges as lifted Bézier arcs.
6. Every form remains represented. A complete `->` or `<-` is projected as its
   operator connection between the same circle/square blocks in 2D and 3D,
   rather than as a second midpoint object.

Each structural depth receives a faint neutral plane. Compose boundaries are
shaded spheres; mediator squares are extruded boxes with distinct front, back,
and side faces. Pieces and operator connections cast clear screen-space
shadows onto the planes; deeper forms cast a longer, softer shadow, while
painter order still lets nearer forms cover farther ones. The light remains
fixed as the camera orbits, making the structure's rotation easier to read.
These are rendering cues only and carry no Rebis meaning.

Dragging empty space orbits (yaw and pitch); dragging a piece moves its
presentation-only 3D offset. The on-object gizmo exposes world X/Y/Z axes, and
the `free`, `X`, `Y`, `Z` toolbar controls or `G`/`X`/`Y`/`Z` keys constrain the
move. The arrow keys move through the space, the wheel changes perspective
zoom, click selects, **reset view** restores the camera, and **reset pieces**
clears all hand-set offsets. Piece motion is undoable but changes neither exact
source nor the 2D arrangement. Camera operations remain outside undo history.

The mandala is scrollable. Enter `/graph`, then use `hjkl`, arrow keys, Page Up,
Page Down, `Home`, or `g`. `Esc` returns to source focus. `/panel hide` removes
the panel, `/panel show` restores it, and `/panel` or `/panel toggle` toggles
it.
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

Source editing includes normal, insert, character-visual (`v`), line-visual
(`V`), and rectangular block-visual (`Ctrl-V`) modes. Visual selections support
motions plus `y`, `d`, `x`, and `c`; `p`/`P` replace the selection from the
unnamed register in one undo step. Block yanks and puts retain their columns and
pad short rows when necessary.

The terminal workspace and the visual Source tab call the same typed editor
state machine. Counts, motions, operators, Unicode cursor offsets, paired
insertion, grouped insert undo, registers, and selection ranges cannot drift
between the two frontends. Visual mode additionally provides pointer caret
placement, drag selection, syntax-coloured text, OS clipboard copy, and its own
Vim command line. `:w`, `:w FILE`, `:e FILE`, `:q`, `:q!`, and `:wq` have the
same dirty-buffer protections as the terminal. `Ctrl-[` is accepted as Escape
in both.

Direct editing is the default: typing inserts text immediately and the usual
arrow, Home, End, Backspace, and Delete keys work without Vim modes. Bare `/`
inserts source text; `Ctrl-K` opens the Kaos command palette. Enable Vim for the
current workspace with `/vim on`, flip it with `/vim toggle`, and disable it
with `/vim off`. To persist the preference, use `/vim always` or `/vim never`.
The visual Source tab exposes the same session control as a clear
**Vim mode · ON/OFF** toggle. Persistence updates the `vim_mode` entry
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
`%` gates return the selected branch, and macro calls return their expanded
program.

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
`Ctrl-C` is stop-first on both screens. In the Rebis workspace it terminates
every serial and parallel working, scatters every queued item, drops any
pending authority question, and ends any run left claiming to run with no
subprocess behind it. Only with nothing in flight does it ask to quit: the
first idle `Ctrl-C` arms the question, a second confirms it, and any other key
answers "stay". (The chat screen is the same, with a typed draft as one more
thing to clear before the ask.)
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

Kaos defaults to 256 macro expansions, 64 distinct module loads, 1,024 model
calls per run, and four parallel square branches. Override them with
`KAOS_REBIS_MAX_EXPANSIONS`, `KAOS_REBIS_MAX_MODULES`, `KAOS_REBIS_MAX_CALLS`,
and `KAOS_REBIS_MAX_CONCURRENCY`. Zero disables macro expansion, imports, or
model calls; zero concurrency means sequential evaluation. Each tool-using
agent model turn has a 600-second wall-clock limit; set
`KAOS_REBIS_TIMEOUT_S` to accommodate a slower local model.

Model-only `[]` children can use that concurrency directly. Live children with
file and command tools remain sequential unless
`KAOS_REBIS_GIT_WORKTREES=1` is enabled. In that mode Kaos snapshots the current
Git working tree, gives each child a detached worktree, and reconciles the
resulting edits in source order immediately before the mediator. The user's
branch and index are untouched and the combined edits remain unstaged. Git is
optional: a missing executable, old worktree implementation, or non-repository
directory produces guidance and transparently falls back to normal sequential
execution.

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
