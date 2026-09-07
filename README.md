# bilro

Context economy for coding agents. Runs your command, returns only what informs,
and learns what your tools always print so it stops repeating it back to you.

```
$ bilro filter git log --oneline -40
9f3c1ab fix(worker): retry the job that fails when the queue is empty

  98% smaller (39 repeated lines)
```

Out of forty commits, the one line mentioning a failure survived. The other
thirty-nine you had already seen on previous runs.

---

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/brandaodeveloperapp/bilro/main/install.sh | sh
```

Downloads the binary for your platform, or builds from source if there is none,
puts it on your PATH, and registers the hooks and the MCP server. Running it
twice registers nothing twice.

Check it:

```sh
bilro doctor
```

From source, if you prefer:

```sh
git clone https://github.com/brandaodeveloperapp/bilro
cd bilro && cargo build --release && ./target/release/bilro install
```

Rust 1.75+ to build. At runtime the binary is self-contained — SQLite with FTS5
is compiled in. Only `bilro fetch` shells out, to `curl`, which keeps a TLS
stack and a certificate store out of the binary.

---

## The first five minutes

**1. See what your context costs before you ask for anything.**

```sh
bilro bill
```

Adds up CLAUDE.md, MEMORY.md and the agent catalogue. That number is paid on
every request, used or not.

**2. Run a verbose command two or three times.**

```sh
bilro filter npm test
bilro filter npm test
bilro filter npm test
```

The first time it cuts nothing — there is nothing to compare against. From the
third run on it knows what that command always prints, and returns what changed.

**3. See what it kept.**

```sh
bilro stats
bilro recall
```

**4. Open the panel.**

```sh
bilro-panel
```

---

## What it does

```
bilro filter <cmd>     runs the command and returns only what informs
bilro read <file>      reads a file compressed (--outline for structure only)
bilro grep <pattern>   search grouped by file, without the repetition
bilro exec <lang>      runs a snippet, only what it prints comes back
bilro run <cmd>        runs and indexes; only the passage you ask for returns
bilro find <term>      searches what has already been indexed
bilro fetch <url>      fetches a page, indexes it, returns what you asked for
bilro recall [term]    what has already happened in this project
bilro ready            can the tools it replaces be retired yet?
bilro bill             fixed context cost per request
bilro verify [--run]   checks memories that assert a dated fact
bilro lint             broken links, orphaned memories, most-cited
bilro propose          memories worth writing
bilro stats            what it holds and how much it saves
bilro doctor           diagnose the installation
bilro purge <target>   delete stored data (--confirm confirms)
bilro serve [port]     panel in the browser
bilro install          register hooks and MCP server
bilro mcp              MCP server over stdio
```

---

## How the compression works

Two passes, and the order matters.

**Structure first.** Output has a shape — a columnar table, a diff, a test
report, a file listing, a diagnostic list, an install log, a JSON dump — and a
shape can be compressed on a command that has never been seen before. Seven
shapes cover the output of dozens of tools, including tools nobody wrote a rule
for, because what repeats between them is the shape, not the program's name.

**History second.** Every run is recorded: how many times that command shape has
been seen, and in how many of those runs each line appeared. A line present in
nearly every run carries no information and is dropped. A line never seen before
is always kept. Below three runs there is no history to judge with, and nothing
is dropped.

Numbers and hashes collapse when deciding what is noise — a duration or a
counter should not make every run look new. They are preserved when deciding
what is *new*, because `module 3 failed` and `module 7 failed` are different
events that happen to share a shape.

---

## The rule that outranks compression

**A line reporting a failure is never dropped.** Not when it repeats, not when
it appears in every run, not when output is trimmed to a budget. A build that
breaks the same way every day is still the answer to what happened, and a tool
that hides it is worse than no tool.

Severity is recognised two ways: by the words and marks a line uses, and by the
**shape** every compiler and linter on earth prints — a path, a line, a column.
The second matters because the first only knows the languages someone listed.

When nothing survives compression, bilro says how many lines were suppressed
instead of printing an empty screen. It does not claim none of them reported a
failure: absence of failure is not provable from a list of words.

---

## Credentials

Output is redacted before it is shown and before it is stored, because a secret
written to the database once survives every later run. What is inspected is the
**shape of the value**, not the name of the field: connection strings, query
parameters, `Authorization` headers, bare JWTs, password flags and private key
blocks all carry secrets under innocent labels.

What is not a secret stays readable — the host and database of a connection
string, the `page` parameter beside the API key, the word `Bearer` without its
token.

---

## The panel

```sh
bilro-panel      # native application
bilro serve      # in the browser, at http://127.0.0.1:7777
```

Shows what the tool **did**, not what it estimates: the savings come from
replaying denoise over the last output of every learned command.

The graph has four modes, because a network of memories asks more than one
question:

| mode | what it answers |
|---|---|
| **Global** | the whole network, positioned by the pull of its links |
| **Local** | only what one memory touches, to the depth you choose |
| **By type** | each kind pulled into its own cluster |
| **Radial** | the most-cited at the centre, the rest in rings |

A memory that is cited but was never written becomes a red node on a dashed
line — a broken link is the interesting kind of link.

The browser panel listens on **loopback only**: it serves a record of your work,
and has no business being reachable from another machine.

---

## As an MCP server

`bilro install` registers bilro as a stdio MCP server, so the tools appear in
the model's own list rather than in documentation someone has to remember:

`bilro_script` · `bilro_batch` · `bilro_run` · `bilro_fetch` · `bilro_recall` ·
`bilro_remember` · `bilro_find` · `bilro_filter` · `bilro_read` · `bilro_grep`

A capability that has to be remembered is a capability that goes unused.

---

## Memories

A memory is a markdown file with frontmatter, under
`~/.claude/projects/<project>/memory/`:

```markdown
---
name: local-redis-port
description: the local API needs REDIS_PORT set explicitly
metadata:
  type: project
verify: git rev-parse HEAD
expect: ""
---

Compose publishes a different port than the config defaults to, so the API
cannot reach the queue until it is set. Related to [[local-setup]].
```

`bilro lint` finds broken links and orphaned memories. `bilro propose` suggests
memories from what you run most and what fails most. `bilro verify` checks the
ones asserting a dated fact.

**A declared check is deliberately restricted.** Nothing runs without `--run`,
no shell is involved, shell syntax is refused, and only a short list of
read-only programs may be invoked. `git` is allowed but must name a read-only
subcommand, because an alias beginning with `!` runs through a shell and `-c`
can define one inline.

---

## The journal

bilro records what was asked for, which commands failed, what subagents
concluded, and — through `bilro_remember` — what was decided, rejected, or
discovered as a constraint. A hook cannot infer those last three from a tool
call; they are judgements, and they are written at the moment they are made.

```sh
bilro recall                    # timeline
bilro recall "redis"            # by term
```

It is what answers "where were we" when a session resumes, instead of asking
someone to repeat themselves.

---

## Deleting

```sh
bilro purge index      # dry run, deletes nothing
bilro purge index --confirm
bilro purge all --confirm
```

Without `--confirm` it reports what it would delete and deletes nothing.

---

## Licence

MIT.
