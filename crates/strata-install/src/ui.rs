//! L'écran plein terminal de l'installateur (ratatui), aux couleurs de Strata, piloté au clavier :
//! accueil, système, nom, applis, matériel, réglages, compte administrateur, récapitulatif,
//! installation, fin. Sur un serveur déjà installé : mettre à jour, ajouter des applis, installer
//! DeckLink / NDI, réparer, désinstaller.
use std::{
    path::PathBuf,
    sync::mpsc::{channel, Receiver, Sender},
    time::{Duration, Instant},
};

use crossterm::event::{self, Event as Ev, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    DefaultTerminal, Frame,
};

use crate::{
    checks::{self, Check, Level},
    ndi, paths,
    steps::{self, Action, Event, Plan},
    Args,
};

// Les couleurs de Strata, sur un fond noir posé par l'installateur (lisibles quel que soit le
// thème du terminal, clair ou sombre).
const BG: Color = Color::Rgb(0, 0, 0);
const FG: Color = Color::Rgb(236, 238, 243);
const ACCENT: Color = Color::Rgb(143, 163, 255);
const OK: Color = Color::Rgb(34, 197, 94);
const WARN: Color = Color::Rgb(245, 165, 36);
const BAD: Color = Color::Rgb(255, 59, 92);
const MUTED: Color = Color::Rgb(140, 146, 160);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Welcome,
    Menu,
    Checks,
    Name,
    Apps,
    Hardware,
    License,
    Settings,
    Admin,
    Summary,
    Progress,
    End,
}

enum Msg {
    Checks(Vec<Check>),
    Dns(Check),
    Ndi(Result<ndi::Sdk, String>),
    Step(Event),
}

#[derive(Clone)]
enum StepState {
    Waiting,
    Running(String),
    Done(String),
    Failed(String),
}

/// Un champ de saisie d'une ligne.
#[derive(Default, Clone)]
struct Input {
    value: String,
    secret: bool,
}

impl Input {
    fn key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => self.value.push(c),
            KeyCode::Backspace => {
                self.value.pop();
            }
            _ => {}
        }
    }
    fn shown(&self) -> String {
        if self.secret {
            "•".repeat(self.value.chars().count())
        } else {
            self.value.clone()
        }
    }
}

struct App {
    args: Args,
    screen: Screen,
    history: Vec<Screen>,
    plan: Plan,
    installed: Option<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    checks: Option<Vec<Check>>,
    // Nom
    name: Input,
    dns: Input,
    tz: Input,
    focus: usize,
    dns_check: Option<Check>,
    // Applis
    app_input: Input,
    // Matériel
    cards: usize,
    dv_installed: Option<String>,
    dv_wanted: bool,
    dv_waiting: bool,
    last_scan: Instant,
    ndi_present: Option<PathBuf>,
    ndi_wanted: bool,
    ndi_fetching: bool,
    ndi_sdk: Option<ndi::Sdk>,
    ndi_error: Option<String>,
    hw_focus: usize,
    /// GPU : ce qui manque (pilote NVIDIA, paquets VA-API), et si on l'installe.
    gpu_nvidia: bool,
    gpu_vaapi: Vec<String>,
    gpu_names: Vec<String>,
    gpu_wanted: bool,
    secure_boot: bool,
    license_scroll: u16,
    // Réglages, compte
    admin_user: Input,
    admin_pw: Input,
    admin_pw2: Input,
    // Menu d'un serveur déjà installé
    menu: usize,
    // Installation
    labels: Vec<String>,
    states: Vec<StepState>,
    result: Option<bool>,
    error: Option<String>,
    quit: bool,
}

const MENU: [&str; 7] = [
    "Mettre à jour Strata Core",
    "Ajouter des applis",
    "Installer DeckLink / NDI",
    "Réparer",
    "Désinstaller (données gardées)",
    "Tout effacer (repartir de zéro)",
    "Quitter",
];

pub fn run(args: Args) -> std::io::Result<()> {
    let mut app = new_app(args);
    let mut term = ratatui::try_init()?;
    let r = main_loop(&mut term, &mut app);
    ratatui::restore();
    r
}

fn new_app(args: Args) -> App {
    let (tx, rx) = channel();
    let root = args.root.clone().unwrap_or_else(|| PathBuf::from("/"));
    let installed = checks::installed_core(&root);
    let mut app = App {
        plan: Plan { package: args.package.clone(), version: args.version_core.clone(), firewall: !args.no_firewall, ..Plan::default() },
        args,
        screen: Screen::Welcome,
        history: Vec::new(),
        installed,
        tx,
        rx,
        checks: None,
        name: Input::default(),
        dns: Input::default(),
        tz: Input { value: "Europe/Paris".into(), secret: false },
        focus: 0,
        dns_check: None,
        app_input: Input::default(),
        cards: checks::blackmagic_cards(),
        dv_installed: checks::desktop_video(),
        dv_wanted: false,
        dv_waiting: false,
        last_scan: Instant::now(),
        ndi_present: checks::ndi(),
        ndi_wanted: false,
        ndi_fetching: false,
        ndi_sdk: None,
        ndi_error: None,
        hw_focus: 0,
        gpu_nvidia: false,
        gpu_vaapi: Vec::new(),
        gpu_names: Vec::new(),
        gpu_wanted: false,
        secure_boot: checks::secure_boot(),
        license_scroll: 0,
        admin_user: Input { value: "admin".into(), secret: false },
        admin_pw: Input { value: String::new(), secret: true },
        admin_pw2: Input { value: String::new(), secret: true },
        menu: 0,
        labels: Vec::new(),
        states: Vec::new(),
        result: None,
        error: None,
        quit: false,
    };
    app.dv_wanted = app.cards > 0 && app.dv_installed.is_none();
    let gpus = checks::gpus();
    app.gpu_names = gpus.iter().map(|(v, d)| format!("{v} ({})", d.as_deref().unwrap_or("sans pilote"))).collect();
    (app.gpu_nvidia, app.gpu_vaapi) = checks::gpu_needs(&gpus);
    app.gpu_wanted = app.gpu_nvidia || !app.gpu_vaapi.is_empty();
    app
}

fn main_loop(term: &mut DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    while !app.quit {
        term.draw(|f| draw(f, app))?;
        while let Ok(m) = app.rx.try_recv() {
            app.message(m);
        }
        if app.dv_waiting && app.last_scan.elapsed() > Duration::from_secs(2) {
            app.last_scan = Instant::now();
            if let Some(p) = checks::find_desktop_video() {
                app.plan.desktop_video = Some(p);
                app.dv_waiting = false;
            }
        }
        if event::poll(Duration::from_millis(120))? {
            if let Ev::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    app.key(k);
                }
            }
        }
    }
    Ok(())
}

impl App {
    fn root(&self) -> PathBuf {
        self.args.root.clone().unwrap_or_else(|| PathBuf::from("/"))
    }

    fn go(&mut self, s: Screen) {
        self.history.push(self.screen);
        self.screen = s;
        self.error = None;
        self.focus = 0;
        if s == Screen::Checks && self.checks.is_none() {
            let tx = self.tx.clone();
            let offline = self.plan.package.is_some();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Checks(checks::system(offline)));
            });
        }
    }

    fn back(&mut self) {
        if matches!(self.screen, Screen::Progress | Screen::End) {
            return;
        }
        if let Some(s) = self.history.pop() {
            self.screen = s;
            self.error = None;
        }
    }

    fn message(&mut self, m: Msg) {
        match m {
            Msg::Checks(c) => self.checks = Some(c),
            Msg::Dns(c) => self.dns_check = Some(c),
            Msg::Ndi(r) => {
                self.ndi_fetching = false;
                match r {
                    Ok(sdk) => {
                        self.ndi_sdk = Some(sdk);
                        self.license_scroll = 0;
                        self.go(Screen::License);
                    }
                    // Sans le SDK : NDI repasse à « non », Entrée continue sans lui.
                    Err(e) => {
                        self.ndi_error = Some(e);
                        self.ndi_wanted = false;
                    }
                }
            }
            Msg::Step(e) => match e {
                Event::Start(i) => self.states[i] = StepState::Running(String::new()),
                Event::Progress(i, m) => self.states[i] = StepState::Running(m),
                Event::Done(i, Ok(m)) => self.states[i] = StepState::Done(m),
                Event::Done(i, Err(e)) => self.states[i] = StepState::Failed(e),
                Event::End(ok) => {
                    self.result = Some(ok);
                }
            },
        }
    }

    /// Lance les étapes du plan dans un fil à part.
    fn launch(&mut self) {
        let list = steps::steps(&self.plan);
        self.labels = list.iter().map(|s| s.label.clone()).collect();
        self.states = vec![StepState::Waiting; list.len()];
        self.result = None;
        let tx = self.tx.clone();
        let root = self.root();
        std::thread::spawn(move || {
            let ctx = steps::Ctx::new(&root);
            steps::run(&ctx, &list, &|e| {
                let _ = tx.send(Msg::Step(e));
            });
        });
        self.go(Screen::Progress);
    }

    fn key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            if self.screen != Screen::Progress || self.result.is_some() {
                self.quit = true;
            }
            return;
        }
        if k.code == KeyCode::Esc {
            if self.screen == Screen::Welcome || (self.screen == Screen::End) {
                self.quit = true;
            } else {
                self.back();
            }
            return;
        }
        match self.screen {
            Screen::Welcome => {
                if k.code == KeyCode::Enter {
                    if self.installed.is_some() {
                        self.go(Screen::Menu);
                    } else {
                        self.go(Screen::Checks);
                    }
                }
            }
            Screen::Menu => match k.code {
                KeyCode::Up => self.menu = self.menu.saturating_sub(1),
                KeyCode::Down => self.menu = (self.menu + 1).min(MENU.len() - 1),
                KeyCode::Enter => match self.menu {
                    0 => {
                        self.plan.action = Action::Update;
                        self.go(Screen::Summary);
                    }
                    1 => {
                        self.plan.action = Action::Update;
                        self.go(Screen::Apps);
                    }
                    2 => {
                        self.plan.action = Action::Update;
                        self.go(Screen::Hardware);
                    }
                    3 => {
                        self.plan.action = Action::Repair;
                        self.go(Screen::Summary);
                    }
                    4 => {
                        self.plan.action = Action::Uninstall { purge: false };
                        self.go(Screen::Summary);
                    }
                    5 => {
                        self.plan.action = Action::Uninstall { purge: true };
                        self.go(Screen::Summary);
                    }
                    _ => self.quit = true,
                },
                _ => {}
            },
            Screen::Checks => {
                if k.code == KeyCode::Enter {
                    if let Some(c) = &self.checks {
                        if c.iter().any(|c| c.level == Level::Bad) && self.args.root.is_none() {
                            self.error = Some("Ce serveur ne peut pas recevoir Strata : voir les ✗.".into());
                        } else {
                            self.go(Screen::Name);
                        }
                    }
                }
            }
            Screen::Name => match k.code {
                KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % 3,
                KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + 2) % 3,
                KeyCode::Enter => {
                    let dns = self.dns.value.trim().trim_end_matches('.').to_lowercase();
                    if !checks::valid_dns(&dns) {
                        self.error = Some("Le nom DNS : des mots de lettres, chiffres et tirets, séparés par des points (strata01.regie.exemple.fr).".into());
                        return;
                    }
                    self.plan.dns_name = dns.clone();
                    self.plan.name = if self.name.value.trim().is_empty() {
                        dns.split('.').next().unwrap_or_default().to_string()
                    } else {
                        self.name.value.trim().to_string()
                    };
                    self.plan.timezone = self.tz.value.trim().to_string();
                    self.dns_check = None;
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(Msg::Dns(checks::dns(&dns)));
                    });
                    self.go(Screen::Apps);
                }
                _ => [&mut self.name, &mut self.dns, &mut self.tz][self.focus].key(k),
            },
            Screen::Apps => match k.code {
                KeyCode::Enter if !self.app_input.value.trim().is_empty() => {
                    let v = self.app_input.value.trim().to_string();
                    if !self.plan.apps.contains(&v) {
                        self.plan.apps.push(v);
                    }
                    self.app_input.value.clear();
                }
                KeyCode::Enter => {
                    if self.plan.action == Action::Update {
                        self.go(Screen::Summary);
                    } else {
                        self.go(Screen::Hardware);
                    }
                }
                KeyCode::Delete => {
                    self.plan.apps.pop();
                }
                _ => self.app_input.key(k),
            },
            Screen::Hardware => match k.code {
                KeyCode::Down | KeyCode::Tab => self.hw_focus = (self.hw_focus + 1) % 3,
                KeyCode::Up | KeyCode::BackTab => self.hw_focus = (self.hw_focus + 2) % 3,
                KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                    if self.hw_focus == 0 {
                        self.dv_wanted = !self.dv_wanted;
                        if !self.dv_wanted {
                            self.dv_waiting = false;
                            self.plan.desktop_video = None;
                        }
                    } else if self.hw_focus == 1 {
                        self.ndi_wanted = !self.ndi_wanted;
                        if !self.ndi_wanted {
                            self.plan.ndi = None;
                        }
                    } else {
                        self.gpu_wanted = !self.gpu_wanted;
                    }
                }
                KeyCode::Char('p') => {
                    self.dv_wanted = false;
                    self.dv_waiting = false;
                }
                KeyCode::Enter => {
                    // Le pilote Blackmagic : l'archive, ou l'attendre.
                    if self.dv_wanted && self.plan.desktop_video.is_none() {
                        match checks::find_desktop_video() {
                            Some(p) => self.plan.desktop_video = Some(p),
                            None => {
                                self.dv_waiting = true;
                                return;
                            }
                        }
                    }
                    // NDI : le SDK et sa licence, avant d'aller plus loin.
                    if self.ndi_wanted && self.plan.ndi.is_none() && !self.ndi_fetching {
                        self.ndi_fetching = true;
                        self.ndi_error = None;
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(Msg::Ndi(ndi::get(&std::env::temp_dir().join("strata-install-ndi"))));
                        });
                        return;
                    }
                    if self.ndi_fetching {
                        return;
                    }
                    self.plan.nvidia = self.gpu_wanted && self.gpu_nvidia;
                    self.plan.vaapi = if self.gpu_wanted { self.gpu_vaapi.clone() } else { Vec::new() };
                    if self.plan.action == Action::Update {
                        self.go(Screen::Summary);
                    } else {
                        self.go(Screen::Settings);
                    }
                }
                _ => {}
            },
            Screen::License => match k.code {
                KeyCode::Down => self.license_scroll = self.license_scroll.saturating_add(1),
                KeyCode::Up => self.license_scroll = self.license_scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => self.license_scroll = self.license_scroll.saturating_add(15),
                KeyCode::PageUp => self.license_scroll = self.license_scroll.saturating_sub(15),
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.plan.ndi = self.ndi_sdk.as_ref().map(|s| s.lib.clone());
                    self.back();
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.ndi_wanted = false;
                    self.plan.ndi = None;
                    self.back();
                }
                _ => {}
            },
            Screen::Settings => match k.code {
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => self.plan.firewall = !self.plan.firewall,
                KeyCode::Enter => self.go(Screen::Admin),
                _ => {}
            },
            Screen::Admin => match k.code {
                KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % 3,
                KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + 2) % 3,
                KeyCode::Enter => {
                    let (u, p, p2) = (self.admin_user.value.trim().to_string(), self.admin_pw.value.clone(), self.admin_pw2.value.clone());
                    if u.is_empty() || !u.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@')) {
                        self.error = Some("Le nom : lettres, chiffres et . - _ @".into());
                    } else if p.chars().count() < 8 {
                        self.error = Some("Le mot de passe a 8 caractères au moins.".into());
                    } else if p != p2 {
                        self.error = Some("Les deux mots de passe diffèrent.".into());
                    } else {
                        self.plan.admin = Some((u, p));
                        self.go(Screen::Summary);
                    }
                }
                _ => [&mut self.admin_user, &mut self.admin_pw, &mut self.admin_pw2][self.focus].key(k),
            },
            Screen::Summary => {
                if k.code == KeyCode::Enter {
                    self.launch();
                }
            }
            Screen::Progress => {
                if k.code == KeyCode::Enter && self.result.is_some() {
                    self.go(Screen::End);
                }
            }
            Screen::End => {
                if k.code == KeyCode::Enter || k.code == KeyCode::Char('q') {
                    self.quit = true;
                }
            }
        }
    }
}

// ---------- Le dessin ----------

fn title(t: &str) -> Line<'static> {
    Line::from(vec![Span::styled(format!(" {t} "), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))])
}

fn mark(l: Level) -> Span<'static> {
    match l {
        Level::Ok => Span::styled("✓", Style::new().fg(OK)),
        Level::Warn => Span::styled("!", Style::new().fg(WARN)),
        Level::Bad => Span::styled("✗", Style::new().fg(BAD)),
    }
}

fn field(f: &mut Frame, area: Rect, label: &str, input: &Input, focused: bool) {
    let style = if focused { Style::new().fg(ACCENT) } else { Style::new().fg(MUTED) };
    let b = Block::bordered().border_type(BorderType::Rounded).border_style(style).title(Span::styled(format!(" {label} "), style));
    let inner = b.inner(area);
    f.render_widget(Paragraph::new(input.shown()).block(b), area);
    if focused {
        f.set_cursor_position((inner.x + input.shown().chars().count() as u16, inner.y));
    }
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(FG)), area);
    let [head, body, foot] = Layout::vertical([Constraint::Length(3), Constraint::Min(5), Constraint::Length(2)]).areas(area);
    let dry = if app.args.root.is_some() { "  ·  essai à blanc" } else { "" };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ▲ Strata", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
            Span::styled(" Install", Style::new().fg(ACCENT)),
            Span::styled(format!("   {}{dry}", env!("CARGO_PKG_VERSION")), Style::new().fg(MUTED)),
        ]))
        .block(Block::new().borders(Borders::BOTTOM).border_style(Style::new().fg(Color::Rgb(45, 50, 62)))),
        head,
    );
    let body = body.inner(ratatui::layout::Margin { horizontal: 3, vertical: 1 });
    let hint = match app.screen {
        Screen::Welcome => "Entrée : commencer   ·   Échap : quitter",
        Screen::Menu => "↑↓ : choisir   ·   Entrée : valider   ·   Échap : retour",
        Screen::Name | Screen::Admin => "Tab : champ suivant   ·   Entrée : continuer   ·   Échap : retour",
        Screen::Apps => "Entrée : ajouter (champ rempli) ou continuer   ·   Suppr : retirer la dernière   ·   Échap : retour",
        Screen::Hardware => "↑↓ : DeckLink / NDI / GPU   ·   Espace : oui / non   ·   Entrée : continuer   ·   Échap : retour",
        Screen::License => "↑↓ PgPréc PgSuiv : lire   ·   A : accepter   ·   R : refuser",
        Screen::Settings => "Espace : oui / non   ·   Entrée : continuer   ·   Échap : retour",
        Screen::Summary => "Entrée : lancer   ·   Échap : retour",
        Screen::Progress => {
            if app.result.is_some() {
                "Entrée : continuer"
            } else {
                "Installation en cours…"
            }
        }
        Screen::End => "Entrée : quitter",
        Screen::Checks => "Entrée : continuer   ·   Échap : retour",
    };
    let mut foot_line = vec![Span::styled(format!("  {hint}"), Style::new().fg(MUTED))];
    if let Some(e) = &app.error {
        foot_line = vec![Span::styled(format!("  ✗ {e}"), Style::new().fg(BAD))];
    }
    f.render_widget(Paragraph::new(Line::from(foot_line)), foot);

    match app.screen {
        Screen::Welcome => {
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled("Bienvenue", Style::new().add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from("Cet installateur pose Strata Core sur ce serveur : le cockpit, les comptes, le matériel,"),
                Line::from("la santé et les mises à jour. Les applis (LiveCut, CG, Multiview…) s'ajoutent ensuite."),
                Line::from(""),
            ];
            let what = match (&app.plan.package, &app.plan.version) {
                (Some(p), _) => format!("Strata Core depuis {}", p.display()),
                (None, Some(v)) => format!("Strata Core {v}"),
                (None, None) => "Strata Core, dernière version".into(),
            };
            lines.push(Line::from(vec![Span::styled("À installer : ", Style::new().fg(MUTED)), Span::raw(what)]));
            if let Some(v) = &app.installed {
                lines.push(Line::from(vec![
                    Span::styled("Déjà installé : ", Style::new().fg(MUTED)),
                    Span::styled(format!("Strata Core {v}"), Style::new().fg(OK)),
                ]));
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
        }
        Screen::Menu => {
            let mut lines = vec![title("Ce serveur a déjà Strata"), Line::from("")];
            for (i, m) in MENU.iter().enumerate() {
                let on = i == app.menu;
                lines.push(Line::from(vec![
                    Span::styled(if on { " › " } else { "   " }, Style::new().fg(ACCENT)),
                    Span::styled(m.to_string(), if on { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new() }),
                ]));
            }
            f.render_widget(Paragraph::new(lines), body);
        }
        Screen::Checks => {
            let mut lines = vec![title("Le système"), Line::from("")];
            match &app.checks {
                None => lines.push(Line::from(Span::styled("Vérification…", Style::new().fg(MUTED)))),
                Some(list) => {
                    for c in list {
                        lines.push(Line::from(vec![
                            mark(c.level),
                            Span::raw(format!("  {:12}", c.label)),
                            Span::styled(c.detail.clone(), Style::new().fg(if c.level == Level::Ok { MUTED } else { FG })),
                        ]));
                    }
                }
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
        }
        Screen::Name => {
            let [t, a, b, c, n] =
                Layout::vertical([Constraint::Length(3), Constraint::Length(3), Constraint::Length(3), Constraint::Length(3), Constraint::Min(1)]).areas(body);
            f.render_widget(
                Paragraph::new(vec![
                    title("Le serveur"),
                    Line::from(Span::styled(
                        "Le nom DNS donne les adresses : le cockpit à strata01.regie.exemple.fr, chaque appli à livecut.strata01…",
                        Style::new().fg(MUTED),
                    )),
                ])
                .wrap(Wrap { trim: false }),
                t,
            );
            field(f, a, "Nom d'usage (« Régie A »)", &app.name, app.focus == 0);
            field(f, b, "Nom DNS", &app.dns, app.focus == 1);
            field(f, c, "Fuseau horaire", &app.tz, app.focus == 2);
            f.render_widget(Paragraph::new(Span::styled("Il faudra deux enregistrements DNS : le nom, et *.le nom (les applis).", Style::new().fg(MUTED))), n);
        }
        Screen::Apps => {
            let [t, a, l] = Layout::vertical([Constraint::Length(3), Constraint::Length(3), Constraint::Min(1)]).areas(body);
            f.render_widget(
                Paragraph::new(vec![
                    title("Les applis"),
                    Line::from(Span::styled(
                        "Leur dépôt GitHub (owner/repo) ou leur archive ; installées et activées. Aussi plus tard, depuis la Bibliothèque.",
                        Style::new().fg(MUTED),
                    )),
                ])
                .wrap(Wrap { trim: false }),
                t,
            );
            field(f, a, "Ajouter une appli", &app.app_input, true);
            let mut lines: Vec<Line> =
                app.plan.apps.iter().map(|x| Line::from(vec![Span::styled("  • ", Style::new().fg(ACCENT)), Span::raw(x.clone())])).collect();
            if lines.is_empty() {
                lines.push(Line::from(Span::styled("  Aucune pour l'instant.", Style::new().fg(MUTED))));
            }
            f.render_widget(Paragraph::new(lines), l);
        }
        Screen::Hardware => draw_hardware(f, body, app),
        Screen::License => {
            let [text_area, ask] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(body);
            let text = app.ndi_sdk.as_ref().map(|s| s.license.clone()).unwrap_or_default();
            let b = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(ACCENT)).title(title("Licence du SDK NDI"));
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).scroll((app.license_scroll, 0)).block(b), text_area);
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::styled(" A ", Style::new().fg(BG).bg(OK).add_modifier(Modifier::BOLD)),
                        Span::styled("  j'accepte la licence : la bibliothèque NDI sera installée", Style::new().fg(FG)),
                        Span::raw("        "),
                        Span::styled(" R ", Style::new().fg(BG).bg(BAD).add_modifier(Modifier::BOLD)),
                        Span::styled("  je refuse : sans NDI", Style::new().fg(FG)),
                    ]),
                ]),
                ask,
            );
        }
        Screen::Settings => {
            let ufw = if app.plan.firewall { Span::styled("[x] ", Style::new().fg(ACCENT)) } else { Span::styled("[ ] ", Style::new().fg(MUTED)) };
            let lines = vec![
                title("Réglages"),
                Line::from(""),
                Line::from(vec![Span::styled("Données      ", Style::new().fg(MUTED)), Span::raw(paths::DATA)]),
                Line::from(vec![Span::styled("Programme    ", Style::new().fg(MUTED)), Span::raw(paths::BIN)]),
                Line::from(vec![Span::styled("Cockpit      ", Style::new().fg(MUTED)), Span::raw(format!("port {} (et 443 en HTTPS)", paths::PORT))]),
                Line::from(""),
                Line::from(vec![ufw, Span::raw("Ouvrir les ports 80 et 443 dans le pare-feu (si ufw est actif)")]),
            ];
            f.render_widget(Paragraph::new(lines), body);
        }
        Screen::Admin => {
            let [t, a, b, c] = Layout::vertical([Constraint::Length(3), Constraint::Length(3), Constraint::Length(3), Constraint::Length(3)]).areas(body);
            f.render_widget(
                Paragraph::new(vec![
                    title("Le compte administrateur"),
                    Line::from(Span::styled("Il règle le serveur depuis le cockpit ; c'est aussi le compte de secours.", Style::new().fg(MUTED))),
                ])
                .wrap(Wrap { trim: false }),
                t,
            );
            field(f, a, "Nom", &app.admin_user, app.focus == 0);
            field(f, b, "Mot de passe (8 caractères au moins)", &app.admin_pw, app.focus == 1);
            field(f, c, "Le même, encore", &app.admin_pw2, app.focus == 2);
        }
        Screen::Summary => draw_summary(f, body, app),
        Screen::Progress => {
            let mut lines = vec![
                title(match app.plan.action {
                    Action::Uninstall { .. } => "Désinstallation",
                    Action::Update => "Mise à jour",
                    Action::Repair => "Réparation",
                    Action::Install => "Installation",
                }),
                Line::from(""),
            ];
            for (i, l) in app.labels.iter().enumerate() {
                let (m, detail, style) = match &app.states[i] {
                    StepState::Waiting => (Span::styled("·", Style::new().fg(MUTED)), String::new(), Style::new().fg(MUTED)),
                    StepState::Running(d) => (Span::styled("…", Style::new().fg(ACCENT)), d.clone(), Style::new().fg(FG)),
                    StepState::Done(d) => (Span::styled("✓", Style::new().fg(OK)), d.clone(), Style::new()),
                    StepState::Failed(e) => (Span::styled("✗", Style::new().fg(BAD)), e.clone(), Style::new().fg(BAD)),
                };
                lines.push(Line::from(vec![m, Span::styled(format!("  {:34}", l), style), Span::styled(detail, Style::new().fg(MUTED))]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(format!("Journal complet : {}", paths::under(&app.root(), paths::LOG).display()), Style::new().fg(MUTED))));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
        }
        Screen::End => draw_end(f, body, app),
    }
    if app.screen == Screen::Hardware && app.dv_waiting {
        draw_wait(f, area);
    }
}

fn yes_no(on: bool, focused: bool) -> Span<'static> {
    let t = if on { "[ Installer ]" } else { "[ Passer ]" };
    Span::styled(t, if focused { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(MUTED) })
}

fn draw_hardware(f: &mut Frame, body: Rect, app: &App) {
    let mut lines = vec![title("Le matériel"), Line::from("")];
    lines.push(Line::from(Span::styled("DeckLink (Blackmagic)", Style::new().add_modifier(Modifier::BOLD))));
    let state = match (&app.dv_installed, app.cards) {
        (Some(v), n) => format!("pilote Desktop Video {v} installé · {n} carte(s)"),
        (None, 0) => "aucune carte Blackmagic sur ce serveur".into(),
        (None, n) => format!("{n} carte(s) Blackmagic, pilote Desktop Video absent"),
    };
    lines.push(Line::from(vec![Span::styled("  ", Style::new()), Span::styled(state, Style::new().fg(MUTED))]));
    lines.push(Line::from(vec![Span::raw("  Pilote Desktop Video : "), yes_no(app.dv_wanted, app.hw_focus == 0)]));
    if let Some(p) = &app.plan.desktop_video {
        lines.push(Line::from(vec![Span::styled("  ✓ ", Style::new().fg(OK)), Span::raw(format!("archive : {}", p.display()))]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("NDI", Style::new().add_modifier(Modifier::BOLD))));
    let ndi_state = match (&app.ndi_present, &app.plan.ndi) {
        (_, Some(_)) => "licence acceptée : la bibliothèque sera installée".to_string(),
        (Some(p), None) => format!("bibliothèque présente ({})", p.display()),
        (None, None) => "bibliothèque absente. Oui : le SDK NDI est téléchargé, puis sa licence s'affiche, à accepter (A)".into(),
    };
    lines.push(Line::from(Span::styled(format!("  {ndi_state}"), Style::new().fg(MUTED))));
    lines.push(Line::from(vec![Span::raw("  Bibliothèque NDI : "), yes_no(app.ndi_wanted, app.hw_focus == 1)]));
    if app.ndi_fetching {
        lines.push(Line::from(Span::styled("  Téléchargement du SDK NDI…", Style::new().fg(ACCENT))));
    }
    if let Some(e) = &app.ndi_error {
        lines.push(Line::from(Span::styled(format!("  ✗ {e}"), Style::new().fg(BAD))));
        lines.push(Line::from(Span::styled("  NDI ne sera pas installé (Entrée pour continuer, ou Oui pour réessayer).", Style::new().fg(MUTED))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("GPU", Style::new().add_modifier(Modifier::BOLD))));
    let gpu_state = if app.gpu_names.is_empty() {
        "aucun GPU : les applis encodent avec les processeurs".to_string()
    } else {
        let need = match (app.gpu_nvidia, app.gpu_vaapi.is_empty()) {
            (false, true) => " · prêts".to_string(),
            (true, true) => " · pilote NVIDIA à installer (NVENC / NVDEC)".to_string(),
            (false, false) => format!(" · VA-API à installer ({})", app.gpu_vaapi.join(", ")),
            (true, false) => format!(" · pilote NVIDIA et VA-API ({}) à installer", app.gpu_vaapi.join(", ")),
        };
        format!("{}{need}", app.gpu_names.join(", "))
    };
    lines.push(Line::from(Span::styled(format!("  {gpu_state}"), Style::new().fg(MUTED))));
    if app.gpu_nvidia || !app.gpu_vaapi.is_empty() {
        lines.push(Line::from(vec![Span::raw("  Pilotes GPU : "), yes_no(app.gpu_wanted, app.hw_focus == 2)]));
    }
    if app.secure_boot && (app.dv_wanted || (app.gpu_wanted && app.gpu_nvidia)) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  ! Secure Boot est actif : au redémarrage, enrôlez la clé MOK (écran bleu « Perform MOK management »), sinon les pilotes Blackmagic et NVIDIA ne se chargent pas.",
            Style::new().fg(WARN),
        )));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

/// La marche à suivre pour le pilote Blackmagic, en attendant l'archive. Plein écran et sans
/// cadre : les commandes se copient à la souris telles quelles, chacune sur une ligne.
fn draw_wait(f: &mut Frame, area: Rect) {
    let r = Rect { x: area.x, y: area.y + 3, width: area.width, height: area.height.saturating_sub(5) };
    f.render_widget(Clear, r);
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(FG)), r);
    let who = std::env::var("SUDO_USER").unwrap_or_else(|_| "root".into());
    let host = checks::local_ips().first().map(|i| i.to_string()).unwrap_or_else(|| "<ce serveur>".into());
    let cmd = |c: String| Line::from(Span::styled(c, Style::new().fg(ACCENT)));
    let lines = vec![
        title("Pilote Blackmagic Desktop Video"),
        Line::from(""),
        Line::from("1. Sur votre ordinateur, téléchargez « Desktop Video » pour Linux :"),
        Line::from("   blackmagicdesign.com › Support › Capture and Playback (Blackmagic demande un formulaire)."),
        Line::from(""),
        Line::from("2. Copiez-le sur ce serveur tel quel (.tar.gz), ou ce qu'il contient (.sh, .deb)."),
        Line::from("   Dans un terminal de votre ordinateur :"),
        Line::from(""),
        Line::from(Span::styled("   Mac, Linux :", Style::new().fg(MUTED))),
        Line::from(""),
        cmd(format!("scp ~/Downloads/Blackmagic_Desktop_Video_Linux_* {who}@{host}:")),
        Line::from(""),
        Line::from(Span::styled("   Windows (PowerShell) :", Style::new().fg(MUTED))),
        Line::from(""),
        cmd(format!("Get-ChildItem $HOME\\Downloads\\Blackmagic_Desktop_Video_Linux_* | % {{ scp $_.FullName {who}@{host}: }}")),
        Line::from(""),
        Line::from("3. Le fichier est trouvé tout seul (dossier personnel, /root, /tmp) : l'installation continue."),
        Line::from(""),
        Line::from(Span::styled("En attente du fichier…   P : passer, sans pilote Blackmagic", Style::new().fg(WARN))),
    ];
    // Sans retour à la ligne : une commande n'est jamais coupée en deux.
    f.render_widget(Paragraph::new(lines), r.inner(ratatui::layout::Margin { horizontal: 3, vertical: 1 }));
}

fn draw_summary(f: &mut Frame, body: Rect, app: &App) {
    let p = &app.plan;
    let row = |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:16}"), Style::new().fg(MUTED)), Span::raw(v)]);
    let mut lines = vec![title("Récapitulatif"), Line::from("")];
    match &p.action {
        Action::Install => {
            lines.push(row("Serveur", format!("{} · {} · {}", p.name, p.dns_name, p.timezone)));
            if let Some(c) = &app.dns_check {
                lines.push(Line::from(vec![Span::raw(" ".repeat(16)), mark(c.level), Span::styled(format!(" {}", c.detail), Style::new().fg(MUTED))]));
            }
            lines.push(row("Applis", if p.apps.is_empty() { "aucune".into() } else { p.apps.join(", ") }));
            lines.push(row("DeckLink", p.desktop_video.as_ref().map_or("—".into(), |d| format!("pilote depuis {}", d.display()))));
            lines.push(row("NDI", if p.ndi.is_some() { "bibliothèque (licence acceptée)".into() } else { "—".into() }));
            let gpu = [p.nvidia.then(|| "pilote NVIDIA".to_string()), (!p.vaapi.is_empty()).then(|| format!("VA-API ({})", p.vaapi.join(", ")))]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            lines.push(row("GPU", if gpu.is_empty() { "—".into() } else { gpu.join(", ") }));
            lines.push(row("Pare-feu", if p.firewall { "80 et 443 ouverts (ufw)".into() } else { "inchangé".into() }));
            lines.push(row("Administrateur", p.admin.as_ref().map_or("—".into(), |a| a.0.clone())));
        }
        Action::Update => {
            lines.push(row("Mettre à jour", "Strata Core (dernière version)".into()));
            if !p.apps.is_empty() {
                lines.push(row("Applis", p.apps.join(", ")));
            }
            if p.desktop_video.is_some() || p.ndi.is_some() {
                lines.push(row(
                    "Matériel",
                    [p.desktop_video.as_ref().map(|_| "pilote Blackmagic"), p.ndi.as_ref().map(|_| "NDI")].into_iter().flatten().collect::<Vec<_>>().join(", "),
                ));
            }
        }
        Action::Repair => lines.push(row("Réparer", "fichiers du système, droits, puis relance".into())),
        Action::Uninstall { purge } => {
            lines.push(row("Désinstaller", "le service, le programme, les fichiers du système".into()));
            if *purge {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:16}", "Effacer"), Style::new().fg(MUTED)),
                    Span::styled(format!("les données ({}) : comptes, réglages, applis ; le compte « {} »", paths::DATA, paths::USER), Style::new().fg(BAD)),
                ]));
            } else {
                lines.push(row("Garder", format!("les données ({})", paths::DATA)));
            }
            lines.push(row("Garder", "les pilotes (Blackmagic, NDI, NVIDIA)".into()));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Entrée pour lancer.", Style::new().fg(ACCENT))));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

fn draw_end(f: &mut Frame, body: Rect, app: &App) {
    let ok = app.result == Some(true);
    let mut lines = vec![
        Line::from(if ok {
            Span::styled("Terminé.", Style::new().fg(OK).add_modifier(Modifier::BOLD))
        } else {
            Span::styled("Arrêté sur une erreur.", Style::new().fg(BAD).bold())
        }),
        Line::from(""),
    ];
    if ok && app.plan.action == Action::Install {
        lines.push(Line::from(vec![
            Span::styled("Le cockpit   ", Style::new().fg(MUTED)),
            Span::styled(format!("http://{}/", app.plan.dns_name), Style::new().fg(ACCENT)),
        ]));
        for ip in checks::local_ips().iter().take(2) {
            lines.push(Line::from(vec![Span::raw(" ".repeat(13)), Span::raw(format!("http://{ip}/"))]));
        }
        lines.push(Line::from(""));
        let mut todo = Vec::new();
        if let Some(c) = app.dns_check.as_ref().filter(|c| c.level != Level::Ok) {
            todo.push(c.detail.clone());
        }
        if app.plan.desktop_video.is_some() && !std::path::Path::new("/dev/blackmagic").exists() {
            todo.push("Redémarrer le serveur pour charger le pilote Blackmagic.".into());
        }
        if app.plan.nvidia {
            todo.push("Redémarrer le serveur pour charger le pilote NVIDIA.".into());
        }
        if app.secure_boot && (app.plan.desktop_video.is_some() || app.plan.nvidia) {
            todo.push("Secure Boot : au redémarrage, enrôler la clé MOK (écran « Perform MOK management »).".into());
        }
        if !todo.is_empty() {
            lines.push(Line::from(Span::styled("Reste à faire", Style::new().add_modifier(Modifier::BOLD))));
            for t in todo {
                lines.push(Line::from(format!("  • {t}")));
            }
        }
    }
    if !ok {
        if let Some(StepState::Failed(e)) = app.states.iter().find(|s| matches!(s, StepState::Failed(_))) {
            lines.push(Line::from(Span::styled(e.clone(), Style::new().fg(BAD))));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!("Journal : {}", paths::under(&app.root(), paths::LOG).display()), Style::new().fg(MUTED))));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn black_background_everywhere() {
        let mut app = new_app(Args::default());
        for screen in [Screen::Welcome, Screen::Hardware, Screen::Summary] {
            app.screen = screen;
            app.dv_waiting = screen == Screen::Hardware;
            let mut term = Terminal::new(TestBackend::new(110, 36)).unwrap();
            term.draw(|f| draw(f, &app)).unwrap();
            let buf = term.backend().buffer();
            assert!(buf.content().iter().all(|c| c.bg == BG), "{screen:?}");
            assert!(buf.content().iter().all(|c| c.fg != Color::Reset), "{screen:?}");
        }
    }

    #[test]
    fn the_copy_commands_stay_on_one_line() {
        let mut app = new_app(Args::default());
        app.screen = Screen::Hardware;
        app.dv_waiting = true;
        let mut term = Terminal::new(TestBackend::new(130, 36)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let buf = term.backend().buffer();
        let rows: Vec<String> = (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect();
        if std::env::var("SHOW").is_ok() {
            println!("{}", rows.join("\n"));
        }
        let line = |start: &str| rows.iter().map(|r| r.trim()).find(|r| r.starts_with(start)).map(str::to_string);
        assert!(line("scp ~/Downloads/Blackmagic_Desktop_Video_Linux_* ").unwrap().ends_with(':'));
        assert!(line("Get-ChildItem $HOME\\Downloads\\Blackmagic_Desktop_Video_Linux_* | % { scp $_.FullName ").unwrap().ends_with(": }"));
        // Rien d'autre sur la ligne d'une commande (pas de cadre).
        assert!(!rows.iter().any(|r| r.contains('│')));
    }

    #[test]
    fn the_license_says_how_to_accept() {
        let mut app = new_app(Args::default());
        app.screen = Screen::License;
        app.ndi_sdk = Some(ndi::Sdk { lib: "/tmp/libndi.so.6".into(), license: "NDI SDK License Agreement\nTerms.".into() });
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let buf = term.backend().buffer();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("NDI SDK License Agreement"));
        assert!(text.contains(" A   j'accepte la licence") && text.contains(" R   je refuse"));
    }
}
