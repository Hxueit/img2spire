use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub edge_threshold_low: f32,
    pub edge_threshold_high: f32,
    pub draw_delay_ms: u64,
    pub drag_step_px: f32,
    pub simplify_tolerance: f32,
    /// 暴力填涂时相邻两行扫描线的间距 (屏幕像素)
    pub mist_spacing_px: f32,
    /// 绘制区域边距，单位为屏幕宽/高的百分比
    pub left_margin: f32,
    pub right_margin: f32,
    pub top_margin: f32,
    pub bottom_margin: f32,
    pub monitor_x: i32,
    pub monitor_y: i32,
    pub monitor_w: i32,
    pub monitor_h: i32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            edge_threshold_low: 50.0,
            edge_threshold_high: 150.0,
            draw_delay_ms: 2,
            drag_step_px: 15.0,
            simplify_tolerance: 2.0,
            mist_spacing_px: 12.0,
            left_margin: 16.0,
            right_margin: 19.0,
            top_margin: 9.0,
            bottom_margin: 7.0,
            monitor_x: 0,
            monitor_y: 0,
            monitor_w: 0,
            monitor_h: 0,
        }
    }
}

/// 只有这些参数变化时才需要重新解析图片
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParseParams {
    pub edge_threshold_low: f32,
    pub edge_threshold_high: f32,
    pub simplify_tolerance: f32,
}

impl AppConfig {
    pub fn parse_params(&self) -> ParseParams {
        ParseParams {
            edge_threshold_low: self.edge_threshold_low,
            edge_threshold_high: self.edge_threshold_high,
            simplify_tolerance: self.simplify_tolerance,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    pub start: String,
    pub stop: String,
    pub pause: String,
    pub mist: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            start: "F9".into(),
            stop: "F10".into(),
            pause: "F8".into(),
            mist: "F11".into(),
        }
    }
}

/// 持久化到磁盘的全部设置
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub config: AppConfig,
    pub hotkeys: HotkeyConfig,
    pub image_path: Option<String>,
    pub show_travel: bool,
}
