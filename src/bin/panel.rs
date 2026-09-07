use bilro::{graph, journal, learn, ledger, ops};
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Rounding, Sense, Stroke, Vec2};
use std::path::PathBuf;

const BACKGROUND: Color32 = Color32::from_rgb(0x14, 0x13, 0x18);
const PANEL: Color32 = Color32::from_rgb(0x1c, 0x1b, 0x21);
const RAISED: Color32 = Color32::from_rgb(0x25, 0x24, 0x2c);
const INK: Color32 = Color32::from_rgb(0xdd, 0xda, 0xe4);
const MUTED: Color32 = Color32::from_rgb(0x86, 0x81, 0x94);
const LINE: Color32 = Color32::from_rgb(0x2f, 0x2d, 0x38);
const ACCENT: Color32 = Color32::from_rgb(0xba, 0x8b, 0xe0);
const BAD: Color32 = Color32::from_rgb(0xe2, 0x8d, 0x7a);
const WARN: Color32 = Color32::from_rgb(0xe0, 0xc2, 0x7c);
const GOOD: Color32 = Color32::from_rgb(0x7d, 0xc6, 0x98);

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Global,
    Local,
    ByType,
    Radial,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Global => "Global",
            Mode::Local => "Local",
            Mode::ByType => "By type",
            Mode::Radial => "Radial",
        }
    }
    fn explanation(self) -> &'static str {
        match self {
            Mode::Global => "the whole network, laid out by the pull of its links",
            Mode::Local => "just the neighbourhood of the chosen memory, out to the requested depth",
            Mode::ByType => "each memory type pulled toward its own cluster",
            Mode::Radial => "most-cited at the centre, the rest in rings by how often they are cited",
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Graph,
    Activity,
    Metrics,
}

struct Node {
    id: String,
    kind: String,
    summary: String,
    incoming: usize,
    outgoing: usize,
    ghost: bool,
    pos: Pos2,
    vel: Vec2,
    radius: f32,
}

struct Edge {
    from: usize,
    to: usize,
    broken: bool,
}

struct Panel {
    tab: Tab,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    selected: Option<usize>,
    hovered: Option<usize>,
    dragging: Option<usize>,
    camera: Vec2,
    zoom: f32,
    events: Vec<journal::Event>,
    stats: ops::Stats,
    noisy: Vec<(String, i64, i64)>,
    project: String,
    memories_dir: String,
    reload_at: f64,
    projects: Vec<(String, u64)>,
    mode: Mode,
    depth: usize,
    repulsion: f32,
    length: f32,
    visible: Vec<bool>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

impl Panel {
    fn new() -> Self {
        let mut p = Panel {
            tab: Tab::Graph,
            nodes: Vec::new(),
            edges: Vec::new(),
            selected: None,
            hovered: None,
            dragging: None,
            camera: Vec2::ZERO,
            zoom: 1.0,
            events: Vec::new(),
            stats: ops::stats(),
            noisy: Vec::new(),
            project: String::new(),
            memories_dir: String::new(),
            reload_at: 0.0,
            projects: Vec::new(),
            mode: Mode::Global,
            depth: 1,
            repulsion: 2800.0,
            length: 150.0,
            visible: Vec::new(),
        };
        p.load();
        p
    }

    /// Every project that has memories, the fullest first. A window opened from
    /// the dock has no meaningful working directory, so the panel chooses rather
    /// than inheriting one — and it chooses by weight, because the most recently
    /// touched directory is as often leftover scratch as it is real work.
    fn discover_projects(&mut self) {
        let root = home().join(".claude").join("projects");
        let mut found: Vec<(String, u64)> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let memory = e.path().join("memory");
                if !memory.is_dir() {
                    return None;
                }
                let files: Vec<_> = std::fs::read_dir(&memory)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|f| f.file_name().to_string_lossy().ends_with(".md"))
                    .collect();
                if files.is_empty() {
                    return None;
                }
                let when = files
                    .iter()
                    .filter_map(|f| f.metadata().ok()?.modified().ok())
                    .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .max()
                    .unwrap_or(0);
                let _ = when;
                Some((e.file_name().to_string_lossy().to_string(), files.len() as u64))
            })
            .collect();
        found.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
        self.projects = found;
    }

    fn short_name(slug: &str) -> String {
        slug.rsplit('-').next().unwrap_or(slug).to_string()
    }

    /// Reads everything the panel shows from the same stores the CLI uses, so
    /// the two can never disagree about what happened.
    fn load(&mut self) {
        self.discover_projects();
        if self.project.is_empty() || self.project == "-" {
            let cwd = std::env::current_dir().unwrap_or_default();
            let of_cwd = journal::project_of(&cwd);
            self.project = if self.projects.iter().any(|(p, _)| *p == of_cwd) {
                of_cwd
            } else {
                self.projects.first().map(|(p, _)| p.clone()).unwrap_or(of_cwd)
            };
        }
        let dir = home().join(".claude").join("projects").join(&self.project).join("memory");
        self.memories_dir = dir.display().to_string();

        let g = graph::build(&dir);
        let mut names: Vec<String> = g.memories.iter().map(|m| m.name.clone()).collect();
        let mut ghosts: Vec<String> = Vec::new();
        for targets in g.out.values() {
            for target in targets {
                if !g.names.contains(target) && !ghosts.contains(target) {
                    ghosts.push(target.clone());
                }
            }
        }
        names.extend(ghosts.iter().cloned());

        let index = |name: &str| names.iter().position(|n| n == name);
        let before: Vec<(String, Pos2)> = self.nodes.iter().map(|n| (n.id.clone(), n.pos)).collect();
        let total = names.len().max(1) as f32;

        self.nodes = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let ghost = ghosts.contains(name);
                let incoming = g.back.get(name).map(|v| v.len()).unwrap_or(0);
                let outgoing = g.out.get(name).map(|v| v.len()).unwrap_or(0);
                let m = g.memories.iter().find(|m| &m.name == name);
                let angle = i as f32 / total * std::f32::consts::TAU;
                let pos = before
                    .iter()
                    .find(|(id, _)| id == name)
                    .map(|(_, p)| *p)
                    .unwrap_or(Pos2::new(angle.cos() * 260.0, angle.sin() * 260.0));
                Node {
                    id: name.clone(),
                    kind: m.and_then(|m| m.kind.clone()).unwrap_or_else(|| {
                        if ghost { "cited, never written".into() } else { "no type".into() }
                    }),
                    summary: m
                        .map(|m| {
                            m.body
                                .lines()
                                .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                                .unwrap_or("")
                                .chars()
                                .take(200)
                                .collect()
                        })
                        .unwrap_or_else(|| "this memory is cited but was never written".into()),
                    incoming,
                    outgoing,
                    ghost,
                    pos,
                    vel: Vec2::ZERO,
                    radius: 5.0 + (incoming as f32).sqrt() * 4.2,
                }
            })
            .collect();

        let mut edges = Vec::new();
        for (from, targets) in &g.out {
            for to in targets {
                if let (Some(a), Some(b)) = (index(from), index(to)) {
                    edges.push(Edge { from: a, to: b, broken: !g.names.contains(to) });
                }
            }
        }
        self.edges = edges;

        self.events = journal::open_default()
            .ok()
            .and_then(|db| journal::timeline(&db, Some(&self.project), 40).ok())
            .unwrap_or_default();

        self.stats = ops::stats();
        self.noisy = Vec::new();
        if let Ok(db) = learn::open(&home().join(".claude").join("bilro").join("learn.db")) {
            if let Ok(mut st) = db.prepare("SELECT r.sig, r.n, l.body FROM runs r JOIN last l ON l.sig = r.sig WHERE r.n >= 3") {
                if let Ok(rows) = st.query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
                }) {
                    for (sig, n, body) in rows.flatten() {
                        let before = body.chars().count() as i64;
                        let after = learn::denoise(&db, &sig, &body, 3, 0.8)
                            .map(|d| d.text.chars().count() as i64)
                            .unwrap_or(before);
                        if before > after {
                            self.noisy.push((sig, n, before - after));
                        }
                    }
                }
            }
        }
        self.noisy.sort_by_key(|b| -b.2);
        self.noisy.truncate(10);
    }

    /// Which nodes the current mode shows. Local mode answers the question a
    /// global view cannot: what does this one memory actually touch.
    fn recompute_visible(&mut self) {
        let n = self.nodes.len();
        self.visible = vec![true; n];
        if self.mode != Mode::Local {
            return;
        }
        let Some(root) = self.selected else {
            self.visible = vec![false; n];
            return;
        };
        let mut reach = vec![false; n];
        reach[root] = true;
        for _ in 0..self.depth {
            let current = reach.clone();
            for a in &self.edges {
                if current[a.from] {
                    reach[a.to] = true;
                }
                if current[a.to] {
                    reach[a.from] = true;
                }
            }
        }
        self.visible = reach;
    }

    fn center_of_type(&self, kind: &str) -> Vec2 {
        let mut kinds: Vec<&str> = self.nodes.iter().map(|n| n.kind.as_str()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        let count = kinds.len().max(1) as f32;
        let i = kinds.iter().position(|t| *t == kind).unwrap_or(0) as f32;
        let ang = i / count * std::f32::consts::TAU;
        Vec2::new(ang.cos(), ang.sin()) * 300.0
    }

    fn simulate(&mut self) {
        let n = self.nodes.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let d = self.nodes[j].pos - self.nodes[i].pos;
                let d2 = d.length_sq().max(0.01);
                if d2 > 260_000.0 {
                    continue;
                }
                let dir = d / d2.sqrt();
                let f = dir * (self.repulsion / d2);
                self.nodes[i].vel -= f;
                self.nodes[j].vel += f;
            }
        }
        for a in &self.edges {
            let d = self.nodes[a.to].pos - self.nodes[a.from].pos;
            let dist = d.length().max(0.01);
            let f = d / dist * ((dist - self.length) * 0.012);
            self.nodes[a.from].vel += f;
            self.nodes[a.to].vel -= f;
        }
        for i in 0..n {
            for j in (i + 1)..n {
                let d = self.nodes[j].pos - self.nodes[i].pos;
                let dist = d.length().max(0.01);
                let min = self.nodes[i].radius + self.nodes[j].radius + 30.0;
                if dist >= min {
                    continue;
                }
                let push = d / dist * ((min - dist) * 0.5);
                self.nodes[i].pos -= push;
                self.nodes[j].pos += push;
            }
        }
        let targets: Vec<Vec2> = match self.mode {
            Mode::ByType => self.nodes.iter().map(|n| self.center_of_type(&n.kind)).collect(),
            Mode::Radial => self
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let ring = match n.incoming {
                        0 => 3.0,
                        1..=2 => 2.0,
                        3..=6 => 1.0,
                        _ => 0.15,
                    };
                    let ang = i as f32 * 2.399_963;
                    Vec2::new(ang.cos(), ang.sin()) * (ring * 170.0)
                })
                .collect(),
            _ => vec![Vec2::ZERO; n],
        };

        for (i, node) in self.nodes.iter_mut().enumerate() {
            node.vel += (targets[i] - node.pos.to_vec2()) * 0.004;
            node.vel -= node.pos.to_vec2() * 0.0009;
            if Some(i) == self.dragging {
                node.vel = Vec2::ZERO;
                continue;
            }
            node.vel *= 0.84;
            node.pos += node.vel;
        }
    }
}

fn card(ui: &mut egui::Ui, width: f32, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(PANEL)
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0_f32, LINE))
        .inner_margin(egui::Margin::symmetric(16.0, 14.0))
        .show(ui, |ui| {
            ui.set_width(width);
            body(ui);
        });
}

fn metric(ui: &mut egui::Ui, label: &str, value: String, note: &str, fraction: Option<f32>) {
    ui.label(egui::RichText::new(label).size(11.0).color(MUTED));
    ui.add_space(1.0);
    ui.label(egui::RichText::new(value).size(27.0).color(INK).strong());
    if let Some(f) = fraction {
        ui.add_space(7.0);
        let (response, painter) = ui.allocate_painter(Vec2::new(ui.available_width(), 4.0), Sense::hover());
        let r = response.rect;
        painter.rect_filled(r, Rounding::same(2.0), LINE);
        painter.rect_filled(
            Rect::from_min_size(r.min, Vec2::new(r.width() * f.clamp(0.0, 1.0), r.height())),
            Rounding::same(2.0),
            ACCENT,
        );
    }
    if !note.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(note).size(11.0).color(MUTED));
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

fn age(at: i64) -> String {
    let min = (ledger::now_ms() - at).max(0) / 60_000;
    if min < 1 {
        "now".into()
    } else if min < 60 {
        format!("{min}min")
    } else if min < 1440 {
        format!("{}h", min / 60)
    } else {
        format!("{}d", min / 1440)
    }
}

fn color_of_kind(kind: &str) -> Color32 {
    match kind {
        "falha" | "failure" | "erro-tool" | "tool-error" => BAD,
        "decisao" | "decision" | "descartado" | "rejected" | "restricao" | "constraint" => WARN,
        "agente" | "agent" => GOOD,
        _ => MUTED,
    }
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        if now > self.reload_at {
            self.reload_at = now + 6.0;
            self.load();
        }

        let mut style = (*ctx.style()).clone();
        style.visuals.panel_fill = BACKGROUND;
        style.visuals.window_fill = PANEL;
        style.visuals.override_text_color = Some(INK);
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
        style.visuals.widgets.inactive.bg_fill = RAISED;
        style.visuals.widgets.hovered.bg_fill = RAISED;
        style.visuals.widgets.active.bg_fill = RAISED;
        style.visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
        style.spacing.item_spacing = Vec2::new(8.0, 7.0);
        ctx.set_style(style);

        egui::SidePanel::left("sidebar")
            .exact_width(228.0)
            .frame(egui::Frame::none().fill(PANEL).inner_margin(egui::Margin::symmetric(16.0, 16.0)))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("bilro").size(17.0).strong().color(INK));
                ui.add_space(8.0);
                let current = Self::short_name(&self.project);
                let mut switched = None;
                egui::ComboBox::from_id_salt("project")
                    .selected_text(egui::RichText::new(current).size(12.0).color(INK))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (slug, _) in &self.projects {
                            let marked = *slug == self.project;
                            if ui
                                .selectable_label(marked, egui::RichText::new(Self::short_name(slug)).size(12.0))
                                .on_hover_text(slug)
                                .clicked()
                            {
                                switched = Some(slug.clone());
                            }
                        }
                    });
                if let Some(new_project) = switched {
                    self.project = new_project;
                    self.selected = None;
                    self.nodes.clear();
                    self.reload_at = 0.0;
                }
                ui.add_space(14.0);

                for (tab, name) in [(Tab::Graph, "Graph"), (Tab::Activity, "Activity"), (Tab::Metrics, "Metrics")] {
                    let active = self.tab == tab;
                    let text = egui::RichText::new(name).size(13.0).color(if active { INK } else { MUTED });
                    if ui.add(egui::SelectableLabel::new(active, text)).clicked() {
                        self.tab = tab;
                    }
                }

                ui.add_space(18.0);
                ui.label(egui::RichText::new("MOST CITED").size(10.0).color(MUTED));
                ui.add_space(6.0);
                let mut order: Vec<usize> = (0..self.nodes.len()).filter(|i| self.nodes[*i].incoming > 0).collect();
                order.sort_by_key(|i| std::cmp::Reverse(self.nodes[*i].incoming));
                for i in order.into_iter().take(7) {
                    let name = &self.nodes[i].id;
                    let short: String = if name.chars().count() > 24 {
                        format!("{}…", name.chars().take(23).collect::<String>())
                    } else {
                        name.clone()
                    };
                    let line = format!("{short}  {}", self.nodes[i].incoming);
                    if ui
                        .add(egui::Label::new(egui::RichText::new(line).size(12.0).color(INK)).sense(Sense::click()))
                        .on_hover_text(name)
                        .clicked()
                    {
                        self.selected = Some(i);
                        self.tab = Tab::Graph;
                        self.camera = -self.nodes[i].pos.to_vec2();
                    }
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let (r, p) = ui.allocate_painter(Vec2::splat(7.0), Sense::hover());
                        p.circle_filled(r.rect.center(), 3.0, GOOD);
                        ui.label(egui::RichText::new("live").size(11.0).color(MUTED));
                    });
                    ui.add_space(8.0);
                    let coverage = if self.stats.learned_commands > 0 {
                        self.stats.learned_mature as f32 / self.stats.learned_commands as f32
                    } else {
                        0.0
                    };
                    for (label, value) in [
                        ("broken links", self.edges.iter().filter(|a| a.broken).count().to_string()),
                        ("memories", self.nodes.iter().filter(|n| !n.ghost).count().to_string()),
                        ("coverage", format!("{}%", (coverage * 100.0).round())),
                        ("commands", self.stats.learned_commands.to_string()),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(label).size(12.0).color(MUTED));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new(value).size(12.0).color(INK).strong());
                            });
                        });
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BACKGROUND))
            .show(ctx, |ui| match self.tab {
                Tab::Graph => self.draw_graph(ui),
                Tab::Activity => self.draw_activity(ui),
                Tab::Metrics => self.draw_metrics(ui),
            });

        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

impl Panel {
    fn draw_graph(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(PANEL)
            .inner_margin(egui::Margin::symmetric(14.0, 9.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for m in [Mode::Global, Mode::Local, Mode::ByType, Mode::Radial] {
                        let active = self.mode == m;
                        let text = egui::RichText::new(m.name()).size(12.0)
                            .color(if active { INK } else { MUTED });
                        if ui.add(egui::SelectableLabel::new(active, text)).on_hover_text(m.explanation()).clicked() {
                            self.mode = m;
                        }
                    }
                    ui.add_space(10.0);
                    if self.mode == Mode::Local {
                        ui.label(egui::RichText::new("depth").size(11.0).color(MUTED));
                        ui.add(egui::Slider::new(&mut self.depth, 1..=4).show_value(true));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Slider::new(&mut self.length, 60.0..=340.0).show_value(false))
                            .on_hover_text("link length");
                        ui.label(egui::RichText::new("link").size(11.0).color(MUTED));
                        ui.add(egui::Slider::new(&mut self.repulsion, 600.0..=9000.0).show_value(false))
                            .on_hover_text("repulsion strength between memories");
                        ui.label(egui::RichText::new("repulsion").size(11.0).color(MUTED));
                    });
                });
            });
        ui.painter().line_segment(
            [ui.min_rect().left_top() + Vec2::new(0.0, 40.0), ui.min_rect().right_top() + Vec2::new(0.0, 40.0)],
            Stroke::new(1.0_f32, LINE),
        );

        self.recompute_visible();
        self.simulate();
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let center = response.rect.center();
        let to_screen = |p: Pos2, cam: Vec2, z: f32| center + (p.to_vec2() + cam) * z;

        if let Some(pos) = response.hover_pos() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                let before = (pos - center) / self.zoom - self.camera;
                self.zoom = (self.zoom * (1.0 + scroll * 0.004)).clamp(0.25, 3.0);
                self.camera = (pos - center) / self.zoom - before;
            }
            self.hovered = self.nodes.iter().enumerate().position(|(i, n)| {
                self.visible[i] && (to_screen(n.pos, self.camera, self.zoom) - pos).length() <= n.radius * self.zoom + 6.0
            });
        }

        if response.drag_started() {
            self.dragging = self.hovered;
            if let Some(i) = self.hovered {
                self.selected = Some(i);
            }
        }
        if response.dragged() {
            match self.dragging {
                Some(i) => self.nodes[i].pos += response.drag_delta() / self.zoom,
                None => self.camera += response.drag_delta() / self.zoom,
            }
        }
        if response.drag_stopped() {
            self.dragging = None;
        }
        if response.clicked() {
            self.selected = self.hovered;
        }

        if self.mode == Mode::Local && self.selected.is_none() {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                "pick a memory to see what it touches",
                FontId::proportional(14.0),
                MUTED,
            );
            painter.text(
                center + Vec2::new(0.0, 22.0),
                Align2::CENTER_CENTER,
                "click one of the most-cited ones in the sidebar",
                FontId::proportional(12.0),
                LINE,
            );
            return;
        }

        if self.nodes.is_empty() {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                "this project has no memories yet",
                FontId::proportional(14.0),
                MUTED,
            );
            painter.text(
                center + Vec2::new(0.0, 22.0),
                Align2::CENTER_CENTER,
                &self.memories_dir,
                FontId::monospace(11.0),
                LINE,
            );
            return;
        }

        let focus = self.hovered.or(self.selected);
        let neighbor = |i: usize| match focus {
            None => true,
            Some(f) => {
                i == f
                    || self
                        .edges
                        .iter()
                        .any(|a| (a.from == f && a.to == i) || (a.to == f && a.from == i))
            }
        };

        for a in &self.edges {
            if !self.visible[a.from] || !self.visible[a.to] {
                continue;
            }
            let p1 = to_screen(self.nodes[a.from].pos, self.camera, self.zoom);
            let p2 = to_screen(self.nodes[a.to].pos, self.camera, self.zoom);
            let sharp = neighbor(a.from) && neighbor(a.to);
            let color = if a.broken { BAD } else { LINE };
            let alpha = if sharp { 1.0 } else { 0.18 };
            if a.broken {
                let steps = 14;
                for k in 0..steps {
                    if k % 2 == 1 {
                        continue;
                    }
                    let t0 = k as f32 / steps as f32;
                    let t1 = (k + 1) as f32 / steps as f32;
                    painter.line_segment(
                        [p1.lerp(p2, t0), p1.lerp(p2, t1)],
                        Stroke::new(1.3_f32, color.linear_multiply(alpha)),
                    );
                }
            } else {
                painter.line_segment([p1, p2], Stroke::new(1.0_f32, color.linear_multiply(alpha * 0.8)));
            }
        }

        for (i, node) in self.nodes.iter().enumerate() {
            if !self.visible[i] {
                continue;
            }
            let p = to_screen(node.pos, self.camera, self.zoom);
            let sharp = neighbor(i);
            let alpha = if sharp { 1.0 } else { 0.22 };
            let color = if node.ghost {
                BAD
            } else if node.incoming == 0 && node.outgoing == 0 {
                MUTED
            } else {
                ACCENT
            };
            let radius = node.radius * self.zoom;
            painter.circle_filled(p, radius + 2.0, BACKGROUND.linear_multiply(alpha));
            painter.circle_filled(p, radius, color.linear_multiply(alpha));
            if Some(i) == self.selected {
                painter.circle_stroke(p, radius + 3.0, Stroke::new(1.6_f32, INK));
            }
            let low_density = self.visible.iter().filter(|v| **v).count() <= 24;
            let show = if focus.is_some() || low_density { sharp } else { node.incoming >= 3 };
            if show && self.zoom > 0.5 {
                let label: String = if node.id.chars().count() > 28 {
                    format!("{}…", node.id.chars().take(27).collect::<String>())
                } else {
                    node.id.clone()
                };
                let font = FontId::proportional(11.0);
                let where_ = p + Vec2::new(0.0, radius + 12.0);
                let box_ = painter.layout_no_wrap(label.clone(), font.clone(), INK);
                painter.rect_filled(
                    Rect::from_center_size(where_, box_.size() + Vec2::new(6.0, 2.0)),
                    Rounding::same(3.0),
                    BACKGROUND.linear_multiply(0.85 * alpha),
                );
                painter.text(where_, Align2::CENTER_CENTER, label, font, INK.linear_multiply(alpha));
            }
        }

        if let Some(i) = self.selected {
            let width = 320.0;
            let area = Rect::from_min_size(
                Pos2::new(response.rect.max.x - width, response.rect.min.y),
                Vec2::new(width, response.rect.height()),
            );
            painter.rect_filled(area, Rounding::ZERO, PANEL);
            painter.line_segment([area.left_top(), area.left_bottom()], Stroke::new(1.0_f32, LINE));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(18.0)).layout(egui::Layout::top_down(egui::Align::Min)));
            let node = &self.nodes[i];
            child.label(egui::RichText::new(&node.id).size(15.0).strong().color(INK));
            child.label(egui::RichText::new(&node.kind).size(11.0).color(MUTED).monospace());
            child.add_space(10.0);
            child.horizontal_wrapped(|ui| {
                for badge in [
                    format!("{} citations", node.incoming),
                    format!("{} links", node.outgoing),
                ] {
                    egui::Frame::none()
                        .fill(RAISED)
                        .rounding(Rounding::same(10.0))
                        .inner_margin(egui::Margin::symmetric(8.0, 2.0))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(badge).size(11.0).color(MUTED));
                        });
                }
            });
            child.add_space(12.0);
            child.label(egui::RichText::new(&node.summary).size(12.5).color(MUTED));
            child.add_space(16.0);
            if child.add(egui::Button::new(egui::RichText::new("close").size(11.0).color(MUTED))).clicked() {
                self.selected = None;
            }
        }
    }

    fn draw_activity(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("WHAT HAPPENED HERE").size(11.0).color(MUTED));
            ui.add_space(10.0);
            if self.events.is_empty() {
                ui.label(egui::RichText::new("nothing recorded for this project yet").size(13.0).color(MUTED));
                return;
            }
            let width = ui.available_width() - 24.0;
            for e in &self.events {
                card(ui, width, |ui| {
                    ui.horizontal(|ui| {
                        egui::Frame::none()
                            .stroke(Stroke::new(1.0_f32, color_of_kind(&e.kind)))
                            .rounding(Rounding::same(10.0))
                            .inner_margin(egui::Margin::symmetric(7.0, 1.0))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(&e.kind).size(10.0).color(color_of_kind(&e.kind)));
                            });
                        ui.label(egui::RichText::new(&e.subject).size(13.0).color(INK));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(age(e.at)).size(11.0).color(MUTED));
                        });
                    });
                    if !e.body.trim().is_empty() {
                        ui.add_space(3.0);
                        let body: String = e.body.lines().take(2).collect::<Vec<_>>().join(" ");
                        ui.label(egui::RichText::new(body).size(11.5).color(MUTED));
                    }
                });
                ui.add_space(6.0);
            }
        });
    }

    fn draw_metrics(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(20.0);
            let s = &self.stats;
            let coverage = if s.learned_commands > 0 {
                s.learned_mature as f32 / s.learned_commands as f32
            } else {
                0.0
            };
            let disk = s.learn_db_bytes + s.index_db_bytes + s.journal_db_bytes + s.sessions_bytes;
            let width = (ui.available_width() - 60.0) / 3.0;

            ui.horizontal(|ui| {
                card(ui, width, |ui| {
                    metric(ui, "coverage", format!("{}%", (coverage * 100.0).round()),
                        &format!("{} of {} with 3+ runs", s.learned_mature, s.learned_commands), Some(coverage));
                });
                card(ui, width, |ui| {
                    metric(ui, "would trim today", bytes(s.denoise_savings_bytes),
                        &format!("{} indexed chunks", s.indexed_chunks), None);
                });
                card(ui, width, |ui| {
                    metric(ui, "on disk", bytes(disk), &format!("{} sessions", s.sessions), None);
                });
            });

            ui.add_space(20.0);
            ui.label(egui::RichText::new("WHAT REPEATS MOST").size(11.0).color(MUTED));
            ui.add_space(10.0);
            let full = ui.available_width() - 24.0;
            if self.noisy.is_empty() {
                ui.label(
                    egui::RichText::new("no command has repeated enough yet. Denoise needs three runs before it may judge.")
                        .size(13.0).color(MUTED),
                );
                return;
            }
            for (sig, n, trimmed) in &self.noisy {
                card(ui, full, |ui| {
                    ui.horizontal(|ui| {
                        let short: String = sig.chars().take(76).collect();
                        ui.label(egui::RichText::new(short).size(11.5).color(INK).monospace());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{n}x · {}", bytes(*trimmed as u64))).size(11.0).color(MUTED));
                        });
                    });
                });
                ui.add_space(5.0);
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("bilro"),
        ..Default::default()
    };
    eframe::run_native("bilro", options, Box::new(|_cc| Ok(Box::new(Panel::new()))))
}
