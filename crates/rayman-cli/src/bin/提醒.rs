#![cfg_attr(windows, windows_subsystem = "windows")]

use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_IDLE_SECONDS: u64 = 10;
const DEFAULT_POLL_MS: u64 = 500;
const DEFAULT_MAX_WAIT_SECONDS: u64 = 15;

fn main() {
    let config = Config::parse(std::env::args().skip(1));
    run(config);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    trigger: String,
    idle_seconds: u64,
    poll_ms: u64,
    max_wait_seconds: u64,
}

impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Self {
        let mut config = Self {
            trigger: "manual".into(),
            idle_seconds: DEFAULT_IDLE_SECONDS,
            poll_ms: DEFAULT_POLL_MS,
            max_wait_seconds: DEFAULT_MAX_WAIT_SECONDS,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--trigger" => {
                    if let Some(value) = args.next() {
                        config.trigger = value;
                    }
                }
                "--idle-seconds" => {
                    if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                        config.idle_seconds = value;
                    }
                }
                "--poll-ms" => {
                    if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                        config.poll_ms = value;
                    }
                }
                "--max-wait-seconds" => {
                    if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                        config.max_wait_seconds = value;
                    }
                }
                _ => {}
            }
        }
        config.idle_seconds = config.idle_seconds.max(1);
        config.poll_ms = config.poll_ms.clamp(100, 5_000);
        config.max_wait_seconds = config.max_wait_seconds.max(config.idle_seconds);
        config
    }
}

fn run(config: Config) {
    let started = Instant::now();
    let threshold = Duration::from_secs(config.idle_seconds);
    let max_wait = Duration::from_secs(config.max_wait_seconds);
    let poll = Duration::from_millis(config.poll_ms);
    let initial_input = platform::input_state();

    loop {
        if platform::screen_is_black() {
            platform::beep_pattern();
            return;
        }
        if let Some(current_input) = platform::input_state() {
            if initial_input
                .as_ref()
                .is_some_and(|initial| current_input.last_input_tick != initial.last_input_tick)
            {
                return;
            }
            if current_input.idle_duration() >= threshold {
                platform::beep_pattern();
                return;
            }
        } else if started.elapsed() >= threshold {
            platform::beep_pattern();
            return;
        }
        if started.elapsed() >= max_wait {
            return;
        }
        thread::sleep(poll);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InputState {
    last_input_tick: u32,
    current_tick: u32,
}

impl InputState {
    fn idle_duration(self) -> Duration {
        Duration::from_millis(self.current_tick.wrapping_sub(self.last_input_tick) as u64)
    }
}

#[cfg(windows)]
mod platform {
    use super::InputState;
    use std::mem::size_of;
    use std::time::Duration;
    use windows_sys::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
    use windows_sys::Win32::System::Diagnostics::Debug::Beep;
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    const BLACK_LUMA_THRESHOLD: u32 = 10;
    const MIN_BLACK_SAMPLES: usize = 20;

    pub(super) fn input_state() -> Option<InputState> {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        let ok = unsafe { GetLastInputInfo(&mut info) };
        (ok != 0).then(|| InputState {
            last_input_tick: info.dwTime,
            current_tick: unsafe { GetTickCount() },
        })
    }

    pub(super) fn screen_is_black() -> bool {
        let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if width <= 0 || height <= 0 {
            return false;
        }
        let hdc = unsafe { GetDC(std::ptr::null_mut()) };
        if hdc.is_null() {
            return false;
        }
        let black = sample_black_pixels(hdc, width, height);
        unsafe {
            ReleaseDC(std::ptr::null_mut(), hdc);
        }
        black
    }

    fn sample_black_pixels(
        hdc: windows_sys::Win32::Graphics::Gdi::HDC,
        width: i32,
        height: i32,
    ) -> bool {
        let mut black = 0usize;
        let mut total = 0usize;
        for y_step in 1..=5 {
            let y = height * y_step / 6;
            for x_step in 1..=7 {
                let x = width * x_step / 8;
                let pixel = unsafe { GetPixel(hdc, x, y) };
                if pixel == u32::MAX {
                    continue;
                }
                total += 1;
                if is_black_pixel(pixel) {
                    black += 1;
                }
            }
        }
        total >= MIN_BLACK_SAMPLES && black * 100 >= total * 95
    }

    fn is_black_pixel(pixel: u32) -> bool {
        let red = pixel & 0xff;
        let green = (pixel >> 8) & 0xff;
        let blue = (pixel >> 16) & 0xff;
        (red * 30 + green * 59 + blue * 11) / 100 <= BLACK_LUMA_THRESHOLD
    }

    pub(super) fn beep_pattern() {
        for _ in 0..3 {
            unsafe {
                Beep(880, 160);
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::is_black_pixel;

        #[test]
        fn black_pixel_detector_rejects_non_black_colors() {
            assert!(is_black_pixel(0x000000));
            assert!(is_black_pixel(0x020202));
            assert!(!is_black_pixel(0xffffff));
            assert!(!is_black_pixel(0x303030));
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::InputState;

    pub(super) fn input_state() -> Option<InputState> {
        None
    }

    pub(super) fn screen_is_black() -> bool {
        false
    }

    pub(super) fn beep_pattern() {
        eprint!("\x07\x07\x07");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_bounds() {
        let config = Config::parse([
            "--trigger".to_string(),
            "goal_stopped".to_string(),
            "--idle-seconds".to_string(),
            "0".to_string(),
            "--poll-ms".to_string(),
            "1".to_string(),
        ]);

        assert_eq!(config.trigger, "goal_stopped");
        assert_eq!(config.idle_seconds, 1);
        assert_eq!(config.poll_ms, 100);
        assert_eq!(config.max_wait_seconds, DEFAULT_MAX_WAIT_SECONDS);
    }

    #[test]
    fn input_idle_duration_uses_wrapping_tick_delta() {
        let state = InputState {
            last_input_tick: u32::MAX - 50,
            current_tick: 49,
        };

        assert_eq!(state.idle_duration(), Duration::from_millis(100));
    }
}
