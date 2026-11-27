//! Tasti premuti durante l'osservazione dopo l'incolla.
//!
//! Un hook `WH_KEYBOARD_LL` su un thread suo, acceso solo mentre almeno
//! un'osservazione è in corso (contatore di riferimenti) e spento subito dopo.
//! Non blocca nulla e non registra testo: conta le pressioni, ricorda
//! l'ultimo tasto (nome e modificatori) e conta gli Invio senza Maiusc, che in
//! Claude e ChatGPT mandano il messaggio e svuotano il campo. L'hook parte
//! prima che il tasto arrivi all'app: chi osserva può ancora rileggere il campo.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT,
    WM_SYSKEYDOWN, WM_USER,
};
use windows::core::PCWSTR;

use super::keys::MASK_VK;

static KEY_SEQ: AtomicU64 = AtomicU64::new(0);
static ENTER_SEQ: AtomicU64 = AtomicU64::new(0);
/// Ultimo tasto: vk nei 16 bit bassi, poi bit 16 Ctrl, 17 Maiusc, 18 Alt, 19 Win.
static LAST_KEY: AtomicU32 = AtomicU32::new(0);

const VK_RETURN: u32 = 0x0D;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeySnapshot {
    /// Pressioni viste dall'accensione dell'hook (ripetizioni comprese).
    pub seq: u64,
    /// Invio senza Maiusc.
    pub enter_seq: u64,
    last: u32,
}

impl KeySnapshot {
    /// Nome dell'ultimo tasto, es. "Invio", "Ctrl+Z", "Backspace"; `None` se nessuno.
    pub fn last_key(&self) -> Option<String> {
        (self.last != 0).then(|| key_name(self.last))
    }
}

pub fn snapshot() -> KeySnapshot {
    KeySnapshot {
        seq: KEY_SEQ.load(Ordering::SeqCst),
        enter_seq: ENTER_SEQ.load(Ordering::SeqCst),
        last: LAST_KEY.load(Ordering::SeqCst),
    }
}

fn down(vk: i32) -> bool {
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode;
        // Il tasto neutro della maschera di Win (nostro e di handy-keys) non conta.
        if vk != MASK_VK as u32 && !matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5) {
            let shift = down(0x10);
            let mods = (down(0x11) as u32) << 16
                | (shift as u32) << 17
                | (down(0x12) as u32) << 18
                | ((down(0x5B) || down(0x5C)) as u32) << 19;
            LAST_KEY.store(vk | mods, Ordering::SeqCst);
            KEY_SEQ.fetch_add(1, Ordering::SeqCst);
            if vk == VK_RETURN && !shift {
                ENTER_SEQ.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

pub fn key_name(packed: u32) -> String {
    let vk = packed & 0xFFFF;
    let base = match vk {
        0x08 => "Backspace".to_string(),
        0x09 => "Tab".to_string(),
        0x0D => "Invio".to_string(),
        0x1B => "Esc".to_string(),
        0x20 => "Spazio".to_string(),
        0x25 => "Sinistra".to_string(),
        0x26 => "Su".to_string(),
        0x27 => "Destra".to_string(),
        0x28 => "Giù".to_string(),
        0x2E => "Canc".to_string(),
        0x24 => "Home".to_string(),
        0x23 => "Fine".to_string(),
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap_or('?').to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("vk_{vk:02X}"),
    };
    let mut name = String::new();
    for (bit, label) in [(16, "Ctrl+"), (17, "Maiusc+"), (18, "Alt+"), (19, "Win+")] {
        if packed & (1 << bit) != 0 {
            name.push_str(label);
        }
    }
    name.push_str(&base);
    name
}

struct Running {
    users: usize,
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
}

static RUNNING: Mutex<Running> = Mutex::new(Running { users: 0, thread_id: 0, handle: None });

/// Hook acceso finché la guardia vive.
pub struct KeyWatch;

impl Drop for KeyWatch {
    fn drop(&mut self) {
        let Ok(mut r) = RUNNING.lock() else { return };
        r.users = r.users.saturating_sub(1);
        if r.users == 0 {
            if r.thread_id != 0 {
                unsafe {
                    let _ = PostThreadMessageW(r.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
            }
            r.thread_id = 0;
            if let Some(h) = r.handle.take() {
                let _ = h.join();
            }
        }
    }
}

/// Accende l'hook (o ne aggiunge un utente). `None` se l'hook non si installa:
/// l'osservazione va avanti senza Invio e senza ultimo tasto.
pub fn start() -> Option<KeyWatch> {
    let mut r = RUNNING.lock().ok()?;
    if r.users > 0 {
        r.users += 1;
        return Some(KeyWatch);
    }
    let (tx, rx) = std::sync::mpsc::channel::<Option<u32>>();
    let handle = std::thread::Builder::new()
        .name("win-keywatch".into())
        .spawn(move || unsafe {
            let mut msg = MSG::default();
            // Crea la coda dei messaggi prima di dare il thread id.
            let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
            let hinstance = GetModuleHandleW(PCWSTR::null()).ok();
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hinstance.map(|h| h.into()), 0);
            let Ok(hook) = hook else {
                let _ = tx.send(None);
                return;
            };
            let _ = tx.send(Some(GetCurrentThreadId()));
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}
            let _ = UnhookWindowsHookEx(hook);
        })
        .ok()?;
    match rx.recv_timeout(Duration::from_millis(500)) {
        Ok(Some(tid)) => {
            r.users = 1;
            r.thread_id = tid;
            r.handle = Some(handle);
            Some(KeyWatch)
        }
        _ => {
            log::warn!("[win] hook dei tasti per l'osservazione non installato");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::key_name;

    #[test]
    fn nomi_dei_tasti() {
        assert_eq!(key_name(0x0D), "Invio");
        assert_eq!(key_name(0x5A | 1 << 16), "Ctrl+Z");
        assert_eq!(key_name(0x41 | 1 << 17), "Maiusc+A");
        assert_eq!(key_name(0x2E), "Canc");
        assert_eq!(key_name(0xBA), "vk_BA");
    }
}
