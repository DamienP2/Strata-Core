//! L'écran plein terminal de l'installateur (ratatui) : un assistant pas à pas, une question par
//! écran, sur fond sombre. Le choix en cours est une barre pleine, le champ en cours a un cadre
//! épais et un curseur ; les touches utiles sont rappelées en bas.
//!
//! Installation : système, nom DNS, nom d'usage, fuseau, applis, pilote Blackmagic, NDI, GPU,
//! pare-feu, compte administrateur, mot de passe, récapitulatif, installation, fin. Les étapes
//! sans objet (pas de carte Blackmagic, NDI déjà là…) sont sautées. Sur un serveur déjà installé :
//! mettre à jour, ajouter des applis, matériel, réparer, désinstaller, tout effacer.
use std::{
    path::PathBuf,
    sync::mpsc::{channel, Receiver, Sender},
    time::{Duration, Instant},
};

use crossterm::event::{self, Event as Ev, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
    DefaultTerminal, Frame,
};

use crate::{
    checks::{self, Check, Level},
    ndi, paths,
    steps::{self, Action, Event, Plan},
    Args,
};

// Fond sombre, texte clair ; posés partout (le thème du terminal ne compte pas). Les 256 couleurs
// standard, pas les couleurs 24 bits : le Terminal de macOS, PuTTY et d'autres ignorent ces
// dernières (et SSH ne transmet pas COLORTERM pour le savoir).
const BG: Color = Color::Indexed(233); // #121212
const PANEL: Color = Color::Indexed(235); // #262626
const LINE: Color = Color::Indexed(240); // #585858
const FG: Color = Color::Indexed(255); // #eeeeee
const DIM: Color = Color::Indexed(248); // #a8a8a8
const ACCENT: Color = Color::Indexed(111); // #87afff
/// Le texte sur la barre de sélection.
const ON_ACCENT: Color = Color::Indexed(16); // #000000
const OK: Color = Color::Indexed(78); // #5fd787
const WARN: Color = Color::Indexed(214); // #ffaf00
const BAD: Color = Color::Indexed(204); // #ff5f87

/// La largeur du contenu, centré : lisible même sur un grand écran.
const WIDTH: u16 = 92;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Welcome,
    Menu,
    Checks,
    Dns,
    Name,
    Timezone,
    Apps,
    Blackmagic,
    BlackmagicWait,
    Ndi,
    License,
    Gpu,
    Firewall,
    AdminUser,
    AdminPassword,
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
    fn text(v: &str) -> Self {
        Input { value: v.into(), secret: false }
    }
    fn secret() -> Self {
        Input { value: String::new(), secret: true }
    }
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
    /// Les écrans du parcours choisi, dans l'ordre (pour « Étape n sur N » et « suivant »).
    flow: Vec<Screen>,
    plan: Plan,
    installed: Option<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Le choix en cours (barre pleine) sur un écran à choix.
    sel: usize,
    /// Le champ en cours sur un écran à plusieurs champs.
    focus: usize,
    tick: u64,
    checks: Option<Vec<Check>>,
    dns: Input,
    name: Input,
    tz_choices: Vec<String>,
    tz_other: Input,
    dns_check: Option<Check>,
    app_input: Input,
    cards: usize,
    dv_installed: Option<String>,
    last_scan: Instant,
    ndi_present: Option<PathBuf>,
    ndi_fetching: bool,
    ndi_sdk: Option<ndi::Sdk>,
    ndi_error: Option<String>,
    gpu_nvidia: bool,
    gpu_vaapi: Vec<String>,
    gpu_names: Vec<String>,
    secure_boot: bool,
    license_scroll: u16,
    admin_user: Input,
    admin_pw: Input,
    admin_pw2: Input,
    labels: Vec<String>,
    states: Vec<StepState>,
    result: Option<bool>,
    error: Option<String>,
    quit: bool,
}

/// Le menu d'un serveur déjà installé : (choix, ce qu'il fait).
const MENU: [(&str, &str); 7] = [
    ("Mettre à jour Strata Core", "la dernière version, les réglages et les données gardés"),
    ("Ajouter des applis", "depuis leur dépôt GitHub ou leur archive"),
    ("Installer un pilote ou NDI", "Blackmagic Desktop Video, NDI, pilotes GPU"),
    ("Réparer", "remet les fichiers du système et les droits, puis relance"),
    ("Désinstaller", "le programme et le service ; les données sont gardées"),
    ("Tout effacer", "repartir de zéro : programme, données, comptes, applis"),
    ("Quitter", ""),
];

const ZONES: [&str; 8] = ["Europe/Paris", "Europe/London", "Europe/Brussels", "Europe/Zurich", "Europe/Madrid", "America/New_York", "America/Montreal", "UTC"];

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
    // Le fuseau du système d'abord, s'il en a un vrai.
    let mut tz_choices: Vec<String> = ZONES.iter().map(|s| s.to_string()).collect();
    if let Some(sys) = std::fs::read_to_string("/etc/timezone").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "Etc/UTC") {
        tz_choices.retain(|z| *z != sys);
        tz_choices.insert(0, sys);
    }
    let gpus = checks::gpus();
    let (gpu_nvidia, gpu_vaapi) = checks::gpu_needs(&gpus);
    App {
        plan: Plan {
            package: args.package.clone(),
            version: args.version_core.clone(),
            firewall: !args.no_firewall,
            timezone: tz_choices[0].clone(),
            ..Plan::default()
        },
        args,
        screen: Screen::Welcome,
        history: Vec::new(),
        flow: Vec::new(),
        installed,
        tx,
        rx,
        sel: 0,
        focus: 0,
        tick: 0,
        checks: None,
        dns: Input::default(),
        name: Input::default(),
        tz_choices,
        tz_other: Input::default(),
        dns_check: None,
        app_input: Input::default(),
        cards: checks::blackmagic_cards(),
        dv_installed: checks::desktop_video(),
        last_scan: Instant::now(),
        ndi_present: checks::ndi(),
        ndi_fetching: false,
        ndi_sdk: None,
        ndi_error: None,
        gpu_names: gpus.iter().map(|(v, d)| format!("{v} ({})", d.as_deref().unwrap_or("sans pilote"))).collect(),
        gpu_nvidia,
        gpu_vaapi,
        secure_boot: checks::secure_boot(),
        license_scroll: 0,
        admin_user: Input::text("admin"),
        admin_pw: Input::secret(),
        admin_pw2: Input::secret(),
        labels: Vec::new(),
        states: Vec::new(),
        result: None,
        error: None,
        quit: false,
    }
}

fn main_loop(term: &mut DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    while !app.quit {
        app.tick += 1;
        term.draw(|f| draw(f, app))?;
        while let Ok(m) = app.rx.try_recv() {
            app.message(m);
        }
        if app.screen == Screen::BlackmagicWait && app.last_scan.elapsed() > Duration::from_secs(2) {
            app.last_scan = Instant::now();
            if let Some(p) = checks::find_desktop_video() {
                app.plan.desktop_video = Some(p);
                app.next_after(Screen::Blackmagic);
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

    /// Les écrans de l'installation, sans ceux qui n'ont pas d'objet ici.
    fn install_flow(&self) -> Vec<Screen> {
        let mut v = vec![Screen::Checks, Screen::Dns, Screen::Name, Screen::Timezone, Screen::Apps];
        if self.cards > 0 && self.dv_installed.is_none() {
            v.push(Screen::Blackmagic);
        }
        if self.ndi_present.is_none() {
            v.push(Screen::Ndi);
        }
        if self.gpu_nvidia || !self.gpu_vaapi.is_empty() {
            v.push(Screen::Gpu);
        }
        v.extend([Screen::Firewall, Screen::AdminUser, Screen::AdminPassword, Screen::Summary]);
        v
    }

    fn start(&mut self, flow: Vec<Screen>) {
        self.flow = flow;
        let first = self.flow[0];
        self.go(first);
    }

    fn go(&mut self, s: Screen) {
        self.history.push(self.screen);
        self.screen = s;
        self.error = None;
        self.sel = 0;
        self.focus = 0;
        if s == Screen::Checks && self.checks.is_none() {
            let tx = self.tx.clone();
            let offline = self.plan.package.is_some();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Checks(checks::system(offline)));
            });
        }
        if s == Screen::Name && self.name.value.is_empty() {
            self.name.value = self.plan.dns_name.split('.').next().unwrap_or_default().to_string();
        }
    }

    /// L'écran qui suit `s` dans le parcours.
    fn next_after(&mut self, s: Screen) {
        let i = self.flow.iter().position(|x| *x == s).map_or(self.flow.len(), |i| i + 1);
        match self.flow.get(i).copied() {
            Some(n) => self.go(n),
            None => self.go(Screen::Summary),
        }
    }

    fn next(&mut self) {
        self.next_after(self.screen);
    }

    fn back(&mut self) {
        if matches!(self.screen, Screen::Progress | Screen::End) {
            return;
        }
        if let Some(s) = self.history.pop() {
            self.screen = s;
            self.error = None;
            self.sel = 0;
            self.focus = 0;
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
                    Err(e) => {
                        self.ndi_error = Some(e);
                        self.sel = 1;
                    }
                }
            }
            Msg::Step(e) => match e {
                Event::Start(i) => self.states[i] = StepState::Running(String::new()),
                Event::Progress(i, m) => self.states[i] = StepState::Running(m),
                Event::Done(i, Ok(m)) => self.states[i] = StepState::Done(m),
                Event::Done(i, Err(e)) => self.states[i] = StepState::Failed(e),
                Event::End(ok) => self.result = Some(ok),
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

    /// ↑↓ sur un écran à `n` choix.
    fn pick(&mut self, k: KeyEvent, n: usize) {
        match k.code {
            KeyCode::Up | KeyCode::BackTab => self.sel = (self.sel + n - 1) % n,
            KeyCode::Down | KeyCode::Tab => self.sel = (self.sel + 1) % n,
            _ => {}
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            if self.screen != Screen::Progress || self.result.is_some() {
                self.quit = true;
            }
            return;
        }
        if k.code == KeyCode::Esc {
            match self.screen {
                Screen::Welcome | Screen::End => self.quit = true,
                _ => self.back(),
            }
            return;
        }
        let enter = k.code == KeyCode::Enter;
        match self.screen {
            Screen::Welcome => {
                if enter {
                    if self.installed.is_some() {
                        self.go(Screen::Menu);
                    } else {
                        let flow = self.install_flow();
                        self.start(flow);
                    }
                }
            }
            Screen::Menu => {
                self.pick(k, MENU.len());
                if enter {
                    match self.sel {
                        0 => {
                            self.plan.action = Action::Update;
                            self.start(vec![Screen::Summary]);
                        }
                        1 => {
                            self.plan.action = Action::Update;
                            self.start(vec![Screen::Apps, Screen::Summary]);
                        }
                        2 => {
                            self.plan.action = Action::Update;
                            let mut v = vec![Screen::Blackmagic, Screen::Ndi];
                            if self.gpu_nvidia || !self.gpu_vaapi.is_empty() {
                                v.push(Screen::Gpu);
                            }
                            v.push(Screen::Summary);
                            self.start(v);
                        }
                        3 => {
                            self.plan.action = Action::Repair;
                            self.start(vec![Screen::Summary]);
                        }
                        4 | 5 => {
                            self.plan.action = Action::Uninstall { purge: self.sel == 5 };
                            self.start(vec![Screen::Summary]);
                        }
                        _ => self.quit = true,
                    }
                }
            }
            Screen::Checks => {
                if enter {
                    if let Some(c) = &self.checks {
                        if c.iter().any(|c| c.level == Level::Bad) && self.args.root.is_none() {
                            self.error = Some("Ce serveur ne peut pas recevoir Strata : voir les ✗ ci-dessus.".into());
                        } else {
                            self.next();
                        }
                    }
                }
            }
            Screen::Dns => {
                if enter {
                    let dns = self.dns.value.trim().trim_end_matches('.').to_lowercase();
                    if !checks::valid_dns(&dns) {
                        self.error = Some("Des mots (lettres, chiffres, tirets) séparés par des points : strata01.regie.exemple.fr".into());
                        return;
                    }
                    if dns != self.plan.dns_name {
                        self.name.value.clear();
                    }
                    self.plan.dns_name = dns.clone();
                    self.dns_check = None;
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(Msg::Dns(checks::dns(&dns)));
                    });
                    self.next();
                } else {
                    self.dns.key(k);
                }
            }
            Screen::Name => {
                if enter {
                    let n = self.name.value.trim();
                    self.plan.name = if n.is_empty() { self.plan.dns_name.split('.').next().unwrap_or_default().to_string() } else { n.to_string() };
                    self.next();
                } else {
                    self.name.key(k);
                }
            }
            Screen::Timezone => {
                let n = self.tz_choices.len() + 1;
                match k.code {
                    KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => self.pick(k, n),
                    KeyCode::Enter => {
                        let tz = if self.sel < self.tz_choices.len() { self.tz_choices[self.sel].clone() } else { self.tz_other.value.trim().to_string() };
                        if !tz.contains('/') && tz != "UTC" {
                            self.error = Some("Un fuseau comme Europe/Paris ou America/New_York.".into());
                            return;
                        }
                        self.plan.timezone = tz;
                        self.next();
                    }
                    _ if self.sel == self.tz_choices.len() => self.tz_other.key(k),
                    _ => {}
                }
            }
            Screen::Apps => match k.code {
                KeyCode::Enter if !self.app_input.value.trim().is_empty() => {
                    let v = self.app_input.value.trim().to_string();
                    if !self.plan.apps.contains(&v) {
                        self.plan.apps.push(v);
                    }
                    self.app_input.value.clear();
                }
                KeyCode::Enter => self.next(),
                KeyCode::Delete => {
                    self.plan.apps.pop();
                }
                _ => self.app_input.key(k),
            },
            Screen::Blackmagic => {
                self.pick(k, 2);
                if enter {
                    if self.sel == 0 {
                        match checks::find_desktop_video() {
                            Some(p) => {
                                self.plan.desktop_video = Some(p);
                                self.next();
                            }
                            None => self.go(Screen::BlackmagicWait),
                        }
                    } else {
                        self.plan.desktop_video = None;
                        self.next();
                    }
                }
            }
            Screen::BlackmagicWait => {
                if matches!(k.code, KeyCode::Char('p') | KeyCode::Char('P')) {
                    self.plan.desktop_video = None;
                    self.next_after(Screen::Blackmagic);
                }
            }
            Screen::Ndi => {
                if self.ndi_fetching {
                    return;
                }
                self.pick(k, 2);
                if enter {
                    if self.sel == 0 {
                        self.ndi_fetching = true;
                        self.ndi_error = None;
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(Msg::Ndi(crate::checks::private_tmp("strata-install-ndi").and_then(|dir| ndi::get(&dir))));
                        });
                    } else {
                        self.plan.ndi = None;
                        self.next();
                    }
                }
            }
            Screen::License => match k.code {
                KeyCode::PageDown | KeyCode::Char(' ') => self.license_scroll = self.license_scroll.saturating_add(15),
                KeyCode::PageUp => self.license_scroll = self.license_scroll.saturating_sub(15),
                KeyCode::Char('a') | KeyCode::Char('A') => self.license(true),
                KeyCode::Char('r') | KeyCode::Char('R') => self.license(false),
                KeyCode::Enter => self.license(self.sel == 0),
                _ => self.pick(k, 2),
            },
            Screen::Gpu => {
                self.pick(k, 2);
                if enter {
                    let yes = self.sel == 0;
                    self.plan.nvidia = yes && self.gpu_nvidia;
                    self.plan.vaapi = if yes { self.gpu_vaapi.clone() } else { Vec::new() };
                    self.next();
                }
            }
            Screen::Firewall => {
                self.pick(k, 2);
                if enter {
                    self.plan.firewall = self.sel == 0;
                    self.next();
                }
            }
            Screen::AdminUser => {
                if enter {
                    let u = self.admin_user.value.trim().to_string();
                    if u.is_empty() || !u.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@')) {
                        self.error = Some("Lettres, chiffres et . - _ @ seulement.".into());
                    } else {
                        self.next();
                    }
                } else {
                    self.admin_user.key(k);
                }
            }
            Screen::AdminPassword => match k.code {
                KeyCode::Tab | KeyCode::Down | KeyCode::BackTab | KeyCode::Up => self.focus = 1 - self.focus,
                KeyCode::Enter if self.focus == 0 => self.focus = 1,
                KeyCode::Enter => {
                    let (p, p2) = (self.admin_pw.value.clone(), self.admin_pw2.value.clone());
                    if p.chars().count() < 8 {
                        self.error = Some("8 caractères au moins.".into());
                        self.focus = 0;
                    } else if p != p2 {
                        self.error = Some("Les deux saisies diffèrent : retapez la seconde.".into());
                        self.admin_pw2.value.clear();
                    } else {
                        self.plan.admin = Some((self.admin_user.value.trim().to_string(), p));
                        self.next();
                    }
                }
                _ => [&mut self.admin_pw, &mut self.admin_pw2][self.focus].key(k),
            },
            Screen::Summary => {
                self.pick(k, 2);
                if enter {
                    if self.sel == 0 {
                        self.launch();
                    } else {
                        self.back();
                    }
                }
            }
            Screen::Progress => {
                if enter && self.result.is_some() {
                    self.go(Screen::End);
                }
            }
            Screen::End => {
                if enter || k.code == KeyCode::Char('q') {
                    self.quit = true;
                }
            }
        }
    }

    fn license(&mut self, accept: bool) {
        self.plan.ndi = if accept { self.ndi_sdk.as_ref().map(|s| s.lib.clone()) } else { None };
        self.next_after(Screen::Ndi);
    }
}

// ---------- Le dessin ----------

fn s(fg: Color) -> Style {
    Style::new().fg(fg)
}

fn bold(fg: Color) -> Style {
    Style::new().fg(fg).add_modifier(Modifier::BOLD)
}

fn mark(l: Level) -> Span<'static> {
    match l {
        Level::Ok => Span::styled("✓", bold(OK)),
        Level::Warn => Span::styled("!", bold(WARN)),
        Level::Bad => Span::styled("✗", bold(BAD)),
    }
}

/// Un écran : son titre et sa phrase d'explication ; rend la zone qui reste.
fn heading(f: &mut Frame, area: Rect, title: &str, sub: &str) -> Rect {
    let h = if sub.is_empty() { 2 } else { 2 + (sub.chars().count() as u16 / area.width.max(1)) + 2 };
    let [t, rest] = Layout::vertical([Constraint::Length(h), Constraint::Min(1)]).areas(area);
    let mut lines = vec![Line::from(Span::styled(title.to_string(), bold(FG))), Line::from("")];
    if !sub.is_empty() {
        lines.push(Line::from(Span::styled(sub.to_string(), s(DIM))));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), t);
    rest
}

/// Des choix, l'un sous l'autre : le choix en cours est une barre pleine.
fn choices(f: &mut Frame, area: Rect, opts: &[(String, String)], sel: usize) {
    let w = area.width as usize;
    let mut lines = Vec::new();
    for (i, (label, desc)) in opts.iter().enumerate() {
        let on = i == sel;
        let text = format!(" {}  {label}", if on { "▶" } else { " " });
        let pad = w.saturating_sub(text.chars().count());
        if on {
            lines.push(Line::from(Span::styled(format!("{text}{}", " ".repeat(pad)), Style::new().fg(ON_ACCENT).bg(ACCENT).add_modifier(Modifier::BOLD))));
        } else {
            lines.push(Line::from(Span::styled(text, s(FG))));
        }
        if !desc.is_empty() {
            lines.push(Line::from(Span::styled(format!("     {desc}"), s(if on { FG } else { DIM }))));
        }
        lines.push(Line::from(""));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// Un champ : cadre épais et curseur quand on y tape.
fn field(f: &mut Frame, area: Rect, label: &str, input: &Input, focused: bool, placeholder: &str) {
    let area = Rect { height: 3.min(area.height), ..area };
    let b = Block::bordered()
        .border_type(if focused { BorderType::Thick } else { BorderType::Rounded })
        .border_style(s(if focused { ACCENT } else { LINE }))
        .title(Span::styled(format!(" {label} "), if focused { bold(ACCENT) } else { s(DIM) }))
        .style(Style::new().bg(PANEL));
    let mut spans = vec![Span::raw(" ")];
    if input.value.is_empty() && !placeholder.is_empty() {
        if focused {
            spans.push(Span::styled(" ", Style::new().bg(FG)));
        }
        spans.push(Span::styled(placeholder.to_string(), s(LINE)));
    } else {
        spans.push(Span::styled(input.shown(), bold(FG)));
        if focused {
            spans.push(Span::styled(" ", Style::new().bg(FG)));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)).block(b), area);
}

/// Les touches de l'écran, en bas : [Entrée] Continuer …
fn keys(app: &App) -> Vec<(&'static str, &'static str)> {
    let back = ("Échap", "Retour");
    match app.screen {
        Screen::Welcome => vec![("Entrée", "Commencer"), ("Échap", "Quitter")],
        Screen::Menu | Screen::Timezone | Screen::Blackmagic | Screen::Gpu | Screen::Firewall => vec![("↑ ↓", "Choisir"), ("Entrée", "Valider"), back],
        Screen::Ndi if app.ndi_fetching => vec![],
        Screen::Ndi => vec![("↑ ↓", "Choisir"), ("Entrée", "Valider"), back],
        Screen::Summary => vec![("↑ ↓", "Choisir"), ("Entrée", "Valider"), back],
        Screen::Checks => vec![("Entrée", "Continuer"), back],
        Screen::Dns | Screen::Name | Screen::AdminUser => vec![("Entrée", "Continuer"), back],
        Screen::AdminPassword => vec![("Tab", "Champ suivant"), ("Entrée", "Continuer"), back],
        Screen::Apps => vec![("Entrée", "Ajouter / continuer"), ("Suppr", "Retirer la dernière"), back],
        Screen::BlackmagicWait => vec![("P", "Passer, sans pilote Blackmagic"), back],
        Screen::License => vec![("PgPréc PgSuiv", "Lire"), ("↑ ↓", "Choisir"), ("Entrée", "Valider")],
        Screen::Progress if app.result.is_some() => vec![("Entrée", "Continuer")],
        Screen::Progress => vec![],
        Screen::End => vec![("Entrée", "Quitter")],
    }
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(FG)), area);
    let [head, _, body, err, foot] =
        Layout::vertical([Constraint::Length(3), Constraint::Length(1), Constraint::Min(5), Constraint::Length(2), Constraint::Length(2)]).areas(area);

    // En-tête : Strata, l'étape, la barre d'avancement.
    let pos = app
        .flow
        .iter()
        .position(|s| *s == app.screen)
        .or_else(|| (app.screen == Screen::BlackmagicWait).then(|| app.flow.iter().position(|s| *s == Screen::Blackmagic)).flatten());
    let step = match pos {
        Some(i) if app.flow.len() > 1 => format!("Étape {} sur {}", i + 1, app.flow.len()),
        _ => String::new(),
    };
    let what = match app.plan.action {
        Action::Install => "Installation",
        Action::Update => "Mise à jour",
        Action::Repair => "Réparation",
        Action::Uninstall { .. } => "Désinstallation",
    };
    let dry = if app.args.root.is_some() { "  ·  essai à blanc" } else { "" };
    let left = format!("  ▲ Strata   {what}{dry}");
    let pad = (head.width as usize).saturating_sub(left.chars().count() + step.chars().count() + 2);
    let bar_w = head.width.saturating_sub(4) as usize;
    let filled = match pos {
        Some(i) => bar_w * (i + 1) / app.flow.len().max(1),
        None if matches!(app.screen, Screen::Progress | Screen::End) => bar_w,
        None => 0,
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("  ▲ Strata", bold(FG)),
                Span::styled(format!("   {what}{dry}"), s(ACCENT)),
                Span::raw(" ".repeat(pad)),
                Span::styled(step, bold(FG)),
            ]),
            Line::from(""),
            Line::from(vec![Span::raw("  "), Span::styled("━".repeat(filled), s(ACCENT)), Span::styled("━".repeat(bar_w - filled), s(LINE))]),
        ]),
        head,
    );

    // Le contenu, centré.
    let w = body.width.min(WIDTH);
    let body = Rect { x: body.x + (body.width - w) / 2, width: w, ..body }.inner(Margin { horizontal: 2, vertical: 0 });
    let inner = Rect { x: err.x + (err.width - w) / 2, width: w, ..err }.inner(Margin { horizontal: 2, vertical: 0 });
    if let Some(e) = &app.error {
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled("✗ ", bold(BAD)), Span::styled(e.clone(), s(BAD))])).wrap(Wrap { trim: false }), inner);
    }
    let mut spans = vec![Span::raw("  ")];
    for (k, l) in keys(app) {
        spans.push(Span::styled(format!(" {k} "), Style::new().fg(FG).bg(LINE).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {l}     "), s(DIM)));
    }
    f.render_widget(Paragraph::new(vec![Line::from(""), Line::from(spans)]).style(Style::new().bg(PANEL)), foot);

    match app.screen {
        Screen::Welcome => draw_welcome(f, body, app),
        Screen::Menu => {
            let r = heading(
                f,
                body,
                "Strata est déjà installé sur ce serveur",
                &format!("Strata Core {}. Que voulez-vous faire ?", app.installed.clone().unwrap_or_default()),
            );
            choices(f, r, &MENU.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>(), app.sel);
        }
        Screen::Checks => draw_checks(f, body, app),
        Screen::Dns => {
            let r = heading(
                f,
                body,
                "Le nom DNS du serveur",
                "Le cockpit répondra à ce nom, et chaque appli à un sous-domaine (livecut.<nom>). Il faudra deux enregistrements DNS : le nom, et *.le nom.",
            );
            field(f, r, "Nom DNS", &app.dns, true, "strata01.regie.exemple.fr");
        }
        Screen::Name => {
            let r = heading(f, body, "Le nom d'usage", "Le nom que les équipes voient dans le cockpit, par exemple « Régie A ».");
            field(f, r, "Nom d'usage", &app.name, true, "Régie A");
        }
        Screen::Timezone => {
            let r = heading(f, body, "Le fuseau horaire", "Pour l'heure affichée et le timecode maison.");
            let mut opts: Vec<(String, String)> = app.tz_choices.iter().map(|z| (z.clone(), String::new())).collect();
            opts.push(("Autre…".into(), String::new()));
            let compact: Vec<Line> = opts
                .iter()
                .enumerate()
                .map(|(i, (l, _))| {
                    let on = i == app.sel;
                    let text = format!(" {}  {l}", if on { "▶" } else { " " });
                    if on {
                        Line::from(Span::styled(
                            format!("{text}{}", " ".repeat((r.width as usize).saturating_sub(text.chars().count()))),
                            Style::new().fg(ON_ACCENT).bg(ACCENT).add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        Line::from(Span::styled(text, s(FG)))
                    }
                })
                .collect();
            let n = compact.len() as u16;
            let [l, _, o] = Layout::vertical([Constraint::Length(n), Constraint::Length(1), Constraint::Min(1)]).areas(r);
            f.render_widget(Paragraph::new(compact), l);
            if app.sel == app.tz_choices.len() {
                field(f, o, "Fuseau", &app.tz_other, true, "Asia/Tokyo");
            }
        }
        Screen::Apps => {
            let r = heading(
                f,
                body,
                "Les applis",
                "Le dépôt GitHub (owner/repo) ou l'archive de chaque appli à installer et activer. Rien à mettre ? Entrée : elles s'ajoutent aussi plus tard, depuis la Bibliothèque.",
            );
            let [a, _, l] = Layout::vertical([Constraint::Length(3), Constraint::Length(1), Constraint::Min(1)]).areas(r);
            field(f, a, "Ajouter une appli", &app.app_input, true, "DamienP2/strata-cut");
            let mut lines: Vec<Line> =
                app.plan.apps.iter().map(|x| Line::from(vec![Span::styled("  ✓  ", bold(OK)), Span::styled(x.clone(), s(FG))])).collect();
            if lines.is_empty() {
                lines.push(Line::from(Span::styled("  Aucune appli pour l'instant.", s(DIM))));
            }
            f.render_widget(Paragraph::new(lines), l);
        }
        Screen::Blackmagic => {
            let state = match (&app.dv_installed, app.cards) {
                (Some(v), n) => format!("Pilote Desktop Video {v} déjà installé · {n} carte(s)."),
                (None, 0) => "Aucune carte Blackmagic vue sur ce serveur.".into(),
                (None, n) => format!("{n} carte(s) Blackmagic vue(s), pilote Desktop Video absent."),
            };
            let mut sub = format!("{state} Le pilote vient du site de Blackmagic : il faudra le copier sur ce serveur (la marche à suivre s'affiche).");
            if app.secure_boot {
                sub.push_str(" Secure Boot est actif : au redémarrage, enrôlez la clé MOK.");
            }
            let r = heading(f, body, "Le pilote des cartes Blackmagic", &sub);
            choices(
                f,
                r,
                &[("Installer le pilote Desktop Video".into(), "DeckLink, UltraStudio, Intensity".into()), ("Passer".into(), "sans pilote Blackmagic".into())],
                app.sel,
            );
        }
        Screen::BlackmagicWait => draw_wait(f, body, app),
        Screen::Ndi => {
            let r = heading(
                f,
                body,
                "NDI",
                "La bibliothèque NDI, pour recevoir et envoyer des sources NDI. Son SDK est téléchargé, puis sa licence s'affiche : à accepter pour l'installer.",
            );
            if app.ndi_fetching {
                let spin = ["◐", "◓", "◑", "◒"][(app.tick / 3 % 4) as usize];
                f.render_widget(
                    Paragraph::new(Line::from(vec![Span::styled(format!("{spin}  "), bold(ACCENT)), Span::styled("Téléchargement du SDK NDI…", s(FG))])),
                    r,
                );
            } else {
                let [c, e] = Layout::vertical([Constraint::Length(6), Constraint::Min(1)]).areas(r);
                choices(
                    f,
                    c,
                    &[("Installer NDI".into(), "télécharger le SDK, lire et accepter sa licence".into()), ("Passer".into(), "sans NDI".into())],
                    app.sel,
                );
                if let Some(err) = &app.ndi_error {
                    f.render_widget(
                        Paragraph::new(vec![Line::from(Span::styled(format!("✗ {err}"), s(BAD))), Line::from(Span::styled("Réessayer, ou passer.", s(DIM)))])
                            .wrap(Wrap { trim: false }),
                        e,
                    );
                }
            }
        }
        Screen::License => {
            let [text_area, _, ask] = Layout::vertical([Constraint::Min(3), Constraint::Length(1), Constraint::Length(4)]).areas(body);
            let text = app.ndi_sdk.as_ref().map(|s| s.license.clone()).unwrap_or_default();
            let b = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(s(LINE))
                .title(Span::styled(" Licence du SDK NDI ", bold(FG)))
                .style(Style::new().bg(PANEL));
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).scroll((app.license_scroll, 0)).block(b), text_area);
            let opts = [("J'accepte la licence".to_string(), String::new()), ("Je refuse (sans NDI)".to_string(), String::new())];
            let compact: Vec<Line> = opts
                .iter()
                .enumerate()
                .map(|(i, (l, _))| {
                    let on = i == app.sel;
                    let text = format!(" {}  {l}", if on { "▶" } else { " " });
                    if on {
                        Line::from(Span::styled(
                            format!("{text}{}", " ".repeat((ask.width as usize).saturating_sub(text.chars().count()))),
                            Style::new().fg(ON_ACCENT).bg(ACCENT).add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        Line::from(Span::styled(text, s(FG)))
                    }
                })
                .collect();
            f.render_widget(Paragraph::new(compact), ask);
        }
        Screen::Gpu => {
            let need = match (app.gpu_nvidia, app.gpu_vaapi.is_empty()) {
                (true, true) => "le pilote NVIDIA (NVENC / NVDEC)".to_string(),
                (false, false) => format!("VA-API ({})", app.gpu_vaapi.join(", ")),
                (true, false) => format!("le pilote NVIDIA et VA-API ({})", app.gpu_vaapi.join(", ")),
                (false, true) => "rien".to_string(),
            };
            let mut sub = format!("{} : il manque {need} pour encoder et décoder sur le GPU.", app.gpu_names.join(", "));
            if app.secure_boot && app.gpu_nvidia {
                sub.push_str(" Secure Boot est actif : au redémarrage, enrôlez la clé MOK.");
            }
            let r = heading(f, body, "Les pilotes GPU", &sub);
            choices(
                f,
                r,
                &[(format!("Installer {need}"), "un redémarrage sera nécessaire".into()), ("Passer".into(), "les applis encodent avec les processeurs".into())],
                app.sel,
            );
        }
        Screen::Firewall => {
            let r = heading(
                f,
                body,
                "Le pare-feu",
                "Si ufw est actif, le cockpit a besoin des ports 80 (HTTP) et 443 (HTTPS) ; NMOS, de 3212 et de mDNS (5353/udp) ; SNMP, de 161/udp.",
            );
            choices(
                f,
                r,
                &[("Ouvrir les ports 80, 443, 3212, 5353 et 161".into(), "recommandé".into()), ("Ne pas toucher au pare-feu".into(), String::new())],
                app.sel,
            );
        }
        Screen::AdminUser => {
            let r = heading(
                f,
                body,
                "Le compte administrateur",
                "Il règle le serveur depuis le cockpit ; c'est aussi le compte de secours si l'annuaire ne répond plus.",
            );
            field(f, r, "Nom du compte", &app.admin_user, true, "admin");
        }
        Screen::AdminPassword => {
            let r = heading(f, body, &format!("Le mot de passe de « {} »", app.admin_user.value.trim()), "8 caractères au moins.");
            let [a, _, b] = Layout::vertical([Constraint::Length(3), Constraint::Length(1), Constraint::Length(3)]).areas(r);
            field(f, a, "Mot de passe", &app.admin_pw, app.focus == 0, "");
            field(f, b, "Le même, encore", &app.admin_pw2, app.focus == 1, "");
        }
        Screen::Summary => draw_summary(f, body, app),
        Screen::Progress => draw_progress(f, body, app),
        Screen::End => draw_end(f, body, app),
    }
}

fn draw_welcome(f: &mut Frame, body: Rect, app: &App) {
    let what = match (&app.plan.package, &app.plan.version) {
        (Some(p), _) => format!("Strata Core depuis {}", p.display()),
        (None, Some(v)) => format!("Strata Core {v}"),
        (None, None) => "Strata Core, dernière version".into(),
    };
    let mut lines = vec![
        Line::from(Span::styled("Bienvenue", bold(FG))),
        Line::from(""),
        Line::from(Span::styled("Cet assistant installe Strata Core sur ce serveur : le cockpit, les comptes, le matériel, la santé", s(FG))),
        Line::from(Span::styled("et les mises à jour. Une question par écran ; Échap revient en arrière à tout moment.", s(FG))),
        Line::from(""),
        Line::from(vec![Span::styled("À installer     ", s(DIM)), Span::styled(what, bold(FG))]),
    ];
    if let Some(v) = &app.installed {
        lines.push(Line::from(vec![Span::styled("Déjà installé   ", s(DIM)), Span::styled(format!("Strata Core {v}"), bold(OK))]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Appuyez sur Entrée pour commencer.", bold(ACCENT))));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

fn draw_checks(f: &mut Frame, body: Rect, app: &App) {
    let r = heading(f, body, "La vérification du serveur", "Ce qu'il faut à Strata. Un ✗ bloque l'installation ; un ! est à savoir.");
    let mut lines = Vec::new();
    match &app.checks {
        None => {
            let spin = ["◐", "◓", "◑", "◒"][(app.tick / 3 % 4) as usize];
            lines.push(Line::from(vec![Span::styled(format!("{spin}  "), bold(ACCENT)), Span::styled("Vérification…", s(FG))]));
        }
        Some(list) => {
            for c in list {
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    mark(c.level),
                    Span::styled(format!("  {:12}", c.label), bold(FG)),
                    Span::styled(c.detail.clone(), s(if c.level == Level::Ok { DIM } else { FG })),
                ]));
            }
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), r);
}

/// La marche à suivre pour le pilote Blackmagic, en attendant le fichier. Sans cadre : les
/// commandes se copient à la souris telles quelles, chacune sur une ligne.
fn draw_wait(f: &mut Frame, body: Rect, app: &App) {
    let who = std::env::var("SUDO_USER").unwrap_or_else(|_| "root".into());
    let host = checks::local_ips().first().map(|i| i.to_string()).unwrap_or_else(|| "<ce serveur>".into());
    let cmd = |c: String| Line::from(Span::styled(c, bold(ACCENT)));
    let spin = ["◐", "◓", "◑", "◒"][(app.tick / 3 % 4) as usize];
    let lines = vec![
        Line::from(Span::styled("Copiez le pilote Blackmagic sur ce serveur", bold(FG))),
        Line::from(""),
        Line::from(Span::styled("1. Sur votre ordinateur, téléchargez « Desktop Video » pour Linux :", s(FG))),
        Line::from(Span::styled("   blackmagicdesign.com › Support › Capture and Playback (Blackmagic demande un formulaire).", s(DIM))),
        Line::from(""),
        Line::from(Span::styled("2. Copiez-le ici tel quel, depuis un terminal de votre ordinateur :", s(FG))),
        Line::from(""),
        Line::from(Span::styled("   Mac, Linux", s(DIM))),
        cmd(format!("scp ~/Downloads/Blackmagic_Desktop_Video_Linux_* {who}@{host}:")),
        Line::from(""),
        Line::from(Span::styled("   Windows (PowerShell)", s(DIM))),
        cmd(format!("Get-ChildItem $HOME\\Downloads\\Blackmagic_Desktop_Video_Linux_* | % {{ scp $_.FullName {who}@{host}: }}")),
        Line::from(""),
        Line::from(Span::styled("3. Le fichier est trouvé tout seul une fois la copie finie : l'installation continue.", s(FG))),
        Line::from(""),
        Line::from(vec![Span::styled(format!("{spin}  "), bold(WARN)), Span::styled("En attente du fichier…", bold(WARN))]),
    ];
    // Sans retour à la ligne : une commande n'est jamais coupée en deux. Toute la largeur.
    let area = f.area();
    let wide = Rect { x: area.x + 2, width: area.width.saturating_sub(4), ..body };
    f.render_widget(Block::new().style(Style::new().bg(BG)), wide);
    f.render_widget(Paragraph::new(lines), wide);
}

fn draw_summary(f: &mut Frame, body: Rect, app: &App) {
    let p = &app.plan;
    let row = |k: &str, v: String| Line::from(vec![Span::styled(format!("  {k:16}"), s(DIM)), Span::styled(v, s(FG))]);
    let mut lines = Vec::new();
    let (title, go) = match &p.action {
        Action::Install => {
            lines.push(row("Serveur", format!("{} · {}", p.name, p.dns_name)));
            if let Some(c) = &app.dns_check {
                lines.push(Line::from(vec![Span::raw(" ".repeat(18)), mark(c.level), Span::styled(format!(" {}", c.detail), s(DIM))]));
            }
            lines.push(row("Fuseau", p.timezone.clone()));
            lines.push(row("Applis", if p.apps.is_empty() { "aucune pour l'instant".into() } else { p.apps.join(", ") }));
            lines.push(row("Blackmagic", p.desktop_video.as_ref().map_or("—".into(), |d| format!("pilote depuis {}", d.display()))));
            lines.push(row("NDI", if p.ndi.is_some() { "bibliothèque (licence acceptée)".into() } else { "—".into() }));
            let gpu = [p.nvidia.then(|| "pilote NVIDIA".to_string()), (!p.vaapi.is_empty()).then(|| format!("VA-API ({})", p.vaapi.join(", ")))]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            lines.push(row("GPU", if gpu.is_empty() { "—".into() } else { gpu.join(", ") }));
            lines.push(row("Pare-feu", if p.firewall { "80, 443, 3212, 5353 et 161 ouverts (ufw)".into() } else { "inchangé".into() }));
            lines.push(row("Administrateur", p.admin.as_ref().map_or("—".into(), |a| a.0.clone())));
            ("Tout est prêt", "Installer Strata")
        }
        Action::Update => {
            lines.push(row("Mettre à jour", "Strata Core (dernière version)".into()));
            if !p.apps.is_empty() {
                lines.push(row("Applis", p.apps.join(", ")));
            }
            let hw = [
                p.desktop_video.as_ref().map(|_| "pilote Blackmagic"),
                p.ndi.as_ref().map(|_| "NDI"),
                p.nvidia.then_some("pilote NVIDIA"),
                (!p.vaapi.is_empty()).then_some("VA-API"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if !hw.is_empty() {
                lines.push(row("Matériel", hw.join(", ")));
            }
            ("La mise à jour", "Lancer la mise à jour")
        }
        Action::Repair => {
            lines.push(row("Réparer", "fichiers du système, droits, puis relance".into()));
            ("La réparation", "Réparer")
        }
        Action::Uninstall { purge } => {
            lines.push(row("Retirer", "le service, le programme, les fichiers du système".into()));
            if *purge {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:16}", "Effacer"), s(DIM)),
                    Span::styled(format!("les données ({}) : comptes, réglages, applis ; le compte « {} »", paths::DATA, paths::USER), bold(BAD)),
                ]));
            } else {
                lines.push(row("Garder", format!("les données ({})", paths::DATA)));
            }
            lines.push(row("Garder", "les pilotes (Blackmagic, NDI, NVIDIA)".into()));
            if *purge {
                ("Tout effacer", "Tout effacer")
            } else {
                ("Désinstaller", "Désinstaller")
            }
        }
    };
    let r = heading(f, body, title, "Vérifiez, puis lancez.");
    let n = lines.len() as u16 + 2;
    let [l, _, c] = Layout::vertical([Constraint::Length(n), Constraint::Length(1), Constraint::Min(4)]).areas(r);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().border_type(BorderType::Rounded).border_style(s(LINE)).style(Style::new().bg(PANEL))),
        l,
    );
    choices(f, c, &[(go.into(), String::new()), ("Revenir".into(), String::new())], app.sel);
}

fn draw_progress(f: &mut Frame, body: Rect, app: &App) {
    let title = match (app.result, &app.plan.action) {
        (Some(true), _) => "Terminé",
        (Some(false), _) => "Arrêté sur une erreur",
        (None, Action::Uninstall { .. }) => "Désinstallation en cours",
        (None, Action::Update) => "Mise à jour en cours",
        (None, Action::Repair) => "Réparation en cours",
        (None, Action::Install) => "Installation en cours",
    };
    let r = heading(f, body, title, "");
    let spin = ["◐", "◓", "◑", "◒"][(app.tick / 3 % 4) as usize];
    let mut lines = Vec::new();
    for (i, l) in app.labels.iter().enumerate() {
        let (m, detail, style) = match &app.states[i] {
            StepState::Waiting => (Span::styled(" ·", s(LINE)), String::new(), s(DIM)),
            StepState::Running(d) => (Span::styled(format!(" {spin}"), bold(ACCENT)), d.clone(), bold(FG)),
            StepState::Done(d) => (Span::styled(" ✓", bold(OK)), d.clone(), s(FG)),
            StepState::Failed(e) => (Span::styled(" ✗", bold(BAD)), e.clone(), bold(BAD)),
        };
        lines.push(Line::from(vec![m, Span::styled(format!("  {:34}", l), style), Span::styled(detail, s(DIM))]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!("Journal complet : {}", paths::under(&app.root(), paths::LOG).display()), s(DIM))));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), r);
}

fn draw_end(f: &mut Frame, body: Rect, app: &App) {
    let ok = app.result == Some(true);
    let mut lines =
        vec![Line::from(if ok { Span::styled("✓  Terminé", bold(OK)) } else { Span::styled("✗  Arrêté sur une erreur", bold(BAD)) }), Line::from("")];
    if ok && app.plan.action == Action::Install {
        lines.push(Line::from(Span::styled("Le cockpit", s(DIM))));
        lines.push(Line::from(Span::styled(format!("http://{}/", app.plan.dns_name), bold(ACCENT))));
        for ip in checks::local_ips().iter().take(2) {
            lines.push(Line::from(Span::styled(format!("http://{ip}/"), s(FG))));
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
            lines.push(Line::from(Span::styled("Reste à faire", bold(FG))));
            for t in todo {
                lines.push(Line::from(vec![Span::styled("  •  ", bold(WARN)), Span::styled(t, s(FG))]));
            }
        }
    }
    if !ok {
        if let Some(StepState::Failed(e)) = app.states.iter().find(|s| matches!(s, StepState::Failed(_))) {
            lines.push(Line::from(Span::styled(e.clone(), s(BAD))));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!("Journal : {}", paths::under(&app.root(), paths::LOG).display()), s(DIM))));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn render(app: &App, w: u16, h: u16) -> (Vec<String>, ratatui::buffer::Buffer) {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let rows = (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect();
        (rows, buf)
    }

    fn key(app: &mut App, c: KeyCode) {
        app.key(KeyEvent::new(c, KeyModifiers::NONE));
    }

    fn typed(app: &mut App, t: &str) {
        for c in t.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn every_screen_has_a_dark_background() {
        let mut app = new_app(Args::default());
        app.flow = app.install_flow();
        for screen in [Screen::Welcome, Screen::Dns, Screen::Timezone, Screen::Blackmagic, Screen::BlackmagicWait, Screen::Firewall, Screen::Summary] {
            app.screen = screen;
            let (_, buf) = render(&app, 110, 36);
            assert!(buf.content().iter().all(|c| c.bg != Color::Reset), "{screen:?}");
            assert!(buf.content().iter().all(|c| c.fg != Color::Reset), "{screen:?}");
        }
    }

    #[test]
    fn the_selected_choice_is_a_full_bar() {
        let mut app = new_app(Args::default());
        app.flow = vec![Screen::Firewall, Screen::Summary];
        app.screen = Screen::Firewall;
        key(&mut app, KeyCode::Down);
        let (rows, buf) = render(&app, 110, 30);
        let y = rows.iter().position(|r| r.contains("▶  Ne pas toucher")).unwrap();
        let bar = (0..110).filter(|x| buf[(*x, y as u16)].bg == ACCENT).count();
        assert!(bar > 60, "{bar}");
        assert!(rows.iter().any(|r| r.contains("Étape 1 sur 2")));
    }

    #[test]
    fn a_whole_install_step_by_step() {
        let mut app = new_app(Args { root: Some("/tmp/strata-ui-test".into()), ..Args::default() });
        app.cards = 1;
        app.dv_installed = None;
        app.ndi_present = Some("/usr/local/lib/libndi.so".into());
        app.gpu_nvidia = false;
        app.gpu_vaapi = Vec::new();
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Checks);
        app.checks = Some(Vec::new());
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Dns);
        typed(&mut app, "pas un nom");
        key(&mut app, KeyCode::Enter);
        assert!(app.error.is_some());
        app.dns.value = "strata01.regie.lan".into();
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Name);
        assert_eq!(app.name.value, "strata01");
        key(&mut app, KeyCode::Enter);
        assert_eq!((app.screen, app.plan.name.as_str()), (Screen::Timezone, "strata01"));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Apps);
        key(&mut app, KeyCode::Enter);
        // Une carte Blackmagic, NDI déjà là, pas de GPU à équiper : Blackmagic, puis le pare-feu.
        assert_eq!(app.screen, Screen::Blackmagic);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Firewall);
        key(&mut app, KeyCode::Enter);
        assert!(app.plan.firewall);
        assert_eq!(app.screen, Screen::AdminUser);
        key(&mut app, KeyCode::Enter);
        typed(&mut app, "motdepasse");
        key(&mut app, KeyCode::Enter);
        typed(&mut app, "autrechose");
        key(&mut app, KeyCode::Enter);
        assert!(app.error.is_some());
        typed(&mut app, "motdepasse");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Summary);
        assert_eq!(app.plan.admin, Some(("admin".into(), "motdepasse".into())));
        // Échap revient en arrière, écran par écran.
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.screen, Screen::AdminPassword);
    }

    #[test]
    fn the_copy_commands_stay_on_one_line() {
        let mut app = new_app(Args::default());
        app.flow = app.install_flow();
        app.screen = Screen::BlackmagicWait;
        let (rows, _) = render(&app, 130, 36);
        if std::env::var("SHOW").is_ok() {
            println!("{}", rows.join("\n"));
        }
        let line = |start: &str| rows.iter().map(|r| r.trim()).find(|r| r.starts_with(start)).map(str::to_string);
        assert!(line("scp ~/Downloads/Blackmagic_Desktop_Video_Linux_* ").unwrap().ends_with(':'));
        assert!(line("Get-ChildItem $HOME\\Downloads\\Blackmagic_Desktop_Video_Linux_* | % { scp $_.FullName ").unwrap().ends_with(": }"));
        assert!(!rows.iter().any(|r| r.contains('│')));
    }

    #[test]
    fn the_license_is_accepted_or_refused() {
        let mut app = new_app(Args::default());
        app.flow = vec![Screen::Ndi, Screen::Firewall, Screen::Summary];
        app.screen = Screen::Ndi;
        app.ndi_sdk = Some(ndi::Sdk { lib: "/tmp/libndi.so.6".into(), license: "NDI SDK License Agreement\nTerms.".into() });
        app.go(Screen::License);
        let (rows, _) = render(&app, 120, 30);
        if std::env::var("SHOW").is_ok() {
            println!("{}", rows.join("\n"));
        }
        let text = rows.join("\n");
        assert!(text.contains("NDI SDK License Agreement") && text.contains("▶  J'accepte la licence") && text.contains("Je refuse"));
        key(&mut app, KeyCode::Enter);
        assert_eq!((app.screen, app.plan.ndi.is_some()), (Screen::Firewall, true));
    }

    /// Des captures en HTML (couleurs comprises) : `SNAP_DIR=… cargo test -p strata-install snapshots -- --ignored`.
    #[test]
    #[ignore]
    fn snapshots() {
        let dir = PathBuf::from(std::env::var("SNAP_DIR").unwrap());
        let mut app = new_app(Args::default());
        app.flow = app.install_flow();
        if !app.flow.contains(&Screen::Blackmagic) {
            app.flow.insert(5, Screen::Blackmagic);
        }
        app.installed = Some("0.3.6".into());
        app.dns.value = "strata01.regie.lan".into();
        app.plan.dns_name = "strata01.regie.lan".into();
        app.plan.name = "Régie A".into();
        app.plan.apps = vec!["DamienP2/strata-cut".into()];
        app.plan.admin = Some(("admin".into(), "x".into()));
        app.checks = Some(checks::system(true));
        app.cards = 2;
        let color = |c: Color| match c {
            Color::Indexed(i) => {
                let (r, g, b) = match i {
                    16..=231 => {
                        let v = |x: u8| if x == 0 { 0 } else { 55 + 40 * x };
                        let i = i - 16;
                        (v(i / 36), v(i / 6 % 6), v(i % 6))
                    }
                    232..=255 => {
                        let g = 8 + 10 * (i - 232);
                        (g, g, g)
                    }
                    _ => (0, 0, 0),
                };
                format!("#{r:02x}{g:02x}{b:02x}")
            }
            _ => "#121212".into(),
        };
        for (name, screen, sel) in [
            ("1-welcome", Screen::Welcome, 0),
            ("2-checks", Screen::Checks, 0),
            ("3-dns", Screen::Dns, 0),
            ("4-timezone", Screen::Timezone, 1),
            ("5-blackmagic", Screen::Blackmagic, 0),
            ("6-wait", Screen::BlackmagicWait, 0),
            ("7-firewall", Screen::Firewall, 0),
            ("8-summary", Screen::Summary, 0),
            ("9-menu", Screen::Menu, 2),
        ] {
            app.screen = screen;
            app.sel = sel;
            let (_, buf) = render(&app, 120, 34);
            let mut html = String::from("<html><body style='margin:0;background:#000'><pre style='margin:0;font:15px/1.25 DejaVu Sans Mono,monospace'>");
            for y in 0..buf.area.height {
                for x in 0..buf.area.width {
                    let c = &buf[(x, y)];
                    let sym = c.symbol().replace('&', "&amp;").replace('<', "&lt;");
                    let w = if c.modifier.contains(Modifier::BOLD) { "font-weight:bold;" } else { "" };
                    html.push_str(&format!("<span style='color:{};background:{};{w}'>{sym}</span>", color(c.fg), color(c.bg)));
                }
                html.push('\n');
            }
            html.push_str("</pre></body></html>");
            std::fs::write(dir.join(format!("{name}.html")), html).unwrap();
        }
    }
}
