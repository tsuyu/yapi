use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Color32, RichText};

use crate::chain::{self, ChainMsg};
use crate::model::{
    ApiKeyIn, AuthKind, BodyKind, Chain, ChainStep, ClientAuth, Collection, Extract, ExtractFrom,
    FormPart, Grant, KeyVal, Method, RequestSpec,
};
use crate::oauth::{self, TokenMsg};
use crate::net::{self, Msg, ResponseData, SendOpts, Shape};
use crate::store::{self, Session};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Params,
    Path,
    Headers,
    Cookies,
    Auth,
    Body,
    Extract,
    Options,
}

/// The sidebar shows requests or chains; the centre follows.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    Request,
    Chain,
}

/// One line of a chain run, as it happened.
struct ChainLog {
    label: String,
    detail: String,
    color: Color32,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum RespTab {
    Body,
    Headers,
    Cookies,
}

/// How the response body is rendered. Which of these are offered depends on
/// what the body turned out to be.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum BodyView {
    Pretty,
    Raw,
    Tree,
    Text,
    Image,
    Hex,
}

impl BodyView {
    fn as_str(self) -> &'static str {
        match self {
            BodyView::Pretty => "pretty",
            BodyView::Raw => "raw",
            BodyView::Tree => "tree",
            BodyView::Text => "text",
            BodyView::Image => "image",
            BodyView::Hex => "hex",
        }
    }
}

/// Views on offer for a given body shape, best first.
fn views_for(shape: Shape) -> Vec<BodyView> {
    match shape {
        Shape::Json => vec![BodyView::Pretty, BodyView::Tree, BodyView::Raw],
        Shape::Xml => vec![BodyView::Pretty, BodyView::Raw],
        Shape::Html => vec![BodyView::Pretty, BodyView::Text, BodyView::Raw],
        Shape::Text => vec![BodyView::Raw],
        Shape::Image => vec![BodyView::Image, BodyView::Hex],
        Shape::Binary => vec![BodyView::Hex],
    }
}

/// How long the draft has to sit unchanged before it is written to disk.
const DRAFT_AUTOSAVE_DELAY: Duration = Duration::from_millis(800);

pub struct YapiApp {
    coll: Collection,
    path: PathBuf,
    session_path: PathBuf,
    draft_on_disk: RequestSpec,
    draft_dirty_since: Option<Instant>,
    selected: Option<usize>,
    cur: RequestSpec,
    tab: Tab,
    resp_tab: RespTab,
    body_view: BodyView,
    search: String,
    search_case: bool,
    search_only_matches: bool,
    jsonpath: String,
    jsonpath_result: Option<Result<String, String>>,
    /// Decoded image for the current response, rebuilt when the response changes.
    image: Option<(egui::TextureHandle, [usize; 2])>,
    resp_seq: u64,
    wrap: bool,
    show_secrets: bool,
    timeout_secs: u64,
    insecure_tls: bool,
    inflight: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    resp: Option<ResponseData>,
    error: Option<String>,
    toast: String,
    token_tx: Sender<TokenMsg>,
    token_rx: Receiver<TokenMsg>,
    token_inflight: bool,
    /// Variables available for `{{name}}` substitution right now.
    vars: BTreeMap<String, String>,
    mode: Mode,
    selected_chain: Option<usize>,
    chain_tx: Sender<ChainMsg>,
    chain_rx: Receiver<ChainMsg>,
    chain_running: bool,
    chain_log: Vec<ChainLog>,
    new_var: String,
    curl_window: bool,
    curl_import_window: bool,
    curl_import_text: String,
    curl_import_error: String,
    /// Substitute `{{vars}}` when writing curl out.
    curl_resolve_vars: bool,
    oauth_status: String,
    oauth_raw: Option<String>,
    jwt_error: String,
}

impl YapiApp {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let path = store::default_path();
        let coll = store::load(&path);
        let session_path = store::session_path();

        // pick up exactly where the last run left off, unsaved edits included
        let session = store::load_session(&session_path);
        let session_vars = session.as_ref().map(|s| s.vars.clone());
        let (cur, selected, timeout_secs, insecure_tls) = match session {
            Some(s) => {
                let selected = s.selected.filter(|i| *i < coll.requests.len());
                (s.draft, selected, s.timeout_secs, s.insecure_tls)
            }
            None => {
                let cur = coll.requests.first().cloned().unwrap_or_default();
                let selected = if coll.requests.is_empty() {
                    None
                } else {
                    Some(0)
                };
                (cur, selected, 30, false)
            }
        };

        let (tx, rx) = channel();
        let (token_tx, token_rx) = channel();
        let (chain_tx, chain_rx) = channel();
        let defaults: BTreeMap<String, String> = coll
            .variables
            .iter()
            .filter(|v| v.active())
            .map(|v| (v.key.trim().to_owned(), v.value.clone()))
            .collect();
        // anything extracted before the app closed is still useful now
        let vars = match &session_vars {
            Some(saved) if !saved.is_empty() => {
                let mut merged = defaults;
                merged.extend(saved.clone());
                merged
            }
            _ => defaults,
        };
        Self {
            token_tx,
            token_rx,
            token_inflight: false,
            vars,
            mode: Mode::Request,
            selected_chain: None,
            chain_tx,
            chain_rx,
            chain_running: false,
            chain_log: Vec::new(),
            new_var: String::new(),
            curl_window: false,
            curl_import_window: false,
            curl_import_text: String::new(),
            curl_import_error: String::new(),
            curl_resolve_vars: false,
            oauth_status: String::new(),
            oauth_raw: None,
            jwt_error: String::new(),
            draft_on_disk: cur.clone(),
            draft_dirty_since: None,
            session_path,
            coll,
            path,
            selected,
            cur,
            tab: Tab::Params,
            resp_tab: RespTab::Body,
            body_view: BodyView::Pretty,
            search: String::new(),
            search_case: false,
            search_only_matches: false,
            jsonpath: String::new(),
            jsonpath_result: None,
            image: None,
            resp_seq: 0,
            wrap: true,
            show_secrets: false,
            timeout_secs,
            insecure_tls,
            inflight: false,
            tx,
            rx,
            resp: None,
            error: None,
            toast: String::new(),
        }
    }

    /// Write the draft (url, params, headers, auth, body) + ui state to disk.
    fn persist_session(&mut self) {
        let session = Session {
            draft: self.cur.clone(),
            selected: self.selected,
            timeout_secs: self.timeout_secs,
            insecure_tls: self.insecure_tls,
            vars: self.vars.clone(),
        };
        if store::save_session(&self.session_path, &session).is_ok() {
            self.draft_on_disk = self.cur.clone();
            self.draft_dirty_since = None;
        }
    }

    /// Debounced autosave: track edits, flush once typing pauses.
    fn autosave_draft(&mut self, ctx: &egui::Context) {
        if self.cur != self.draft_on_disk {
            let since = *self.draft_dirty_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= DRAFT_AUTOSAVE_DELAY {
                self.persist_session();
            } else {
                ctx.request_repaint_after(DRAFT_AUTOSAVE_DELAY);
            }
        }
    }

    /// This request as a curl command line.
    fn as_curl(&self) -> String {
        let spec = if self.curl_resolve_vars {
            self.cur.resolve(&self.vars)
        } else {
            self.cur.clone()
        };
        crate::curl::generate(&spec, self.insecure_tls, Some(self.timeout_secs))
    }

    fn curl_windows(&mut self, ctx: &egui::Context) {
        let mut open = self.curl_window;
        egui::Window::new("curl")
            .open(&mut open)
            .resizable(true)
            .default_width(640.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.curl_resolve_vars, "substitute {{variables}}");
                    if ui.button("copy").clicked() {
                        let command = self.as_curl();
                        ui.ctx().copy_text(command);
                        self.toast = "copied as curl".to_owned();
                    }
                });
                ui.separator();
                let command = self.as_curl();
                let mut text = command.as_str();
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
            });
        self.curl_window = open;

        let mut import_open = self.curl_import_window;
        let mut do_import = false;
        let mut do_import_send = false;
        egui::Window::new("import curl")
            .open(&mut import_open)
            .resizable(true)
            .default_width(640.0)
            .show(ctx, |ui| {
                // catch a paste even when the box is not focused, so the window
                // fills in however the user pastes
                let pasted: Option<String> = ui.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Paste(text) => Some(text.clone()),
                        _ => None,
                    })
                });
                if let Some(text) = pasted {
                    if !self.curl_import_text.contains(text.trim()) {
                        self.curl_import_text = text;
                    }
                }

                ui.weak("paste a curl command (ctrl+v), then import - line continuations and quotes are fine");
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.curl_import_text)
                                .code_editor()
                                .desired_rows(8)
                                .desired_width(f32::INFINITY)
                                .hint_text("curl -X POST https://api.example.com/users -H ..."),
                        );
                    });
                ui.horizontal(|ui| {
                    let has_text = !self.curl_import_text.trim().is_empty();
                    if ui
                        .add_enabled(has_text, egui::Button::new("import"))
                        .on_hover_text("load it into the editor to review before sending")
                        .clicked()
                    {
                        do_import = true;
                    }
                    if ui
                        .add_enabled(has_text, egui::Button::new("import & send"))
                        .clicked()
                    {
                        do_import_send = true;
                    }
                    if ui.button("clear").clicked() {
                        self.curl_import_text.clear();
                        self.curl_import_error.clear();
                    }
                });
                if !self.curl_import_error.is_empty() {
                    ui.colored_label(
                        Color32::from_rgb(0xef, 0x53, 0x50),
                        self.curl_import_error.clone(),
                    );
                }
            });
        self.curl_import_window = import_open;

        if do_import || do_import_send {
            let ok = self.import_curl();
            if ok && do_import_send {
                self.send(ctx);
            }
        }
    }

    /// Returns whether the import succeeded.
    fn import_curl(&mut self) -> bool {
        match crate::curl::parse_full(&self.curl_import_text) {
            Ok(imported) => {
                self.cur = imported.spec;
                self.selected = None;
                self.resp = None;
                self.error = None;
                self.mode = Mode::Request;
                self.tab = Tab::Params;
                if imported.insecure_tls {
                    self.insecure_tls = true;
                }
                if let Some(secs) = imported.timeout_secs {
                    self.timeout_secs = secs;
                }
                self.toast = if imported.ignored.is_empty() {
                    "imported curl command".to_owned()
                } else {
                    format!("imported; ignored {}", imported.ignored.join(", "))
                };
                self.curl_import_window = false;
                self.curl_import_text.clear();
                self.curl_import_error.clear();
                self.persist_session();
                true
            }
            Err(e) => {
                self.curl_import_error = e;
                false
            }
        }
    }

    /// Something about the credential that will bite on send.
    fn credential_warning(&self) -> Option<String> {
        match self.cur.auth.kind {
            AuthKind::OAuth2 => {
                let o = &self.cur.auth.oauth;
                if o.access_token.trim().is_empty() {
                    Some("oauth: no token yet".to_owned())
                } else if o.expired() {
                    Some("oauth token expired - hit refresh".to_owned())
                } else {
                    None
                }
            }
            AuthKind::Jwt => crate::jwt::decode(&self.cur.auth.jwt.token)
                .ok()
                .filter(|d| d.expired())
                .map(|_| "jwt expired".to_owned()),
            _ => None,
        }
    }

    /// Does the open request differ from its saved entry?
    fn unsaved(&self) -> bool {
        match self.selected {
            Some(i) => self.coll.requests.get(i) != Some(&self.cur),
            None => true,
        }
    }

    fn persist(&mut self) {
        match store::save(&self.path, &self.coll) {
            Ok(()) => self.toast = format!("saved -> {}", self.path.display()),
            Err(e) => self.toast = format!("save failed: {e}"),
        }
    }

    fn save_current(&mut self) {
        if self.cur.name.trim().is_empty() {
            self.cur.name = "unnamed".to_owned();
        }
        match self.selected {
            Some(i) if i < self.coll.requests.len() => self.coll.requests[i] = self.cur.clone(),
            _ => {
                self.coll.requests.push(self.cur.clone());
                self.selected = Some(self.coll.requests.len() - 1);
            }
        }
        self.persist();
        self.persist_session();
    }

    fn send(&mut self, ctx: &egui::Context) {
        if self.inflight {
            return;
        }
        // a whole curl command in the url bar is a paste in the wrong box - parse
        // it instead of firing a request at a mashed-up host
        if crate::curl::looks_like_curl(&self.cur.url) {
            self.curl_import_text = self.cur.url.clone();
            if self.import_curl() {
                self.toast = "parsed curl from the url field".to_owned();
            } else {
                self.error = Some(format!(
                    "that looks like a curl command, not a url - {}",
                    self.curl_import_error
                ));
                return;
            }
        }
        self.inflight = true;
        self.error = None;
        self.toast.clear();
        let resolved = self.cur.resolve(&self.vars);
        net::spawn(resolved, self.send_opts(), self.tx.clone(), ctx.clone());
    }

    fn send_opts(&self) -> SendOpts {
        SendOpts {
            timeout_secs: self.timeout_secs,
            insecure_tls: self.insecure_tls,
        }
    }

    fn drain_chain(&mut self) {
        while let Ok(msg) = self.chain_rx.try_recv() {
            match msg {
                ChainMsg::Started { index, name } => self.chain_log.push(ChainLog {
                    label: format!("{}. {name}", index + 1),
                    detail: "running...".to_owned(),
                    color: Color32::GRAY,
                }),
                ChainMsg::Done {
                    index,
                    name,
                    status,
                    elapsed_ms,
                    extracted,
                    warnings,
                } => {
                    for (k, v) in &extracted {
                        self.vars.insert(k.clone(), v.clone());
                    }
                    let mut detail = format!("{status} in {elapsed_ms} ms");
                    if !extracted.is_empty() {
                        let names: Vec<&str> =
                            extracted.iter().map(|(k, _)| k.as_str()).collect();
                        detail.push_str(&format!("  ->  {}", names.join(", ")));
                    }
                    if !warnings.is_empty() {
                        detail.push_str(&format!("  ({})", warnings.join("; ")));
                    }
                    let color = if status < 400 {
                        Color32::from_rgb(0x4c, 0xaf, 0x50)
                    } else {
                        Color32::from_rgb(0xef, 0x53, 0x50)
                    };
                    self.replace_log(index, &name, detail, color);
                }
                ChainMsg::Failed { index, name, error } => {
                    self.replace_log(index, &name, error, Color32::from_rgb(0xef, 0x53, 0x50));
                }
                ChainMsg::Finished { ran, ok } => {
                    self.chain_running = false;
                    self.chain_log.push(ChainLog {
                        label: if ok { "done".to_owned() } else { "stopped".to_owned() },
                        detail: format!("{ran} step(s)"),
                        color: if ok {
                            Color32::from_rgb(0x4c, 0xaf, 0x50)
                        } else {
                            Color32::from_rgb(0xff, 0xa7, 0x26)
                        },
                    });
                }
            }
        }
    }

    /// Overwrite the "running..." line for a step with its outcome.
    fn replace_log(&mut self, index: usize, name: &str, detail: String, color: Color32) {
        let label = format!("{}. {name}", index + 1);
        match self.chain_log.iter_mut().rev().find(|l| l.label == label) {
            Some(line) => {
                line.detail = detail;
                line.color = color;
            }
            None => self.chain_log.push(ChainLog {
                label,
                detail,
                color,
            }),
        }
    }

    fn run_chain(&mut self, ctx: &egui::Context) {
        let Some(chain) = self.selected_chain.and_then(|i| self.coll.chains.get(i)) else {
            return;
        };
        let mut steps = Vec::new();
        let mut missing = Vec::new();
        for step in chain.steps.iter().filter(|s| s.on) {
            match self
                .coll
                .requests
                .iter()
                .find(|r| r.name == step.request)
            {
                Some(spec) => steps.push(chain::Step {
                    spec: spec.clone(),
                    keep_going: step.keep_going,
                }),
                None => missing.push(step.request.clone()),
            }
        }
        if !missing.is_empty() {
            self.chain_log = vec![ChainLog {
                label: "cannot run".to_owned(),
                detail: format!("no saved request named {}", missing.join(", ")),
                color: Color32::from_rgb(0xef, 0x53, 0x50),
            }];
            return;
        }
        if steps.is_empty() {
            self.chain_log = vec![ChainLog {
                label: "cannot run".to_owned(),
                detail: "this chain has no enabled steps".to_owned(),
                color: Color32::from_rgb(0xff, 0xa7, 0x26),
            }];
            return;
        }

        self.chain_log.clear();
        self.chain_running = true;
        chain::spawn_run(
            steps,
            self.vars.clone(),
            self.send_opts(),
            self.chain_tx.clone(),
            ctx.clone(),
        );
    }

    fn drain(&mut self) {
        self.drain_chain();
        while let Ok(msg) = self.token_rx.try_recv() {
            match msg {
                TokenMsg::Status(line) => self.oauth_status = line,
                TokenMsg::Ok(token) => {
                    self.token_inflight = false;
                    let oauth = &mut self.cur.auth.oauth;
                    oauth.access_token = token.access_token;
                    if !token.refresh_token.is_empty() {
                        oauth.refresh_token = token.refresh_token;
                    }
                    oauth.token_type = token.token_type;
                    oauth.expires_at = token.expires_at;
                    self.oauth_raw = Some(token.raw);
                    self.oauth_status = "token acquired".to_owned();
                }
                TokenMsg::Failed(e) => {
                    self.token_inflight = false;
                    self.oauth_status = e;
                }
            }
        }

        while let Ok(msg) = self.rx.try_recv() {
            self.inflight = false;
            match msg {
                Msg::Done(r) => {
                    // whatever this response teaches us is available to the next request
                    let out = crate::extract::apply(&self.cur.extract, &r);
                    if !out.values.is_empty() {
                        let names: Vec<&str> =
                            out.values.iter().map(|(k, _)| k.as_str()).collect();
                        self.toast = format!("extracted {}", names.join(", "));
                        for (k, v) in out.values {
                            self.vars.insert(k, v);
                        }
                    }
                    if !out.errors.is_empty() {
                        self.toast = format!("extract: {}", out.errors.join("; "));
                    }
                    self.body_view = views_for(r.shape)[0];
                    self.resp = Some(*r);
                    self.resp_tab = RespTab::Body;
                    self.jsonpath_result = None;
                    self.image = None;
                    self.resp_seq += 1;
                }
                Msg::Failed(e) => {
                    self.resp = None;
                    self.error = Some(e);
                }
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.mode, Mode::Request, "requests");
            ui.selectable_value(&mut self.mode, Mode::Chain, "chains");
        });
        ui.separator();

        if self.mode == Mode::Chain {
            self.chain_sidebar(ui);
            return;
        }

        ui.horizontal(|ui| {
            if ui.button("new").clicked() {
                self.cur = RequestSpec::default();
                self.selected = None;
                self.resp = None;
                self.error = None;
            }
            if ui.button("save").clicked() {
                self.save_current();
            }
            if ui
                .button("copy")
                .on_hover_text("duplicate as a new entry")
                .clicked()
            {
                self.cur.name = format!("{} copy", self.cur.name);
                self.coll.requests.push(self.cur.clone());
                self.selected = Some(self.coll.requests.len() - 1);
                self.persist();
            }
        });

        ui.separator();

        let mut delete: Option<usize> = None;
        let list_height = (ui.available_height() - 64.0).max(80.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(list_height)
            .show(ui, |ui| {
                for i in 0..self.coll.requests.len() {
                    let r = &self.coll.requests[i];
                    let label = format!("{}  {}", r.method.as_str(), r.name);
                    let color = r.method.color();
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(
                                self.selected == Some(i),
                                RichText::new(label).color(color),
                            )
                            .clicked()
                        {
                            self.selected = Some(i);
                            self.cur = self.coll.requests[i].clone();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("x").clicked() {
                                delete = Some(i);
                            }
                        });
                    });
                }
                if self.coll.requests.is_empty() {
                    ui.weak("nothing saved yet");
                }
            });

        if let Some(i) = delete {
            self.coll.requests.remove(i);
            self.selected = match self.selected {
                Some(s) if s == i => None,
                Some(s) if s > i => Some(s - 1),
                other => other,
            };
            self.persist();
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("import").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("json", &["json"])
                    .pick_file()
                {
                    let imported = store::load(&p);
                    let n = imported.requests.len();
                    self.coll.requests.extend(imported.requests);
                    self.persist();
                    self.toast = format!("imported {n} request(s)");
                }
            }
            if ui.button("export").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("json", &["json"])
                    .set_file_name("yAPI-collection.json")
                    .save_file()
                {
                    match store::save(&p, &self.coll) {
                        Ok(()) => self.toast = format!("exported -> {}", p.display()),
                        Err(e) => self.toast = format!("export failed: {e}"),
                    }
                }
            }
        });
        ui.weak(RichText::new(self.path.display().to_string()).size(9.0));
        ui.separator();
        self.variables_panel(ui);
    }

    /// Current `{{name}}` values: collection defaults plus anything extracted.
    fn variables_panel(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(format!("variables ({})", self.vars.len()))
            .default_open(true)
            .show(ui, |ui| {
                let mut remove: Option<String> = None;
                egui::ScrollArea::vertical()
                    .id_salt("vars_scroll")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        egui::Grid::new("vars_grid")
                            .num_columns(3)
                            .striped(true)
                            .spacing([6.0, 3.0])
                            .show(ui, |ui| {
                                for (name, value) in self.vars.iter_mut() {
                                    ui.label(RichText::new(name).strong());
                                    ui.add(
                                        egui::TextEdit::singleline(value)
                                            .desired_width(110.0)
                                            .font(egui::TextStyle::Monospace),
                                    );
                                    if ui.small_button("x").clicked() {
                                        remove = Some(name.clone());
                                    }
                                    ui.end_row();
                                }
                            });
                        if self.vars.is_empty() {
                            ui.weak("none yet - extract some, or add one below");
                        }
                    });
                if let Some(name) = remove {
                    self.vars.remove(&name);
                }

                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_var)
                            .desired_width(110.0)
                            .hint_text("new name"),
                    );
                    if ui.small_button("+").clicked() && !self.new_var.trim().is_empty() {
                        self.vars
                            .insert(self.new_var.trim().to_owned(), String::new());
                        self.new_var.clear();
                    }
                });
                ui.horizontal(|ui| {
                    if ui
                        .small_button("save as defaults")
                        .on_hover_text("store these in the collection file")
                        .clicked()
                    {
                        self.coll.variables = self
                            .vars
                            .iter()
                            .map(|(k, v)| KeyVal {
                                on: true,
                                key: k.clone(),
                                value: v.clone(),
                            })
                            .collect();
                        self.persist();
                    }
                    if ui
                        .small_button("reset")
                        .on_hover_text("back to the saved defaults")
                        .clicked()
                    {
                        self.vars = self
                            .coll
                            .variables
                            .iter()
                            .filter(|v| v.active())
                            .map(|v| (v.key.trim().to_owned(), v.value.clone()))
                            .collect();
                    }
                });
            });
    }

    fn chain_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("new chain").clicked() {
                self.coll.chains.push(Chain::default());
                self.selected_chain = Some(self.coll.chains.len() - 1);
                self.persist();
            }
        });
        ui.separator();

        let mut delete = None;
        egui::ScrollArea::vertical()
            .id_salt("chains_scroll")
            .max_height(ui.available_height() - 190.0)
            .show(ui, |ui| {
                for i in 0..self.coll.chains.len() {
                    let label = format!(
                        "{}  ({} steps)",
                        self.coll.chains[i].name,
                        self.coll.chains[i].steps.len()
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(self.selected_chain == Some(i), label)
                            .clicked()
                        {
                            self.selected_chain = Some(i);
                            self.chain_log.clear();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("x").clicked() {
                                delete = Some(i);
                            }
                        });
                    });
                }
                if self.coll.chains.is_empty() {
                    ui.weak("no chains yet");
                }
            });
        if let Some(i) = delete {
            self.coll.chains.remove(i);
            self.selected_chain = None;
            self.persist();
        }

        ui.separator();
        self.variables_panel(ui);
    }

    /// Centre panel when a chain is selected: steps, run button, run log.
    fn chain_editor(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(index) = self.selected_chain else {
            ui.add_space(8.0);
            ui.weak("pick a chain on the left, or make one");
            return;
        };
        if index >= self.coll.chains.len() {
            self.selected_chain = None;
            return;
        }

        let request_names: Vec<String> =
            self.coll.requests.iter().map(|r| r.name.clone()).collect();
        let mut dirty = false;
        let mut run_now = false;

        ui.horizontal(|ui| {
            ui.label("chain");
            let chain = &mut self.coll.chains[index];
            if ui
                .add(egui::TextEdit::singleline(&mut chain.name).desired_width(240.0))
                .changed()
            {
                dirty = true;
            }
            if ui
                .add_enabled(!self.chain_running, egui::Button::new("run chain"))
                .clicked()
            {
                run_now = true;
            }
            if self.chain_running {
                ui.spinner();
            }
            if ui.button("save").clicked() {
                dirty = true;
            }
        });
        ui.weak("each step runs in order; values extracted by one step feed the next");
        ui.separator();

        let mut remove: Option<usize> = None;
        let mut move_up: Option<usize> = None;
        {
            let chain = &mut self.coll.chains[index];
            egui::Grid::new("chain_steps")
                .num_columns(5)
                .striped(true)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for (i, step) in chain.steps.iter_mut().enumerate() {
                        dirty |= ui.checkbox(&mut step.on, "").changed();
                        ui.label(format!("{}.", i + 1));
                        egui::ComboBox::from_id_salt(("chain_step", i))
                            .width(240.0)
                            .selected_text(if step.request.is_empty() {
                                "pick a request".to_owned()
                            } else {
                                step.request.clone()
                            })
                            .show_ui(ui, |ui| {
                                for name in &request_names {
                                    if ui
                                        .selectable_label(&step.request == name, name)
                                        .clicked()
                                    {
                                        step.request = name.clone();
                                        dirty = true;
                                    }
                                }
                            });
                        dirty |= ui
                            .checkbox(&mut step.keep_going, "keep going on failure")
                            .changed();
                        ui.horizontal(|ui| {
                            if i > 0 && ui.small_button("up").clicked() {
                                move_up = Some(i);
                            }
                            if ui.small_button("x").clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.end_row();
                    }
                });

            if let Some(i) = remove {
                chain.steps.remove(i);
                dirty = true;
            }
            if let Some(i) = move_up {
                chain.steps.swap(i - 1, i);
                dirty = true;
            }
            if ui.button("+ step").clicked() {
                chain.steps.push(ChainStep::default());
                dirty = true;
            }
        }

        if dirty {
            self.persist();
        }
        if run_now {
            self.run_chain(ctx);
        }

        ui.add_space(10.0);
        ui.separator();
        ui.strong("run log");
        egui::ScrollArea::vertical()
            .id_salt("chain_log")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.chain_log.is_empty() {
                    ui.weak("not run yet");
                }
                for line in &self.chain_log {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(line.color, &line.label);
                        ui.weak(&line.detail);
                    });
                }
            });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("name");
            ui.add(
                egui::TextEdit::singleline(&mut self.cur.name)
                    .desired_width(240.0)
                    .hint_text("request name"),
            );
            if let Some(warning) = self.credential_warning() {
                ui.colored_label(Color32::from_rgb(0xff, 0xa7, 0x26), warning)
                    .on_hover_text("check the auth tab before sending");
                ui.separator();
            }
            if self.unsaved() {
                ui.colored_label(
                    Color32::from_rgb(0xff, 0xa7, 0x26),
                    match self.selected {
                        Some(_) => "unsaved edits",
                        None => "not in collection",
                    },
                )
                .on_hover_text("ctrl+s to add it to the sidebar - the draft itself is autosaved");
            }
            ui.separator();
            ui.label("timeout");
            ui.add(
                egui::DragValue::new(&mut self.timeout_secs)
                    .range(1..=600)
                    .suffix(" s"),
            );
            ui.separator();
            if ui
                .button("import curl")
                .on_hover_text("paste a curl command and turn it into this request")
                .clicked()
            {
                self.curl_import_window = true;
                self.curl_import_error.clear();
            }
            if ui
                .button("copy as curl")
                .on_hover_text("copy this request as a curl command line")
                .clicked()
            {
                let command = self.as_curl();
                ui.ctx().copy_text(command);
                self.toast = "copied as curl".to_owned();
            }
            if ui.button("show curl").clicked() {
                self.curl_window = true;
            }
            ui.separator();
            ui.checkbox(&mut self.insecure_tls, "insecure tls")
                .on_hover_text(
                    "skip certificate checks - for a local https dev server with a                      self-signed cert. Leave it off for anything you do not control.",
                );
            if self.insecure_tls {
                ui.colored_label(Color32::from_rgb(0xef, 0x53, 0x50), "certs unchecked");
            }
        });

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("method")
                .width(96.0)
                .selected_text(
                    RichText::new(self.cur.method.as_str())
                        .color(self.cur.method.color())
                        .strong(),
                )
                .show_ui(ui, |ui| {
                    for m in Method::ALL {
                        ui.selectable_value(
                            &mut self.cur.method,
                            m,
                            RichText::new(m.as_str()).color(m.color()),
                        );
                    }
                });

            if ui
                .add_enabled(!self.inflight, egui::Button::new("send"))
                .clicked()
            {
                self.send(ctx);
            }

            ui.add(
                egui::TextEdit::singleline(&mut self.cur.url)
                    .desired_width(ui.available_width())
                    .hint_text("api.example.com/v1/things  or  localhost:3000/api"),
            );
        });

        ui.horizontal(|ui| {
            ui.weak("localhost:");
            for port in [3000, 5000, 8000, 8080] {
                if ui
                    .small_button(port.to_string())
                    .on_hover_text(format!("point the url at http://localhost:{port}"))
                    .clicked()
                {
                    self.cur.url = swap_to_localhost(&self.cur.url, port);
                }
            }
            // caught a curl command in the url field: offer to parse it
            if crate::curl::looks_like_curl(&self.cur.url) {
                ui.separator();
                ui.colored_label(Color32::from_rgb(0xff, 0xa7, 0x26), "that's a curl command");
                if ui.small_button("parse it").clicked() {
                    self.curl_import_text = self.cur.url.clone();
                    self.import_curl();
                }
            }
        });

        let resolved = self.cur.resolve(&self.vars);
        ui.horizontal_wrapped(|ui| {
            ui.weak(RichText::new(resolved.full_url()).size(10.0));
            let missing = self.cur.missing_vars(&self.vars);
            if !missing.is_empty() {
                ui.colored_label(
                    Color32::from_rgb(0xff, 0xa7, 0x26),
                    RichText::new(format!("unset: {}", missing.join(", "))).size(10.0),
                )
                .on_hover_text("set these in the variables panel, or extract them from an earlier request");
            }
        });
        ui.add_space(4.0);
    }

    fn request_editor(&mut self, ui: &mut egui::Ui) {
        let n_params = self.cur.params.iter().filter(|p| p.active()).count();
        let n_headers = self.cur.headers.iter().filter(|h| h.active()).count();
        let n_cookies = self.cur.cookies.iter().filter(|c| c.active()).count();
        let n_path = crate::model::placeholders(&self.cur.url).len();
        let missing = self.cur.unresolved_path_params();

        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Params, format!("query ({n_params})"));
            let path_label = if missing.is_empty() {
                format!("path ({n_path})")
            } else {
                format!("path ({} missing)", missing.len())
            };
            let path_tab = ui.selectable_value(&mut self.tab, Tab::Path, path_label);
            if !missing.is_empty() {
                path_tab.highlight();
            }
            ui.selectable_value(
                &mut self.tab,
                Tab::Headers,
                format!("headers ({n_headers})"),
            );
            ui.selectable_value(&mut self.tab, Tab::Cookies, format!("cookies ({n_cookies})"));
            ui.selectable_value(
                &mut self.tab,
                Tab::Auth,
                self.cur.auth.summary().to_owned(),
            );
            ui.selectable_value(
                &mut self.tab,
                Tab::Body,
                format!("body ({})", self.cur.body_kind.as_str()),
            );
            let n_extract = self.cur.extract.iter().filter(|e| e.active()).count();
            ui.selectable_value(&mut self.tab, Tab::Extract, format!("extract ({n_extract})"));
            ui.selectable_value(&mut self.tab, Tab::Options, "options".to_owned());
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("req_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                Tab::Params => {
                    if ui
                        .button("pull params out of url")
                        .on_hover_text("move ?a=b&c=d from the url field into this table")
                        .clicked()
                    {
                        self.cur.pull_params_from_url();
                    }
                    kv_editor(ui, "params", &mut self.cur.params);
                }
                Tab::Path => {
                    ui.horizontal(|ui| {
                        if ui
                            .button("scan url")
                            .on_hover_text("add a row for every :name / {name} in the url")
                            .clicked()
                        {
                            self.cur.sync_path_params();
                        }
                        ui.weak("write /users/:id/posts/{postId} in the url, fill values here");
                    });
                    let missing = self.cur.unresolved_path_params();
                    if !missing.is_empty() {
                        ui.colored_label(
                            Color32::from_rgb(0xff, 0xa7, 0x26),
                            format!("no value yet for: {}", missing.join(", ")),
                        );
                    }
                    kv_editor(ui, "path_params", &mut self.cur.path_params);
                }
                Tab::Cookies => {
                    ui.horizontal(|ui| {
                        ui.weak("sent as one Cookie header");
                        if let Some(resp) = &self.resp {
                            let set: Vec<(String, String)> = resp
                                .headers
                                .iter()
                                .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
                                .filter_map(|(_, v)| parse_set_cookie(v))
                                .collect();
                            if !set.is_empty()
                                && ui
                                    .button(format!("take {} from last response", set.len()))
                                    .on_hover_text("copy Set-Cookie values from the response")
                                    .clicked()
                            {
                                for (k, v) in set {
                                    upsert(&mut self.cur.cookies, &k, &v);
                                }
                            }
                        }
                    });
                    kv_editor(ui, "cookies", &mut self.cur.cookies);
                }
                Tab::Headers => {
                    ui.horizontal(|ui| {
                        if ui.small_button("+ json").clicked() {
                            push_header(&mut self.cur.headers, "Content-Type", "application/json");
                        }
                        if ui.small_button("+ accept").clicked() {
                            push_header(&mut self.cur.headers, "Accept", "application/json");
                        }
                    });
                    kv_editor(ui, "headers", &mut self.cur.headers);
                }
                Tab::Auth => {
                    ui.horizontal(|ui| {
                        for k in AuthKind::ALL {
                            ui.selectable_value(&mut self.cur.auth.kind, k, k.as_str());
                        }
                        ui.separator();
                        ui.checkbox(&mut self.show_secrets, "reveal");
                    });
                    ui.add_space(6.0);

                    let hide = !self.show_secrets;
                    let mut pending_oauth = None;
                    let auth = &mut self.cur.auth;
                    match auth.kind {
                        AuthKind::None => {
                            ui.weak("no auth header added");
                        }
                        AuthKind::Bearer => {
                            egui::Grid::new("auth_bearer")
                                .num_columns(2)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label("token");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut auth.token)
                                            .password(hide)
                                            .desired_width(420.0)
                                            .hint_text("paste token (no need to type Bearer)"),
                                    );
                                    ui.end_row();
                                });
                            ui.add_space(4.0);
                            ui.weak("sends: Authorization: Bearer <token>");
                        }
                        AuthKind::Basic => {
                            egui::Grid::new("auth_basic")
                                .num_columns(2)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label("username");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut auth.username)
                                            .desired_width(300.0),
                                    );
                                    ui.end_row();
                                    ui.label("password");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut auth.password)
                                            .password(hide)
                                            .desired_width(300.0),
                                    );
                                    ui.end_row();
                                });
                            ui.add_space(4.0);
                            ui.weak("sends: Authorization: Basic <base64(user:pass)>");
                        }
                        AuthKind::OAuth2 => {
                            oauth_ui(
                                ui,
                                &mut auth.oauth,
                                hide,
                                self.token_inflight,
                                &mut pending_oauth,
                            );
                        }
                        AuthKind::Jwt => {
                            jwt_ui(ui, &mut auth.jwt, hide, &mut self.jwt_error);
                        }
                        AuthKind::ApiKey => {
                            egui::Grid::new("auth_apikey")
                                .num_columns(2)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label("name");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut auth.api_key_name)
                                            .desired_width(300.0)
                                            .hint_text("X-API-Key"),
                                    );
                                    ui.end_row();
                                    ui.label("value");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut auth.api_key_value)
                                            .password(hide)
                                            .desired_width(420.0),
                                    );
                                    ui.end_row();
                                    ui.label("send in");
                                    ui.horizontal(|ui| {
                                        ui.selectable_value(
                                            &mut auth.api_key_in,
                                            ApiKeyIn::Header,
                                            "header",
                                        );
                                        ui.selectable_value(
                                            &mut auth.api_key_in,
                                            ApiKeyIn::Query,
                                            "query param",
                                        );
                                    });
                                    ui.end_row();
                                });
                        }
                    }

                    if auth.kind != AuthKind::None {
                        ui.add_space(8.0);
                        ui.weak("stored as plain text in the collection file");
                        if let Some((k, v)) = auth.header() {
                            let shown = if hide { mask(&v) } else { v };
                            ui.weak(format!("{k}: {shown}"));
                        }
                    }

                    if !self.oauth_status.is_empty() && auth.kind == AuthKind::OAuth2 {
                        ui.add_space(6.0);
                        ui.weak(self.oauth_status.clone());
                    }
                    if let (Some(raw), AuthKind::OAuth2) = (&self.oauth_raw, auth.kind) {
                        ui.collapsing("token endpoint response", |ui| {
                            let mut text = raw.as_str();
                            ui.add(
                                egui::TextEdit::multiline(&mut text)
                                    .code_editor()
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    }

                    match pending_oauth {
                        Some(OAuthAction::Fetch) => {
                            self.token_inflight = true;
                            self.oauth_status = "requesting token...".to_owned();
                            self.oauth_raw = None;
                            oauth::spawn_fetch(
                                self.cur.auth.oauth.clone(),
                                SendOpts {
                                    timeout_secs: self.timeout_secs,
                                    insecure_tls: self.insecure_tls,
                                },
                                self.token_tx.clone(),
                                ui.ctx().clone(),
                            );
                        }
                        Some(OAuthAction::Refresh) => {
                            self.token_inflight = true;
                            self.oauth_status = "refreshing token...".to_owned();
                            oauth::spawn_refresh(
                                self.cur.auth.oauth.clone(),
                                SendOpts {
                                    timeout_secs: self.timeout_secs,
                                    insecure_tls: self.insecure_tls,
                                },
                                self.token_tx.clone(),
                                ui.ctx().clone(),
                            );
                        }
                        Some(OAuthAction::Clear) => {
                            let o = &mut self.cur.auth.oauth;
                            o.access_token.clear();
                            o.refresh_token.clear();
                            o.expires_at = 0;
                            self.oauth_raw = None;
                            self.oauth_status = "token cleared".to_owned();
                        }
                        None => {}
                    }
                }
                Tab::Options => {
                    let t = &mut self.cur.transport;
                    ui.weak("connection options, saved with the request and carried in curl");
                    ui.add_space(6.0);
                    egui::Grid::new("transport_opts")
                        .num_columns(2)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("redirects");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut t.follow_redirects, "follow (-L)");
                                ui.add_enabled(
                                    t.follow_redirects,
                                    egui::DragValue::new(&mut t.max_redirects)
                                        .range(0..=50)
                                        .prefix("max "),
                                );
                            });
                            ui.end_row();

                            ui.label("compression");
                            ui.checkbox(&mut t.compressed, "gzip / brotli / deflate (--compressed)");
                            ui.end_row();

                            ui.label("proxy");
                            ui.add(
                                egui::TextEdit::singleline(&mut t.proxy)
                                    .desired_width(360.0)
                                    .hint_text("http://proxy:8080 - empty uses the system proxy"),
                            );
                            ui.end_row();

                            ui.label("ca cert");
                            ui.horizontal(|ui| {
                                if ui.small_button("file...").clicked() {
                                    if let Some(f) = rfd::FileDialog::new().pick_file() {
                                        t.ca_cert = f.display().to_string();
                                    }
                                }
                                ui.add(
                                    egui::TextEdit::singleline(&mut t.ca_cert)
                                        .desired_width(300.0)
                                        .hint_text("extra root certificate, PEM (--cacert)"),
                                );
                            });
                            ui.end_row();

                            ui.label("client cert");
                            ui.horizontal(|ui| {
                                if ui.small_button("file...").clicked() {
                                    if let Some(f) = rfd::FileDialog::new().pick_file() {
                                        t.client_cert = f.display().to_string();
                                    }
                                }
                                ui.add(
                                    egui::TextEdit::singleline(&mut t.client_cert)
                                        .desired_width(300.0)
                                        .hint_text("PEM (-E)"),
                                );
                            });
                            ui.end_row();

                            ui.label("client key");
                            ui.horizontal(|ui| {
                                if ui.small_button("file...").clicked() {
                                    if let Some(f) = rfd::FileDialog::new().pick_file() {
                                        t.client_key = f.display().to_string();
                                    }
                                }
                                ui.add(
                                    egui::TextEdit::singleline(&mut t.client_key)
                                        .desired_width(300.0)
                                        .hint_text("PEM (--key)"),
                                );
                            });
                            ui.end_row();
                        });
                    ui.add_space(8.0);
                    ui.weak("timeout and insecure tls live in the top bar - they apply to every request");
                }
                Tab::Extract => {
                    ui.weak(
                        "pull values out of this response into variables, then use them as \
                         {{name}} in any later request",
                    );
                    ui.add_space(4.0);
                    extract_editor(ui, &mut self.cur.extract, &self.vars);
                }
                Tab::Body => {
                    ui.horizontal_wrapped(|ui| {
                        for k in BodyKind::ALL {
                            ui.selectable_value(&mut self.cur.body_kind, k, k.as_str());
                        }
                        if self.cur.body_kind == BodyKind::Json && ui.button("format").clicked() {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&self.cur.body)
                            {
                                if let Ok(s) = serde_json::to_string_pretty(&v) {
                                    self.cur.body = s;
                                }
                            }
                        }
                    });
                    if let Some(ct) = self.cur.body_kind.content_type() {
                        ui.weak(format!("Content-Type: {ct}"));
                    }
                    ui.add_space(4.0);

                    match self.cur.body_kind {
                        BodyKind::None => {
                            ui.weak("no body sent");
                        }
                        BodyKind::Form => {
                            ui.weak("one key=value per line");
                        }
                        BodyKind::Multipart => {
                            ui.weak("text fields and file parts; reqwest sets the boundary");
                            form_data_editor(ui, &mut self.cur.form_parts);
                        }
                        BodyKind::Binary => {
                            egui::Grid::new("binary_body")
                                .num_columns(2)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label("file");
                                    ui.horizontal(|ui| {
                                        if ui.button("choose...").clicked() {
                                            if let Some(f) = rfd::FileDialog::new().pick_file() {
                                                self.cur.binary_path =
                                                    f.display().to_string();
                                            }
                                        }
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.cur.binary_path,
                                            )
                                            .desired_width(420.0)
                                            .hint_text("path to the file to send as the body"),
                                        );
                                    });
                                    ui.end_row();
                                    ui.label("content-type");
                                    ui.add(
                                        egui::TextEdit::singleline(
                                            &mut self.cur.binary_content_type,
                                        )
                                        .desired_width(300.0)
                                        .hint_text(crate::net::guess_content_type(
                                            &self.cur.binary_path,
                                        )),
                                    );
                                    ui.end_row();
                                });
                            match file_size(&self.cur.binary_path) {
                                Some(n) => ui.weak(format!("{n} bytes on disk")),
                                None if self.cur.binary_path.trim().is_empty() => {
                                    ui.weak("no file chosen")
                                }
                                None => ui.colored_label(
                                    Color32::from_rgb(0xef, 0x53, 0x50),
                                    "file not found",
                                ),
                            };
                        }
                        _ => {}
                    }

                    if self.cur.body_kind.is_text() || self.cur.body_kind == BodyKind::Form {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.cur.body)
                                .code_editor()
                                .desired_rows(12)
                                .desired_width(f32::INFINITY),
                        );
                    }
                }
            });
    }

    fn response_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.strong("response");
            if self.inflight {
                ui.spinner();
                ui.weak("sending...");
            }
            if let Some(e) = &self.error {
                ui.colored_label(Color32::from_rgb(0xef, 0x53, 0x50), e);
            }
            if let Some(r) = &self.resp {
                let c = if r.status < 300 {
                    Color32::from_rgb(0x4c, 0xaf, 0x50)
                } else if r.status < 400 {
                    Color32::from_rgb(0xff, 0xa7, 0x26)
                } else {
                    Color32::from_rgb(0xef, 0x53, 0x50)
                };
                ui.colored_label(c, format!("{} {}", r.status, r.status_text));
                ui.weak(format!("{} ms", r.elapsed_ms));
                ui.weak(crate::pretty::human_size(r.size));
                ui.weak(r.shape.as_str());
                if !r.content_type.is_empty() {
                    ui.weak(r.content_type.clone());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !self.toast.is_empty() {
                    ui.weak(self.toast.clone());
                }
            });
        });

        if self.resp.is_none() {
            ui.separator();
            ui.weak("no response yet - hit send (ctrl+enter)");
            return;
        }

        let n_headers = self.resp.as_ref().map(|r| r.headers.len()).unwrap_or(0);
        let n_cookies = self.resp.as_ref().map(|r| r.cookies().len()).unwrap_or(0);
        let shape = self.resp.as_ref().map(|r| r.shape).unwrap_or(Shape::Text);

        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.resp_tab, RespTab::Body, "body".to_owned());
            ui.selectable_value(
                &mut self.resp_tab,
                RespTab::Headers,
                format!("headers ({n_headers})"),
            );
            ui.selectable_value(
                &mut self.resp_tab,
                RespTab::Cookies,
                format!("cookies ({n_cookies})"),
            );
            ui.separator();
            if ui.small_button("save body...").clicked() {
                self.save_body();
            }
        });

        if self.resp_tab == RespTab::Body {
            let views = views_for(shape);
            if !views.contains(&self.body_view) {
                self.body_view = views[0];
            }
            ui.horizontal_wrapped(|ui| {
                for v in views {
                    ui.selectable_value(&mut self.body_view, v, v.as_str());
                }
                if shape.is_text() {
                    ui.checkbox(&mut self.wrap, "wrap");
                    if ui.small_button("copy").clicked() {
                        let text = self.shown_text();
                        ui.ctx().copy_text(text);
                    }
                }
            });

            if shape.is_text() {
                let text = self.shown_text();
                ui.horizontal_wrapped(|ui| {
                    ui.label("find");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .desired_width(220.0)
                            .hint_text("search the body"),
                    );
                    ui.checkbox(&mut self.search_case, "case");
                    ui.checkbox(&mut self.search_only_matches, "matching lines");
                    if !self.search.is_empty() {
                        let hits = count_matches(&text, &self.search, self.search_case);
                        let lines = matching_lines(&text, &self.search, self.search_case).len();
                        ui.weak(format!("{hits} matches in {lines} lines"));
                        if ui.small_button("clear").clicked() {
                            self.search.clear();
                        }
                    }
                });
            }

            if shape == Shape::Json {
                ui.horizontal_wrapped(|ui| {
                    ui.label("jsonpath");
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.jsonpath)
                            .desired_width(320.0)
                            .hint_text("$.data[0].id   $..name   $.items[*].price"),
                    );
                    let run = ui.small_button("extract").clicked()
                        || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if run {
                        let body = self
                            .resp
                            .as_ref()
                            .map(|r| r.body.clone())
                            .unwrap_or_default();
                        self.jsonpath_result = (!self.jsonpath.trim().is_empty())
                            .then(|| crate::jsonpath::select_text(&body, &self.jsonpath));
                    }
                    if self.jsonpath_result.is_some() && ui.small_button("clear").clicked() {
                        self.jsonpath_result = None;
                        self.jsonpath.clear();
                    }
                    if let Some(Ok(found)) = &self.jsonpath_result {
                        if ui.small_button("copy result").clicked() {
                            ui.ctx().copy_text(found.clone());
                        }
                    }
                });
            }
        }
        ui.separator();

        egui::ScrollArea::both()
            .id_salt("resp_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.resp_tab {
                RespTab::Body => self.body_view_ui(ui),
                RespTab::Headers => {
                    let Some(resp) = &self.resp else { return };
                    egui::Grid::new("resp_headers")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("url");
                            ui.label(&resp.final_url);
                            ui.end_row();
                            for (k, v) in &resp.headers {
                                ui.strong(k);
                                ui.label(v);
                                ui.end_row();
                            }
                        });
                }
                RespTab::Cookies => {
                    let Some(resp) = &self.resp else { return };
                    let cookies = resp.cookies();
                    if cookies.is_empty() {
                        ui.weak("no Set-Cookie headers in this response");
                        return;
                    }
                    egui::Grid::new("resp_cookies")
                        .num_columns(3)
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            ui.strong("name");
                            ui.strong("value");
                            ui.strong("attributes");
                            ui.end_row();
                            for (name, value, attrs) in &cookies {
                                ui.label(name);
                                ui.label(value);
                                ui.weak(attrs);
                                ui.end_row();
                            }
                        });
                    ui.add_space(6.0);
                    if ui
                        .button("send these back with the request")
                        .on_hover_text("copy them into the request cookies tab")
                        .clicked()
                    {
                        for (name, value, _) in cookies {
                            upsert(&mut self.cur.cookies, &name, &value);
                        }
                        self.tab = Tab::Cookies;
                    }
                }
            });
    }

    /// The text currently on show, after view mode and jsonpath extraction.
    fn shown_text(&self) -> String {
        let Some(resp) = &self.resp else {
            return String::new();
        };
        if let Some(Ok(found)) = &self.jsonpath_result {
            return found.clone();
        }
        match self.body_view {
            BodyView::Raw => resp.body.clone(),
            BodyView::Text => crate::pretty::strip_tags(&resp.body),
            BodyView::Hex => crate::pretty::hex_dump(&resp.bytes, 64 * 1024),
            _ => resp
                .pretty
                .clone()
                .unwrap_or_else(|| resp.body.clone()),
        }
    }

    fn body_view_ui(&mut self, ui: &mut egui::Ui) {
        let Some(resp) = &self.resp else { return };

        if let Some(Err(e)) = &self.jsonpath_result {
            ui.colored_label(Color32::from_rgb(0xef, 0x53, 0x50), format!("jsonpath: {e}"));
            ui.separator();
        }

        match self.body_view {
            BodyView::Image => {
                self.image_ui(ui);
                return;
            }
            BodyView::Tree if self.jsonpath_result.is_none() => {
                match serde_json::from_str::<serde_json::Value>(&resp.body) {
                    Ok(value) => {
                        let mut copied = None;
                        json_tree(ui, "$", &value, 0, &mut copied);
                        if let Some(path) = copied {
                            ui.ctx().copy_text(path);
                        }
                    }
                    Err(e) => {
                        ui.colored_label(
                            Color32::from_rgb(0xef, 0x53, 0x50),
                            format!("not json: {e}"),
                        );
                    }
                }
                return;
            }
            _ => {}
        }

        let text = self.shown_text();
        let text = if self.search_only_matches && !self.search.is_empty() {
            matching_lines(&text, &self.search, self.search_case)
                .into_iter()
                .map(|(n, line)| format!("{n:>6}  {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            text
        };

        let width = if self.wrap {
            ui.available_width()
        } else {
            f32::INFINITY
        };
        let needle = self.search.clone();
        let case = self.search_case;
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut job = highlight_job(buf.as_str(), &needle, case, ui);
            job.wrap.max_width = wrap_width;
            ui.painter().layout_job(job)
        };

        let mut shown = text.as_str();
        ui.add(
            egui::TextEdit::multiline(&mut shown)
                .code_editor()
                .desired_width(width)
                .layouter(&mut layouter),
        );
    }

    fn image_ui(&mut self, ui: &mut egui::Ui) {
        if self.image.is_none() {
            let Some(resp) = &self.resp else { return };
            match image::load_from_memory(&resp.bytes) {
                Ok(decoded) => {
                    let rgba = decoded.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let texture = ui.ctx().load_texture(
                        format!("resp-image-{}", self.resp_seq),
                        color,
                        egui::TextureOptions::default(),
                    );
                    self.image = Some((texture, size));
                }
                Err(e) => {
                    ui.colored_label(
                        Color32::from_rgb(0xef, 0x53, 0x50),
                        format!("cannot decode this image: {e}"),
                    );
                    ui.weak("try the hex view, or save the body to a file");
                    return;
                }
            }
        }

        if let Some((texture, size)) = &self.image {
            ui.weak(format!("{} x {} px", size[0], size[1]));
            let available = ui.available_size();
            let scale = (available.x / size[0] as f32)
                .min(available.y / size[1] as f32)
                .clamp(0.05, 1.0);
            ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(egui::vec2(size[0] as f32 * scale, size[1] as f32 * scale)),
            );
        }
    }

    /// Write the raw response bytes to a file the user picks.
    fn save_body(&mut self) {
        let Some(resp) = &self.resp else { return };
        let suggested = resp
            .final_url
            .rsplit('/')
            .find(|segment| !segment.is_empty() && !segment.contains('?'))
            .filter(|name| name.contains('.'))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("response.{}", extension_for(resp.shape)));

        if let Some(path) = rfd::FileDialog::new().set_file_name(suggested).save_file() {
            self.toast = match std::fs::write(&path, &resp.bytes) {
                Ok(()) => format!("saved {} -> {}", crate::pretty::human_size(resp.bytes.len()), path.display()),
                Err(e) => format!("save failed: {e}"),
            };
        }
    }
}

fn extract_editor(
    ui: &mut egui::Ui,
    rows: &mut Vec<Extract>,
    vars: &BTreeMap<String, String>,
) {
    let mut delete: Option<usize> = None;
    egui::Grid::new("extract_rules")
        .num_columns(6)
        .striped(true)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            ui.strong("variable");
            ui.strong("from");
            ui.strong("expression");
            ui.strong("current value");
            ui.label("");
            ui.end_row();

            for (i, row) in rows.iter_mut().enumerate() {
                ui.checkbox(&mut row.on, "");
                ui.add(
                    egui::TextEdit::singleline(&mut row.var)
                        .desired_width(140.0)
                        .hint_text("access_token"),
                );
                egui::ComboBox::from_id_salt(("extract_from", i))
                    .width(110.0)
                    .selected_text(row.from.as_str())
                    .show_ui(ui, |ui| {
                        for f in ExtractFrom::ALL {
                            ui.selectable_value(&mut row.from, f, f.as_str());
                        }
                    });
                ui.add_enabled(
                    row.from.needs_expr(),
                    egui::TextEdit::singleline(&mut row.expr)
                        .desired_width(260.0)
                        .hint_text(row.from.hint()),
                );
                match vars.get(row.var.trim()) {
                    Some(value) => {
                        ui.colored_label(
                            Color32::from_rgb(0x9c, 0xcc, 0x65),
                            RichText::new(truncate(value, 40)).monospace(),
                        );
                    }
                    None => {
                        ui.weak("-");
                    }
                }
                if ui.small_button("x").clicked() {
                    delete = Some(i);
                }
                ui.end_row();
            }
        });

    if let Some(i) = delete {
        rows.remove(i);
    }
    if ui.button("+ rule").clicked() {
        rows.push(Extract::default());
    }
    if rows.last().map(|r| !r.var.trim().is_empty()).unwrap_or(true) {
        rows.push(Extract::default());
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

fn extension_for(shape: Shape) -> &'static str {
    match shape {
        Shape::Json => "json",
        Shape::Xml => "xml",
        Shape::Html => "html",
        Shape::Text => "txt",
        Shape::Image => "png",
        Shape::Binary => "bin",
    }
}

fn count_matches(haystack: &str, needle: &str, case_sensitive: bool) -> usize {
    if needle.is_empty() {
        return 0;
    }
    if case_sensitive {
        haystack.matches(needle).count()
    } else {
        haystack
            .to_lowercase()
            .matches(&needle.to_lowercase())
            .count()
    }
}

/// Lines containing the needle, with their 1-based line numbers.
fn matching_lines<'a>(
    haystack: &'a str,
    needle: &str,
    case_sensitive: bool,
) -> Vec<(usize, &'a str)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let needle_cmp = if case_sensitive {
        needle.to_owned()
    } else {
        needle.to_lowercase()
    };
    haystack
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            if case_sensitive {
                line.contains(&needle_cmp)
            } else {
                line.to_lowercase().contains(&needle_cmp)
            }
        })
        .map(|(i, line)| (i + 1, line))
        .collect()
}

/// Byte ranges of every match, for highlighting.
fn match_ranges(haystack: &str, needle: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let (hay, need) = if case_sensitive {
        (haystack.to_owned(), needle.to_owned())
    } else {
        (haystack.to_lowercase(), needle.to_lowercase())
    };
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = hay[from..].find(&need) {
        let start = from + rel;
        let end = start + need.len();
        // lowercasing can change byte lengths; only keep ranges that still line up
        if haystack.is_char_boundary(start) && haystack.is_char_boundary(end) {
            out.push((start, end));
        }
        from = end.max(start + 1);
    }
    out
}

/// A layout job with every search hit painted.
fn highlight_job(text: &str, needle: &str, case_sensitive: bool, ui: &egui::Ui) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = TextFormat {
        font_id: font.clone(),
        color: ui.visuals().text_color(),
        ..Default::default()
    };
    let hit = TextFormat {
        font_id: font,
        color: Color32::BLACK,
        background: Color32::from_rgb(0xff, 0xe0, 0x66),
        ..Default::default()
    };

    let mut job = LayoutJob::default();
    let ranges = match_ranges(text, needle, case_sensitive);
    if ranges.is_empty() {
        job.append(text, 0.0, plain);
        return job;
    }
    let mut cursor = 0;
    for (start, end) in ranges {
        if start < cursor {
            continue;
        }
        job.append(&text[cursor..start], 0.0, plain.clone());
        job.append(&text[start..end], 0.0, hit.clone());
        cursor = end;
    }
    job.append(&text[cursor..], 0.0, plain);
    job
}

/// Collapsible json tree. Clicking a node's path button copies that path.
fn json_tree(
    ui: &mut egui::Ui,
    path: &str,
    value: &serde_json::Value,
    depth: usize,
    copied: &mut Option<String>,
) {
    const MAX_CHILDREN: usize = 500;
    let label = path.rsplit(['.', '[']).next().unwrap_or(path).trim_end_matches(']');

    match value {
        serde_json::Value::Object(map) => {
            let header = format!("{label}  {{{}}}", map.len());
            egui::CollapsingHeader::new(header)
                .id_salt(path)
                .default_open(depth < 2)
                .show(ui, |ui| {
                    for (k, v) in map.iter().take(MAX_CHILDREN) {
                        json_tree(ui, &format!("{path}.{k}"), v, depth + 1, copied);
                    }
                    if map.len() > MAX_CHILDREN {
                        ui.weak(format!("... {} more keys", map.len() - MAX_CHILDREN));
                    }
                });
        }
        serde_json::Value::Array(items) => {
            let header = format!("{label}  [{}]", items.len());
            egui::CollapsingHeader::new(header)
                .id_salt(path)
                .default_open(depth < 2)
                .show(ui, |ui| {
                    for (i, v) in items.iter().enumerate().take(MAX_CHILDREN) {
                        json_tree(ui, &format!("{path}[{i}]"), v, depth + 1, copied);
                    }
                    if items.len() > MAX_CHILDREN {
                        ui.weak(format!("... {} more items", items.len() - MAX_CHILDREN));
                    }
                });
        }
        leaf => {
            let (text, color) = match leaf {
                serde_json::Value::String(s) => (
                    format!("\"{s}\""),
                    Color32::from_rgb(0x9c, 0xcc, 0x65),
                ),
                serde_json::Value::Number(n) => {
                    (n.to_string(), Color32::from_rgb(0x64, 0xb5, 0xf6))
                }
                serde_json::Value::Bool(b) => {
                    (b.to_string(), Color32::from_rgb(0xba, 0x68, 0xc8))
                }
                _ => ("null".to_owned(), Color32::GRAY),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).strong());
                ui.colored_label(color, text);
                if ui
                    .small_button("path")
                    .on_hover_text(format!("copy {path}"))
                    .clicked()
                {
                    *copied = Some(path.to_owned());
                }
            });
        }
    }
}

/// Keep the path, move the request to a localhost port.
fn swap_to_localhost(url: &str, port: u16) -> String {
    let normalized = crate::model::normalize_url(url);
    let tail = normalized
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&normalized);
    let path = tail.find(['/', '?', '#']).map(|i| &tail[i..]).unwrap_or("");
    format!("http://localhost:{port}{path}")
}

enum OAuthAction {
    Fetch,
    Refresh,
    Clear,
}

fn oauth_ui(
    ui: &mut egui::Ui,
    cfg: &mut crate::model::OAuth2,
    hide: bool,
    inflight: bool,
    action: &mut Option<OAuthAction>,
) {
    ui.horizontal(|ui| {
        ui.label("grant");
        for g in Grant::ALL {
            ui.selectable_value(&mut cfg.grant, g, g.as_str());
        }
    });
    ui.add_space(4.0);

    egui::Grid::new("oauth_fields")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            if cfg.grant == Grant::AuthorizationCode {
                ui.label("auth url");
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.auth_url)
                        .desired_width(460.0)
                        .hint_text("https://issuer.example.com/authorize"),
                );
                ui.end_row();
            }
            ui.label("token url");
            ui.add(
                egui::TextEdit::singleline(&mut cfg.token_url)
                    .desired_width(460.0)
                    .hint_text("https://issuer.example.com/oauth/token"),
            );
            ui.end_row();

            ui.label("client id");
            ui.add(egui::TextEdit::singleline(&mut cfg.client_id).desired_width(360.0));
            ui.end_row();

            ui.label("client secret");
            ui.add(
                egui::TextEdit::singleline(&mut cfg.client_secret)
                    .password(hide)
                    .desired_width(360.0)
                    .hint_text("leave empty for a public client"),
            );
            ui.end_row();

            if cfg.grant == Grant::Password {
                ui.label("username");
                ui.add(egui::TextEdit::singleline(&mut cfg.username).desired_width(300.0));
                ui.end_row();
                ui.label("password");
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.password)
                        .password(hide)
                        .desired_width(300.0),
                );
                ui.end_row();
            }

            ui.label("scope");
            ui.add(
                egui::TextEdit::singleline(&mut cfg.scope)
                    .desired_width(360.0)
                    .hint_text("space separated"),
            );
            ui.end_row();

            ui.label("audience");
            ui.add(
                egui::TextEdit::singleline(&mut cfg.audience)
                    .desired_width(360.0)
                    .hint_text("optional, some issuers require it"),
            );
            ui.end_row();

            ui.label("client auth");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut cfg.client_auth, ClientAuth::BasicHeader, "basic header");
                ui.selectable_value(&mut cfg.client_auth, ClientAuth::RequestBody, "request body");
            });
            ui.end_row();

            if cfg.grant == Grant::AuthorizationCode {
                ui.label("redirect");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut cfg.redirect_port).range(1024..=65535));
                    ui.weak(cfg.redirect_uri());
                });
                ui.end_row();

                ui.label("pkce");
                ui.checkbox(&mut cfg.use_pkce, "S256 code challenge");
                ui.end_row();
            }
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let label = match cfg.grant {
            Grant::AuthorizationCode => "authorize in browser",
            _ => "get token",
        };
        if ui
            .add_enabled(!inflight, egui::Button::new(label))
            .clicked()
        {
            *action = Some(OAuthAction::Fetch);
        }
        if ui
            .add_enabled(
                !inflight && !cfg.refresh_token.is_empty(),
                egui::Button::new("refresh"),
            )
            .clicked()
        {
            *action = Some(OAuthAction::Refresh);
        }
        if ui
            .add_enabled(!cfg.access_token.is_empty(), egui::Button::new("clear"))
            .clicked()
        {
            *action = Some(OAuthAction::Clear);
        }
        if inflight {
            ui.spinner();
        }
    });

    ui.add_space(4.0);
    if cfg.access_token.is_empty() {
        ui.weak("no token yet");
    } else {
        ui.horizontal(|ui| {
            ui.label("access token");
            let shown = if hide {
                mask(&cfg.access_token)
            } else {
                cfg.access_token.clone()
            };
            ui.weak(shown);
            if ui.small_button("copy").clicked() {
                ui.ctx().copy_text(cfg.access_token.clone());
            }
        });
        match cfg.seconds_left() {
            Some(secs) if secs > 0 => {
                ui.weak(format!("expires in {}", human_secs(secs)));
            }
            Some(_) => {
                ui.colored_label(Color32::from_rgb(0xef, 0x53, 0x50), "token expired");
            }
            None => {
                ui.weak("no expiry reported");
            }
        }
    }
}

fn jwt_ui(
    ui: &mut egui::Ui,
    jwt: &mut crate::model::JwtAuth,
    hide: bool,
    sign_error: &mut String,
) {
    egui::Grid::new("jwt_fields")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("token");
            ui.add(
                egui::TextEdit::multiline(&mut jwt.token)
                    .password(hide)
                    .desired_rows(3)
                    .desired_width(520.0)
                    .hint_text("paste a jwt, or sign one below"),
            );
            ui.end_row();
            ui.label("header");
            ui.add(
                egui::TextEdit::singleline(&mut jwt.header_name)
                    .desired_width(240.0)
                    .hint_text("Authorization"),
            );
            ui.end_row();
            ui.label("prefix");
            ui.add(
                egui::TextEdit::singleline(&mut jwt.prefix)
                    .desired_width(240.0)
                    .hint_text("Bearer (empty sends the bare token)"),
            );
            ui.end_row();
        });

    if !jwt.token.trim().is_empty() {
        ui.add_space(6.0);
        match crate::jwt::decode(&jwt.token) {
            Ok(d) => {
                ui.horizontal(|ui| {
                    ui.strong(format!("alg {}", d.alg));
                    if let Some(sub) = &d.subject {
                        ui.weak(format!("sub {sub}"));
                    }
                    if let Some(iss) = &d.issuer {
                        ui.weak(format!("iss {iss}"));
                    }
                    match d.seconds_left() {
                        Some(secs) if secs > 0 => {
                            ui.weak(format!("expires in {}", human_secs(secs)));
                        }
                        Some(_) => {
                            ui.colored_label(Color32::from_rgb(0xef, 0x53, 0x50), "expired");
                        }
                        None => {
                            ui.weak("no exp claim");
                        }
                    }
                });
                ui.collapsing("decoded claims", |ui| {
                    for (title, text) in [("header", &d.header), ("payload", &d.payload)] {
                        ui.label(title);
                        let mut view = text.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut view)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    }
                    ui.weak("signature is not verified here");
                });
            }
            Err(e) => {
                ui.colored_label(Color32::from_rgb(0xef, 0x53, 0x50), e);
            }
        }
    }

    ui.add_space(8.0);
    ui.collapsing("sign a new token (HS256)", |ui| {
        ui.label("claims");
        ui.add(
            egui::TextEdit::multiline(&mut jwt.claims)
                .code_editor()
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            ui.label("secret");
            ui.add(
                egui::TextEdit::singleline(&mut jwt.secret)
                    .password(hide)
                    .desired_width(300.0),
            );
            ui.checkbox(&mut jwt.secret_is_base64, "base64 secret");
        });
        ui.horizontal(|ui| {
            if ui.button("sign").clicked() {
                match crate::jwt::sign_hs256(&jwt.claims, &jwt.secret, jwt.secret_is_base64) {
                    Ok(token) => {
                        jwt.token = token;
                        sign_error.clear();
                    }
                    Err(e) => *sign_error = e,
                }
            }
            if ui
                .small_button("+ exp 1h")
                .on_hover_text("add an exp claim one hour out")
                .clicked()
            {
                jwt.claims = with_exp(&jwt.claims, 3600);
            }
        });
        if !sign_error.is_empty() {
            ui.colored_label(Color32::from_rgb(0xef, 0x53, 0x50), sign_error.clone());
        }
    });
}

/// Set `exp` on a claims json blob, keeping the rest as it is.
fn with_exp(claims: &str, seconds: u64) -> String {
    let mut value: serde_json::Value =
        serde_json::from_str(claims).unwrap_or(serde_json::json!({}));
    if let Some(map) = value.as_object_mut() {
        map.insert(
            "exp".to_owned(),
            serde_json::json!(crate::model::now_unix() + seconds),
        );
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| claims.to_owned())
}

fn human_secs(secs: i64) -> String {
    match secs {
        s if s < 90 => format!("{s}s"),
        s if s < 5400 => format!("{}m", s / 60),
        s => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

fn file_size(path: &str) -> Option<u64> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// `name=value; Path=/; HttpOnly` -> ("name", "value")
fn parse_set_cookie(value: &str) -> Option<(String, String)> {
    let first = value.split(';').next()?.trim();
    let (k, v) = first.split_once('=')?;
    (!k.trim().is_empty()).then(|| (k.trim().to_owned(), v.trim().to_owned()))
}

fn upsert(rows: &mut Vec<KeyVal>, key: &str, value: &str) {
    if let Some(row) = rows.iter_mut().find(|r| r.key.trim() == key) {
        row.value = value.to_owned();
        row.on = true;
        return;
    }
    if let Some(blank) = rows.iter_mut().find(|r| r.key.trim().is_empty()) {
        blank.key = key.to_owned();
        blank.value = value.to_owned();
        blank.on = true;
    } else {
        rows.push(KeyVal {
            on: true,
            key: key.to_owned(),
            value: value.to_owned(),
        });
    }
}

fn form_data_editor(ui: &mut egui::Ui, rows: &mut Vec<FormPart>) {
    let mut delete: Option<usize> = None;
    egui::Grid::new("form_data")
        .num_columns(6)
        .striped(true)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            ui.strong("key");
            ui.strong("value / file");
            ui.strong("");
            ui.strong("content-type");
            ui.label("");
            ui.end_row();

            for (i, row) in rows.iter_mut().enumerate() {
                ui.checkbox(&mut row.on, "");
                ui.add(
                    egui::TextEdit::singleline(&mut row.key)
                        .desired_width(150.0)
                        .hint_text("field"),
                );
                if row.is_file() {
                    ui.add(
                        egui::TextEdit::singleline(&mut row.file)
                            .desired_width(280.0)
                            .hint_text("file path"),
                    );
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut row.value)
                            .desired_width(280.0)
                            .hint_text("text value"),
                    );
                }
                ui.horizontal(|ui| {
                    if ui.small_button("file...").clicked() {
                        if let Some(f) = rfd::FileDialog::new().pick_file() {
                            row.file = f.display().to_string();
                        }
                    }
                    if row.is_file() && ui.small_button("as text").clicked() {
                        row.file.clear();
                    }
                });
                let ct_hint = if row.is_file() {
                    crate::net::guess_content_type(&row.file)
                } else {
                    "auto"
                };
                ui.add(
                    egui::TextEdit::singleline(&mut row.content_type)
                        .desired_width(160.0)
                        .hint_text(ct_hint),
                );
                if ui.small_button("x").clicked() {
                    delete = Some(i);
                }
                ui.end_row();
            }
        });

    if let Some(i) = delete {
        rows.remove(i);
    }
    if ui.button("+ part").clicked() {
        rows.push(FormPart::default());
    }
    if rows.last().map(|r| !r.key.trim().is_empty()).unwrap_or(true) {
        rows.push(FormPart::default());
    }
}

/// Show enough of a secret to recognise it, not enough to leak it over a shoulder.
fn mask(value: &str) -> String {
    let n = value.chars().count();
    if n <= 12 {
        "*".repeat(n)
    } else {
        let head: String = value.chars().take(8).collect();
        let tail: String = value.chars().skip(n - 4).collect();
        format!("{head}{}{tail}", "*".repeat(n - 12))
    }
}

fn push_header(headers: &mut Vec<KeyVal>, key: &str, value: &str) {
    if let Some(h) = headers.iter_mut().find(|h| h.key.eq_ignore_ascii_case(key)) {
        h.value = value.to_owned();
        h.on = true;
        return;
    }
    if let Some(blank) = headers.iter_mut().find(|h| h.key.trim().is_empty()) {
        blank.key = key.to_owned();
        blank.value = value.to_owned();
        blank.on = true;
    } else {
        headers.push(KeyVal {
            on: true,
            key: key.to_owned(),
            value: value.to_owned(),
        });
    }
}

fn kv_editor(ui: &mut egui::Ui, id: &str, rows: &mut Vec<KeyVal>) {
    let mut delete: Option<usize> = None;
    let width = ((ui.available_width() - 80.0).max(200.0)) / 2.0;

    egui::Grid::new(id)
        .num_columns(4)
        .striped(true)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            for (i, row) in rows.iter_mut().enumerate() {
                ui.checkbox(&mut row.on, "");
                ui.add(
                    egui::TextEdit::singleline(&mut row.key)
                        .desired_width(width)
                        .hint_text("key"),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut row.value)
                        .desired_width(width)
                        .hint_text("value"),
                );
                if ui.small_button("x").clicked() {
                    delete = Some(i);
                }
                ui.end_row();
            }
        });

    if let Some(i) = delete {
        rows.remove(i);
    }
    if ui.button("+ row").clicked() {
        rows.push(KeyVal::default());
    }
    // always keep one blank row ready to type into
    if rows.last().map(|r| !r.key.trim().is_empty()).unwrap_or(true) {
        rows.push(KeyVal::default());
    }
}

impl eframe::App for YapiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain();

        let ctx = ui.ctx().clone();
        let (save_hit, send_hit) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::Enter),
            )
        });
        if save_hit {
            self.save_current();
        }
        if send_hit {
            self.send(&ctx);
        }

        egui::Panel::left("saved")
            .resizable(true)
            .default_size(250.0)
            .show(ui, |ui| self.sidebar(ui));

        if self.mode == Mode::Request {
            egui::Panel::top("req_top").show(ui, |ui| self.top_bar(ui, &ctx));

            egui::Panel::bottom("resp")
                .resizable(true)
                .default_size(340.0)
                .min_size(120.0)
                .show(ui, |ui| self.response_panel(ui));
        }

        egui::CentralPanel::default().show(ui, |ui| match self.mode {
            Mode::Request => self.request_editor(ui),
            Mode::Chain => self.chain_editor(ui, &ctx),
        });

        self.curl_windows(&ctx);
        self.autosave_draft(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.persist_session();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_counts_matches_and_lines() {
        let text = "alpha Beta\nbeta gamma\nnothing here";
        assert_eq!(count_matches(text, "beta", false), 2);
        assert_eq!(count_matches(text, "beta", true), 1);
        assert_eq!(count_matches(text, "", false), 0);

        let lines = matching_lines(text, "beta", false);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], (1, "alpha Beta"));
        assert_eq!(lines[1], (2, "beta gamma"));
        assert!(matching_lines(text, "zzz", false).is_empty());
    }

    #[test]
    fn highlight_ranges_cover_every_hit() {
        let text = "ab AB ab";
        assert_eq!(match_ranges(text, "ab", true), vec![(0, 2), (6, 8)]);
        assert_eq!(
            match_ranges(text, "ab", false),
            vec![(0, 2), (3, 5), (6, 8)]
        );
        // overlapping needles advance rather than loop forever
        assert_eq!(match_ranges("aaaa", "aa", true), vec![(0, 2), (2, 4)]);
        assert!(match_ranges("anything", "", false).is_empty());
    }

    #[test]
    fn multibyte_text_does_not_split_a_char() {
        let text = "héllo HÉLLO";
        let ranges = match_ranges(text, "héllo", false);
        for (start, end) in ranges {
            assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
        }
    }

    #[test]
    fn views_match_the_body_shape() {
        assert_eq!(
            views_for(Shape::Json),
            vec![BodyView::Pretty, BodyView::Tree, BodyView::Raw]
        );
        assert_eq!(views_for(Shape::Image), vec![BodyView::Image, BodyView::Hex]);
        assert_eq!(views_for(Shape::Binary), vec![BodyView::Hex]);
        assert!(views_for(Shape::Html).contains(&BodyView::Text));
        // every shape offers at least one view, so the picker can never be empty
        for shape in [
            Shape::Json,
            Shape::Xml,
            Shape::Html,
            Shape::Text,
            Shape::Image,
            Shape::Binary,
        ] {
            assert!(!views_for(shape).is_empty());
            assert!(!extension_for(shape).is_empty());
        }
    }

    #[test]
    fn set_cookie_parses_off_its_attributes() {
        assert_eq!(
            parse_set_cookie("session=abc; Path=/; HttpOnly"),
            Some(("session".to_owned(), "abc".to_owned()))
        );
        assert_eq!(parse_set_cookie("novalue"), None);
    }
}
