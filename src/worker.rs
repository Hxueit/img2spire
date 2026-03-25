use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::Context;
use enigo::{Button, Coordinate, Direction, Enigo, Mouse, Settings};
use image::GrayImage;

use crate::config::AppConfig;
use crate::image_parser;
use crate::messages::{AppMessage, WorkerToAppMsg};
use crate::trajectory_optimizer;

pub fn drawing_worker_thread(
    rx_app: Receiver<AppMessage>,
    tx_ui: Sender<WorkerToAppMsg>,
    is_drawing: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    ctx: Context,
) {
    let mut cached_gray_image: Option<GrayImage> = None;
    let mut current_trajectories: Option<Arc<Vec<Vec<(f32, f32)>>>> = None;

    // 初始化一次 Enigo 实例并长期持有复用
    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(e) => e,
        Err(err) => {
            let _ = tx_ui.send(WorkerToAppMsg::Error(format!("初始化输入设备失败: {:?}", err)));
            return;
        }
    };

    while let Ok(msg) = rx_app.recv() {
        match msg {
            AppMessage::LoadImage(path, config) => {
                if let Ok(img) = image::open(&path) {
                    let mut gray = img.into_luma8();
                    let max_dim = 1024;
                    if gray.width() > max_dim || gray.height() > max_dim {
                        let scale = (max_dim as f32 / gray.width() as f32)
                            .min(max_dim as f32 / gray.height() as f32);
                        gray = image::imageops::resize(
                            &gray,
                            (gray.width() as f32 * scale) as u32,
                            (gray.height() as f32 * scale) as u32,
                            image::imageops::FilterType::Triangle,
                        );
                    }

                    let dims = gray.dimensions();
                    let raw_lines = image_parser::parse_image_to_lines(
                        &gray,
                        config.edge_threshold_low,
                        config.edge_threshold_high,
                    );
                    let orig_pts = raw_lines.iter().map(|l| l.len()).sum();
                    
                    let opt_lines = Arc::new(trajectory_optimizer::optimize_trajectories(
                        &raw_lines,
                        config.simplify_tolerance,
                    ));
                    let opt_pts = opt_lines.iter().map(|l| l.len()).sum();

                    let _ = tx_ui.send(WorkerToAppMsg::ParsedTrajectories(
                        Arc::clone(&opt_lines),
                        orig_pts,
                        opt_pts,
                        dims,
                    ));
                    ctx.request_repaint();
                    current_trajectories = Some(opt_lines);
                    cached_gray_image = Some(gray);
                } else {
                    let _ = tx_ui.send(WorkerToAppMsg::Error("无法加载图片".into()));
                    ctx.request_repaint();
                }
            }
            AppMessage::Reparse(config) => {
                if let Some(gray) = &cached_gray_image {
                    let dims = gray.dimensions();
                    let raw_lines = image_parser::parse_image_to_lines(
                        gray,
                        config.edge_threshold_low,
                        config.edge_threshold_high,
                    );
                    let orig_pts = raw_lines.iter().map(|l| l.len()).sum();
                    
                    let opt_lines = Arc::new(trajectory_optimizer::optimize_trajectories(
                        &raw_lines,
                        config.simplify_tolerance,
                    ));
                    let opt_pts = opt_lines.iter().map(|l| l.len()).sum();

                    let _ = tx_ui.send(WorkerToAppMsg::ParsedTrajectories(
                        Arc::clone(&opt_lines),
                        orig_pts,
                        opt_pts,
                        dims,
                    ));
                    ctx.request_repaint();
                    current_trajectories = Some(opt_lines);
                }
            }
            AppMessage::StartDrawing(config) => {
                if let Some(lines) = &current_trajectories {
                    is_drawing.store(true, Ordering::Release);
                    is_paused.store(false, Ordering::Release);

                    let img_w = cached_gray_image.as_ref().map(|g| g.width() as f32).unwrap_or(1.0);
                    let img_h = cached_gray_image.as_ref().map(|g| g.height() as f32).unwrap_or(1.0);
                    
                    if let Err(e) = execute_drawing(&mut enigo, lines, config, img_w, img_h, &is_drawing, &is_paused) {
                        let _ = tx_ui.send(WorkerToAppMsg::Error(e));
                    }
                    
                    is_drawing.store(false, Ordering::Release);
                    ctx.request_repaint();
                } else {
                    let _ = tx_ui.send(WorkerToAppMsg::Error("没有可用的轨迹".into()));
                    is_drawing.store(false, Ordering::Release);
                    ctx.request_repaint();
                }
            }
            AppMessage::StartMistMode(config) => {
                is_drawing.store(true, Ordering::Release);
                is_paused.store(false, Ordering::Release);
                
                if let Err(e) = start_brute_mist_task(&mut enigo, config, &is_drawing, &is_paused) {
                    let _ = tx_ui.send(WorkerToAppMsg::Error(e));
                }
                
                is_drawing.store(false, Ordering::Release);
                ctx.request_repaint();
            }
        }
    }
}

fn execute_drawing(
    enigo: &mut Enigo,
    lines: &[Vec<(f32, f32)>],
    config: AppConfig,
    img_w: f32,
    img_h: f32,
    is_drawing: &AtomicBool,
    is_paused: &AtomicBool,
) -> Result<(), String> {
    // 统一物理坐标空间：兼容多屏并正确叠加显示器物理偏移量
    let (mw, mh) = if config.monitor_w > 0 {
        (config.monitor_w, config.monitor_h)
    } else {
        enigo.main_display().unwrap_or((1920, 1080))
    };
    
    let sw = mw as f32;
    let sh = mh as f32;
    let base_x = config.monitor_x as f32;
    let base_y = config.monitor_y as f32;

    let box_x = base_x + sw * (config.left_margin as f32 / 100.0);
    let box_y = base_y + sh * (config.top_margin as f32 / 100.0);
    let box_w = sw * (1.0 - (config.left_margin + config.right_margin) as f32 / 100.0);
    let box_h = sh * (1.0 - (config.top_margin + config.bottom_margin) as f32 / 100.0);

    let scale = (box_w / img_w).min(box_h / img_h);
    let offset_x = box_x + (box_w - img_w * scale) / 2.0;
    let offset_y = box_y + (box_h - img_h * scale) / 2.0;

    let delay = Duration::from_millis(config.draw_delay_ms);

    'outer: for line in lines {
        if !is_drawing.load(Ordering::Acquire) { break; }
        if line.is_empty() { continue; }

        let start_x = (offset_x + line[0].0 * scale) as i32;
        let start_y = (offset_y + line[0].1 * scale) as i32;
        
        enigo.move_mouse(start_x, start_y, Coordinate::Abs).map_err(|e| format!("鼠标移动失败: {:?}", e))?;
        thread::sleep(Duration::from_millis(10));
        enigo.button(Button::Right, Direction::Press).map_err(|e| format!("鼠标点击失败: {:?}", e))?;
        thread::sleep(Duration::from_millis(5));

        let mut last_px = start_x;
        let mut last_py = start_y;

        for pt in line.iter().skip(1) {
            if !is_drawing.load(Ordering::Acquire) { break 'outer; }
            
            if is_paused.load(Ordering::Acquire) {
                enigo.button(Button::Right, Direction::Release).map_err(|e| format!("鼠标点击失败: {:?}", e))?;
                while is_paused.load(Ordering::Acquire) {
                    if !is_drawing.load(Ordering::Acquire) { break 'outer; }
                    thread::sleep(Duration::from_millis(50));
                }
                enigo.move_mouse(last_px, last_py, Coordinate::Abs).map_err(|e| format!("鼠标移动失败: {:?}", e))?;
                thread::sleep(Duration::from_millis(10));
                enigo.button(Button::Right, Direction::Press).map_err(|e| format!("鼠标点击失败: {:?}", e))?;
            }

            let px = (offset_x + pt.0 * scale) as i32;
            let py = (offset_y + pt.1 * scale) as i32;
            
            let dx = px as f32 - last_px as f32;
            let dy = py as f32 - last_py as f32;
            let d = (dx * dx + dy * dy).sqrt();
            
            if d > config.drag_step_px {
                let steps = (d / config.drag_step_px) as i32;
                for s in 1..=steps {
                    let ix = (last_px as f32 + dx * (s as f32 / steps as f32)) as i32;
                    let iy = (last_py as f32 + dy * (s as f32 / steps as f32)) as i32;
                    enigo.move_mouse(ix, iy, Coordinate::Abs).map_err(|e| format!("鼠标移动失败: {:?}", e))?;
                    if config.drag_step_px < 20.0 && delay.as_millis() == 0 {
                        thread::sleep(Duration::from_micros(500));
                    }
                }
            } else {
                enigo.move_mouse(px, py, Coordinate::Abs).map_err(|e| format!("鼠标移动失败: {:?}", e))?;
            }
            last_px = px;
            last_py = py;
            
            if delay.as_millis() > 0 {
                thread::sleep(delay);
            }
        }

        enigo.button(Button::Right, Direction::Release).map_err(|e| format!("鼠标点击失败: {:?}", e))?;
        thread::sleep(Duration::from_millis(5));
    }

    let _ = enigo.button(Button::Right, Direction::Release);
    Ok(())
}

fn start_brute_mist_task(
    enigo: &mut Enigo,
    config: AppConfig,
    is_drawing: &AtomicBool,
    is_paused: &AtomicBool,
) -> Result<(), String> {
    let (mw, mh) = if config.monitor_w > 0 {
        (config.monitor_w, config.monitor_h)
    } else {
        enigo.main_display().unwrap_or((1920, 1080))
    };
    
    let sw = mw as f32;
    let sh = mh as f32;
    let base_x = config.monitor_x as f32;
    let base_y = config.monitor_y as f32;

    let box_x = base_x + sw * (config.left_margin as f32 / 100.0);
    let box_y = base_y + sh * (config.top_margin as f32 / 100.0);
    let box_w = sw * (1.0 - (config.left_margin + config.right_margin) as f32 / 100.0);
    let box_h = sh * (1.0 - (config.top_margin + config.bottom_margin) as f32 / 100.0);

    enigo.move_mouse(box_x as i32, box_y as i32, Coordinate::Abs).map_err(|e| format!("鼠标移动失败: {:?}", e))?;
    thread::sleep(Duration::from_millis(50));
    enigo.button(Button::Right, Direction::Press).map_err(|e| format!("鼠标点击失败: {:?}", e))?;
    thread::sleep(Duration::from_millis(10));

    // 方案 A: 随机乱麻填涂 (Random Hairball)
    // 使用极简 LCG 伪随机数生成器以保证极速
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let iterations = 8000; // 极高频跳跃
    for i in 0..iterations {
        // 每 100 次检查一次状态，兼顾速度与响应
        if i % 100 == 0 {
            if !is_drawing.load(Ordering::Acquire) { break; }
            if check_pause(enigo, is_drawing, is_paused)? {
                // 恢复后重新定位
                enigo.move_mouse(box_x as i32, box_y as i32, Coordinate::Abs).map_err(|e| format!("鼠标移动失败: {:?}", e))?;
            }
        }

        // 极简 LCG: next = (a * seed + c) % m
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let rx = (seed % 1000) as f32 / 1000.0;
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let ry = (seed % 1000) as f32 / 1000.0;

        let px = (box_x + rx * box_w) as i32;
        let py = (box_y + ry * box_h) as i32;

        enigo.move_mouse(px, py, Coordinate::Abs).map_err(|e| format!("鼠标移动失败: {:?}", e))?;
        
        // 可选：极微小延迟给系统一点处理喘息（通常不需要，取决于驱动）
        // if i % 10 == 0 { std::hint::spin_loop(); }
    }

    let _ = enigo.button(Button::Right, Direction::Release);
    Ok(())
}

fn check_pause(enigo: &mut Enigo, is_drawing: &AtomicBool, is_paused: &AtomicBool) -> Result<bool, String> {
    if is_paused.load(Ordering::Acquire) {
        enigo.button(Button::Right, Direction::Release).map_err(|e| format!("鼠标点击失败: {:?}", e))?;
        while is_paused.load(Ordering::Acquire) {
            if !is_drawing.load(Ordering::Acquire) { return Ok(false); }
            thread::sleep(Duration::from_millis(100));
        }
        enigo.button(Button::Right, Direction::Press).map_err(|e| format!("鼠标点击失败: {:?}", e))?;
        thread::sleep(Duration::from_millis(20));
        return Ok(true);
    }
    Ok(false)
}
