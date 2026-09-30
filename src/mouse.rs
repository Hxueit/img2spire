//! 鼠标输入封装：绝对坐标移动 + 右键按下/抬起。
//!
//! Windows 下直接调用 `SendInput` 并带上 `MOUSEEVENTF_VIRTUALDESK`，
//! 坐标按整个虚拟桌面归一化，副屏（包括负坐标的屏幕）也能正确定位。
//! enigo 的绝对移动只按主显示器归一化，无法在副屏上作画。

pub use imp::Mouse;

#[cfg(windows)]
mod imp {
    use std::mem::size_of;

    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    pub struct Mouse;

    impl Mouse {
        pub fn new() -> Result<Self, String> {
            Ok(Self)
        }

        pub fn primary_size(&self) -> Option<(i32, i32)> {
            let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
            (w > 0 && h > 0).then_some((w, h))
        }

        pub fn move_to(&mut self, x: i32, y: i32) -> Result<(), String> {
            let (vx, vy, vw, vh) = unsafe {
                (
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    GetSystemMetrics(SM_CXVIRTUALSCREEN),
                    GetSystemMetrics(SM_CYVIRTUALSCREEN),
                )
            };
            let nx = normalize(x - vx, vw);
            let ny = normalize(y - vy, vh);
            send(
                nx,
                ny,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            )
            .map_err(|e| format!("鼠标移动失败: {e}"))
        }

        pub fn press(&mut self) -> Result<(), String> {
            send(0, 0, MOUSEEVENTF_RIGHTDOWN).map_err(|e| format!("鼠标按下失败: {e}"))
        }

        pub fn release(&mut self) -> Result<(), String> {
            send(0, 0, MOUSEEVENTF_RIGHTUP).map_err(|e| format!("鼠标抬起失败: {e}"))
        }
    }

    /// 像素坐标 (0..extent-1) 映射到 SendInput 的 0..65535
    fn normalize(p: i32, extent: i32) -> i32 {
        if extent <= 1 {
            return 0;
        }
        let max = (extent - 1) as i64;
        let p = (p as i64).clamp(0, max);
        ((p * 65535 + max / 2) / max) as i32
    }

    fn send(dx: i32, dy: i32, flags: u32) -> Result<(), String> {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
        if sent == 1 {
            Ok(())
        } else {
            Err("输入被系统拦截（若游戏以管理员身份运行，请同样以管理员身份运行本程序）".into())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use enigo::{Button, Coordinate, Direction, Enigo, Mouse as _, Settings};

    pub struct Mouse(Enigo);

    impl Mouse {
        pub fn new() -> Result<Self, String> {
            Enigo::new(&Settings::default())
                .map(Self)
                .map_err(|e| format!("初始化输入设备失败: {e:?}"))
        }

        pub fn primary_size(&self) -> Option<(i32, i32)> {
            self.0.main_display().ok()
        }

        pub fn move_to(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.0
                .move_mouse(x, y, Coordinate::Abs)
                .map_err(|e| format!("鼠标移动失败: {e:?}"))
        }

        pub fn press(&mut self) -> Result<(), String> {
            self.0
                .button(Button::Right, Direction::Press)
                .map_err(|e| format!("鼠标按下失败: {e:?}"))
        }

        pub fn release(&mut self) -> Result<(), String> {
            self.0
                .button(Button::Right, Direction::Release)
                .map_err(|e| format!("鼠标抬起失败: {e:?}"))
        }
    }
}

/// 画笔：记录右键是否按下，离开作用域（包括出错提前返回）时自动抬笔
pub struct Pen<'a> {
    mouse: &'a mut Mouse,
    down: bool,
}

impl<'a> Pen<'a> {
    pub fn new(mouse: &'a mut Mouse) -> Self {
        Self { mouse, down: false }
    }

    pub fn move_to(&mut self, p: (i32, i32)) -> Result<(), String> {
        self.mouse.move_to(p.0, p.1)
    }

    pub fn down(&mut self) -> Result<(), String> {
        self.mouse.press()?;
        self.down = true;
        Ok(())
    }

    pub fn up(&mut self) -> Result<(), String> {
        if self.down {
            self.mouse.release()?;
            self.down = false;
        }
        Ok(())
    }
}

impl Drop for Pen<'_> {
    fn drop(&mut self) {
        if self.down {
            let _ = self.mouse.release();
        }
    }
}
