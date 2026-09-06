# bilro

**Weigh what your AI context costs before you ask it anything.**

Every request to a coding agent carries a fixed load you never see: instruction
files, the agent catalogue, project memory, tool schemas. You pay it whether or
not anything uses it. `bilro` puts that on a scale.

```
$ bilro bill

  o que você paga antes de pedir qualquer coisa

    3288 tok  ████████████████████████  catálogo de 50 agentes
    2468 tok  ██████████████████░░░░░░  CLAUDE.md
    1442 tok  ███████████░░░░░░░░░░░░░  MEMORY.md (56 entradas)
     240 tok  ██░░░░░░░░░░░░░░░░░░░░░░  RTK.md
  ──────────────────────────────────────────────────────────
    7438 tok  por request, use ou não

  ⚠  4 agentes sem `tools:` herdam o catálogo inteiro
```

## Why

Tools that track AI spend tell you what you already burned. `bilro` shows the
weight of the empty container — the tare — so you can see what you are paying
for before the work starts, and which of it is waste.

## Commands

```
bilro bill      what every request costs, ranked by weight
bilro doctor    agents that inherit the whole catalogue, or cannot reach
                your sandbox tools
bilro verify    memories that assert a fact and cannot check themselves
```

## Memory that checks itself

A memory file can declare how to prove it is still true:

```yaml
---
name: ios-ship
verify: grep -oE '"version": "[0-9.]+"' app.json | head -1
expect: 1.1.8
---
```

```
$ bilro verify
  ✗ ios-ship
      esperava "1.1.4", veio "version": "1.1.8"

  1 memoria afirma fato que envelhece e nao sabe se verificar
    landing-reformulacao    23d  valor R$250
```

Memories that state a version, an amount or a count and have gone untouched
are flagged even without a `verify:` — those are the ones that quietly lie.

## Run a command without paying for its output

```
$ bilro run "npx tsc --noEmit --listFiles" --find "features/references"

  ok  7 trechos indexados, 11240 tok ficaram fora do contexto
```

The output is chunked into SQLite FTS5 and ranked by BM25 — both built into
Node, no dependency — so the bytes stay on disk and only what you asked for
comes back. `bilro find <term>` retrieves the rest whenever you need it.

## Cut the noise a command makes

```
$ bilro filter "npx jest src/features"
  ...
  bilro filter (jest): 1061 tok cortados, 64%
```

Per-tool rules keep the lines you act on and drop the ones that only prove the
tool ran — jest stack frames through `node_modules`, gradle's `UP-TO-DATE`,
docker layer chatter, fastlane's compiler warnings. Repeated lines collapse
with a count. An unknown command still gets the collapse.

## Where the session went

```
$ bilro sessions

  09-06 21:30    4 agentes    85k de custo fixo   2 repetidos
```

Every subagent dispatch is priced and remembered, so a session can be read
back afterwards: how many agents, what they cost before doing anything, and
how many asked a question the session had already asked.

## Style

```
bilro style terse
```

Re-emits a concision ruleset at every session start, because a rule stated
once decays in a long conversation. Code, commits, security notes and ordered
steps are exempt.

## Name

A *bilro* is the wooden bobbin used in the bobbin lace of Ceará, Brazil — the
tool that places one thread at a time, with nothing left over.

MIT.
