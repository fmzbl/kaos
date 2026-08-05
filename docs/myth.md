# The myth — a migration note

**The myth language is retired.** Kaos has one orchestration language, and it is
Rebis: see [`REBIS.md`](REBIS.md) for what it is and
[`../kaos-conformance/README.md`](../kaos-conformance/README.md) for what is
proven about it.

A `KAOS_MYTH` written in the old syntax still works. It is read, translated once
on load, and the run prints the Rebis program to write instead. That translation
goes away a release from now; this note is how you do it by hand before then.

## Why

Kaos had **two** orchestration languages, which is the shape of an accident
rather than a design:

| | Rebis | myth |
|---|---|---|
| where | `rebis-lang`, a git dependency | `kaos-agent/src/myth.rs`, 518 lines |
| surface | 22 operators, frozen | 5 forms, 4 gates |
| cost model | one written prompt is one firing, tested | none |
| tests | 462 in-repo | 12 unit tests |

Only one of them could say what a program would cost before it ran, and that is
the claim the whole harness is ordered around.

## The translation

| myth | Rebis |
|---|---|
| `fire` | `($ task)` |
| `(ask "role")` | `(+ "role" ($ task))` |
| `(spread N X)` under a gather | N branches of the square, written out |
| `(gather vote X)` | `([vote] …)` |
| `(gather first X)` | `([first] …)` |
| `(gather (check "cmd") X)` | `([check] …)`, and see below |
| `(gather (mirror P) X)` | `([mirror] …)`, and see below |
| `(pipe A B …)` | `(-> A B …)` |

A whole program binds the task once, because a Rebis program *is* the prompt and
has no arguments — `(&)` obtains it:

```lisp
; (gather vote (spread 5 fire))
(= task (&) ([vote] ($ task) ($ task) ($ task) ($ task) ($ task)))
```

**The spread width becomes a written count.** That is the point rather than a
cost of the change: a Rebis program's price is countable by reading it, and a
width hidden behind `${KAOS_K}` is exactly what stopped a myth's price from
being. `kaos rebis run` and the run browser show the firing count before launch.

## Two things the translation cannot carry

**`(check "cmd")` loses its command.** A Rebis mediator head is one atom, so
`[check]` names a gate and cannot carry a command into one. Set the verifier as
`KAOS_GATE` and run with `--allow-tools`; a program that names `[check]` without
command authority is refused rather than answering unverified. This is a
deliberate narrowing — a program is a thing people share, and running one should
not be able to start a subprocess the person running it did not choose.

**`(mirror P)` loses its threshold.** `Gate::Mirror(P)` scored each candidate
against *the task*, under `P`. `[mirror]` is an ordinary symbol mediator scored
against *the mediator's own word*. They are not the same gate, and the
translation says so when it runs.

## Three things that changed, deliberately

- **A vote with no majority refuses.** `myth` returned a candidate — whichever
  one thread scheduling put first. A conclave whose branches agree about nothing
  has found no signal, and reporting one anyway is a false ship.
- **The bowtie does not translate.** A Rebis square's mediator is handed every
  accepted firing since the square started, not the values its branches
  returned — so a gate nested inside a vote has its verdict discarded. Write the
  stages as an arrow instead, where the stage boundary keeps them apart.
- **A piped stage reads no runtime-authored text.** `myth` appended `Work so
  far:`; Rebis authors nothing but `INPUT:`.

Each is pinned in `kaos-agent/tests/myth_equivalence.rs`, which is also where the
rest of the evidence lives.

## What did not change

`KAOS_AGENTIC` still makes every leaf a full read/edit/bash session in an
isolated copy of the tree, and `[check]` still re-applies each attempt's diff and
runs the gate there. The graph became Rebis; the leaf is still a trait, and that
split is the one worth keeping.
