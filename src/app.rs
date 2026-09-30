use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};

use crate::config::{AppConfig, HotkeyConfig, ParseParams, Settings};
use crate::geometry::{self, DrawBox};
use crate::hotkeys::{Action, Hotkeys};
use crate::messages::{AppMessage, ParseResult, WorkerToAppMsg};
use crate::worker::{DrawControl, drawing_worker_thread};

/// 未能检测到显示器时用于预览估算的屏幕尺寸
const FALLBACK_SCREEN: (i32, i32) = (1920, 1080);

#[derive(Clone, Copy, PartialEq)]
enum DrawMode {
    Edges,
    Mist,
}

/// 预览区显示的统计（依赖轨迹与绘画参数，变化时才重新计算）
struct PreviewStats {
    key: (usize, AppConfig),
    estimate: Duration,
    drawn_px: f32,
    travel_px: f32,
}

#[derive(Default)]
struct Selection {
    active: bool,
    start: Option<egui::Pos2>,
    current: Option<egui::Pos2>,
    screenshot: Option<egui::TextureHandle>,
}

pub struct AutoDrawerApp {
    settings: Settings,
    hotkey_draft: HotkeyConfig,
    hotkeys: Hotkeys,
    hotkey_errors: Vec<String>,
    primary_size: (i32, i32),

    control: Arc<DrawControl>,
    tx_app: Sender<AppMessage>,
    rx_worker: Receiver<WorkerToAppMsg>,
    rx_hotkey: Receiver<u32>,

    last_parse_params: ParseParams,
    parsed: Option<ParseResult>,
    preview_stats: Option<PreviewStats>,
    drawing_mode: Option<DrawMode>,
    error_msg: Option<String>,
    selection: Selection,
}

impl AutoDrawerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_font(&cc.egui_ctx);

        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();

        let primary = primary_monitor();
        let primary_size = primary
            .map(|(_, _, w, h)| (w, h))
            .unwrap_or(FALLBACK_SCREEN);
        if settings.config.monitor_w <= 0
            && let Some((x, y, w, h)) = primary
        {
            let c = &mut settings.config;
            (c.monitor_x, c.monitor_y, c.monitor_w, c.monitor_h) = (x, y, w, h);
        }

        let control = Arc::new(DrawControl::default());
        let (tx_app, rx_app) = unbounded();
        let (tx_worker, rx_worker) = unbounded();
        let (tx_hotkey, rx_hotkey) = unbounded();

        let worker_control = Arc::clone(&control);
        let worker_ctx = cc.egui_ctx.clone();
        thread::spawn(move || drawing_worker_thread(rx_app, tx_worker, worker_control, worker_ctx));

        let (mut hotkeys, hotkey_init_err) = Hotkeys::new();
        let mut hotkey_errors: Vec<String> = hotkey_init_err.into_iter().collect();
        if hotkey_errors.is_empty() {
            hotkey_errors = hotkeys.apply(&settings.hotkeys);
        }

        let hotkey_ctx = cc.egui_ctx.clone();
        // 全局热键事件统一转发到 UI 线程处理
        thread::spawn(move || {
            let rx = GlobalHotKeyEvent::receiver();
            while let Ok(event) = rx.recv() {
                if event.state == HotKeyState::Pressed {
                    let _ = tx_hotkey.send(event.id);
                    hotkey_ctx.request_repaint();
                }
            }
        });

        // 恢复上次加载的图片
        if let Some(path) = &settings.image_path {
            if std::path::Path::new(path).exists() {
                let _ = tx_app.send(AppMessage::LoadImage(
                    path.clone(),
                    settings.config.parse_params(),
                ));
            } else {
                settings.image_path = None;
            }
        }

        Self {
            last_parse_params: settings.config.parse_params(),
            hotkey_draft: settings.hotkeys.clone(),
            settings,
            hotkeys,
            hotkey_errors,
            primary_size,
            control,
            tx_app,
            rx_worker,
            rx_hotkey,
            parsed: None,
            preview_stats: None,
            drawing_mode: None,
            error_msg: None,
            selection: Selection::default(),
        }
    }

    fn is_drawing(&self) -> bool {
        self.control.is_drawing.load(Ordering::Acquire)
    }

    fn is_paused(&self) -> bool {
        self.control.is_paused.load(Ordering::Acquire)
    }

    fn start(&mut self, mode: DrawMode) {
        if mode == DrawMode::Edges && self.parsed.is_none() {
            self.error_msg = Some("请先加载图片".into());
            return;
        }
        if self
            .control
            .is_drawing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.control.is_paused.store(false, Ordering::Release);
        self.control.progress_done.store(0, Ordering::Release);
        self.drawing_mode = Some(mode);
        self.error_msg = None;
        let msg = match mode {
            DrawMode::Edges => AppMessage::StartDrawing(self.settings.config),
            DrawMode::Mist => AppMessage::StartMistMode(self.settings.config),
        };
        let _ = self.tx_app.send(msg);
    }

    fn stop(&self) {
        self.control.is_drawing.store(false, Ordering::Release);
    }

    fn toggle_pause(&self) {
        if self.is_drawing() {
            self.control.is_paused.fetch_xor(true, Ordering::AcqRel);
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Start => self.start(DrawMode::Edges),
            Action::Stop => self.stop(),
            Action::Pause => self.toggle_pause(),
            Action::Mist => self.start(DrawMode::Mist),
        }
    }

    fn handle_hotkeys(&mut self) {
        while let Ok(id) = self.rx_hotkey.try_recv() {
            if let Some(action) = self.hotkeys.action_for(id) {
                self.run_action(action);
            }
        }
    }

    fn handle_worker_messages(&mut self) {
        while let Ok(msg) = self.rx_worker.try_recv() {
            match msg {
                WorkerToAppMsg::Parsed(result) => {
                    self.parsed = Some(result);
                    self.error_msg = None;
                }
                WorkerToAppMsg::DrawingFinished => {}
                WorkerToAppMsg::Error(err) => self.error_msg = Some(err),
            }
        }
    }

    fn button_label(&self, text: &str, action: Action) -> String {
        match self.hotkeys.describe(action) {
            Some(key) => format!("{text} ({key})"),
            None => text.to_string(),
        }
    }

    fn update_preview_stats(&mut self) {
        let Some(parsed) = &self.parsed else {
            self.preview_stats = None;
            return;
        };
        let key = (Arc::as_ptr(&parsed.lines) as usize, self.settings.config);
        if self.preview_stats.as_ref().is_some_and(|s| s.key == key) {
            return;
        }
        let (w, h) = parsed.image_size;
        let draw_box = DrawBox::from_config(&self.settings.config, self.primary_size);
        let strokes = draw_box.map_strokes(&parsed.lines, w as f32, h as f32);
        let (drawn_px, travel_px) = geometry::path_lengths(&strokes);
        self.preview_stats = Some(PreviewStats {
            key,
            estimate: geometry::estimate_duration(&strokes, &self.settings.config),
            drawn_px,
            travel_px,
        });
    }

    fn enter_selection_mode(&mut self, ctx: &egui::Context) {
        // 截取程序窗口当前所在的显示器，与全屏后的窗口保持一致
        let window_center = ctx.input(|i| {
            let ppp = i.viewport().native_pixels_per_point.unwrap_or(1.0);
            i.viewport().outer_rect.map(|r| r.center().to_vec2() * ppp)
        });
        let monitor = window_center
            .and_then(|c| xcap::Monitor::from_point(c.x as i32, c.y as i32).ok())
            .or_else(|| {
                let all = xcap::Monitor::all().ok()?;
                let primary = all
                    .iter()
                    .position(|m| m.is_primary().unwrap_or(false))
                    .unwrap_or(0);
                all.into_iter().nth(primary)
            });
        let Some(monitor) = monitor else {
            self.error_msg = Some("无法获取显示器设备".into());
            return;
        };

        let captured = (|| {
            let geometry = (
                monitor.x()?,
                monitor.y()?,
                monitor.width()?,
                monitor.height()?,
            );
            Ok::<_, xcap::XCapError>((geometry, monitor.capture_image()?))
        })();

        match captured {
            Ok(((x, y, w, h), image)) => {
                let c = &mut self.settings.config;
                (c.monitor_x, c.monitor_y, c.monitor_w, c.monitor_h) = (x, y, w as i32, h as i32);
                let size = [image.width() as usize, image.height() as usize];
                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &image.into_raw());
                self.selection.screenshot =
                    Some(ctx.load_texture("screenshot", color_image, egui::TextureOptions::LINEAR));
                self.selection.active = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            }
            Err(e) => self.error_msg = Some(format!("读取显示器属性或截屏失败: {e:?}")),
        }
    }

    fn exit_selection_mode(&mut self, ctx: &egui::Context) {
        self.selection = Selection::default();
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }

    fn render_selection_mode(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let screen = ui.max_rect();
                if let Some(tex) = &self.selection.screenshot {
                    ui.painter().image(
                        tex.id(),
                        screen,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(102));

                ui.label(
                    egui::RichText::new("已冻结屏幕。请拖拽鼠标框选游戏画布区域\n按 ESC 退出")
                        .color(egui::Color32::WHITE)
                        .size(24.0),
                );

                let response = ui.allocate_response(ui.available_size(), egui::Sense::drag());
                if response.drag_started() {
                    self.selection.start = response.interact_pointer_pos();
                }
                if response.dragged() {
                    self.selection.current = response.interact_pointer_pos();
                }
                if let (Some(start), Some(curr)) = (self.selection.start, self.selection.current) {
                    ui.painter().rect_stroke(
                        egui::Rect::from_two_pos(start, curr),
                        0.0,
                        egui::Stroke::new(2.0, egui::Color32::RED),
                        egui::StrokeKind::Middle,
                    );
                }

                if response.drag_stopped() {
                    if let (Some(start), Some(curr)) =
                        (self.selection.start, self.selection.current)
                    {
                        let r = egui::Rect::from_two_pos(start, curr);
                        if r.width() > 10.0 && r.height() > 10.0 {
                            let pct_x = |v: f32| {
                                ((v - screen.min.x) / screen.width() * 100.0).clamp(0.0, 100.0)
                            };
                            let pct_y = |v: f32| {
                                ((v - screen.min.y) / screen.height() * 100.0).clamp(0.0, 100.0)
                            };
                            let c = &mut self.settings.config;
                            c.left_margin = pct_x(r.min.x);
                            c.right_margin = 100.0 - pct_x(r.max.x);
                            c.top_margin = pct_y(r.min.y);
                            c.bottom_margin = 100.0 - pct_y(r.max.y);
                        }
                    }
                    self.exit_selection_mode(ctx);
                } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.exit_selection_mode(ctx);
                }
            });
    }

    fn render_side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("left_panel")
            .default_width(320.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("img2spire");
                    ui.separator();
                    self.ui_image_picker(ui);
                    ui.separator();
                    self.ui_parameters(ui);
                    ui.separator();
                    self.ui_draw_area(ui, ctx);
                    ui.separator();
                    self.ui_hotkeys(ui);
                    ui.separator();
                    self.ui_controls(ui);
                    ui.separator();
                    self.ui_status(ui);
                });
            });
    }

    fn ui_image_picker(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("加载图片").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Image", &["png", "jpg", "jpeg", "bmp", "webp", "gif"])
                    .pick_file()
            {
                let path_str = path.display().to_string();
                self.settings.image_path = Some(path_str.clone());
                let _ = self.tx_app.send(AppMessage::LoadImage(
                    path_str,
                    self.settings.config.parse_params(),
                ));
            }
            match &self.settings.image_path {
                Some(path) => {
                    let mut file_name = std::path::Path::new(path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if file_name.chars().count() > 25 {
                        file_name = file_name.chars().take(22).collect::<String>() + "...";
                    }
                    ui.label(format!("已选择: {file_name}")).on_hover_text(path);
                }
                None => {
                    ui.label("未选择图片");
                }
            }
        });
    }

    fn ui_parameters(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.settings.config;
        ui.label("视觉参数设置:");
        ui.add(egui::Slider::new(&mut c.edge_threshold_low, 0.0..=255.0).text("Canny 低阈值"));
        ui.add(egui::Slider::new(&mut c.edge_threshold_high, 0.0..=255.0).text("Canny 高阈值"));
        ui.add(egui::Slider::new(&mut c.simplify_tolerance, 0.1..=10.0).text("防抖精度"));

        ui.label("绘画参数设置:");
        ui.add(egui::Slider::new(&mut c.draw_delay_ms, 0..=50).text("每点延迟 (ms)"));
        ui.add(egui::Slider::new(&mut c.drag_step_px, 1.0..=100.0).text("最大直线冲刺 (像素)"));
        ui.add(egui::Slider::new(&mut c.mist_spacing_px, 2.0..=100.0).text("填涂行间距 (像素)"));
    }

    fn ui_draw_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label("绘制区域 (屏幕占比 %):");
        let c = &mut self.settings.config;
        fn margin<'a>(v: &'a mut f32, text: &str) -> egui::Slider<'a> {
            egui::Slider::new(v, 0.0..=95.0)
                .text(text)
                .fixed_decimals(1)
        }
        ui.horizontal(|ui| {
            ui.add(margin(&mut c.left_margin, "左"));
            ui.add(margin(&mut c.right_margin, "右"));
        });
        ui.horizontal(|ui| {
            ui.add(margin(&mut c.top_margin, "上"));
            ui.add(margin(&mut c.bottom_margin, "下"));
        });
        ui.label(
            egui::RichText::new(format!(
                "目标显示器: {}×{} @ ({}, {})",
                c.monitor_w, c.monitor_h, c.monitor_x, c.monitor_y
            ))
            .weak(),
        );

        if ui.button("✂ 手动框选作画区域").clicked() {
            self.enter_selection_mode(ctx);
        }
    }

    fn ui_hotkeys(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("热键设置").show(ui, |ui| {
            ui.label(
                egui::RichText::new("格式示例: F9、ctrl+F9、alt+shift+KeyD；留空表示不绑定").weak(),
            );
            egui::Grid::new("hotkey_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    for action in Action::ALL {
                        ui.label(action.label());
                        ui.add(
                            egui::TextEdit::singleline(action.key_mut(&mut self.hotkey_draft))
                                .desired_width(140.0),
                        );
                        ui.end_row();
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("应用").clicked() {
                    self.hotkey_errors = self.hotkeys.apply(&self.hotkey_draft);
                    if crate::hotkeys::parse_config(&self.hotkey_draft).is_ok() {
                        self.settings.hotkeys = self.hotkey_draft.clone();
                    }
                }
                if ui.button("恢复默认").clicked() {
                    self.hotkey_draft = HotkeyConfig::default();
                }
            });
        });
        for err in &self.hotkey_errors {
            ui.colored_label(egui::Color32::RED, err);
        }
    }

    fn ui_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if !self.is_drawing() {
                if ui
                    .button(self.button_label("▶ 开始绘画", Action::Start))
                    .clicked()
                {
                    self.start(DrawMode::Edges);
                }
                if ui
                    .button(self.button_label("暴力涂抹绘画区域", Action::Mist))
                    .clicked()
                {
                    self.start(DrawMode::Mist);
                }
            } else {
                if ui
                    .button(self.button_label("⏹ 停止绘画", Action::Stop))
                    .clicked()
                {
                    self.stop();
                }
                let text = if self.is_paused() {
                    "▶ 继续"
                } else {
                    "⏸ 暂停"
                };
                if ui.button(self.button_label(text, Action::Pause)).clicked() {
                    self.toggle_pause();
                }
            }
        });
    }

    fn ui_status(&mut self, ui: &mut egui::Ui) {
        ui.label("状态监控:");
        let status = match (self.is_drawing(), self.is_paused()) {
            (true, true) => "已暂停",
            (true, false) => "正在绘画...",
            _ => "空闲",
        };
        ui.label(format!("当前状态: {status}"));

        if self.is_drawing() {
            let done = self.control.progress_done.load(Ordering::Acquire);
            let total = self.control.progress_total.load(Ordering::Acquire).max(1);
            ui.add(egui::ProgressBar::new(done as f32 / total as f32).show_percentage());
            // 绘画中需要持续刷新进度
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        if let Some(err) = &self.error_msg {
            ui.colored_label(egui::Color32::RED, format!("错误: {err}"));
        }

        if let Some(parsed) = &self.parsed {
            let (orig, opt) = (parsed.raw_points, parsed.optimized_points);
            ui.label(format!("解析结果: {} 条线段", parsed.lines.len()));
            ui.label(format!("原始点数: {orig}"));
            ui.label(format!("防抖后点数: {opt}"));
            let reduction = if orig > 0 {
                orig.saturating_sub(opt) as f32 / orig as f32 * 100.0
            } else {
                0.0
            };
            ui.label(format!("坐标压缩率: {reduction:.1}%"));
        }
        if let Some(stats) = &self.preview_stats {
            ui.label(format!("预计耗时: 约 {}", format_duration(stats.estimate)));
            ui.label(format!(
                "落笔长度: {:.0} px / 空走长度: {:.0} px",
                stats.drawn_px, stats.travel_px
            ));
        }
    }

    fn render_preview(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("轨迹预览");
                ui.checkbox(&mut self.settings.show_travel, "显示抬笔空走路径");
            });

            let Some(parsed) = &self.parsed else {
                ui.centered_and_justified(|ui| {
                    ui.label("请加载图片以生成预览");
                });
                return;
            };
            let (img_w, img_h) = parsed.image_size;

            // 绘画中已画过的点数，用于区分已画/未画的部分
            let drawn_points = (self.is_drawing() && self.drawing_mode == Some(DrawMode::Edges))
                .then(|| self.control.progress_done.load(Ordering::Acquire));

            egui::Frame::dark_canvas(ui.style()).show(ui, |ui| {
                let available = ui.available_size();
                let scale = (available.x / img_w as f32)
                    .min(available.y / img_h as f32)
                    .min(1.0);
                if scale <= 0.0 {
                    return;
                }
                let (response, painter) = ui.allocate_painter(
                    egui::vec2(img_w as f32 * scale, img_h as f32 * scale),
                    egui::Sense::hover(),
                );
                let rect = response.rect;
                painter.rect_filled(rect, 0.0, egui::Color32::from_gray(30));

                let to_screen = |&(x, y): &(f32, f32)| {
                    egui::pos2(rect.min.x + x * scale, rect.min.y + y * scale)
                };
                let pending = egui::Stroke::new(1.5, egui::Color32::GREEN);
                let done = egui::Stroke::new(1.5, egui::Color32::from_rgb(60, 120, 60));
                let travel =
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 170, 0, 140));

                let mut counted = 0usize;
                let mut prev_end: Option<egui::Pos2> = None;
                for line in parsed.lines.iter() {
                    let (Some(first), Some(last)) = (line.first(), line.last()) else {
                        continue;
                    };
                    if self.settings.show_travel {
                        if let Some(end) = prev_end {
                            painter.add(egui::Shape::dashed_line(
                                &[end, to_screen(first)],
                                travel,
                                4.0,
                                3.0,
                            ));
                        }
                        prev_end = Some(to_screen(last));
                    }

                    counted += line.len();
                    let stroke = match drawn_points {
                        Some(n) if counted <= n => done,
                        _ => pending,
                    };
                    if line.len() >= 2 {
                        painter.add(egui::Shape::line(
                            line.iter().map(to_screen).collect(),
                            stroke,
                        ));
                    }
                }
            });
        });
    }
}

impl eframe::App for AutoDrawerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_hotkeys();
        self.handle_worker_messages();

        if self.selection.active {
            self.render_selection_mode(ctx);
            return;
        }

        self.render_side_panel(ctx);

        // 只有影响解析结果的参数变化时才重新解析；绘画期间发送的请求会在绘画结束后处理
        let params = self.settings.config.parse_params();
        if params != self.last_parse_params {
            self.last_parse_params = params;
            if self.settings.image_path.is_some() {
                let _ = self.tx_app.send(AppMessage::Reparse(params));
            }
        }

        self.update_preview_stats();
        self.render_preview(ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f32();
    if secs < 60.0 {
        format!("{secs:.1} 秒")
    } else {
        let secs = d.as_secs();
        format!("{} 分 {:02} 秒", secs / 60, secs % 60)
    }
}

/// 返回主显示器的 (x, y, w, h)
fn primary_monitor() -> Option<(i32, i32, i32, i32)> {
    let monitors = xcap::Monitor::all().ok()?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())?;
    Some((
        monitor.x().ok()?,
        monitor.y().ok()?,
        monitor.width().ok()? as i32,
        monitor.height().ok()? as i32,
    ))
}

fn install_cjk_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var("windir").unwrap_or_else(|_| "C:\\Windows".to_string());
    let font_paths = [
        format!("{windir}\\Fonts\\msyh.ttc"),
        format!("{windir}\\Fonts\\msyh.ttf"),
        format!("{windir}\\Fonts\\simhei.ttf"),
    ];

    for path in font_paths {
        if let Ok(font_data) = std::fs::read(&path) {
            fonts.font_data.insert(
                "cjk".to_owned(),
                Arc::new(egui::FontData::from_owned(font_data)),
            );
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                family.insert(0, "cjk".to_owned());
            }
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                family.push("cjk".to_owned());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}
