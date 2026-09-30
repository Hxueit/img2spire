use std::sync::Arc;

use crate::config::{AppConfig, ParseParams};

pub type Trajectories = Arc<Vec<Vec<(f32, f32)>>>;

pub enum AppMessage {
    LoadImage(String, ParseParams),
    Reparse(ParseParams),
    StartDrawing(AppConfig),
    StartMistMode(AppConfig),
}

pub struct ParseResult {
    /// 已按绘制顺序排好的笔画（图片坐标）
    pub lines: Trajectories,
    pub raw_points: usize,
    pub optimized_points: usize,
    pub image_size: (u32, u32),
}

pub enum WorkerToAppMsg {
    Parsed(ParseResult),
    DrawingFinished,
    Error(String),
}
