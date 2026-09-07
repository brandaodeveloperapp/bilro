# bilro

Economia de contexto para agentes de código. Roda o teu comando, devolve só o
que informa, e aprende o que as tuas ferramentas sempre imprimem para parar de
repetir isso de volta pra ti.

O nome vem dos bilros da renda cearense: muitos fios finos, um desenho só.

```
$ bilro filter git log --oneline -40
2945d6b ops(monitoring): alert Igor on WhatsApp when prod health fails

  98% menor (39 linhas repetidas)
```

De quarenta commits, sobrou a única linha que menciona uma falha. As outras
trinta e nove tu já tinha visto nas execuções anteriores.

---

## Instalar

```sh
curl -fsSL https://raw.githubusercontent.com/brandaodeveloperapp/bilro/main/install.sh | sh
```

Baixa o binário da tua plataforma, ou compila se não houver um pronto, coloca no
PATH e registra os hooks e o servidor MCP. Rodar duas vezes não duplica nada.

Confere:

```sh
bilro doctor
```

Compilando do código, se preferires:

```sh
git clone https://github.com/brandaodeveloperapp/bilro
cd bilro && cargo build --release && ./target/release/bilro install
```

Precisa de Rust 1.75+ para compilar. Em execução o binário se basta — SQLite com
FTS5 vai compilado dentro. Só o `bilro fetch` chama o `curl` do sistema, o que
mantém uma pilha TLS e um armazém de certificados fora do binário.

---

## Os primeiros cinco minutos

**1. Veja o que teu contexto custa antes de pedir qualquer coisa.**

```sh
bilro bill
```

Soma CLAUDE.md, MEMORY.md e o catálogo de agentes. Esse número é pago em toda
requisição, uses ou não.

**2. Rode um comando verboso duas ou três vezes.**

```sh
bilro filter npm test
bilro filter npm test
bilro filter npm test
```

Na primeira, ele não corta nada — não tem com o que comparar. Da terceira em
diante ele já sabe o que aquele comando sempre imprime, e devolve só o que mudou.

**3. Veja o que ele guardou.**

```sh
bilro stats
bilro recall
```

**4. Abra o painel.**

```sh
bilro-painel
```

---

## O que ele faz

```
bilro filter <cmd>     roda o comando e devolve só o que informa
bilro read <arq>       lê arquivo comprimido (--outline só a estrutura)
bilro grep <padrão>    busca agrupada por arquivo, sem repetição
bilro exec <ling>      roda trecho de código, só o impresso volta
bilro run <cmd>        roda e indexa; só o trecho pedido volta
bilro find <termo>     busca no que já foi indexado
bilro fetch <url>      busca página, indexa, devolve só o pedido
bilro recall [termo]   o que já aconteceu neste projeto
bilro ready            já dá pra aposentar as ferramentas que ele substitui?
bilro bill             custo fixo de contexto por requisição
bilro verify [--run]   confere memórias que afirmam fato datado
bilro lint             link quebrado, memória órfã, mais citada
bilro propose          memórias que valeria escrever
bilro stats            o que ele guarda e quanto poupa
bilro doctor           diagnóstico da instalação
bilro purge <alvo>     apaga dado guardado (--sim confirma)
bilro serve [porta]    painel no navegador
bilro install          registra hooks e servidor MCP
bilro mcp              servidor MCP por stdio
```

---

## Como a compressão funciona

Duas passadas, e a ordem importa.

**Primeiro a estrutura.** Saída tem forma — tabela em colunas, diff, relatório
de teste, listagem de arquivo, lista de diagnóstico, log de instalação, despejo
de JSON — e forma pode ser comprimida num comando que nunca foi visto antes.
Sete formas cobrem a saída de dezenas de ferramentas, inclusive de ferramentas
que ninguém escreveu regra para, porque o que se repete entre elas é a forma,
não o nome do programa.

**Depois o histórico.** Toda execução é registrada: quantas vezes aquela forma
de comando já apareceu, e em quantas dessas execuções cada linha apareceu. Linha
presente em quase toda execução não carrega informação e é descartada. Linha
nunca vista é sempre mantida. Abaixo de três execuções não há histórico para
julgar, e nada é descartado.

Número e hash colapsam na hora de decidir o que é ruído — uma duração ou um
contador não faz toda execução parecer nova. Mas são preservados na hora de
decidir o que é **novo**, porque `modulo 3 falhou` e `modulo 7 falhou` são
eventos diferentes que por acaso compartilham uma forma.

---

## A regra que vence a compressão

**Linha que relata falha nunca é descartada.** Nem quando se repete, nem quando
aparece em toda execução, nem quando a saída é cortada por orçamento. Um build
que quebra do mesmo jeito todo dia continua sendo a resposta para o que
aconteceu, e ferramenta que esconde isso é pior que ferramenta nenhuma.

Severidade é reconhecida de duas maneiras: pelas palavras e marcas que a linha
usa, e pela **forma** que todo compilador e linter do mundo imprime — caminho,
linha, coluna. A segunda importa porque a primeira só conhece as linguagens que
alguém listou.

Quando nada sobrevive à compressão, o bilro diz quantas linhas suprimiu em vez
de imprimir uma tela vazia. E não afirma que nenhuma delas relatava falha:
ausência de falha não é demonstrável a partir de uma lista de palavras.

---

## Credenciais

A saída é redigida antes de ser mostrada e antes de ser gravada, porque segredo
escrito no banco uma vez sobrevive a toda execução seguinte. O que é inspecionado
é a **forma do valor**, não o nome do campo: string de conexão, parâmetro de
query, cabeçalho `Authorization`, JWT solto, flag de senha e bloco de chave
privada carregam segredo sob rótulo inocente.

O que não é segredo continua legível — o host e o banco de uma string de conexão,
o parâmetro `page` ao lado da chave de API, a palavra `Bearer` sem o token.

---

## O painel

```sh
bilro-painel     # aplicação nativa
bilro serve      # no navegador, em http://127.0.0.1:7777
```

Mostra o que a ferramenta **fez**, não o que estima: a economia vem de
reprocessar o denoise sobre a última saída de cada comando aprendido.

O grafo tem quatro modos, porque uma rede de memórias tem mais de uma pergunta:

| modo | o que responde |
|---|---|
| **Global** | a rede inteira, posicionada pela atração dos links |
| **Local** | só o que uma memória toca, na profundidade escolhida |
| **Por tipo** | cada tipo puxado para o próprio agrupamento |
| **Radial** | as mais citadas no centro, o resto em anéis |

Memória citada que nunca foi escrita vira um nó vermelho ligado por linha
tracejada — link quebrado é o tipo interessante de link.

O painel no navegador escuta **só em loopback**: ele serve um registro do teu
trabalho, e não tem por que ser alcançável de outra máquina.

---

## Como servidor MCP

`bilro install` registra o bilro como servidor MCP por stdio, então as
ferramentas aparecem na lista do próprio modelo em vez de numa documentação que
alguém precisa lembrar de ler:

`bilro_script` · `bilro_batch` · `bilro_run` · `bilro_fetch` · `bilro_recall` ·
`bilro_remember` · `bilro_find` · `bilro_filter` · `bilro_read` · `bilro_grep`

Capacidade que precisa ser lembrada é capacidade que não se usa.

---

## Memórias

Uma memória é um markdown com frontmatter, em
`~/.claude/projects/<projeto>/memory/`:

```markdown
---
name: redis-porta-local
description: backend local precisa de REDIS_PORT=6380
metadata:
  type: project
verify: git rev-parse HEAD
expect: ""
---

O compose publica 6380 e a config padrão é 6379. Relacionado a
[[deploy-gate-red-team-and-local]].
```

`bilro lint` acha link quebrado e memória órfã. `bilro propose` sugere memórias
a partir do que tu mais roda e do que mais falha. `bilro verify` confere as que
afirmam fato datado.

**Checagem declarada é restrita de propósito.** Nada roda sem `--run`, nenhum
shell é envolvido, sintaxe de shell é recusada, e só uma lista curta de programas
de leitura pode ser invocada. `git` é permitido mas precisa nomear um subcomando
de leitura, porque um alias começando com `!` roda por shell e `-c` define um
inline.

---

## Diário

O bilro registra o que foi pedido, quais comandos falharam, o que os subagentes
concluíram, e — através de `bilro_remember` — o que foi decidido, descartado ou
descoberto como restrição. As três últimas nenhum hook deduz de uma chamada de
ferramenta; são julgamentos, e são gravadas no momento em que acontecem.

```sh
bilro recall                    # linha do tempo
bilro recall "redis"            # busca por termo
```

É o que responde "onde a gente estava" quando uma sessão recomeça, em vez de
pedir para alguém repetir.

---

## Apagar

```sh
bilro purge index      # simula, não apaga
bilro purge index --sim
bilro purge all --sim
```

Sem `--sim` ele relata o que apagaria e não apaga nada.

---

## Licença

MIT.
