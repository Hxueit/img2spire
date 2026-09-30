use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::Context;
use image::GrayImage;

use crate::config::{AppConfig, ParseParams};
use crate::geometry::{
    self, DrawBox, MOVE_SETTLE, PRESS_SETTLE, RELEASE_SETTLE, SUBSTEP_SLEEP, ScreenStroke,
};
use crate::image_parser;
use crate::messages::{AppMessage, ParseResult, Trajectories, WorkerToAppMsg};
use crate::mouse::{Mouse, Pen};
use crate::trajectory_optimizer;

/// 未能获取显示器尺寸时的兜底值
const FALLBACK_SCREEN: (i32, i32) = (1920, 1080);
const PAUSE_POLL: Duration = Duration::from_millis(50);

/// UI 线程与绘画线程共享的状态
#[derive(Default)]
pub struct DrawControl {
    pub is_drawing: AtomicBool,
    pub is_paused: AtomicBool,
    /// 已绘制 / 总计的轨迹点数，用于进度显示
    pub progress_done: AtomicUsize,
    pub progress_total: AtomicUsize,
}

impl DrawControl {
    fn stopped(&self) -> bool {
        !self.is_drawing.load(Ordering::Acquire)
    }
}

pub fn drawing_worker_thread(
    rx_app: Receiver<AppMessage>,
    tx_ui: Sender<WorkerToAppMsg>,
    control: Arc<DrawControl>,
    ctx: Context,
) {
    let send = |msg: WorkerToAppMsg| {
        let _ = tx_ui.send(msg);
        ctx.request_repaint();
    };

    let mut mouse = match Mouse::new() {
        Ok(m) => Some(m),
        Err(e) => {
            send(WorkerToAppMsg::Error(e));
            None
        }
    };

    let mut cached_gray_image: Option<GrayImage> = None;
    let mut current_trajectories: Option<Trajectories> = None;
    let mut pending: Option<AppMessage> = None;

    loop {
        let msg = match pending.take() {
            Some(m) => m,
            None => match rx_app.recv() {
                Ok(m) => m,
                Err(_) => break,
            },
        };

        match msg {
            AppMessage::LoadImage(path, params) => match image_parser::load_gray(&path) {
                Ok(gray) => {
                    let result = parse(&gray, params);
                    current_trajectories = Some(Arc::clone(&result.lines));
                    cached_gray_image = Some(gray);
                    send(WorkerToAppMsg::Parsed(result));
                }
                Err(e) => send(WorkerToAppMsg::Error(e)),
            },
            AppMessage::Reparse(mut params) => {
                // 拖动滑条会连续发送大量 Reparse，只处理最新的一条
                while let Ok(next) = rx_app.try_recv() {
                    match next {
                        AppMessage::Reparse(p) => params = p,
                        other => {
                            pending = Some(other);
                            break;
                        }
                    }
                }
                if let Some(gray) = &cached_gray_image {
                    let result = parse(gray, params);
                    current_trajectories = Some(Arc::clone(&result.lines));
                    send(WorkerToAppMsg::Parsed(result));
                }
            }
            AppMessage::StartDrawing(config) => {
                let result = match (&mut mouse, &current_trajectories, &cached_gray_image) {
                    (None, ..) => Err("输入设备不可用".to_string()),
                    (_, None, _) | (_, _, None) => Err("没有可用的轨迹".to_string()),
                    (Some(mouse), Some(lines), Some(gray)) => {
                        let draw_box = DrawBox::from_config(&config, screen_size(mouse));
                        let strokes =
                            draw_box.map_strokes(lines, gray.width() as f32, gray.height() as f32);
                        draw_strokes(mouse, &strokes, &config, &control)
                    }
                };
                finish(&control, result, &send);
            }
            AppMessage::StartMistMode(config) => {
                let result = match &mut mouse {
                    None => Err("输入设备不可用".to_string()),
                    Some(mouse) => {
                        let draw_box = DrawBox::from_config(&config, screen_size(mouse));
                        let path = draw_box.mist_path(config.mist_spacing_px);
                        draw_strokes(mouse, &[path], &config, &control)
                    }
                };
                finish(&control, result, &send);
            }
        }
    }
}

fn screen_size(mouse: &Mouse) -> (i32, i32) {
    mouse.primary_size().unwrap_or(FALLBACK_SCREEN)
}

fn finish(control: &DrawControl, result: Result<(), String>, send: &impl Fn(WorkerToAppMsg)) {
    control.is_drawing.store(false, Ordering::Release);
    control.is_paused.store(false, Ordering::Release);
    send(match result {
        Ok(()) => WorkerToAppMsg::DrawingFinished,
        Err(e) => WorkerToAppMsg::Error(e),
    });
}

fn parse(gray: &GrayImage, params: ParseParams) -> ParseResult {
    let raw_lines = image_parser::parse_image_to_lines(
        gray,
        params.edge_threshold_low,
        params.edge_threshold_high,
    );
    let raw_points = raw_lines.iter().map(|l| l.len()).sum();
    let optimized =
        trajectory_optimizer::optimize_trajectories(&raw_lines, params.simplify_tolerance);
    let ordered = trajectory_optimizer::order_strokes(optimized, (0.0, 0.0));
    let optimized_points = ordered.iter().map(|l| l.len()).sum();
    ParseResult {
        lines: Arc::new(ordered),
        raw_points,
        optimized_points,
        image_size: gray.dimensions(),
    }
}

/// 按顺序绘制屏幕坐标下的笔画。随时响应停止与暂停；
/// 任何情况下退出（包括出错）都会由 `Pen` 自动抬起右键。
fn draw_strokes(
    mouse: &mut Mouse,
    strokes: &[ScreenStroke],
    config: &AppConfig,
    control: &DrawControl,
) -> Result<(), String> {
    let delay = Duration::from_millis(config.draw_delay_ms);
    let max_step = config.drag_step_px.max(1.0);
    let substep_sleep = geometry::needs_substep_sleep(config);

    control.progress_done.store(0, Ordering::Release);
    control
        .progress_total
        .store(strokes.iter().map(|s| s.len()).sum(), Ordering::Release);

    let mut pen = Pen::new(mouse);

    for stroke in strokes {
        let Some(&start) = stroke.first() else {
            continue;
        };
        if control.stopped() {
            return Ok(());
        }
        pen.move_to(start)?;
        thread::sleep(MOVE_SETTLE);
        pen.down()?;
        thread::sleep(PRESS_SETTLE);
        control.progress_done.fetch_add(1, Ordering::AcqRel);

        let mut last = start;
        for &target in &stroke[1..] {
            if control.is_paused.load(Ordering::Acquire) {
                pen.up()?;
                while control.is_paused.load(Ordering::Acquire) {
                    if control.stopped() {
                        return Ok(());
                    }
                    thread::sleep(PAUSE_POLL);
                }
                // 先回到断点再按下，避免在用户暂停期间移走的鼠标位置落笔
                pen.move_to(last)?;
                thread::sleep(MOVE_SETTLE);
                pen.down()?;
                thread::sleep(PRESS_SETTLE);
            }

            let steps = geometry::segment_steps(last, target, max_step);
            let interpolated = steps.len() > 1;
            for p in steps {
                if control.stopped() {
                    return Ok(());
                }
                pen.move_to(p)?;
                if interpolated && substep_sleep {
                    thread::sleep(SUBSTEP_SLEEP);
                }
            }
            last = target;
            control.progress_done.fetch_add(1, Ordering::AcqRel);

            if !delay.is_zero() {
                thread::sleep(delay);
            }
        }

        pen.up()?;
        thread::sleep(RELEASE_SETTLE);
    }

    Ok(())
}
