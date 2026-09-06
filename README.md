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

## What it is not

`bilro` measures. It does not compress. Pair it with the tools that actually
save: a shell proxy for command output, a terse output style, a sandbox that
keeps raw bytes out of the window.

## Name

A *bilro* is the wooden bobbin used in the bobbin lace of Ceará, Brazil — the
tool that places one thread at a time, with nothing left over.

MIT.
