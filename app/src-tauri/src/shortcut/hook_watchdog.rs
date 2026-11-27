//! Watchdog for the handy-keys low-level keyboard hook (eagleVoice).
//!
//! Windows silently removes a `WH_KEYBOARD_LL` hook whose callback exceeds
//! `LowLevelHooksTimeout`, and gives no notification. handy-keys reinstalls
//! its hook after unlock and resume, but not after a timeout removal. This
//! watchdog detects a dead hook from the outside: if every key of a
//! registered binding is physically down (`GetAsyncKeyState`, which works
//! without hooks) for longer than `HELD_THRESHOLD` while the hook has not
//! reported the binding as pressed, the hook is presumed gone and the
//! manager thread rebuilds the `HotkeyManager` (new hook, same bindings).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long a binding must be physically held without a hook event.
pub const HELD_THRESHOLD: Duration = Duration::from_millis(600);
/// Minimum time between two rebuilds.
pub const REBUILD_COOLDOWN: Duration = Duration::from_secs(10);

const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;
const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;

/// Physical keys a binding needs, as Windows virtual-key codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpec {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    pub key_vk: Option<u16>,
}

fn key_name_to_vk(name: &str) -> Option<u16> {
    let n = name.to_lowercase();
    match n.as_str() {
        "space" => return Some(0x20),
        "escape" | "esc" => return Some(0x1B),
        "enter" | "return" => return Some(0x0D),
        "tab" => return Some(0x09),
        _ => {}
    }
    let chars: Vec<char> = n.chars().collect();
    if chars.len() == 1 {
        let c = chars[0];
        if c.is_ascii_lowercase() {
            return Some(c.to_ascii_uppercase() as u16);
        }
        if c.is_ascii_digit() {
            return Some(c as u16);
        }
    }
    if let Some(num) = n.strip_prefix('f').and_then(|d| d.parse::<u16>().ok()) {
        if (1..=24).contains(&num) {
            return Some(0x70 + num - 1);
        }
    }
    None
}

/// Parses a handy-keys binding string ("ctrl+super+space"). `None` when a part
/// is not understood: such bindings are simply not watched.
pub fn key_spec(hotkey: &str) -> Option<KeySpec> {
    let mut spec = KeySpec {
        ctrl: false,
        shift: false,
        alt: false,
        win: false,
        key_vk: None,
    };
    for part in hotkey.split('+').map(|p| p.trim().to_lowercase()) {
        match part.as_str() {
            "" => {}
            "ctrl" | "control" => spec.ctrl = true,
            "shift" => spec.shift = true,
            "alt" | "opt" | "option" => spec.alt = true,
            "super" | "win" | "windows" | "cmd" | "command" | "meta" => spec.win = true,
            other => {
                if spec.key_vk.is_some() {
                    return None;
                }
                spec.key_vk = Some(key_name_to_vk(other)?);
            }
        }
    }
    if !(spec.ctrl || spec.shift || spec.alt || spec.win || spec.key_vk.is_some()) {
        return None;
    }
    Some(spec)
}

/// The binding's keys are down and no other modifier group is (mirrors
/// handy-keys' matching, which rejects extra modifier groups).
pub fn physically_matches(spec: &KeySpec, is_down: &impl Fn(u16) -> bool) -> bool {
    let win_down = is_down(VK_LWIN) || is_down(VK_RWIN);
    let groups = [
        (spec.ctrl, is_down(VK_CONTROL)),
        (spec.shift, is_down(VK_SHIFT)),
        (spec.alt, is_down(VK_MENU)),
        (spec.win, win_down),
    ];
    if groups.iter().any(|(wanted, down)| wanted != down) {
        return false;
    }
    match spec.key_vk {
        Some(vk) => is_down(vk),
        None => true,
    }
}

#[derive(Debug, Default)]
pub struct HookWatchdog {
    held_since: HashMap<String, Instant>,
    last_rebuild: Option<Instant>,
}

impl HookWatchdog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when the hook should be rebuilt.
    /// `bindings`: (binding_id, hotkey_string) currently registered;
    /// `pressed`: binding ids the hook reported pressed and not yet released.
    pub fn tick(
        &mut self,
        now: Instant,
        bindings: &[(String, String)],
        pressed: &HashSet<String>,
        is_down: impl Fn(u16) -> bool,
    ) -> bool {
        let mut suspicious = false;
        let mut still_held: HashSet<&str> = HashSet::new();
        for (id, hotkey) in bindings {
            let Some(spec) = key_spec(hotkey) else {
                continue;
            };
            if pressed.contains(id) || !physically_matches(&spec, &is_down) {
                continue;
            }
            still_held.insert(id.as_str());
            let since = *self.held_since.entry(id.clone()).or_insert(now);
            if now.saturating_duration_since(since) >= HELD_THRESHOLD {
                suspicious = true;
            }
        }
        self.held_since.retain(|id, _| still_held.contains(id.as_str()));
        if !suspicious {
            return false;
        }
        if let Some(last) = self.last_rebuild {
            if now.saturating_duration_since(last) < REBUILD_COOLDOWN {
                return false;
            }
        }
        self.last_rebuild = Some(now);
        self.held_since.clear();
        true
    }
}

/// `GetAsyncKeyState` high bit.
#[cfg(target_os = "windows")]
pub fn async_key_down(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

#[cfg(not(target_os = "windows"))]
pub fn async_key_down(_vk: u16) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_bindings() {
        let ptt = key_spec("ctrl+super").unwrap();
        assert!(ptt.ctrl && ptt.win && !ptt.shift && !ptt.alt && ptt.key_vk.is_none());
        assert_eq!(key_spec("ctrl+super+space").unwrap().key_vk, Some(0x20));
        assert_eq!(key_spec("shift+alt+z").unwrap().key_vk, Some(0x5A));
        assert_eq!(key_spec("alt+super+1").unwrap().key_vk, Some(0x31));
        assert_eq!(key_spec("escape").unwrap().key_vk, Some(0x1B));
        assert!(key_spec("ctrl+mouse4").is_none());
    }

    #[test]
    fn detects_dead_hook_only_after_threshold() {
        let bindings = vec![("transcribe".to_string(), "ctrl+super".to_string())];
        let down = |vk: u16| vk == 0x11 || vk == 0x5B;
        let mut wd = HookWatchdog::new();
        let t = Instant::now();
        let none = HashSet::new();
        assert!(!wd.tick(t, &bindings, &none, down));
        assert!(!wd.tick(t + Duration::from_millis(300), &bindings, &none, down));
        assert!(wd.tick(t + Duration::from_millis(700), &bindings, &none, down));
        // Cooldown: no second rebuild right away.
        assert!(!wd.tick(t + Duration::from_millis(1400), &bindings, &none, down));
        // Hook alive: the binding is reported pressed, nothing to do.
        let mut wd = HookWatchdog::new();
        let pressed: HashSet<String> = ["transcribe".to_string()].into();
        assert!(!wd.tick(t, &bindings, &pressed, down));
        assert!(!wd.tick(t + Duration::from_secs(2), &bindings, &pressed, down));
        // Extra modifier (Ctrl+Win+Shift) does not match the binding.
        let mut wd = HookWatchdog::new();
        let down_shift = |vk: u16| vk == 0x11 || vk == 0x5B || vk == 0x10;
        assert!(!wd.tick(t, &bindings, &none, down_shift));
        assert!(!wd.tick(t + Duration::from_secs(2), &bindings, &none, down_shift));
    }
}
