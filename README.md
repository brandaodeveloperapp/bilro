# bilro

Token economy for coding agents. Runs your command, keeps what informs, and
learns what your tools always print so it stops repeating it back to you.

Named after the bobbins of Ceará lacework: many small threads, one pattern.

## Install

```sh
cargo build --release
./target/release/bilro install
```

`install` puts the binary on your PATH and registers four hooks with Claude
Code, keeping a copy of the settings file it changes. Running it twice changes
nothing.

## What it does

```
bilro filter <cmd>     runs the command and returns only what informs
bilro read <file>      reads a file compressed (--outline for structure only)
bilro grep <pattern>   search grouped by file, without the repetition
bilro run <cmd>        runs and indexes; only the passage you ask for comes back
bilro find <term>      searches what has already been indexed
bilro ready            can the tools bilro replaces be retired yet?
bilro bill             what context costs before you ask for anything
bilro verify [--run]   checks memories that assert a dated fact
bilro lint             broken links, orphaned memories, most-cited
bilro propose          memories worth writing, from what you keep running
bilro sessions         agents dispatched per session
```

## How the compression works

Two passes, and the order matters.

**Structure first.** Output has a shape — a columnar table, a diff, a test
report, a file listing, a diagnostic list, an install log, a JSON dump — and a
shape can be compressed on a command that has never been seen before. Seven
shapes cover the output of dozens of tools, including tools nobody wrote a rule
for, because the shape is what repeats across them, not the program name.

**History second.** Every run is recorded: how many times a command shape has
been seen, and in how many of those runs each line appeared. A line present in
nearly every run carries no information and is dropped. A line never seen
before is always kept. Below three runs there is no history to judge with, so
nothing is dropped at all.

Numbers and hashes collapse when deciding what is noise, so a duration or a
counter does not make every run look new — but they are kept when deciding what
is *new*, because `module 3 failed` and `module 7 failed` are different events
that happen to share a shape.

## The one rule that outranks compression

**A line reporting a failure is never dropped.** Not when it repeats, not when
it appears in every run, not when the output is trimmed to a budget. A build
that breaks the same way every day is still the answer to what happened, and a
tool that hides it is worse than no tool.

Severity is recognised two ways: by the words and marks a line uses, and by the
shape every compiler and linter prints — a path, a line, a column. The second
matters because the first only knows the languages someone listed.

Where nothing survives compression, bilro says how many lines were suppressed
rather than printing an empty screen, and it does not claim none of them
reported a failure. Absence of failure is not provable from a list of words.

## Credentials

Output is redacted before it is shown and before it is stored, because a secret
written to the learning database once survives every later run. The value's
shape is what is inspected, not the name of the field: connection strings,
query parameters, Authorization headers, bare JWTs, password flags and private
key blocks all carry secrets under innocent labels.

## Declared checks

A memory may carry a `verify:` line asserting how to confirm a dated fact.
Nothing runs without `--run`, no shell is involved, shell syntax is refused,
and only a short list of read-only programs may be invoked at all. `git` is
allowed but must name a read-only subcommand, because an alias beginning with
`!` runs through a shell and `-c` can define one inline.

## Requirements

Rust 1.75+ to build. Nothing at runtime — the binary is self-contained,
including SQLite with FTS5.

## Licence

MIT.
