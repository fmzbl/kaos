# Drawn sigils, and Carroll's operation

A **drawn sigil** is not a mandala. A mandala renders a program — every node is
a form and the picture *is* the code, and it can be turned back into source and
checked against what it came from. A sigil is the other direction: a mark
standing for an intent, which a model reads and answers with a program. They
share no type, deliberately, because asking a drawing to be a program is asking
the wrong thing of it.

The code is `kaos-core/src/ink.rs`; the pane is `Pane::Ink`.

## The operation has three parts

Carroll, *Liber Null*, "Sigils": a sigil is **constructed**, it is **lost to the
mind**, and it is **charged**. Each part is already somewhere in the code, and
this is where they map — written down so the next reader does not rebuild it.

| Carroll | here | what it is |
|---|---|---|
| construct | **compose** — draw, and stack | strokes become a raster; several stack into one intent |
| lose | **commit** — `Wall::commit(…, Retention)` | the glyph is written; the desire is or is not |
| charge | **run** — the program the stack asks for | the model reads the marks and answers with Rebis |

**The run is the charge.** There is no fourth step to build. A sigil is charged
by being spent on something, and what a sigil is spent on here is one model
call that carries the picture — so the operation is complete the moment the
program runs, and inventing a ritual around it would be decoration.

## Construction

Two ways in, and they compose:

**Draw it.** Strokes on the canvas. Nothing about them is interpreted — a sigil
is compressed intent, and the model is asked to *read* it, not to decode it.

**The word method.** `ink::letter_skeleton(desire)` is Carroll's: write the
desire in a sentence, strike out every letter that has already appeared, and
rearrange what survives into a glyph.

```
"I wish to obtain the Necronomicon"  →  IWSHTOBANECRM
```

The surviving letters are offered as a **scaffold** — drawn faint, to draw over
and then discard. The scaffold is never saved and is never in the raster a model
is shown: a model reading the letters would be reading the desire, which is the
opposite of what a sigil is for.

> *Liber Null* reproduces this example as `INSHTOBANECRM`. The method applied to
> that sentence gives a **W** where the transcription has an **N** — the W of *"I
> wish"*. The method is specified precisely enough to win over a transcription,
> so `letter_skeleton` returns the W, and the discrepancy is pinned in
> `kaos-core/tests/sigil_wall.rs` so nobody quietly "corrects" it back.

Sigils **stack**. Several marks compose into one intent before anything runs,
which is the same discipline the language applies everywhere else: intent is
composed before it is spent. Stacked drawings become nested framings, because
that is what "all of these are in scope at once" already means:

```lisp
(+ (&: "one.png")
   (+ (&: "two.png")
      (>< "…")))
```

## Losing it — a mode, never a default

> *"To successfully lose the sigil, both the sigil form and the associated
> desire must be banished from normal waking consciousness."*

What the code does by default is the exact opposite. `Stack::said` keeps the
stated desire in readable prose beside the glyph, and the wall lists every sigil
by name, permanently.

**That stays the default.** Losing the desire trades a real feature — finding
your sigil again, knowing what it was for — for fidelity to the source, and
making it the default would be choosing the book over the person using it.

So it is a choice, per working:

```rust
wall.commit(name, &sigil, desire, Retention::Keep, size)?;  // today's behaviour
wall.commit(name, &sigil, desire, Retention::Lose, size)?;  // the glyph only
```

`Retention::Lose` writes the glyph and **discards the desire unwritten**. Not
redacted afterwards — never written, so there is no copy anywhere to miss. A
`Lose` over a name that was previously `Keep` removes what was kept, because
otherwise the old sentence would sit beside the new glyph.

**It is irreversible, and the control has to say so before it does it.** There is
nothing to recover.

## The record

> *"A record should be kept of all work with sigils but not in such a way as to
> cause conscious deliberation over the sigilized desire."*

A run's record echoes the program it ran. So the desire is kept out of the
*program*, not filtered out of the record afterwards: a `Stack` carrying
`Retention::Lose` omits its sentence from `instruction()`, so the sentence never
reaches the prompt, the record, or a retained trace.

What a lost-sigil run's record therefore keeps is the firing, the glyph's
filename, and the answer — the work is fully traceable, and the sentence is not
in it. Which is what the passage asks for.

The tests are the acceptance criteria as written: commit a distinctive desire
under `Lose`, then read every file on the wall and assert none of them contains
it.

## What a sigil does not promise

`><` guarantees the answer parses. It guarantees nothing about whether the
program means what was drawn — a sigil is compressed intent and the
decompression is a guess. The generated source is something to **read before
running**, and the interaction is built around that rather than around trusting
it.
