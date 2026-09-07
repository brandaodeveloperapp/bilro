#!/bin/sh
# Removes bilro and the three tools it replaces, so a fresh install can be
# tested against a machine that holds none of them. It refuses to run without
# a backup path, because the learning history is the one thing a reinstall
# cannot rebuild.
set -eu

BACKUP="${1:-}"
[ -n "$BACKUP" ] && [ -d "$BACKUP" ] || {
  printf '\n  usage: %s <backup-directory>\n\n  Refusing to remove anything without somewhere to restore from.\n\n' "$0" >&2
  exit 1
}

step() { printf '  %s\n' "$1"; }

printf '\n  removing bilro and the tools it replaces\n\n'

step "unregistering bilro hooks and the MCP server"
python3 - "$HOME" <<'PY'
import json, os, sys
home = sys.argv[1]

settings = os.path.join(home, ".claude", "settings.json")
if os.path.exists(settings):
    s = json.load(open(settings))
    removed = 0
    for event, groups in list(s.get("hooks", {}).items()):
        for g in groups:
            before = len(g.get("hooks", []))
            g["hooks"] = [h for h in g.get("hooks", []) if "bilro" not in h.get("command", "")]
            removed += before - len(g["hooks"])
        s["hooks"][event] = [g for g in groups if g.get("hooks")]
        if not s["hooks"][event]:
            del s["hooks"][event]
    json.dump(s, open(settings, "w"), indent=2)
    print(f"    {removed} hooks removed")

claude = os.path.join(home, ".claude.json")
if os.path.exists(claude):
    c = json.load(open(claude))
    if c.get("mcpServers", {}).pop("bilro", None) is not None:
        json.dump(c, open(claude, "w"), indent=2)
        print("    MCP server unregistered")
PY

step "disabling the caveman and context-mode plugins"
python3 - "$HOME" <<'PY'
import json, os, sys
settings = os.path.join(sys.argv[1], ".claude", "settings.json")
s = json.load(open(settings))
plugins = s.get("enabledPlugins", {})
for name in list(plugins):
    if "caveman" in name or "context-mode" in name:
        plugins[name] = False
        print(f"    {name} disabled")
json.dump(s, open(settings, "w"), indent=2)
PY

step "removing binaries and stored data"
rm -f "$HOME/.local/bin/bilro" "$HOME/.local/bin/bilro-panel"
rm -rf "$HOME/.claude/bilro"
rm -rf "$HOME/tools/bilro/dist"

step "dropping the build cache so the next build starts from nothing"
if [ -d "$HOME/tools/bilro/target" ]; then
  ( cd "$HOME/tools/bilro" && cargo clean )
fi

printf '\n  done. Nothing of bilro, caveman or context-mode is wired up.\n'
printf '  Backup kept at: %s\n\n' "$BACKUP"
printf '  Reinstall with:\n    curl -fsSL https://raw.githubusercontent.com/brandaodeveloperapp/bilro/main/install.sh | sh\n\n'
