#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppConfig {
    pub edge_threshold_low: f32,
    pub edge_threshold_high: f32,
    pub draw_delay_ms: u64,
    pub drag_step_px: f32,
    pub simplify_tolerance: f32,
    pub left_margin: i32,
    pub right_margin: i32,
    pub top_margin: i32,
    pub bottom_margin: i32,
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
            left_margin: 16,
            right_margin: 19,
            top_margin: 9,
            bottom_margin: 7,
            monitor_x: 0,
            monitor_y: 0,
            monitor_w: 0,
            monitor_h: 0,
        }
    }
}
