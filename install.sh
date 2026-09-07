#!/bin/sh
# bilro installer. Downloads a release binary when one exists for this machine,
# builds from source when it does not, then registers the hooks and the MCP
# server. Safe to run twice: nothing is registered a second time.
set -eu

REPO="brandaodeveloperapp/bilro"
DESTINO="${BILRO_DIR:-$HOME/.local/bin}"

erro() { printf '\n  %s\n\n' "$1" >&2; exit 1; }
passo() { printf '  %s\n' "$1"; }

alvo() {
  sistema=$(uname -s)
  arquitetura=$(uname -m)
  case "$sistema-$arquitetura" in
    Darwin-arm64)  echo "aarch64-apple-darwin" ;;
    Darwin-x86_64) echo "x86_64-apple-darwin" ;;
    Linux-x86_64)  echo "x86_64-unknown-linux-gnu" ;;
    Linux-aarch64) echo "aarch64-unknown-linux-gnu" ;;
    *) echo "" ;;
  esac
}

baixar() {
  url="https://github.com/$REPO/releases/latest/download/bilro-$1.tar.gz"
  passo "baixando $1"
  tmp=$(mktemp -d)
  if ! curl -fsSL "$url" -o "$tmp/bilro.tar.gz" 2>/dev/null; then
    rm -rf "$tmp"
    return 1
  fi
  tar -xzf "$tmp/bilro.tar.gz" -C "$tmp"
  mkdir -p "$DESTINO"
  mv "$tmp"/bilro "$DESTINO/bilro"
  [ -f "$tmp/bilro-painel" ] && mv "$tmp/bilro-painel" "$DESTINO/bilro-painel"
  chmod +x "$DESTINO/bilro" "$DESTINO/bilro-painel" 2>/dev/null || true
  rm -rf "$tmp"
  return 0
}

compilar() {
  command -v cargo >/dev/null 2>&1 || erro "sem binario pronto para esta maquina e sem cargo para compilar. Instale Rust em https://rustup.rs e rode de novo."
  passo "compilando do codigo (leva alguns minutos na primeira vez)"
  tmp=$(mktemp -d)
  git clone --depth 1 "https://github.com/$REPO" "$tmp/bilro" >/dev/null 2>&1 \
    || erro "nao consegui clonar $REPO. Se o repositorio for privado, rode: gh auth login"
  ( cd "$tmp/bilro" && cargo build --release --quiet )
  mkdir -p "$DESTINO"
  cp "$tmp/bilro/target/release/bilro" "$DESTINO/bilro"
  cp "$tmp/bilro/target/release/bilro-painel" "$DESTINO/bilro-painel" 2>/dev/null || true
  rm -rf "$tmp"
}

printf '\n  bilro\n\n'

PLATAFORMA=$(alvo)
if [ -n "$PLATAFORMA" ] && baixar "$PLATAFORMA"; then
  passo "binario instalado"
else
  compilar
fi

case ":$PATH:" in
  *":$DESTINO:"*) ;;
  *) printf '\n  %s nao esta no PATH. Adicione ao seu shell:\n    export PATH="%s:$PATH"\n' "$DESTINO" "$DESTINO" ;;
esac

passo "registrando hooks e servidor MCP"
"$DESTINO/bilro" install

printf '  pronto. Comece por:\n\n    bilro doctor      confere a instalacao\n    bilro stats       o que ele ja guarda\n    bilro-painel      abre o painel\n\n'
