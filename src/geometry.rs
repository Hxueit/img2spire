use std::time::Duration;

use crate::config::AppConfig;

pub type ScreenPoint = (i32, i32);
pub type ScreenStroke = Vec<ScreenPoint>;

/// 移动到笔画起点后、按下前的等待
pub const MOVE_SETTLE: Duration = Duration::from_millis(10);
/// 按下后开始拖动前的等待
pub const PRESS_SETTLE: Duration = Duration::from_millis(5);
/// 抬笔后的等待
pub const RELEASE_SETTLE: Duration = Duration::from_millis(5);
/// 无延迟且步长较小时，每个插值点之间的微小停顿
pub const SUBSTEP_SLEEP: Duration = Duration::from_micros(500);

/// 屏幕上的绘制区域（物理像素，已包含显示器偏移）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl DrawBox {
    /// `fallback_size` 在未记录显示器尺寸时使用（主显示器尺寸）
    pub fn from_config(config: &AppConfig, fallback_size: (i32, i32)) -> Self {
        let (mw, mh) = if config.monitor_w > 0 && config.monitor_h > 0 {
            (config.monitor_w, config.monitor_h)
        } else {
            fallback_size
        };
        let (sw, sh) = (mw as f32, mh as f32);
        let x = config.monitor_x as f32 + sw * config.left_margin / 100.0;
        let y = config.monitor_y as f32 + sh * config.top_margin / 100.0;
        let w = sw * (1.0 - (config.left_margin + config.right_margin) / 100.0);
        let h = sh * (1.0 - (config.top_margin + config.bottom_margin) / 100.0);
        Self {
            x,
            y,
            w: w.max(1.0),
            h: h.max(1.0),
        }
    }

    /// 把图片坐标系下的笔画等比缩放并居中到绘制区域
    pub fn map_strokes(
        &self,
        lines: &[Vec<(f32, f32)>],
        img_w: f32,
        img_h: f32,
    ) -> Vec<ScreenStroke> {
        let scale = (self.w / img_w.max(1.0)).min(self.h / img_h.max(1.0));
        let offset_x = self.x + (self.w - img_w * scale) / 2.0;
        let offset_y = self.y + (self.h - img_h * scale) / 2.0;
        lines
            .iter()
            .filter(|l| !l.is_empty())
            .map(|l| {
                l.iter()
                    .map(|&(x, y)| ((offset_x + x * scale) as i32, (offset_y + y * scale) as i32))
                    .collect()
            })
            .collect()
    }

    /// 暴力填涂：按行间距蛇形往返扫描整个区域，保证完整覆盖
    pub fn mist_path(&self, spacing: f32) -> ScreenStroke {
        let spacing = spacing.max(1.0);
        let (left, right) = (self.x as i32, (self.x + self.w) as i32);
        let bottom = self.y + self.h;
        let mut path = Vec::new();
        let mut y = self.y;
        let mut left_to_right = true;
        loop {
            let row = y.min(bottom) as i32;
            let (a, b) = if left_to_right {
                (left, right)
            } else {
                (right, left)
            };
            path.push((a, row));
            path.push((b, row));
            if y >= bottom {
                break;
            }
            y += spacing;
            left_to_right = !left_to_right;
        }
        path
    }
}

/// 两点之间按最大步长插值出的中间点（不含起点，含终点）
pub fn segment_steps(from: ScreenPoint, to: ScreenPoint, max_step: f32) -> Vec<ScreenPoint> {
    let dx = (to.0 - from.0) as f32;
    let dy = (to.1 - from.1) as f32;
    let d = (dx * dx + dy * dy).sqrt();
    if d <= max_step {
        return vec![to];
    }
    let steps = (d / max_step) as i32;
    (1..=steps)
        .map(|s| {
            let t = s as f32 / steps as f32;
            (
                (from.0 as f32 + dx * t) as i32,
                (from.1 as f32 + dy * t) as i32,
            )
        })
        .collect()
}

/// 返回 (落笔绘制总长度, 笔画之间抬笔空走总长度)，单位为屏幕像素
pub fn path_lengths(strokes: &[ScreenStroke]) -> (f32, f32) {
    let dist = |a: ScreenPoint, b: ScreenPoint| {
        (((a.0 - b.0) as f32).powi(2) + ((a.1 - b.1) as f32).powi(2)).sqrt()
    };
    let drawn = strokes
        .iter()
        .flat_map(|s| s.windows(2))
        .map(|w| dist(w[0], w[1]))
        .sum();
    let travel = strokes
        .windows(2)
        .filter_map(|w| Some(dist(*w[0].last()?, *w[1].first()?)))
        .sum();
    (drawn, travel)
}

/// 插值点之间是否需要微小停顿（与绘画线程的行为保持一致）
pub fn needs_substep_sleep(config: &AppConfig) -> bool {
    config.drag_step_px < 20.0 && config.draw_delay_ms == 0
}

/// 估算绘制这些笔画需要的时间（只计入主动等待，不含系统输入开销）
pub fn estimate_duration(strokes: &[ScreenStroke], config: &AppConfig) -> Duration {
    let delay = Duration::from_millis(config.draw_delay_ms);
    let substep_sleep = needs_substep_sleep(config);
    let mut total = Duration::ZERO;
    for stroke in strokes {
        total += MOVE_SETTLE + PRESS_SETTLE + RELEASE_SETTLE;
        for pair in stroke.windows(2) {
            let steps = segment_steps(pair[0], pair[1], config.drag_step_px.max(1.0)).len();
            if substep_sleep && steps > 1 {
                total += SUBSTEP_SLEEP * steps as u32;
            }
            total += delay;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_margins(l: f32, r: f32, t: f32, b: f32) -> AppConfig {
        AppConfig {
            left_margin: l,
            right_margin: r,
            top_margin: t,
            bottom_margin: b,
            monitor_x: -1920,
            monitor_y: 0,
            monitor_w: 1920,
            monitor_h: 1080,
            ..AppConfig::default()
        }
    }

    #[test]
    fn draw_box_includes_monitor_offset() {
        let b = DrawBox::from_config(&config_with_margins(10.0, 10.0, 0.0, 50.0), (0, 0));
        assert_eq!(
            b,
            DrawBox {
                x: -1920.0 + 192.0,
                y: 0.0,
                w: 1536.0,
                h: 540.0
            }
        );
    }

    #[test]
    fn map_strokes_centers_and_scales() {
        let b = DrawBox {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let mapped = b.map_strokes(&[vec![(0.0, 0.0), (10.0, 10.0)]], 10.0, 10.0);
        // 等比缩放 10x，水平居中留出 50 像素
        assert_eq!(mapped, vec![vec![(50, 0), (150, 100)]]);
    }

    #[test]
    fn mist_path_covers_whole_box() {
        let b = DrawBox {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 25.0,
        };
        let path = b.mist_path(10.0);
        assert_eq!(
            path,
            vec![
                (0, 0),
                (100, 0),
                (100, 10),
                (0, 10),
                (0, 20),
                (100, 20),
                (100, 25),
                (0, 25)
            ]
        );
    }

    #[test]
    fn segment_steps_respects_max_step() {
        let steps = segment_steps((0, 0), (100, 0), 30.0);
        assert_eq!(steps, vec![(33, 0), (66, 0), (100, 0)]);
        assert_eq!(segment_steps((0, 0), (5, 0), 30.0), vec![(5, 0)]);
    }

    #[test]
    fn path_lengths_split_drawn_and_travel() {
        let strokes = vec![vec![(0, 0), (10, 0)], vec![(10, 5), (10, 25)]];
        assert_eq!(path_lengths(&strokes), (30.0, 5.0));
    }

    #[test]
    fn estimate_counts_overhead_and_delay() {
        let config = AppConfig {
            draw_delay_ms: 2,
            drag_step_px: 50.0,
            ..AppConfig::default()
        };
        let strokes = vec![vec![(0, 0), (10, 0), (20, 0)]];
        let expected = MOVE_SETTLE + PRESS_SETTLE + RELEASE_SETTLE + Duration::from_millis(4);
        assert_eq!(estimate_duration(&strokes, &config), expected);
    }
}
