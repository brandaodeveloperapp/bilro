use bilro::{graph, journal, learn, ledger, ops};
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Rounding, Sense, Stroke, Vec2};
use std::path::PathBuf;

const FUNDO: Color32 = Color32::from_rgb(0x14, 0x13, 0x18);
const PAINEL: Color32 = Color32::from_rgb(0x1c, 0x1b, 0x21);
const ELEVADO: Color32 = Color32::from_rgb(0x25, 0x24, 0x2c);
const TINTA: Color32 = Color32::from_rgb(0xdd, 0xda, 0xe4);
const FRACO: Color32 = Color32::from_rgb(0x86, 0x81, 0x94);
const LINHA: Color32 = Color32::from_rgb(0x2f, 0x2d, 0x38);
const DESTAQUE: Color32 = Color32::from_rgb(0xba, 0x8b, 0xe0);
const RUIM: Color32 = Color32::from_rgb(0xe2, 0x8d, 0x7a);
const AVISO: Color32 = Color32::from_rgb(0xe0, 0xc2, 0x7c);
const BOM: Color32 = Color32::from_rgb(0x7d, 0xc6, 0x98);

#[derive(PartialEq, Clone, Copy)]
enum Modo {
    Global,
    Local,
    PorTipo,
    Radial,
}

impl Modo {
    fn nome(self) -> &'static str {
        match self {
            Modo::Global => "Global",
            Modo::Local => "Local",
            Modo::PorTipo => "Por tipo",
            Modo::Radial => "Radial",
        }
    }
    fn explicacao(self) -> &'static str {
        match self {
            Modo::Global => "toda a rede, posicionada pela atracao dos links",
            Modo::Local => "so a vizinhanca da memoria escolhida, ate a profundidade pedida",
            Modo::PorTipo => "cada tipo de memoria puxado para o seu proprio agrupamento",
            Modo::Radial => "as mais citadas no centro, as demais em aneis por distancia",
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Aba {
    Grafo,
    Atividade,
    Metricas,
}

struct No {
    id: String,
    tipo: String,
    resumo: String,
    entrando: usize,
    saindo: usize,
    fantasma: bool,
    pos: Pos2,
    vel: Vec2,
    raio: f32,
}

struct Aresta {
    de: usize,
    para: usize,
    quebrada: bool,
}

struct Painel {
    aba: Aba,
    nos: Vec<No>,
    arestas: Vec<Aresta>,
    escolhido: Option<usize>,
    sobre: Option<usize>,
    arrastando: Option<usize>,
    camera: Vec2,
    zoom: f32,
    eventos: Vec<journal::Event>,
    stats: ops::Stats,
    barulhentos: Vec<(String, i64, i64)>,
    projeto: String,
    dir_memorias: String,
    recarregar_em: f64,
    modo: Modo,
    profundidade: usize,
    repulsao: f32,
    comprimento: f32,
    visivel: Vec<bool>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

impl Painel {
    fn novo() -> Self {
        let mut p = Painel {
            aba: Aba::Grafo,
            nos: Vec::new(),
            arestas: Vec::new(),
            escolhido: None,
            sobre: None,
            arrastando: None,
            camera: Vec2::ZERO,
            zoom: 1.0,
            eventos: Vec::new(),
            stats: ops::stats(),
            barulhentos: Vec::new(),
            projeto: String::new(),
            dir_memorias: String::new(),
            recarregar_em: 0.0,
            modo: Modo::Global,
            profundidade: 1,
            repulsao: 2800.0,
            comprimento: 150.0,
            visivel: Vec::new(),
        };
        p.carregar();
        p
    }

    /// Reads everything the panel shows from the same stores the CLI uses, so
    /// the two can never disagree about what happened.
    fn carregar(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_default();
        self.projeto = journal::project_of(&cwd);
        let dir = home().join(".claude").join("projects").join(&self.projeto).join("memory");
        self.dir_memorias = dir.display().to_string();

        let g = graph::build(&dir);
        let mut nomes: Vec<String> = g.memories.iter().map(|m| m.name.clone()).collect();
        let mut fantasmas: Vec<String> = Vec::new();
        for alvos in g.out.values() {
            for alvo in alvos {
                if !g.names.contains(alvo) && !fantasmas.contains(alvo) {
                    fantasmas.push(alvo.clone());
                }
            }
        }
        nomes.extend(fantasmas.iter().cloned());

        let indice = |nome: &str| nomes.iter().position(|n| n == nome);
        let antes: Vec<(String, Pos2)> = self.nos.iter().map(|n| (n.id.clone(), n.pos)).collect();
        let total = nomes.len().max(1) as f32;

        self.nos = nomes
            .iter()
            .enumerate()
            .map(|(i, nome)| {
                let fantasma = fantasmas.contains(nome);
                let entrando = g.back.get(nome).map(|v| v.len()).unwrap_or(0);
                let saindo = g.out.get(nome).map(|v| v.len()).unwrap_or(0);
                let m = g.memories.iter().find(|m| &m.name == nome);
                let angulo = i as f32 / total * std::f32::consts::TAU;
                let pos = antes
                    .iter()
                    .find(|(id, _)| id == nome)
                    .map(|(_, p)| *p)
                    .unwrap_or(Pos2::new(angulo.cos() * 260.0, angulo.sin() * 260.0));
                No {
                    id: nome.clone(),
                    tipo: m.and_then(|m| m.kind.clone()).unwrap_or_else(|| {
                        if fantasma { "citada, nunca escrita".into() } else { "sem tipo".into() }
                    }),
                    resumo: m
                        .map(|m| {
                            m.body
                                .lines()
                                .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                                .unwrap_or("")
                                .chars()
                                .take(200)
                                .collect()
                        })
                        .unwrap_or_else(|| "esta memoria e citada mas nunca foi escrita".into()),
                    entrando,
                    saindo,
                    fantasma,
                    pos,
                    vel: Vec2::ZERO,
                    raio: 5.0 + (entrando as f32).sqrt() * 4.2,
                }
            })
            .collect();

        let mut arestas = Vec::new();
        for (de, alvos) in &g.out {
            for para in alvos {
                if let (Some(a), Some(b)) = (indice(de), indice(para)) {
                    arestas.push(Aresta { de: a, para: b, quebrada: !g.names.contains(para) });
                }
            }
        }
        self.arestas = arestas;

        self.eventos = journal::open_default()
            .ok()
            .and_then(|db| journal::timeline(&db, Some(&self.projeto), 40).ok())
            .unwrap_or_default();

        self.stats = ops::stats();
        self.barulhentos = Vec::new();
        if let Ok(db) = learn::open(&home().join(".claude").join("bilro").join("learn.db")) {
            if let Ok(mut st) = db.prepare("SELECT r.sig, r.n, l.body FROM runs r JOIN last l ON l.sig = r.sig WHERE r.n >= 3") {
                if let Ok(linhas) = st.query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
                }) {
                    for (sig, n, corpo) in linhas.flatten() {
                        let antes = corpo.chars().count() as i64;
                        let depois = learn::denoise(&db, &sig, &corpo, 3, 0.8)
                            .map(|d| d.text.chars().count() as i64)
                            .unwrap_or(antes);
                        if antes > depois {
                            self.barulhentos.push((sig, n, antes - depois));
                        }
                    }
                }
            }
        }
        self.barulhentos.sort_by_key(|b| -b.2);
        self.barulhentos.truncate(10);
    }

    /// Which nodes the current mode shows. Local mode answers the question a
    /// global view cannot: what does this one memory actually touch.
    fn recalcular_visiveis(&mut self) {
        let n = self.nos.len();
        self.visivel = vec![true; n];
        if self.modo != Modo::Local {
            return;
        }
        let Some(raiz) = self.escolhido else {
            self.visivel = vec![false; n];
            return;
        };
        let mut alcance = vec![false; n];
        alcance[raiz] = true;
        for _ in 0..self.profundidade {
            let atual = alcance.clone();
            for a in &self.arestas {
                if atual[a.de] {
                    alcance[a.para] = true;
                }
                if atual[a.para] {
                    alcance[a.de] = true;
                }
            }
        }
        self.visivel = alcance;
    }

    fn centro_do_tipo(&self, tipo: &str) -> Vec2 {
        let mut tipos: Vec<&str> = self.nos.iter().map(|n| n.tipo.as_str()).collect();
        tipos.sort_unstable();
        tipos.dedup();
        let quantos = tipos.len().max(1) as f32;
        let i = tipos.iter().position(|t| *t == tipo).unwrap_or(0) as f32;
        let ang = i / quantos * std::f32::consts::TAU;
        Vec2::new(ang.cos(), ang.sin()) * 300.0
    }

    fn simular(&mut self) {
        let n = self.nos.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let d = self.nos[j].pos - self.nos[i].pos;
                let d2 = d.length_sq().max(0.01);
                if d2 > 260_000.0 {
                    continue;
                }
                let dir = d / d2.sqrt();
                let f = dir * (self.repulsao / d2);
                self.nos[i].vel -= f;
                self.nos[j].vel += f;
            }
        }
        for a in &self.arestas {
            let d = self.nos[a.para].pos - self.nos[a.de].pos;
            let dist = d.length().max(0.01);
            let f = d / dist * ((dist - self.comprimento) * 0.012);
            self.nos[a.de].vel += f;
            self.nos[a.para].vel -= f;
        }
        for i in 0..n {
            for j in (i + 1)..n {
                let d = self.nos[j].pos - self.nos[i].pos;
                let dist = d.length().max(0.01);
                let min = self.nos[i].raio + self.nos[j].raio + 30.0;
                if dist >= min {
                    continue;
                }
                let empurra = d / dist * ((min - dist) * 0.5);
                self.nos[i].pos -= empurra;
                self.nos[j].pos += empurra;
            }
        }
        let alvos: Vec<Vec2> = match self.modo {
            Modo::PorTipo => self.nos.iter().map(|n| self.centro_do_tipo(&n.tipo)).collect(),
            Modo::Radial => self
                .nos
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let anel = match n.entrando {
                        0 => 3.0,
                        1..=2 => 2.0,
                        3..=6 => 1.0,
                        _ => 0.15,
                    };
                    let ang = i as f32 * 2.399_963;
                    Vec2::new(ang.cos(), ang.sin()) * (anel * 170.0)
                })
                .collect(),
            _ => vec![Vec2::ZERO; n],
        };

        for (i, no) in self.nos.iter_mut().enumerate() {
            no.vel += (alvos[i] - no.pos.to_vec2()) * 0.004;
            no.vel -= no.pos.to_vec2() * 0.0009;
            if Some(i) == self.arrastando {
                no.vel = Vec2::ZERO;
                continue;
            }
            no.vel *= 0.84;
            no.pos += no.vel;
        }
    }
}

fn cartao(ui: &mut egui::Ui, largura: f32, corpo: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(PAINEL)
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0_f32, LINHA))
        .inner_margin(egui::Margin::symmetric(16.0, 14.0))
        .show(ui, |ui| {
            ui.set_width(largura);
            corpo(ui);
        });
}

fn metrica(ui: &mut egui::Ui, rotulo: &str, valor: String, nota: &str, fracao: Option<f32>) {
    ui.label(egui::RichText::new(rotulo).size(11.0).color(FRACO));
    ui.add_space(1.0);
    ui.label(egui::RichText::new(valor).size(27.0).color(TINTA).strong());
    if let Some(f) = fracao {
        ui.add_space(7.0);
        let (resposta, pintor) = ui.allocate_painter(Vec2::new(ui.available_width(), 4.0), Sense::hover());
        let r = resposta.rect;
        pintor.rect_filled(r, Rounding::same(2.0), LINHA);
        pintor.rect_filled(
            Rect::from_min_size(r.min, Vec2::new(r.width() * f.clamp(0.0, 1.0), r.height())),
            Rounding::same(2.0),
            DESTAQUE,
        );
    }
    if !nota.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(nota).size(11.0).color(FRACO));
    }
}

fn bytes(n: u64) -> String {
    if n > 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n > 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{n} B")
    }
}

fn idade(at: i64) -> String {
    let min = (ledger::now_ms() - at).max(0) / 60_000;
    if min < 1 {
        "agora".into()
    } else if min < 60 {
        format!("{min}min")
    } else if min < 1440 {
        format!("{}h", min / 60)
    } else {
        format!("{}d", min / 1440)
    }
}

fn cor_do_tipo(tipo: &str) -> Color32 {
    match tipo {
        "falha" | "erro-tool" => RUIM,
        "decisao" | "descartado" | "restricao" => AVISO,
        "agente" => BOM,
        _ => FRACO,
    }
}

impl eframe::App for Painel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let agora = ctx.input(|i| i.time);
        if agora > self.recarregar_em {
            self.recarregar_em = agora + 6.0;
            self.carregar();
        }

        let mut estilo = (*ctx.style()).clone();
        estilo.visuals.panel_fill = FUNDO;
        estilo.visuals.window_fill = PAINEL;
        estilo.visuals.override_text_color = Some(TINTA);
        estilo.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINHA);
        estilo.visuals.widgets.inactive.bg_fill = ELEVADO;
        estilo.visuals.widgets.hovered.bg_fill = ELEVADO;
        estilo.visuals.widgets.active.bg_fill = ELEVADO;
        estilo.visuals.selection.bg_fill = DESTAQUE.linear_multiply(0.35);
        estilo.spacing.item_spacing = Vec2::new(8.0, 7.0);
        ctx.set_style(estilo);

        egui::SidePanel::left("lateral")
            .exact_width(228.0)
            .frame(egui::Frame::none().fill(PAINEL).inner_margin(egui::Margin::symmetric(16.0, 16.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("bilro").size(17.0).strong().color(TINTA));
                    let curto = self.projeto.rsplit('-').next().unwrap_or("").to_string();
                    ui.label(egui::RichText::new(curto).size(11.0).color(FRACO));
                });
                ui.add_space(14.0);

                for (aba, nome) in [(Aba::Grafo, "Grafo"), (Aba::Atividade, "Atividade"), (Aba::Metricas, "Metricas")] {
                    let ativa = self.aba == aba;
                    let texto = egui::RichText::new(nome).size(13.0).color(if ativa { TINTA } else { FRACO });
                    if ui.add(egui::SelectableLabel::new(ativa, texto)).clicked() {
                        self.aba = aba;
                    }
                }

                ui.add_space(18.0);
                ui.label(egui::RichText::new("MAIS CITADAS").size(10.0).color(FRACO));
                ui.add_space(6.0);
                let mut ordem: Vec<usize> = (0..self.nos.len()).filter(|i| self.nos[*i].entrando > 0).collect();
                ordem.sort_by_key(|i| std::cmp::Reverse(self.nos[*i].entrando));
                for i in ordem.into_iter().take(7) {
                    let nome = &self.nos[i].id;
                    let curto: String = if nome.chars().count() > 24 {
                        format!("{}…", nome.chars().take(23).collect::<String>())
                    } else {
                        nome.clone()
                    };
                    let linha = format!("{curto}  {}", self.nos[i].entrando);
                    if ui
                        .add(egui::Label::new(egui::RichText::new(linha).size(12.0).color(TINTA)).sense(Sense::click()))
                        .on_hover_text(nome)
                        .clicked()
                    {
                        self.escolhido = Some(i);
                        self.aba = Aba::Grafo;
                        self.camera = -self.nos[i].pos.to_vec2();
                    }
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let (r, p) = ui.allocate_painter(Vec2::splat(7.0), Sense::hover());
                        p.circle_filled(r.rect.center(), 3.0, BOM);
                        ui.label(egui::RichText::new("ao vivo").size(11.0).color(FRACO));
                    });
                    ui.add_space(8.0);
                    let cobertura = if self.stats.learned_commands > 0 {
                        self.stats.learned_mature as f32 / self.stats.learned_commands as f32
                    } else {
                        0.0
                    };
                    for (rotulo, valor) in [
                        ("links quebrados", self.arestas.iter().filter(|a| a.quebrada).count().to_string()),
                        ("memorias", self.nos.iter().filter(|n| !n.fantasma).count().to_string()),
                        ("cobertura", format!("{}%", (cobertura * 100.0).round())),
                        ("comandos", self.stats.learned_commands.to_string()),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(rotulo).size(12.0).color(FRACO));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new(valor).size(12.0).color(TINTA).strong());
                            });
                        });
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(FUNDO))
            .show(ctx, |ui| match self.aba {
                Aba::Grafo => self.desenhar_grafo(ui),
                Aba::Atividade => self.desenhar_atividade(ui),
                Aba::Metricas => self.desenhar_metricas(ui),
            });

        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

impl Painel {
    fn desenhar_grafo(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(PAINEL)
            .inner_margin(egui::Margin::symmetric(14.0, 9.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for m in [Modo::Global, Modo::Local, Modo::PorTipo, Modo::Radial] {
                        let ativo = self.modo == m;
                        let texto = egui::RichText::new(m.nome()).size(12.0)
                            .color(if ativo { TINTA } else { FRACO });
                        if ui.add(egui::SelectableLabel::new(ativo, texto)).on_hover_text(m.explicacao()).clicked() {
                            self.modo = m;
                        }
                    }
                    ui.add_space(10.0);
                    if self.modo == Modo::Local {
                        ui.label(egui::RichText::new("profundidade").size(11.0).color(FRACO));
                        ui.add(egui::Slider::new(&mut self.profundidade, 1..=4).show_value(true));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Slider::new(&mut self.comprimento, 60.0..=340.0).show_value(false))
                            .on_hover_text("comprimento do link");
                        ui.label(egui::RichText::new("link").size(11.0).color(FRACO));
                        ui.add(egui::Slider::new(&mut self.repulsao, 600.0..=9000.0).show_value(false))
                            .on_hover_text("forca de repulsao entre memorias");
                        ui.label(egui::RichText::new("repulsao").size(11.0).color(FRACO));
                    });
                });
            });
        ui.painter().line_segment(
            [ui.min_rect().left_top() + Vec2::new(0.0, 40.0), ui.min_rect().right_top() + Vec2::new(0.0, 40.0)],
            Stroke::new(1.0_f32, LINHA),
        );

        self.recalcular_visiveis();
        self.simular();
        let (resposta, pintor) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let centro = resposta.rect.center();
        let para_tela = |p: Pos2, cam: Vec2, z: f32| centro + (p.to_vec2() + cam) * z;

        if let Some(pos) = resposta.hover_pos() {
            let rolagem = ui.input(|i| i.raw_scroll_delta.y);
            if rolagem != 0.0 {
                let antes = (pos - centro) / self.zoom - self.camera;
                self.zoom = (self.zoom * (1.0 + rolagem * 0.004)).clamp(0.25, 3.0);
                self.camera = (pos - centro) / self.zoom - antes;
            }
            self.sobre = self.nos.iter().enumerate().position(|(i, n)| {
                self.visivel[i] && (para_tela(n.pos, self.camera, self.zoom) - pos).length() <= n.raio * self.zoom + 6.0
            });
        }

        if resposta.drag_started() {
            self.arrastando = self.sobre;
            if let Some(i) = self.sobre {
                self.escolhido = Some(i);
            }
        }
        if resposta.dragged() {
            match self.arrastando {
                Some(i) => self.nos[i].pos += resposta.drag_delta() / self.zoom,
                None => self.camera += resposta.drag_delta() / self.zoom,
            }
        }
        if resposta.drag_stopped() {
            self.arrastando = None;
        }
        if resposta.clicked() {
            self.escolhido = self.sobre;
        }

        if self.modo == Modo::Local && self.escolhido.is_none() {
            pintor.text(
                centro,
                Align2::CENTER_CENTER,
                "escolha uma memoria para ver a vizinhanca dela",
                FontId::proportional(14.0),
                FRACO,
            );
            pintor.text(
                centro + Vec2::new(0.0, 22.0),
                Align2::CENTER_CENTER,
                "clique em uma das mais citadas na lateral",
                FontId::proportional(12.0),
                LINHA,
            );
            return;
        }

        if self.nos.is_empty() {
            pintor.text(
                centro,
                Align2::CENTER_CENTER,
                "este projeto ainda nao tem memorias",
                FontId::proportional(14.0),
                FRACO,
            );
            pintor.text(
                centro + Vec2::new(0.0, 22.0),
                Align2::CENTER_CENTER,
                &self.dir_memorias,
                FontId::monospace(11.0),
                LINHA,
            );
            return;
        }

        let foco = self.sobre.or(self.escolhido);
        let vizinho = |i: usize| match foco {
            None => true,
            Some(f) => {
                i == f
                    || self
                        .arestas
                        .iter()
                        .any(|a| (a.de == f && a.para == i) || (a.para == f && a.de == i))
            }
        };

        for a in &self.arestas {
            if !self.visivel[a.de] || !self.visivel[a.para] {
                continue;
            }
            let p1 = para_tela(self.nos[a.de].pos, self.camera, self.zoom);
            let p2 = para_tela(self.nos[a.para].pos, self.camera, self.zoom);
            let claro = vizinho(a.de) && vizinho(a.para);
            let cor = if a.quebrada { RUIM } else { LINHA };
            let alfa = if claro { 1.0 } else { 0.18 };
            if a.quebrada {
                let passos = 14;
                for k in 0..passos {
                    if k % 2 == 1 {
                        continue;
                    }
                    let t0 = k as f32 / passos as f32;
                    let t1 = (k + 1) as f32 / passos as f32;
                    pintor.line_segment(
                        [p1.lerp(p2, t0), p1.lerp(p2, t1)],
                        Stroke::new(1.3_f32, cor.linear_multiply(alfa)),
                    );
                }
            } else {
                pintor.line_segment([p1, p2], Stroke::new(1.0_f32, cor.linear_multiply(alfa * 0.8)));
            }
        }

        for (i, no) in self.nos.iter().enumerate() {
            if !self.visivel[i] {
                continue;
            }
            let p = para_tela(no.pos, self.camera, self.zoom);
            let claro = vizinho(i);
            let alfa = if claro { 1.0 } else { 0.22 };
            let cor = if no.fantasma {
                RUIM
            } else if no.entrando == 0 && no.saindo == 0 {
                FRACO
            } else {
                DESTAQUE
            };
            let raio = no.raio * self.zoom;
            pintor.circle_filled(p, raio + 2.0, FUNDO.linear_multiply(alfa));
            pintor.circle_filled(p, raio, cor.linear_multiply(alfa));
            if Some(i) == self.escolhido {
                pintor.circle_stroke(p, raio + 3.0, Stroke::new(1.6_f32, TINTA));
            }
            let densidade_baixa = self.visivel.iter().filter(|v| **v).count() <= 24;
            let mostrar = if foco.is_some() || densidade_baixa { claro } else { no.entrando >= 3 };
            if mostrar && self.zoom > 0.5 {
                let rotulo: String = if no.id.chars().count() > 28 {
                    format!("{}…", no.id.chars().take(27).collect::<String>())
                } else {
                    no.id.clone()
                };
                let em = FontId::proportional(11.0);
                let onde = p + Vec2::new(0.0, raio + 12.0);
                let caixa = pintor.layout_no_wrap(rotulo.clone(), em.clone(), TINTA);
                pintor.rect_filled(
                    Rect::from_center_size(onde, caixa.size() + Vec2::new(6.0, 2.0)),
                    Rounding::same(3.0),
                    FUNDO.linear_multiply(0.85 * alfa),
                );
                pintor.text(onde, Align2::CENTER_CENTER, rotulo, em, TINTA.linear_multiply(alfa));
            }
        }

        if let Some(i) = self.escolhido {
            let largura = 320.0;
            let area = Rect::from_min_size(
                Pos2::new(resposta.rect.max.x - largura, resposta.rect.min.y),
                Vec2::new(largura, resposta.rect.height()),
            );
            pintor.rect_filled(area, Rounding::ZERO, PAINEL);
            pintor.line_segment([area.left_top(), area.left_bottom()], Stroke::new(1.0_f32, LINHA));
            let mut filho = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(18.0)).layout(egui::Layout::top_down(egui::Align::Min)));
            let no = &self.nos[i];
            filho.label(egui::RichText::new(&no.id).size(15.0).strong().color(TINTA));
            filho.label(egui::RichText::new(&no.tipo).size(11.0).color(FRACO).monospace());
            filho.add_space(10.0);
            filho.horizontal_wrapped(|ui| {
                for selo in [
                    format!("{} citacoes", no.entrando),
                    format!("{} links", no.saindo),
                ] {
                    egui::Frame::none()
                        .fill(ELEVADO)
                        .rounding(Rounding::same(10.0))
                        .inner_margin(egui::Margin::symmetric(8.0, 2.0))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(selo).size(11.0).color(FRACO));
                        });
                }
            });
            filho.add_space(12.0);
            filho.label(egui::RichText::new(&no.resumo).size(12.5).color(FRACO));
            filho.add_space(16.0);
            if filho.add(egui::Button::new(egui::RichText::new("fechar").size(11.0).color(FRACO))).clicked() {
                self.escolhido = None;
            }
        }
    }

    fn desenhar_atividade(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("O QUE PASSOU POR AQUI").size(11.0).color(FRACO));
            ui.add_space(10.0);
            if self.eventos.is_empty() {
                ui.label(egui::RichText::new("nada registrado neste projeto ainda").size(13.0).color(FRACO));
                return;
            }
            let largura = ui.available_width() - 24.0;
            for e in &self.eventos {
                cartao(ui, largura, |ui| {
                    ui.horizontal(|ui| {
                        egui::Frame::none()
                            .stroke(Stroke::new(1.0_f32, cor_do_tipo(&e.kind)))
                            .rounding(Rounding::same(10.0))
                            .inner_margin(egui::Margin::symmetric(7.0, 1.0))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(&e.kind).size(10.0).color(cor_do_tipo(&e.kind)));
                            });
                        ui.label(egui::RichText::new(&e.subject).size(13.0).color(TINTA));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(idade(e.at)).size(11.0).color(FRACO));
                        });
                    });
                    if !e.body.trim().is_empty() {
                        ui.add_space(3.0);
                        let corpo: String = e.body.lines().take(2).collect::<Vec<_>>().join(" ");
                        ui.label(egui::RichText::new(corpo).size(11.5).color(FRACO));
                    }
                });
                ui.add_space(6.0);
            }
        });
    }

    fn desenhar_metricas(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(20.0);
            let s = &self.stats;
            let cobertura = if s.learned_commands > 0 {
                s.learned_mature as f32 / s.learned_commands as f32
            } else {
                0.0
            };
            let disco = s.learn_db_bytes + s.index_db_bytes + s.journal_db_bytes + s.sessions_bytes;
            let largura = (ui.available_width() - 60.0) / 3.0;

            ui.horizontal(|ui| {
                cartao(ui, largura, |ui| {
                    metrica(ui, "cobertura", format!("{}%", (cobertura * 100.0).round()),
                        &format!("{} de {} com 3+ execucoes", s.learned_mature, s.learned_commands), Some(cobertura));
                });
                cartao(ui, largura, |ui| {
                    metrica(ui, "cortaria hoje", bytes(s.denoise_savings_bytes),
                        &format!("{} trechos indexados", s.indexed_chunks), None);
                });
                cartao(ui, largura, |ui| {
                    metrica(ui, "em disco", bytes(disco), &format!("{} sessoes", s.sessions), None);
                });
            });

            ui.add_space(20.0);
            ui.label(egui::RichText::new("O QUE MAIS SE REPETE").size(11.0).color(FRACO));
            ui.add_space(10.0);
            let cheia = ui.available_width() - 24.0;
            if self.barulhentos.is_empty() {
                ui.label(
                    egui::RichText::new("nenhum comando repetiu o bastante. O denoise precisa de tres execucoes antes de julgar.")
                        .size(13.0).color(FRACO),
                );
                return;
            }
            for (sig, n, cortado) in &self.barulhentos {
                cartao(ui, cheia, |ui| {
                    ui.horizontal(|ui| {
                        let curto: String = sig.chars().take(76).collect();
                        ui.label(egui::RichText::new(curto).size(11.5).color(TINTA).monospace());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{n}x · {}", bytes(*cortado as u64))).size(11.0).color(FRACO));
                        });
                    });
                });
                ui.add_space(5.0);
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let opcoes = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("bilro"),
        ..Default::default()
    };
    eframe::run_native("bilro", opcoes, Box::new(|_cc| Ok(Box::new(Painel::novo()))))
}
