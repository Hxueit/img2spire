use crate::config::AppConfig;
use std::sync::Arc;

pub enum AppMessage {
    LoadImage(String, AppConfig),
    Reparse(AppConfig),
    StartDrawing(AppConfig),
    StartMistMode(AppConfig),
}

pub enum WorkerToAppMsg {
    ParsedTrajectories(Arc<Vec<Vec<(f32, f32)>>>, usize, usize, (u32, u32)),
    Error(String),
}
