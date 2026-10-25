//! eframe/egui front-end.

use crate::audio::AudioEngine;
use crate::backup::{BackupEntry, BackupStore};
use crate::config::Config;
use crate::launcher::{self, HackOptions, HackOutcome, Level, LogLine};
use crate::patch;
use crate::paths;
use crate::target::{Target, TargetKind};
use crate::theme;
use crate::theme::*;
use crate::util;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Layout, Margin,
    RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, TextStyle, Ui, Vec2,
};
use std::path::{Path, PathBuf};

const ISSUE_URL: &str = "https://github.com/Ericlu123456/LauncherLauncher/issues/new/choose";
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

enum WorkerMsg {
    Log(LogLine),
    HackDone(Result<HackOutcome, String>),
}

struct LogView {
    time: String,
    level: Level,
    text: String,
}

#[derive(Clone)]
struct Toast {
    text: String,
    level: Level,
    until: f64,
}

pub struct App {
    cfg: Config,
    path_input: String,
    target: Option<Target>,
    detect_rx: Option<Receiver<Result<Target, String>>>,
    detect_error: Option<String>,
    busy: bool,
    hack_rx: Option<Receiver<WorkerMsg>>,
    logs: Vec<LogView>,
    outcome: Option<HackOutcome>,
    backups: Vec<BackupEntry>,
    audio: Option<AudioEngine>,
    audio_error: Option<String>,
    show_advanced: bool,
    show_backups: bool,
    show_about: bool,
    custom_rules: String,
    dry_run: bool,
    started: Instant,
    flash_until: f64,
    toast: Option<Toast>,
    pending_restore: Option<BackupEntry>,
    browse_file: bool,
    browse_dir: bool,
    hack_clicked: bool,
    open_backups: bool,
    drop_hover: bool,
    auto_hack: bool,
    auto_hack_done: bool,
    screenshot_path: Option<PathBuf>,
    screenshot_exit: bool,
    shot_frames: u32,
    shot_done: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(target_os = "windows")]
        {
            // Windows 100% DPI would render the 8px pixel UI too small.
            let ppp = cc.egui_ctx.pixels_per_point();
            if ppp < 1.2 {
                cc.egui_ctx.set_pixels_per_point(1.2);
                // set_pixels_per_point does not resize the already-created
                // window; re-request the same logical size so the layout keeps
                // its two columns while the pixels get larger.
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(920.0, 570.0)));
            }
        }
        theme::install(&cc.egui_ctx);
        let cfg = Config::load();
        let mut audio = None;
        let mut audio_error = None;
        if cfg.music_enabled {
            match AudioEngine::new() {
                Ok(mut a) => {
                    a.set_muted(cfg.muted);
                    a.set_volume(cfg.volume);
                    a.set_playing(true);
                    audio = Some(a);
                }
                Err(e) => audio_error = Some(format!("{e:#}")),
            }
        }
        let screenshot_path = std::env::var_os("LL_SCREENSHOT").map(PathBuf::from);
        let screenshot_exit = std::env::var_os("LL_SCREENSHOT_EXIT").is_some();
        let auto_hack = std::env::var_os("LL_AUTOHACK").is_some();
        let mut app = Self {
            path_input: cfg
                .launcher_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            cfg,
            target: None,
            detect_rx: None,
            detect_error: None,
            busy: false,
            hack_rx: None,
            logs: Vec::new(),
            outcome: None,
            backups: BackupStore::list(),
            audio,
            audio_error,
            show_advanced: false,
            show_backups: true,
            show_about: std::env::var_os("LL_ABOUT").is_some(),
            custom_rules: String::new(),
            dry_run: false,
            started: Instant::now(),
            flash_until: 0.0,
            toast: None,
            pending_restore: None,
            browse_file: false,
            browse_dir: false,
            hack_clicked: false,
            open_backups: false,
            drop_hover: false,
            auto_hack,
            auto_hack_done: false,
            screenshot_path,
            screenshot_exit,
            shot_frames: 0,
            shot_done: false,
        };
        app.custom_rules = app.cfg.custom_rules.clone();
        app.push_log(Level::Info, "LauncherLauncher 0.1.1 // 2026.10.20");
        app.push_log(Level::Info, "离线运行：不联网、不上传任何数据。");
        app.push_log(Level::Info, "选择启动器，或把文件 / 文件夹拖进窗口。");
        if app.cfg.launcher_path.is_some() {
            let p = app.cfg.launcher_path.clone().unwrap();
            if p.exists() {
                app.start_detect(p);
            }
        }
        app
    }

    fn push_log(&mut self, level: Level, text: impl Into<String>) {
        self.logs.push(LogView {
            time: clock(),
            level,
            text: text.into(),
        });
        if self.logs.len() > 700 {
            self.logs.drain(..120);
        }
    }

    fn start_detect(&mut self, path: PathBuf) {
        self.path_input = path.to_string_lossy().to_string();
        self.cfg.launcher_path = Some(path.clone());
        self.cfg.save();
        self.target = None;
        self.detect_error = None;
        let (tx, rx) = mpsc::channel();
        self.detect_rx = Some(rx);
        self.push_log(Level::Info, format!("识别目标：{}", path.display()));
        std::thread::spawn(move || {
            let res = Target::detect(&path).map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
        });
    }

    fn detect_from_input(&mut self) {
        let p = PathBuf::from(self.path_input.trim());
        if p.as_os_str().is_empty() {
            return;
        }
        self.start_detect(p);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let detect_msg = self.detect_rx.as_ref().map(|rx| rx.try_recv());
        match detect_msg {
            Some(Ok(Ok(t))) => {
                self.detect_rx = None;
                self.push_log(
                    Level::Ok,
                    format!(
                        "识别为 {} {} · {} · {}",
                        t.label,
                        t.version,
                        t.kind.label(),
                        util::human_bytes(t.bytes)
                    ),
                );
                self.cfg.launcher_path = Some(t.path.clone());
                self.cfg.save();
                self.target = Some(t);
                if self.auto_hack && !self.auto_hack_done {
                    self.auto_hack_done = true;
                    self.hack_clicked = true;
                }
            }
            Some(Ok(Err(e))) => {
                self.detect_rx = None;
                self.push_log(Level::Error, format!("识别失败：{e}"));
                self.detect_error = Some(e);
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.detect_rx = None;
                self.detect_error = Some("识别线程异常退出".to_string());
            }
            _ => {}
        }

        let mut msgs = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.hack_rx {
            loop {
                match rx.try_recv() {
                    Ok(m) => msgs.push(m),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let got_done = msgs.iter().any(|m| matches!(m, WorkerMsg::HackDone(_)));
        if disconnected && !got_done {
            self.hack_rx = None;
            self.busy = false;
            self.push_log(Level::Error, "后台线程异常退出，请重试");
        }
        for m in msgs {
            match m {
                WorkerMsg::Log(line) => self.push_log(line.level, line.text),
                WorkerMsg::HackDone(res) => {
                    self.hack_rx = None;
                    self.busy = false;
                    match res {
                        Ok(outcome) => {
                            self.push_log(
                                Level::Ok,
                                format!(
                                    "完成：命中 {} 条，{}",
                                    outcome.report.applied.len(),
                                    if outcome.changed {
                                        "已写入"
                                    } else if outcome.already_patched {
                                        "已是 Hook 版本"
                                    } else {
                                        "未写入"
                                    }
                                ),
                            );
                            if let Some(pid) = outcome.pid {
                                self.push_log(Level::Ok, format!("启动器 PID {pid}"));
                            }
                            self.toast = Some(Toast {
                                text: if outcome.changed {
                                    "Hack 完成，启动器已启动".into()
                                } else if outcome.already_patched {
                                    "已是 Hook 版本，直接启动".into()
                                } else {
                                    "完成，但本次没有命中规则".into()
                                },
                                level: if outcome.changed {
                                    Level::Ok
                                } else {
                                    Level::Warn
                                },
                                until: ctx.input(|i| i.time) + 3.4,
                            });
                            self.flash_until = ctx.input(|i| i.time) + 0.9;
                            self.backups = BackupStore::list();
                            self.outcome = Some(outcome);
                        }
                        Err(e) => {
                            self.push_log(Level::Error, format!("Hack 失败：{e}"));
                            self.toast = Some(Toast {
                                text: format!("失败：{e}"),
                                level: Level::Error,
                                until: ctx.input(|i| i.time) + 5.0,
                            });
                        }
                    }
                }
            }
        }
    }

    fn start_hack(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        let Some(target) = self.target.clone() else {
            self.toast = Some(Toast {
                text: "先选择并识别一个启动器".into(),
                level: Level::Warn,
                until: ctx.input(|i| i.time) + 3.0,
            });
            return;
        };
        self.busy = true;
        self.outcome = None;
        self.logs.clear();
        self.push_log(Level::Info, "================ HACK ================");
        self.push_log(
            Level::Info,
            format!(
                "目标 {} {} · {}",
                target.label,
                target.version,
                target.primary.display()
            ),
        );
        self.push_log(
            Level::Info,
            format!(
                "选项：移除更新={} · 预览={} · 自定义规则={} 字符",
                self.cfg.remove_updates,
                self.dry_run,
                self.custom_rules.trim().len()
            ),
        );
        self.cfg.custom_rules = self.custom_rules.clone();
        self.cfg.save();

        let opts = HackOptions {
            remove_updates: self.cfg.remove_updates,
            custom_rules: self.custom_rules.clone(),
            launch: !self.dry_run,
            dry_run: self.dry_run,
        };
        let (tx, rx) = mpsc::channel();
        self.hack_rx = Some(rx);
        let path = target.path.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let mut log = |line: LogLine| {
                let _ = tx2.send(WorkerMsg::Log(line));
            };
            let res = launcher::hack(&path, &opts, &mut log).map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerMsg::HackDone(res));
        });
    }

    fn do_browse(&mut self, file: bool) {
        let mut dlg = rfd::FileDialog::new().set_title(if file {
            "选择启动器（PCL2 / HMCL / PCL2-CE）"
        } else {
            "选择启动器所在文件夹"
        });
        if let Some(dir) = &self.cfg.last_dir {
            if dir.exists() {
                dlg = dlg.set_directory(dir);
            }
        }
        if file {
            dlg = dlg.add_filter("启动器", &["jar", "exe", "dll"]);
        }
        let picked = if file {
            dlg.pick_file()
        } else {
            dlg.pick_folder()
        };
        if let Some(p) = picked {
            self.cfg.last_dir = p.parent().map(Path::to_path_buf);
            self.start_detect(p);
        }
    }

    fn set_music(&mut self, on: bool) {
        self.cfg.music_enabled = on;
        if on && self.audio.is_none() {
            match AudioEngine::new() {
                Ok(mut a) => {
                    a.set_muted(self.cfg.muted);
                    a.set_volume(self.cfg.volume);
                    self.audio = Some(a);
                    self.audio_error = None;
                }
                Err(e) => self.audio_error = Some(format!("{e:#}")),
            }
        }
        if let Some(a) = &self.audio {
            a.set_muted(self.cfg.muted);
            a.set_playing(on);
        }
        self.cfg.save();
    }

    fn set_muted(&mut self, muted: bool) {
        self.cfg.muted = muted;
        if let Some(a) = &self.audio {
            a.set_muted(muted);
        }
        self.cfg.save();
    }

    fn do_restore(&mut self, entry: BackupEntry, ctx: &egui::Context) {
        let mut lines = Vec::new();
        match launcher::restore(&entry, &mut |l| lines.push(l)) {
            Ok(()) => {
                for l in lines {
                    self.push_log(l.level, l.text);
                }
                self.backups = BackupStore::list();
                self.target = None;
                self.detect_error = None;
                if entry.original_selection.exists() {
                    self.start_detect(entry.original_selection.clone());
                }
                self.toast = Some(Toast {
                    text: "已恢复备份".into(),
                    level: Level::Ok,
                    until: ctx.input(|i| i.time) + 3.2,
                });
            }
            Err(e) => {
                for l in lines {
                    self.push_log(l.level, l.text);
                }
                self.push_log(Level::Error, format!("恢复失败：{e:#}"));
            }
        }
    }

    fn restore_modal(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.pending_restore.clone() else {
            return;
        };
        let mut close = false;
        egui::Window::new("恢复备份")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(520.0);
                ui.label(format!("将 {} {} 恢复到：", entry.label, entry.version));
                ui.label(
                    RichText::new(entry.restore_target().display().to_string())
                        .monospace()
                        .color(MUTED),
                );
                ui.add_space(6.0);
                ui.label(RichText::new("当前文件会先自动备份，再覆盖。").color(AMBER));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                    let b =
                        egui::Button::new(RichText::new("确认恢复").color(Color32::BLACK).strong())
                            .fill(NEON);
                    if ui.add(b).clicked() {
                        close = true;
                        self.pending_restore = None;
                        self.do_restore(entry.clone(), ctx);
                    }
                });
            });
        if close {
            self.pending_restore = None;
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some(t) = self.toast.clone() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if now > t.until && t.until > 0.0 {
            self.toast = None;
            return;
        }
        let col = match t.level {
            Level::Ok => NEON,
            Level::Warn => AMBER,
            Level::Error => RED,
            Level::Info => TEXT,
        };
        egui::Area::new(Id::unique("toast"))
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 16.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(PANEL_3)
                    .stroke(Stroke::new(1.0, col))
                    .corner_radius(20)
                    .inner_margin(Margin::symmetric(18, 7))
                    .show(ui, |ui| {
                        ui.label(RichText::new(&t.text).color(col).size(13.0));
                    });
            });
    }

    // ---------- UI sections ----------

    fn top_bar(&mut self, ui: &mut Ui, t: f32) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(430.0, 22.0), Sense::hover());
                let p = ui.painter();
                let font = FontId::new(17.0, FontFamily::Name("press".into()));
                p.text(
                    rect.left_top() + Vec2::new(2.0, 2.0),
                    Align2::LEFT_TOP,
                    "LAUNCHER LAUNCHER",
                    font.clone(),
                    RED.linear_multiply(0.60),
                );
                p.text(
                    rect.left_top() + Vec2::new(1.0, 1.0),
                    Align2::LEFT_TOP,
                    "LAUNCHER LAUNCHER",
                    font.clone(),
                    CYAN.linear_multiply(0.55),
                );
                p.text(
                    rect.left_top(),
                    Align2::LEFT_TOP,
                    "LAUNCHER LAUNCHER",
                    font,
                    NEON,
                );
                ui.label(
                    RichText::new("// OFFLINE KEYGEN EDITION · 2026.10.20")
                        .size(8.5)
                        .color(CYAN)
                        .monospace(),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("[ ABOUT ]").size(8.5).monospace())
                            .fill(PANEL_2)
                            .stroke(Stroke::new(1.0, BORDER)),
                    )
                    .clicked()
                {
                    self.show_about = true;
                }
                ui.add_space(4.0);
                let (txt, col) = if self.busy {
                    ("BUSY", AMBER)
                } else if self.target.is_some() {
                    ("READY", NEON)
                } else {
                    ("IDLE", MUTED)
                };
                Frame::new()
                    .fill(Color32::BLACK)
                    .stroke(Stroke::new(1.0, col))
                    .corner_radius(0)
                    .inner_margin(Margin::symmetric(7, 3))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("● {txt}"))
                                .color(col)
                                .size(8.5)
                                .monospace(),
                        );
                    });
                ui.add_space(4.0);
                ui.label(RichText::new("v0.1.1").size(8.5).color(DIM).monospace());
            });
        });
        ui.add_space(2.0);
        let y = ui.cursor().top();
        let left = ui.max_rect().left();
        let right = ui.max_rect().right();
        let p = ui.painter();
        p.line_segment(
            [egui::pos2(left, y), egui::pos2(right, y)],
            Stroke::new(1.0, NEON_DIM),
        );
        p.line_segment(
            [egui::pos2(left, y + 2.0), egui::pos2(right, y + 2.0)],
            Stroke::new(1.0, BORDER),
        );
        let x = left + ((t * 150.0) % (right - left).max(1.0));
        p.line_segment(
            [egui::pos2(x - 54.0, y), egui::pos2(x, y)],
            Stroke::new(2.0, CYAN),
        );
        ui.add_space(5.0);
    }

    fn target_card(&mut self, ui: &mut Ui, _t: f32) {
        let fr = Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(0)
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                section_header(ui, "01", "// TARGET", "PCL2 · HMCL · PCL2-CE");
                ui.add_space(5.0);

                ui.horizontal(|ui| {
                    let total = ui.available_width();
                    let btn = 56.0;
                    let te_w = (total - btn * 2.0 - 18.0).max(120.0);
                    let resp = Frame::new()
                        .fill(Color32::BLACK)
                        .stroke(Stroke::new(1.0, NEON_DIM))
                        .corner_radius(0)
                        .inner_margin(Margin::symmetric(5, 3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(">").size(10.0).color(NEON).monospace(),
                                );
                                ui.add_sized(
                                    [ (te_w - 16.0).max(40.0), 18.0 ],
                                    TextEdit::singleline(&mut self.path_input)
                                        .font(TextStyle::Monospace)
                                        .frame(Frame::NONE)
                                        .clip_text(true)
                                        .hint_text(r"C:\...\PCL2.exe"),
                                )
                            })
                            .inner
                        })
                        .inner;
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.detect_from_input();
                    }
                    if ui
                        .add_sized(
                            [btn, 24.0],
                            egui::Button::new(RichText::new("[FILE]").size(8.0).monospace()),
                        )
                        .clicked()
                    {
                        self.browse_file = true;
                    }
                    if ui
                        .add_sized(
                            [btn, 24.0],
                            egui::Button::new(RichText::new("[DIR]").size(8.0).monospace()),
                        )
                        .clicked()
                    {
                        self.browse_dir = true;
                    }
                });

                ui.add_space(4.0);
                if self.detect_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("SCANNING TARGET ...")
                                .size(8.5)
                                .color(CYAN)
                                .monospace(),
                        );
                    });
                } else if let Some(err) = &self.detect_error {
                    ui.label(
                        RichText::new(format!("XX {err}"))
                            .size(9.0)
                            .color(RED)
                            .monospace(),
                    );
                } else if let Some(t) = &self.target {
                    ui.horizontal_wrapped(|ui| {
                        chip(ui, &t.label, NEON);
                        chip(ui, &format!("v{}", t.version), AMBER);
                        chip(ui, t.kind.label(), TEXT);
                        chip(ui, &util::human_bytes(t.bytes), MUTED);
                        chip(ui, &format!("#{}", util::short_hash(&t.fingerprint)), MUTED);
                        if t.kind == TargetKind::Pe && !cfg!(windows) {
                            chip(ui, "WINDOWS-ONLY", RED);
                        }
                    });
                    let path_w = ui.available_width();
                    ui.add_sized(
                        [path_w, 11.0],
                        egui::Label::new(
                            RichText::new(t.primary.display().to_string())
                                .monospace()
                                .size(7.5)
                                .color(DIM),
                        )
                        .truncate(),
                    );
                } else {
                    ui.label(
                        RichText::new("> NO TARGET SELECTED // 选择 .jar / .exe 或拖入窗口")
                            .color(MUTED)
                            .size(8.5)
                            .monospace(),
                    );
                }

                ui.add_space(7.0);
                warning_box(ui);
                ui.add_space(7.0);

                let mut remove_updates = self.cfg.remove_updates;
                if ui
                    .checkbox(
                        &mut remove_updates,
                        RichText::new("REMOVE UPDATE CHECKS / 移除更新")
                            .size(9.0)
                            .color(TEXT)
                            .monospace(),
                    )
                    .changed()
                {
                    self.cfg.remove_updates = remove_updates;
                    self.cfg.save();
                }
                ui.label(
                    RichText::new("> patch UpdateChecker / UpdateManager; no files deleted")
                        .size(8.0)
                        .color(DIM)
                        .monospace(),
                );

                ui.add_space(4.0);
                let adv = if self.show_advanced { "[-]" } else { "[+]" };
                if ui
                    .selectable_label(
                        self.show_advanced,
                        RichText::new(format!("{adv} ADVANCED RULES // JSON"))
                            .size(8.0)
                            .color(MUTED)
                            .monospace(),
                    )
                    .clicked()
                {
                    self.show_advanced = !self.show_advanced;
                }
                if self.show_advanced {
                    Frame::new()
                        .fill(Color32::BLACK)
                        .stroke(Stroke::new(1.0, BORDER))
                        .corner_radius(0)
                        .inner_margin(Margin::same(5))
                        .show(ui, |ui| {
                            ui.add(
                                TextEdit::multiline(&mut self.custom_rules)
                                    .font(TextStyle::Monospace)
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY)
                                    .hint_text(
                                        r#"[{"class":"...","method":"...","action":"return_false"}]"#,
                                    ),
                            );
                        });
                    ui.horizontal(|ui| {
                        let valid = patch::parse_custom_rules(&self.custom_rules).is_ok();
                        ui.label(
                            RichText::new(if valid { "JSON OK" } else { "JSON ERR" })
                                .size(8.0)
                                .color(if valid { NEON } else { RED })
                                .monospace(),
                        );
                        if ui
                            .small_button(RichText::new("[SAVE]").size(8.0).monospace())
                            .clicked()
                        {
                            match patch::parse_custom_rules(&self.custom_rules) {
                                Ok(r) => {
                                    self.cfg.custom_rules = self.custom_rules.clone();
                                    self.cfg.save();
                                    self.push_log(
                                        Level::Ok,
                                        format!("已保存 {} 条自定义规则", r.len()),
                                    );
                                }
                                Err(e) => self.push_log(
                                    Level::Error,
                                    format!("自定义规则 JSON 无效：{e}"),
                                ),
                            }
                        }
                        if ui
                            .small_button(RichText::new("[CLR]").size(8.0).monospace())
                            .clicked()
                        {
                            self.custom_rules.clear();
                        }
                        ui.checkbox(
                            &mut self.dry_run,
                            RichText::new("PREVIEW ONLY").size(8.0).monospace(),
                        );
                    });
                }

                ui.add_space(7.0);
                let enabled = self.target.is_some() && !self.busy;
                let label = if self.busy {
                    "[ HACKING ... ]"
                } else if self.dry_run {
                    "[ PREVIEW ]"
                } else {
                    "[ HACK ]"
                };
                let btn = egui::Button::new(
                    RichText::new(label)
                        .font(FontId::new(13.0, FontFamily::Name("press".into())))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(if self.busy {
                    AMBER
                } else if self.dry_run {
                    AMBER
                } else if enabled {
                    NEON
                } else {
                    NEON_DIM
                })
                .stroke(Stroke::new(1.0, if self.dry_run { AMBER } else { NEON }))
                .corner_radius(0)
                .min_size(egui::vec2(ui.available_width(), 38.0));
                let resp = ui.add_enabled(enabled, btn);
                if resp.hovered() {
                    ui.painter().rect_stroke(
                        resp.rect.expand(2.0),
                        CornerRadius::ZERO,
                        Stroke::new(1.0, CYAN),
                        StrokeKind::Outside,
                    );
                }
                if resp.clicked() {
                    self.hack_clicked = true;
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("AUTO-BACKUP // LOCAL ONLY // NO NETWORK")
                            .size(7.0)
                            .color(DIM)
                            .monospace(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button(RichText::new("[BACKUPS]").size(7.5).monospace())
                            .clicked()
                        {
                            self.open_backups = true;
                        }
                    });
                });
            });
        let r = fr.response.rect;
        ui.painter().rect_stroke(
            r.shrink(2.0),
            CornerRadius::ZERO,
            Stroke::new(1.0, BORDER.linear_multiply(0.55)),
            StrokeKind::Inside,
        );
    }

    fn music_bar(&mut self, ui: &mut Ui, t: f32) {
        Frame::new()
            .fill(PANEL_2)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(0)
            .inner_margin(Margin::same(7))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut music = self.cfg.music_enabled;
                    if ui
                        .checkbox(
                            &mut music,
                            RichText::new("CHIPTUNE / 注册机音乐")
                                .size(8.5)
                                .color(TEXT)
                                .monospace(),
                        )
                        .changed()
                    {
                        self.set_music(music);
                    }
                    if let Some(audio) = &self.audio {
                        let muted = audio.muted();
                        if ui
                            .small_button(
                                RichText::new(if muted { "[UNMUTE]" } else { "[MUTE]" })
                                    .size(8.0)
                                    .monospace(),
                            )
                            .clicked()
                        {
                            self.set_muted(!muted);
                        }
                        let mut vol = self.cfg.volume;
                        if ui
                            .add_sized(
                                [78.0, 16.0],
                                egui::Slider::new(&mut vol, 0.0..=1.0)
                                    .show_value(false)
                                    .trailing_fill(true),
                            )
                            .changed()
                        {
                            self.cfg.volume = vol;
                            if let Some(a) = &mut self.audio {
                                a.set_volume(vol);
                            }
                            self.cfg.save();
                        }
                    } else {
                        ui.label(RichText::new("AUDIO N/A").color(RED).size(8.0).monospace());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let active = self
                            .audio
                            .as_ref()
                            .map(|a| a.playing() && !a.muted())
                            .unwrap_or(false);
                        vu_meter(ui, t, active);
                        ui.label(
                            RichText::new("KEYGEN FM // 128 BPM")
                                .size(7.5)
                                .color(DIM)
                                .monospace(),
                        );
                    });
                });
                if let Some(err) = &self.audio_error {
                    ui.label(
                        RichText::new(format!("AUDIO ERR: {err}"))
                            .size(7.5)
                            .color(AMBER)
                            .monospace(),
                    );
                }
            });
    }

    fn log_panel(&mut self, ui: &mut Ui, log_h: f32) {
        let fr = Frame::new()
            .fill(Color32::from_rgb(1, 4, 2))
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(0)
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("02 // OUTPUT")
                            .size(9.0)
                            .color(NEON)
                            .monospace(),
                    );
                    if self.busy {
                        ui.spinner();
                    }
                    ui.add_space(4.0);
                    ui.spacing_mut().button_padding = egui::vec2(2.0, 1.0);
                    if ui
                        .small_button(RichText::new("CLR").size(6.5).monospace())
                        .clicked()
                    {
                        self.logs.clear();
                    }
                    if ui
                        .small_button(RichText::new("CPY").size(6.5).monospace())
                        .clicked()
                    {
                        let text = self
                            .logs
                            .iter()
                            .map(|l| format!("{} {} {}", l.time, tag_for(l.level), l.text))
                            .collect::<Vec<_>>()
                            .join("\n");
                        ui.ctx().copy_text(text);
                    }
                });
                ui.add_space(3.0);
                let log_w = ui.available_width();
                ScrollArea::vertical()
                    .id_salt("ll-log")
                    .stick_to_bottom(true)
                    .max_height(log_h)
                    .show(ui, |ui| {
                        ui.set_width(log_w);
                        if self.logs.is_empty() {
                            ui.label(
                                RichText::new("> awaiting target ...")
                                    .color(DIM)
                                    .monospace()
                                    .size(9.0),
                            );
                        }
                        for l in &self.logs {
                            let (tag, _) = level_style(l.level);
                            let text_col = match l.level {
                                Level::Error => RED,
                                Level::Warn => AMBER,
                                Level::Ok => NEON,
                                Level::Info => TEXT,
                            };
                            let line = format!("{} {} {}", l.time, tag, l.text);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(soft_break(&line))
                                        .color(text_col)
                                        .monospace()
                                        .size(8.0),
                                )
                                .wrap(),
                            );
                        }
                        let pulse = (ui.ctx().input(|i| i.time) * 2.0) as i64 % 2 == 0;
                        ui.label(
                            RichText::new(if self.busy {
                                "> running ..."
                            } else if pulse {
                                "> _"
                            } else {
                                ">  "
                            })
                            .size(9.0)
                            .color(if self.busy { AMBER } else { NEON })
                            .monospace(),
                        );
                    });
            });
        ui.painter().rect_stroke(
            fr.response.rect.shrink(2.0),
            CornerRadius::ZERO,
            Stroke::new(1.0, NEON_DIM.linear_multiply(0.55)),
            StrokeKind::Inside,
        );
    }

    fn system_panel(&mut self, ui: &mut Ui) {
        Frame::new()
            .fill(PANEL_2)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(0)
            .inner_margin(Margin::same(7))
            .show(ui, |ui| {
                section_header(ui, "04", "// SYSTEM", "LOCAL / OFFLINE");
                ui.add_space(3.0);
                for (k, v, col) in [
                    (
                        "MODE",
                        if self.busy { "BUSY" } else { "READY" },
                        if self.busy { AMBER } else { NEON },
                    ),
                    ("NET", "DISABLED", CYAN),
                    ("TELEMETRY", "FALSE", CYAN),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{k:<10}"))
                                .size(7.5)
                                .color(MUTED)
                                .monospace(),
                        );
                        ui.label(RichText::new(v).size(7.5).color(col).monospace());
                    });
                }
            });
    }

    fn about_modal(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open = true;
        egui::Window::new("ABOUT // LAUNCHER")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(2.0, NEON))
                    .corner_radius(0)
                    .inner_margin(Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.set_width(390.0);
                ui.label(
                    RichText::new("LAUNCHER LAUNCHER // v0.1.1")
                        .size(10.0)
                        .color(NEON)
                        .monospace(),
                );
                ui.separator();
                ui.label(
                    RichText::new("A local, offline launcher patcher.")
                        .size(9.0)
                        .color(TEXT)
                        .monospace(),
                );
                ui.label(
                    RichText::new("PCL2 2.99 / PCL2-CE / HMCL")
                        .size(9.0)
                        .color(CYAN)
                        .monospace(),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new("NO TELEMETRY. NO NETWORK. BACKUP FIRST.")
                        .size(8.0)
                        .color(AMBER)
                        .monospace(),
                );
                ui.label(
                    RichText::new("2026.10.20 // for the players")
                        .size(8.0)
                        .color(MUTED)
                        .monospace(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("[ CLOSE ]").size(9.0).monospace())
                                .fill(PANEL_2)
                                .stroke(Stroke::new(1.0, NEON)),
                        )
                        .clicked()
                    {
                        open = false;
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("[ ISSUE ]").size(9.0).monospace())
                                .fill(PANEL_2)
                                .stroke(Stroke::new(1.0, CYAN)),
                        )
                        .clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(ISSUE_URL));
                    }
                });
            });
        if !open {
            self.show_about = false;
        }
    }

    fn backup_panel(&mut self, ui: &mut Ui) {
        Frame::new()
            .fill(PANEL_2)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(0)
            .inner_margin(Margin::same(7))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let arrow = if self.show_backups { "[-]" } else { "[+]" };
                    if ui
                        .selectable_label(
                            self.show_backups,
                            RichText::new(format!(
                                "{arrow} 03 // BACKUPS ({})",
                                self.backups.len()
                            ))
                            .size(8.0)
                            .color(TEXT)
                            .monospace(),
                        )
                        .clicked()
                    {
                        self.show_backups = !self.show_backups;
                    }
                    ui.spacing_mut().button_padding = egui::vec2(2.0, 1.0);
                    if ui
                        .small_button(RichText::new("D").size(6.5).monospace())
                        .clicked()
                    {
                        self.open_backups = true;
                    }
                });
                if self.show_backups {
                    ui.separator();
                    if self.backups.is_empty() {
                        ui.label(
                            RichText::new("> no snapshots yet")
                                .size(8.0)
                                .color(MUTED)
                                .monospace(),
                        );
                    }
                    let backup_w = ui.available_width();
                    ScrollArea::vertical()
                        .id_salt("ll-backups")
                        .max_height(92.0)
                        .show(ui, |ui| {
                            ui.set_width(backup_w);
                            for i in 0..self.backups.len().min(6) {
                                let b = self.backups[i].clone();
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{} {}", b.label, b.version))
                                            .size(8.0)
                                            .color(TEXT)
                                            .monospace(),
                                    );
                                    ui.spacing_mut().button_padding = egui::vec2(2.0, 1.0);
                                    if ui
                                        .small_button(RichText::new("L").size(6.5).monospace())
                                        .clicked()
                                    {
                                        self.pending_restore = Some(b.clone());
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 5.0;
                                    ui.label(
                                        RichText::new(util::fmt_ts(b.created_at))
                                            .size(7.0)
                                            .color(MUTED)
                                            .monospace(),
                                    );
                                    ui.label(
                                        RichText::new(util::human_bytes(b.size))
                                            .size(7.0)
                                            .color(DIM)
                                            .monospace(),
                                    );
                                    ui.label(
                                        RichText::new(util::short_hash(&b.fingerprint))
                                            .size(7.0)
                                            .color(DIM)
                                            .monospace(),
                                    );
                                });
                                ui.add_space(1.0);
                            }
                            if self.backups.len() > 6 {
                                ui.label(
                                    RichText::new(format!("… +{} older", self.backups.len() - 6))
                                        .size(7.0)
                                        .color(DIM)
                                        .monospace(),
                                );
                            }
                        });
                }
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let t = self.started.elapsed().as_secs_f32();

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(p) = dropped.iter().find_map(|f| Some(f.path().to_path_buf())) {
            self.start_detect(p);
        }
        self.drop_hover = ctx.input(|i| !i.raw.hovered_files.is_empty());

        if !ctx.egui_wants_keyboard_input() {
            let cmd = ctx.input(|i| i.modifiers.command || i.modifiers.ctrl);
            if cmd && ctx.input(|i| i.key_pressed(egui::Key::H)) {
                self.hack_clicked = true;
            }
            if cmd && ctx.input(|i| i.key_pressed(egui::Key::O)) {
                self.browse_file = true;
            }
        }

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG))
            .show(ui, |ui| {
                draw_grid(ui, t);
                ScrollArea::vertical()
                    .id_salt("ll-page")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        let total = ui.available_width();
                        let content_w = (total - 24.0).clamp(320.0, 1200.0);
                        let pad = ((total - content_w) * 0.5).max(0.0);
                        ui.horizontal(|ui| {
                            ui.add_space(pad);
                            ui.vertical(|ui| {
                                ui.set_width(content_w);
                                self.top_bar(ui, t);
                                ui.add_space(10.0);
                                let log_h =
                                    (ctx.content_rect().height() - 320.0).clamp(190.0, 320.0);
                                if content_w >= 760.0 {
                                    ui.horizontal_top(|ui| {
                                        let gap = 12.0;
                                        let left_w = ((content_w - gap) * 0.58).round();
                                        let right_w = content_w - gap - left_w;
                                        ui.vertical(|ui| {
                                            ui.set_width(left_w);
                                            self.target_card(ui, t);
                                            ui.add_space(8.0);
                                            self.music_bar(ui, t);
                                            ui.add_space(4.0);
                                            ui.label(
                                                RichText::new(
                                                    ">> 05 / GREETZ: BACKUP FIRST. PATCH LOCAL. NO TELEMETRY.",
                                                )
                                                .size(7.0)
                                                .color(DIM)
                                                .monospace(),
                                            );
                                        });
                                        ui.add_space(gap);
                                        ui.vertical(|ui| {
                                            ui.set_width(right_w);
                                            self.log_panel(ui, log_h);
                                            ui.add_space(8.0);
                                            self.backup_panel(ui);
                                            ui.add_space(8.0);
                                            self.system_panel(ui);
                                        });
                                    });
                                } else {
                                    self.target_card(ui, t);
                                    ui.add_space(10.0);
                                    self.music_bar(ui, t);
                                    ui.add_space(10.0);
                                    self.log_panel(ui, log_h);
                                    ui.add_space(8.0);
                                    self.backup_panel(ui);
                                }
                                ui.add_space(8.0);
                                scrolltext(ui, t);
                            });
                        });
                        ui.add_space(14.0);
                    });
            });

        screen_frame(&ctx);
        self.restore_modal(&ctx);
        self.about_modal(&ctx);
        self.toast_ui(&ctx);
        if self.drop_hover {
            drop_overlay(&ctx);
        }
        if self.flash_until > ctx.input(|i| i.time) {
            let left = (self.flash_until - ctx.input(|i| i.time)) as f32;
            let p = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::unique("hack-flash"),
            ));
            p.rect_stroke(
                ctx.content_rect().shrink(3.0),
                CornerRadius::ZERO,
                Stroke::new(2.0, NEON.linear_multiply(left.clamp(0.0, 0.9))),
                StrokeKind::Inside,
            );
        }

        if self.browse_file {
            self.browse_file = false;
            self.do_browse(true);
        }
        if self.browse_dir {
            self.browse_dir = false;
            self.do_browse(false);
        }
        if self.hack_clicked {
            self.hack_clicked = false;
            self.start_hack(&ctx);
        }
        if self.open_backups {
            self.open_backups = false;
            open_path(&paths::backups_dir());
        }

        if let Some(path) = &self.screenshot_path {
            self.shot_frames += 1;
            ctx.request_repaint();
            if self.shot_frames >= if self.auto_hack { 55 } else { 20 } && !self.shot_done {
                self.shot_done = true;
                let path = path.clone();
                let exit = self.screenshot_exit;
                let ctx2 = ctx.clone();
                let cb = egui::ScreenshotCallback::new(move |image| {
                    let w = image.size[0] as u32;
                    let h = image.size[1] as u32;
                    let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
                    for px in &image.pixels {
                        rgba.extend_from_slice(&px.to_srgba_unmultiplied());
                    }
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8) {
                        Ok(()) => eprintln!("[screenshot] {}", path.display()),
                        Err(e) => eprintln!("[screenshot] failed: {e}"),
                    }
                    if exit {
                        ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(cb));
            }
        }

        if self.busy || self.cfg.music_enabled {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

fn clock() -> String {
    let s = util::now_secs() % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Insert zero-width spaces after separators so long class/method names wrap
/// inside the narrow log column instead of being clipped.
fn soft_break(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 6);
    for c in s.chars() {
        out.push(c);
        if matches!(c, '/' | '.' | '_' | '(' | ')') {
            out.push('\u{200B}');
        }
    }
    out
}

fn tag_for(l: Level) -> &'static str {
    match l {
        Level::Info => ">>",
        Level::Ok => "OK",
        Level::Warn => "!!",
        Level::Error => "XX",
    }
}

fn level_style(l: Level) -> (&'static str, Color32) {
    match l {
        Level::Info => (">>", MUTED),
        Level::Ok => ("OK", NEON),
        Level::Warn => ("!!", AMBER),
        Level::Error => ("XX", RED),
    }
}

fn chip(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(Color32::BLACK)
        .stroke(Stroke::new(1.0, color.linear_multiply(0.55)))
        .corner_radius(0)
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(7.5).color(color).monospace());
        });
}

fn section_header(ui: &mut Ui, num: &str, title: &str, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{num} >>"))
                .size(8.0)
                .color(AMBER)
                .monospace(),
        );
        ui.label(RichText::new(title).size(10.0).color(NEON).monospace());
        ui.label(RichText::new(hint).size(7.0).color(DIM).monospace());
    });
}

fn warning_box(ui: &mut Ui) {
    Frame::new()
        .fill(RED_BG)
        .stroke(Stroke::new(1.0, RED))
        .corner_radius(0)
        .inner_margin(Margin::same(7))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("!").size(16.0).color(RED).monospace());
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("WARNING // 警告")
                            .size(8.0)
                            .color(RED)
                            .monospace(),
                    );
                    ui.label(
                        RichText::new("千万不要在这里选择你的启动器并按下 Hack 按钮，")
                            .size(9.0)
                            .color(AMBER)
                            .monospace(),
                    );
                    ui.label(
                        RichText::new("这会让你能绕过审查正常下载 Minecraft。")
                            .size(9.0)
                            .color(AMBER)
                            .monospace(),
                    );
                });
            });
        });
}

fn vu_meter(ui: &mut Ui, t: f32, active: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(70.0, 14.0), Sense::hover());
    let p = ui.painter();
    let n = 14usize;
    let w = rect.width() / n as f32;
    for i in 0..n {
        let h = if active {
            let wave = ((t * 7.0 + i as f32 * 0.75).sin() * 0.5 + 0.5).powf(1.4);
            let tilt = 0.30 + 0.70 * (i as f32 / n as f32);
            2.0 + wave * tilt * rect.height()
        } else {
            2.0
        };
        let x = rect.left() + i as f32 * w + 1.0;
        let bar = egui::Rect::from_min_size(
            egui::pos2(x, rect.bottom() - h),
            Vec2::new((w - 2.0).max(1.0), h),
        );
        let col = if i >= n * 3 / 4 { AMBER } else { NEON };
        p.rect_filled(
            bar,
            CornerRadius::ZERO,
            col.linear_multiply(if active { 0.9 } else { 0.22 }),
        );
    }
}

fn scrolltext(ui: &mut Ui, t: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
    let p = ui.painter();
    p.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, NEON_DIM),
    );
    let text = "LAUNCHER LAUNCHER 2026 * OFFLINE * NO TELEMETRY * AUTO BACKUP * FOR THE PLAYERS * GREETZ TO ALL LAUNCHER DEVS * HACK THE PLANET * ";
    let font = FontId::new(8.0, FontFamily::Monospace);
    let galley = p.layout_no_wrap(text.to_owned(), font, NEON);
    let span = galley.size().x + 60.0;
    let x = rect.right() - ((t * 44.0) % span.max(1.0));
    let y = rect.top() + 2.0;
    let clip = p.with_clip_rect(rect);
    clip.galley(egui::pos2(x - span, y), galley.clone(), NEON);
    clip.galley(egui::pos2(x, y), galley.clone(), NEON);
    clip.galley(egui::pos2(x + span, y), galley, NEON.linear_multiply(0.85));
}

fn draw_grid(ui: &Ui, t: f32) {
    let rect = ui.max_rect();
    let p = ui.painter();
    let step = 32.0;
    let mut y = rect.top();
    while y < rect.bottom() {
        p.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(35, 90, 55, 8)),
        );
        y += step;
    }
    let mut x = rect.left();
    while x < rect.right() {
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(35, 90, 55, 6)),
        );
        x += step;
    }
    let mut sy = rect.top();
    while sy < rect.bottom() {
        p.line_segment(
            [egui::pos2(rect.left(), sy), egui::pos2(rect.right(), sy)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 18)),
        );
        sy += 3.0;
    }
    let scan_y = rect.top() + ((t * 28.0) % rect.height().max(1.0));
    p.line_segment(
        [
            egui::pos2(rect.left(), scan_y),
            egui::pos2(rect.right(), scan_y),
        ],
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 229, 255, 22)),
    );
}

fn screen_frame(ctx: &egui::Context) {
    let r = ctx.content_rect().shrink(4.0);
    let p = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        Id::unique("screen-frame"),
    ));
    p.rect_stroke(
        r,
        CornerRadius::ZERO,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(57, 255, 20, 28)),
        StrokeKind::Inside,
    );
    let len = 18.0;
    for (corner, dx, dy) in [
        (r.left_top(), 1.0, 1.0),
        (r.right_top(), -1.0, 1.0),
        (r.left_bottom(), 1.0, -1.0),
        (r.right_bottom(), -1.0, -1.0),
    ] {
        p.line_segment(
            [corner, egui::pos2(corner.x + len * dx, corner.y)],
            Stroke::new(2.0, NEON),
        );
        p.line_segment(
            [corner, egui::pos2(corner.x, corner.y + len * dy)],
            Stroke::new(2.0, NEON),
        );
    }
}

fn drop_overlay(ctx: &egui::Context) {
    let rect = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        Id::unique("drop-overlay"),
    ));
    p.rect_filled(
        rect,
        CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(6, 10, 8, 208),
    );
    p.rect_stroke(
        rect.shrink(22.0),
        CornerRadius::same(12),
        Stroke::new(2.0, NEON),
        StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "松开以载入启动器",
        FontId::new(24.0, FontFamily::Proportional),
        NEON,
    );
}

fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}
