use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;

use crate::config::HotkeyConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Pause,
    Mist,
}

impl Action {
    pub const ALL: [Action; 4] = [Action::Start, Action::Stop, Action::Pause, Action::Mist];

    pub fn label(self) -> &'static str {
        match self {
            Action::Start => "开始绘画",
            Action::Stop => "停止",
            Action::Pause => "暂停 / 继续",
            Action::Mist => "暴力填涂",
        }
    }

    pub fn key(self, cfg: &HotkeyConfig) -> &str {
        match self {
            Action::Start => &cfg.start,
            Action::Stop => &cfg.stop,
            Action::Pause => &cfg.pause,
            Action::Mist => &cfg.mist,
        }
    }

    pub fn key_mut(self, cfg: &mut HotkeyConfig) -> &mut String {
        match self {
            Action::Start => &mut cfg.start,
            Action::Stop => &mut cfg.stop,
            Action::Pause => &mut cfg.pause,
            Action::Mist => &mut cfg.mist,
        }
    }
}

/// 解析并检查热键配置；空字符串表示不绑定
pub fn parse_config(cfg: &HotkeyConfig) -> Result<Vec<(Action, HotKey)>, String> {
    let mut parsed: Vec<(Action, HotKey)> = Vec::new();
    for action in Action::ALL {
        let text = action.key(cfg).trim();
        if text.is_empty() {
            continue;
        }
        let hotkey: HotKey = text
            .parse()
            .map_err(|e| format!("「{}」的热键 \"{text}\" 无效: {e}", action.label()))?;
        if let Some((other, _)) = parsed.iter().find(|(_, h)| h.id() == hotkey.id()) {
            return Err(format!(
                "「{}」与「{}」使用了相同的热键 {hotkey}",
                action.label(),
                other.label()
            ));
        }
        parsed.push((action, hotkey));
    }
    Ok(parsed)
}

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    bound: Vec<(Action, HotKey)>,
}

impl Hotkeys {
    pub fn new() -> (Self, Option<String>) {
        match GlobalHotKeyManager::new() {
            Ok(manager) => (
                Self {
                    manager: Some(manager),
                    bound: Vec::new(),
                },
                None,
            ),
            Err(e) => (
                Self {
                    manager: None,
                    bound: Vec::new(),
                },
                Some(format!("全局热键不可用: {e}")),
            ),
        }
    }

    /// 替换当前绑定。配置无效时保持原绑定不变；返回所有错误信息。
    pub fn apply(&mut self, cfg: &HotkeyConfig) -> Vec<String> {
        let Some(manager) = &self.manager else {
            return vec!["全局热键不可用".into()];
        };
        let parsed = match parse_config(cfg) {
            Ok(p) => p,
            Err(e) => return vec![e],
        };

        for (_, hotkey) in self.bound.drain(..) {
            let _ = manager.unregister(hotkey);
        }

        let mut errors = Vec::new();
        for (action, hotkey) in parsed {
            match manager.register(hotkey) {
                Ok(()) => self.bound.push((action, hotkey)),
                Err(e) => errors.push(format!(
                    "「{}」热键 {hotkey} 注册失败（可能已被其他程序占用）: {e}",
                    action.label()
                )),
            }
        }
        errors
    }

    pub fn action_for(&self, id: u32) -> Option<Action> {
        self.bound
            .iter()
            .find(|(_, h)| h.id() == id)
            .map(|(a, _)| *a)
    }

    /// 按钮上显示的热键文字，未绑定时返回 None
    pub fn describe(&self, action: Action) -> Option<String> {
        self.bound
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, h)| h.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let parsed = parse_config(&HotkeyConfig::default()).unwrap();
        assert_eq!(parsed.len(), 4);
    }

    #[test]
    fn modifiers_and_empty_are_accepted() {
        let cfg = HotkeyConfig {
            start: "ctrl+shift+F9".into(),
            mist: "".into(),
            ..Default::default()
        };
        let parsed = parse_config(&cfg).unwrap();
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn duplicates_and_garbage_are_rejected() {
        let dup = HotkeyConfig {
            stop: "F9".into(),
            ..Default::default()
        };
        assert!(parse_config(&dup).is_err());
        let bad = HotkeyConfig {
            pause: "not-a-key".into(),
            ..Default::default()
        };
        assert!(parse_config(&bad).is_err());
    }
}
