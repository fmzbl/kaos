# kaos-conformance

End-to-end conformance for the Rebis language: every operator, run through the
real `kaos` binary against a real model.

Every other test in this workspace scripts the oracle — an answer is decided in
advance and the test checks what the runtime did with it. That proves the
runtime and proves nothing about the language meeting a model, which is the only
place it is ever used. These run whole programs and check what actually came
back.

## Running it

```bash
ollama serve &                       # once
ollama pull llama3.2:3b              # once — the shell suite's default
ollama pull qwen3:4b                 # once — the in-process tests' default
cargo build --release                # the binary under test

./kaos-conformance/run.sh            # everything
./kaos-conformance/run.sh 03 07 19   # only those
```

Knobs, all optional:

```bash
MODEL=ollama:qwen3:4b ./kaos-conformance/run.sh   # a different model
TIMEOUT=600 ./kaos-conformance/run.sh          # a slower machine
OLLAMA_HOST=http://box:11434 ./kaos-conformance/run.sh
```

Output is one line per program, and a full log per program under
`kaos-conformance/results/`:

```text
01-prompt                  ok  (1 calls)
07-conditional             ok  (2 calls)
13-bind                    FAIL  (3 calls)
    · wanted 2 calls, made 3
```

Expect it to be slow, and know the failure mode. A 3B model on CPU answers a
short prompt in about a second once loaded, and the suite makes a few hundred
calls — so a full pass is a coffee. Run a subset while iterating.

**If everything times out, the server is wedged, not the language.** A
reasoning model that was interrupted mid-generation can leave its runner
spinning at several hundred percent CPU, and every later request — from this
suite, from `curl`, from anything — queues behind it forever. It looks exactly
like a hang in the host, and it is not:

```bash
ps -o pid,pcpu,etime -p "$(pgrep -f 'ollama runner' | head -1)"   # >100% for minutes?
sudo systemctl restart ollama                                     # the only fix
```

The runner is owned by the ollama service user, so `kill -9` from your own
account will not clear it. Restart the service and re-run. This cost an
afternoon to diagnose once; it is written down so it costs nobody else one.

## Why the binary and not the library

The script drives `target/release/kaos`, not a Rust harness linking
`rebis-lang`. What is under test is the whole path a person actually uses —
argument handling, the host's oracle and inlet, the provider, the record, the
dream — and a test that linked the library directly would prove every part
while skipping the assembly. Twice already a bug lived exactly in that gap.

## How a program declares what it promises

Each `.rebis` file carries its own expectations in a header, so the program and
its claim live in one file and cannot drift apart:

```rebis
; Each answer becomes the next stage's INPUT.
; expect: calls 2
; expect: clean
; expect: answers
; expect: contains INPUT:
(-> "Answer with exactly the word: seed"
    "Repeat the word you were given, exactly, and nothing else.")
```

| claim | means |
|---|---|
| `calls N` | exactly N model calls |
| `calls >=N` | at least N |
| `contains TEXT` | TEXT appears somewhere in the run |
| `absent TEXT` | TEXT appears **nowhere** — catches a branch that should not have run |
| `clean` | no diagnostics |
| `diagnostic TEXT` | a diagnostic mentioning TEXT, when one is expected |
| `answers` | the program produced a result, and did not decline |

## What is asserted, and what deliberately is not

**Structure, never prose.** A 4B model will not reliably answer with exactly the
word you asked for, so a test demanding one would fail for the wrong reason and
teach nothing. What a model *cannot* fake is the shape of the run: how many
times it was called, what reached each call as `INPUT:`, which branch expanded,
what was kept.

Counting calls is the workhorse. It is how a text-generating model gets held to
a structural claim — `05-concat` asserts one call because `$` composes text
without running its operands, and no amount of prose can make that two.

`absent` is the other half. A lazy conditional is only lazy if the branch not
taken *never fired*, and the only way to see that is to look for words that
should appear nowhere.

## Adding a program

Drop a `.rebis` file in `programs/` with an `; expect:` header. That is the
whole registration step — the runner globs the directory.

Number them by area, loosely: `01`–`20` single operators, `21`–`30`
combinations, `31`–`40` the standard library and the collection.

## Programs that need a model which can write Rebis

`61-meta` carries `; requires: generation` and is **skipped by default**. Run it
with `GENERATION=1`, or name it explicitly.

Whether a model can write Rebis is a capability, not a property of the language,
and the two should not be conflated in one suite. The mechanism — fire, parse,
run, budget, diagnose — is carried by the in-process tests in the language repo,
which script the oracle and so measure the runtime rather than the model.

Two things were measured getting there, and both are worth knowing.

**A `><` prompt carries no language reference.** The runtime authors no prompt
text of its own, so nothing tells the model what Rebis is unless the program
does. Asked bare for a program, qwen3-32b wrote HCL — `"agent" { prompt = "…" }`
— which `GeneratedSyntax` caught and reported verbatim. Given a two-sentence
reference in the prompt, the same model writes valid Rebis. That is what the
collection's `wright` family is for: every one of its macros carries one.

**Do not ask a model to echo a program.** An earlier `61-meta` asked for a
literal program to be repeated back, and every model tried — llama3.2:3b,
qwen3:4b, qwen3-32b — answered the question embedded in that literal instead of
writing the program around it. Asking for an echo is asking something
ambiguous. Ask for a generation.

With the reference and a real task, qwen3-32b passes and llama3.2:3b does not:
it emitted an unbalanced square. That is the capability line, measured.

## What `><` coverage can and cannot be

`61-meta` is the only end-to-end program for the meta operator, and that is a
measurement rather than an omission.

Generation needs a model to emit literal Rebis, and neither local model does
that reliably. Asked to reply with exactly
`"Answer with exactly the word: generated"`, llama3.2:3b emits the program —
stable across three runs. Asked with the same prompt and the single word
changed to `drafted`, it answers `drafted` instead, also stably. qwen3:4b fails
the working version too, and timed out at 400s on a nested one.

Two things follow. Generation coverage at this model size is word-level
capricious, so a second program was written and removed rather than left
permanently red. And the failure is quiet: a bare word is valid Rebis, so it
parses, runs, answers nothing, and raises no diagnostic. `GeneratedSyntax`
catches prose; nothing catches a model that does the task instead of writing
the program that would.

The mechanism itself — parse, run, budget, diagnostic, the live definition
scope — is carried by the eleven in-process tests in the language repo, which
script the oracle and so measure the runtime rather than the model.

## Two suites, two models, and why

The shell suite defaults to **llama3.2:3b** and the in-process tests to
**qwen3:4b**. That is measured, not sloppy: on this machine qwen3:4b answers a
one-word prompt in ~110s even with reasoning off, so a fifty-program pass is an
hour and a half and nobody runs it twice, while the 3b answers in ~10s. The
in-process suite is twenty-odd tests rather than fifty programs, so it can
afford the slower model — and being a reasoning model is occasionally the
point there.

Both take the same override, so a run can be pinned to one model end to end:

```bash
MODEL=ollama:qwen3:4b ./kaos-conformance/run.sh
KAOS_CONFORMANCE_MODEL=llama3.2:3b cargo test -p kaos-conformance -- --ignored
```

A language property that only holds on one model is not a language property.
If a claim here passes on one and fails on the other, that is a finding.

## The Rust tests beside it

`tests/operators.rs` runs the same programs in-process, which lets it assert
things the log cannot show — that `result.kept` is exactly what the dream's body
answered, that a bound name stood for its value in *both* positions of a
composition, and that the record holds what an imaginary space let cross and
nothing else.

The imaginary space is the clearest case for the in-process suite existing at
all. A space changes neither the prompts nor their order, so `52-imaginary` and
`53-imaginary-real` produce **identical logs** — same call count, same shapes,
both clean. The entire difference is in the record, which the shell runner
cannot see. The two are written as a pair and read as a pair: alone, either
could pass for a reason unrelated to braces; together they isolate the
delimiter as the only thing that changed. They need the same model and are `#[ignore]`d:

```bash
cargo test -p kaos-conformance -- --ignored --test-threads=1 --nocapture
```

Single-threaded on purpose: one machine has one model, and eight tests racing
for it measure the queue.
