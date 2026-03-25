use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use global_hotkey::{
    hotkey::{Code, HotKey},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};

use crate::config::AppConfig;
use crate::messages::{AppMessage, WorkerToAppMsg};
use crate::worker::drawing_worker_thread;

pub struct AutoDrawerApp {
    config: AppConfig,
    last_config: AppConfig,
    is_drawing: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    tx_app: Sender<AppMessage>,
    rx_worker: Receiver<WorkerToAppMsg>,
    rx_hotkey: Receiver<u32>,
    image_path: Option<String>,

    trajectories: Option<Arc<Vec<Vec<(f32, f32)>>>>,
    image_size: Option<(u32, u32)>,
    points_stats: Option<(usize, usize)>,
    error_msg: Option<String>,

    is_selecting_area: bool,
    selection_start: Option<egui::Pos2>,
    selection_current: Option<egui::Pos2>,
    screenshot_texture: Option<egui::TextureHandle>,

    _hotkey_manager: GlobalHotKeyManager,
    hotkey_start_id: u32,
    hotkey_stop_id: u32,
    hotkey_pause_id: u32,
    hotkey_mist_id: u32,
}

impl AutoDrawerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        let windir = std::env::var("windir").unwrap_or_else(|_| "C:\\Windows".to_string());
        let font_paths = [
            format!("{}\\Fonts\\msyh.ttc", windir),
            format!("{}\\Fonts\\msyh.ttf", windir),
            format!("{}\\Fonts\\simhei.ttf", windir),
        ];

        for path in font_paths {
            if let Ok(font_data) = std::fs::read(&path) {
                fonts.font_data.insert(
                    "my_font".to_owned(),
                    std::sync::Arc::new(egui::FontData::from_owned(font_data)),
                );
                fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "my_font".to_owned());
                fonts.families.get_mut(&egui::FontFamily::Monospace).unwrap().push("my_font".to_owned());
                break;
            }
        }
        cc.egui_ctx.set_fonts(fonts);

        let is_drawing = Arc::new(AtomicBool::new(false));
        let is_paused = Arc::new(AtomicBool::new(false));

        let (tx_app, rx_app) = unbounded();
        let (tx_worker, rx_worker) = unbounded();
        let (tx_hotkey, rx_hotkey) = unbounded();

        let is_drawing_clone = is_drawing.clone();
        let is_paused_clone = is_paused.clone();
        let ctx_clone = cc.egui_ctx.clone();
        
        // 分离绘画工作线程
        thread::spawn(move || {
            drawing_worker_thread(rx_app, tx_worker, is_drawing_clone, is_paused_clone, ctx_clone);
        });

        let _hotkey_manager = GlobalHotKeyManager::new().unwrap();
        let hotkey_start = HotKey::new(None, Code::F9);
        let hotkey_stop = HotKey::new(None, Code::F10);
        let hotkey_pause = HotKey::new(None, Code::F8);
        let hotkey_mist = HotKey::new(None, Code::F11);

        for key in [&hotkey_start, &hotkey_stop, &hotkey_pause, &hotkey_mist] {
            let _ = _hotkey_manager.register(*key);
        }

        let ctx_for_hotkey = cc.egui_ctx.clone();
        // 唯一的全局热键事件捕获收敛点
        thread::spawn(move || {
            let rx = GlobalHotKeyEvent::receiver();
            while let Ok(event) = rx.recv() {
                if event.state == HotKeyState::Pressed {
                    let _ = tx_hotkey.send(event.id);
                    ctx_for_hotkey.request_repaint();
                }
            }
        });

        Self {
            config: AppConfig::default(),
            last_config: AppConfig::default(),
            is_drawing,
            is_paused,
            tx_app,
            rx_worker,
            rx_hotkey,
            image_path: None,
            trajectories: None,
            image_size: None,
            points_stats: None,
            error_msg: None,
            is_selecting_area: false,
            selection_start: None,
            selection_current: None,
            screenshot_texture: None,
            _hotkey_manager,
            hotkey_start_id: hotkey_start.id(),
            hotkey_stop_id: hotkey_stop.id(),
            hotkey_pause_id: hotkey_pause.id(),
            hotkey_mist_id: hotkey_mist.id(),
        }
    }

    fn handle_hotkeys(&mut self) {
        while let Ok(id) = self.rx_hotkey.try_recv() {
            if id == self.hotkey_start_id {
                if self.is_drawing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                    let _ = self.tx_app.send(AppMessage::StartDrawing(self.config));
                }
            } else if id == self.hotkey_stop_id {
                self.is_drawing.store(false, Ordering::Release);
            } else if id == self.hotkey_pause_id {
                let paused = self.is_paused.load(Ordering::Acquire);
                self.is_paused.store(!paused, Ordering::Release);
            } else if id == self.hotkey_mist_id {
                if self.is_drawing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                    let _ = self.tx_app.send(AppMessage::StartMistMode(self.config));
                }
            }
        }
    }

    fn handle_worker_messages(&mut self) {
        while let Ok(msg) = self.rx_worker.try_recv() {
            match msg {
                WorkerToAppMsg::ParsedTrajectories(lines, orig_pts, opt_pts, size) => {
                    self.trajectories = Some(lines);
                    self.points_stats = Some((orig_pts, opt_pts));
                    self.image_size = Some(size);
                    self.error_msg = None;
                }
                WorkerToAppMsg::Error(err) => {
                    self.error_msg = Some(err);
                }
            }
        }
    }

    fn render_selection_mode(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                if let Some(tex) = &self.screenshot_texture {
                    ui.painter().image(
                        tex.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                ui.painter().rect_filled(rect, 0.0, egui::Color32::from_black_alpha(102));

                ui.label(
                    egui::RichText::new("已冻结屏幕。请拖拽鼠标框选游戏画布区域\n按 ESC 退出")
                        .color(egui::Color32::WHITE)
                        .size(24.0),
                );

                let response = ui.allocate_response(ui.available_size(), egui::Sense::drag());

                if response.drag_started() {
                    self.selection_start = response.interact_pointer_pos();
                }
                if response.dragged() {
                    self.selection_current = response.interact_pointer_pos();
                    if let (Some(start), Some(curr)) = (self.selection_start, self.selection_current) {
                        ui.painter().rect_stroke(
                            egui::Rect::from_two_pos(start, curr),
                            0.0,
                            egui::Stroke::new(2.0, egui::Color32::RED),
                            egui::StrokeKind::Middle,
                        );
                    }
                }
                if response.drag_stopped() {
                    if let (Some(start), Some(curr)) = (self.selection_start, self.selection_current) {
                        let r = egui::Rect::from_two_pos(start, curr);
                        let screen_rect = ui.max_rect();
                        if r.width() > 10.0 && r.height() > 10.0 {
                            let left = ((r.min.x / screen_rect.width()) * 100.0) as i32;
                            let right = (((screen_rect.width() - r.max.x) / screen_rect.width()) * 100.0) as i32;
                            let top = ((r.min.y / screen_rect.height()) * 100.0) as i32;
                            let bottom = (((screen_rect.height() - r.max.y) / screen_rect.height()) * 100.0) as i32;
                            
                            self.config.left_margin = left.clamp(0, 99);
                            self.config.right_margin = right.clamp(0, 99);
                            self.config.top_margin = top.clamp(0, 99);
                            self.config.bottom_margin = bottom.clamp(0, 99);
                        }
                    }
                    self.exit_selection_mode(ctx);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.exit_selection_mode(ctx);
                }
            });
    }

    fn exit_selection_mode(&mut self, ctx: &egui::Context) {
        self.is_selecting_area = false;
        self.selection_start = None;
        self.selection_current = None;
        self.last_config = self.config;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
}

impl eframe::App for AutoDrawerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_hotkeys();
        self.handle_worker_messages();

        if self.is_selecting_area {
            self.render_selection_mode(ctx);
            ctx.request_repaint_after(Duration::from_millis(100)); // 手动框选时保留高刷
            return;
        }

        egui::SidePanel::left("left_panel").default_width(300.0).show(ctx, |ui| {
            ui.heading("img2spire");
            ui.separator();

            ui.horizontal(|ui| {
                if ui.button("加载图片").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Image", &["png", "jpg", "jpeg", "bmp", "webp", "gif"])
                        .pick_file()
                    {
                        let path_str = path.display().to_string();
                        self.image_path = Some(path_str.clone());
                        let _ = self.tx_app.send(AppMessage::LoadImage(path_str, self.config));
                    }
                }
                if let Some(path) = &self.image_path {
                    let mut file_name = std::path::Path::new(path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if file_name.chars().count() > 25 {
                        file_name = file_name.chars().take(22).collect::<String>() + "...";
                    }
                    ui.label(format!("已选择: {}", file_name));
                } else {
                    ui.label("未选择图片");
                }
            });

            ui.separator();
            ui.label("视觉参数设置:");
            let mut changed = false;
            if ui.add(egui::Slider::new(&mut self.config.edge_threshold_low, 0.0..=255.0).text("Canny 低阈值")).changed() { changed = true; }
            if ui.add(egui::Slider::new(&mut self.config.edge_threshold_high, 0.0..=255.0).text("Canny 高阈值")).changed() { changed = true; }

            ui.label("绘画参数设置:");
            ui.add(egui::Slider::new(&mut self.config.draw_delay_ms, 0..=50).text("起落笔延迟 (ms)"));
            if ui.add(egui::Slider::new(&mut self.config.drag_step_px, 1.0..=100.0).text("最大直线冲刺(像素)")).changed() { changed = true; }
            if ui.add(egui::Slider::new(&mut self.config.simplify_tolerance, 0.1..=10.0).text("防抖精度")).changed() { changed = true; }

            ui.separator();
            ui.label("绘制区域 (屏幕占比 %):");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.config.left_margin, 0..=50).text("左"));
                ui.add(egui::Slider::new(&mut self.config.right_margin, 0..=50).text("右"));
            });
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.config.top_margin, 0..=50).text("上"));
                ui.add(egui::Slider::new(&mut self.config.bottom_margin, 0..=50).text("下"));
            });

            if ui.button("✂️手动框选作画区域").clicked() {
                // 摒弃直接 first() 的硬编码和吞咽式防御性编程
                match xcap::Monitor::all().map(|m| m.into_iter().next()) {
                    Ok(Some(monitor)) => {
                        let res = monitor.x()
                            .and_then(|x| monitor.y().map(|y| (x, y)))
                            .and_then(|(x, y)| monitor.width().map(|w| (x, y, w)))
                            .and_then(|(x, y, w)| monitor.height().map(|h| (x, y, w, h)))
                            .and_then(|(x, y, w, h)| monitor.capture_image().map(|img| (x, y, w, h, img)));

                        match res {
                            Ok((x, y, w, h, image)) => {
                                self.config.monitor_x = x;
                                self.config.monitor_y = y;
                                self.config.monitor_w = w as i32;
                                self.config.monitor_h = h as i32;
                                let size = [image.width() as usize, image.height() as usize];
                                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &image.into_raw());
                                self.screenshot_texture = Some(ctx.load_texture("screenshot", color_image, egui::TextureOptions::LINEAR));
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                                self.is_selecting_area = true;
                            }
                            Err(e) => self.error_msg = Some(format!("读取显示器属性或截屏失败: {:?}", e)),
                        }
                    }
                    _ => self.error_msg = Some("无法获取显示器设备".into()),
                }
            }

            if changed || self.config != self.last_config {
                if self.image_path.is_some() && !self.is_drawing.load(Ordering::Acquire) {
                    let _ = self.tx_app.send(AppMessage::Reparse(self.config));
                }
                self.last_config = self.config;
            }

            ui.separator();
            ui.horizontal(|ui| {
                let drawing = self.is_drawing.load(Ordering::Acquire);
                let paused = self.is_paused.load(Ordering::Acquire);

                if !drawing {
                    if ui.button("▶ 开始绘画 (F9)").clicked() {
                        if self.is_drawing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                            let _ = self.tx_app.send(AppMessage::StartDrawing(self.config));
                        }
                    }
                    if ui.button("暴力涂抹绘画区域 (F11)").clicked() {
                        if self.is_drawing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                            let _ = self.tx_app.send(AppMessage::StartMistMode(self.config));
                        }
                    }
                } else {
                    if ui.button("⏹ 停止绘画 (F10)").clicked() {
                        self.is_drawing.store(false, Ordering::Release);
                    }
                    if ui.button(if paused { "▶ 继续 (F8)" } else { "⏸ 暂停 (F8)" }).clicked() {
                        self.is_paused.store(!paused, Ordering::Release);
                    }
                }
            });

            ui.separator();
            ui.label("状态监控:");
            let status = if self.is_drawing.load(Ordering::Acquire) {
                if self.is_paused.load(Ordering::Acquire) { "已暂停" } else { "正在绘画..." }
            } else {
                "空闲"
            };
            ui.label(format!("当前状态: {}", status));

            if let Some(err) = &self.error_msg {
                ui.colored_label(egui::Color32::RED, format!("错误: {}", err));
            }
            
            if let Some(lines) = &self.trajectories {
                if let Some((orig, opt)) = self.points_stats {
                    ui.label(format!("解析结果: {} 条线段", lines.len()));
                    ui.label(format!("原始点数: {}", orig));
                    ui.label(format!("防抖后点数: {}", opt));
                    let reduction = if orig > 0 { ((orig - opt) as f32 / orig as f32) * 100.0 } else { 0.0 };
                    ui.label(format!("坐标压缩率: {:.1}%", reduction));
                }
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("轨迹预览");
            
            if let (Some(lines), Some((img_w, img_h))) = (&self.trajectories, self.image_size) {
                egui::Frame::dark_canvas(ui.style()).show(ui, |ui| {
                    let available_size = ui.available_size();
                    let scale = (available_size.x / img_w as f32).min(available_size.y / img_h as f32).min(1.0);
                    
                    if scale > 0.0 {
                        let (response, painter) = ui.allocate_painter(
                            egui::vec2(img_w as f32 * scale, img_h as f32 * scale),
                            egui::Sense::hover(),
                        );
                        
                        let rect = response.rect;
                        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(30));

                        let stroke = egui::Stroke::new(1.5, egui::Color32::GREEN);
                        for line in lines.iter() {
                            if line.len() < 2 { continue; }
                            
                            for chunk in line.chunks(5000) {
                                let points: Vec<egui::Pos2> = chunk.iter().map(|&(x, y)| {
                                    egui::pos2(rect.min.x + x * scale, rect.min.y + y * scale)
                                }).collect();
                                
                                if points.len() >= 2 {
                                    painter.add(egui::Shape::line(points, stroke));
                                }
                            }
                        }
                    }
                });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("请加载图片以生成预览");
                });
            }
        });
    }
}
