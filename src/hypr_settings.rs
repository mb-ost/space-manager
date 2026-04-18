use std::sync::{Mutex, OnceLock};

use tracing::{error, info};

#[derive(Debug, Default)]
struct FollowMouseState {
    depth: usize,
    original_value: Option<i64>,
}

fn get_follow_mouse_state() -> &'static Mutex<FollowMouseState> {
    static FOLLOW_MOUSE_STATE: OnceLock<Mutex<FollowMouseState>> = OnceLock::new();
    FOLLOW_MOUSE_STATE.get_or_init(|| Mutex::new(FollowMouseState::default()))
}

fn get_follow_mouse_setting() -> i64 {
    std::process::Command::new("hyprctl")
        .arg("getoption")
        .arg("input:follow_mouse")
        .arg("-j")
        .output()
        .ok()
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
        .and_then(|json| json["int"].as_i64())
        .unwrap_or(1)
}

fn set_follow_mouse(value: i64) {
    let _ = std::process::Command::new("hyprctl")
        .arg("keyword")
        .arg("input:follow_mouse")
        .arg(format!("{}", value))
        .output();
    info!("Set follow_mouse to: {}", value);
}

pub struct FollowMouseGuard {
    active: bool,
}

impl FollowMouseGuard {
    pub fn suppress() -> Self {
        let state = get_follow_mouse_state();
        let mut state = match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                error!("follow_mouse state mutex was poisoned, recovering");
                poisoned.into_inner()
            }
        };

        if state.depth == 0 {
            let original_value = get_follow_mouse_setting();
            state.original_value = Some(original_value);
            set_follow_mouse(0);
            info!("Captured follow_mouse original value: {}", original_value);
        }

        state.depth += 1;
        info!(
            "follow_mouse suppression depth increased to {}",
            state.depth
        );

        Self { active: true }
    }
}

impl Drop for FollowMouseGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let state = get_follow_mouse_state();
        let mut state = match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                error!("follow_mouse state mutex was poisoned during drop, recovering");
                poisoned.into_inner()
            }
        };

        if state.depth == 0 {
            error!("follow_mouse suppression depth underflow");
            return;
        }

        state.depth -= 1;
        info!(
            "follow_mouse suppression depth decreased to {}",
            state.depth
        );

        if state.depth == 0 {
            if let Some(original_value) = state.original_value.take() {
                set_follow_mouse(original_value);
                info!("Restored follow_mouse to: {}", original_value);
            }
        }
    }
}
