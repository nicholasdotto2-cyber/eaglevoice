//! Tasti simulati con `SendInput`: rilascio dei modificatori tenuti con
//! mascheramento di Win/Alt, scorciatoie di incolla e copia, Invio.

use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC,
    VIRTUAL_KEY,
};

/// Marcatore in `dwExtraInfo` dei tasti iniettati da questo modulo.
pub const INJECT_TAG: usize = 0x4547_4C56; // "EGLV"
/// Tasto neutro non assegnato usato come maschera: fra pressione e rilascio
/// di Win (o Alt) la shell vede "un altro tasto" e non apre Start (o il menu).
pub const MASK_VK: u16 = 0xE8;

pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_V: u16 = 0x56;
pub const VK_C: u16 = 0x43;

pub const VK_LEFT: u16 = 0x25;

pub(crate) const VK_LSHIFT: u16 = 0xA0;
const VK_RSHIFT: u16 = 0xA1;
const VK_LCONTROL: u16 = 0xA2;
const VK_RCONTROL: u16 = 0xA3;
const VK_LMENU: u16 = 0xA4;
const VK_RMENU: u16 = 0xA5;
const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;

fn is_extended(vk: u16) -> bool {
    matches!(vk, VK_RCONTROL | VK_RMENU | VK_LWIN | VK_RWIN)
}

pub fn key_input(vk: u16, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECT_TAG,
            },
        },
    }
}

/// Come `key_input` ma con lo scan code del tasto (le app che leggono lo
/// scan code, come i terminali e Chromium, non vedono un tasto con scan 0)
/// e, con `extended`, il flag dei tasti estesi (frecce: senza, con Bloc Num
/// attivo varrebbero come tastierino numerico).
pub fn key_input_scan(vk: u16, up: bool, extended: bool) -> INPUT {
    let mut input = key_input(vk, up);
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    unsafe {
        input.Anonymous.ki.wScan = scan;
        if extended {
            input.Anonymous.ki.dwFlags |= KEYEVENTF_EXTENDEDKEY;
        }
    }
    input
}

const MODIFIER_VKS: [u16; 8] = [
    VK_LCONTROL, VK_RCONTROL, VK_LSHIFT, VK_RSHIFT, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN,
];

/// Modificatori giù secondo lo stato asincrono della tastiera.
pub fn held_modifiers() -> Vec<u16> {
    MODIFIER_VKS.into_iter().filter(|vk| is_down(*vk)).collect()
}

/// Aspetta che l'utente lasci i modificatori della scorciatoia, fino a `timeout`.
/// Va chiamata PRIMA di iniettare rilasci: finché non abbiamo mandato nulla
/// lo stato asincrono è quello fisico; dopo un rilascio iniettato dice "su"
/// anche se il dito è ancora sul tasto. Ritorna vero se sono tutti su.
pub fn wait_modifiers_released(timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if held_modifiers().is_empty() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn send(inputs: &[INPUT]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(format!(
            "SendInput ha inviato {sent} eventi su {} (UIPI o desktop protetto?)",
            inputs.len()
        ))
    }
}

pub fn is_down(vk: u16) -> bool {
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

/// Rilascia Ctrl, Shift, Alt e Win ancora premuti (per esempio la scorciatoia
/// della dettatura), così la scorciatoia di incolla non diventa Ctrl+Win+V.
/// Con `mask`, prima del rilascio di Win o Alt inietta il tasto neutro.
/// Ritorna i tasti rilasciati.
pub fn release_held_modifiers(mask: bool) -> Result<Vec<u16>, String> {
    let held = held_modifiers();
    if held.is_empty() {
        return Ok(held);
    }
    let mut inputs = Vec::new();
    if mask && held.iter().any(|vk| matches!(*vk, VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN)) {
        inputs.push(key_input(MASK_VK, false));
        inputs.push(key_input(MASK_VK, true));
    }
    for vk in &held {
        inputs.push(key_input(*vk, true));
    }
    send(&inputs)?;
    Ok(held)
}

/// Ctrl+tasto oppure Ctrl+Shift+tasto, con i modificatori tenuti `hold`.
pub fn send_ctrl_chord(vk: u16, with_shift: bool, hold: Duration) -> Result<(), String> {
    let mut down = vec![key_input(VK_CONTROL, false)];
    if with_shift {
        down.push(key_input(VK_SHIFT, false));
    }
    down.push(key_input(vk, false));
    down.push(key_input(vk, true));
    send(&down)?;
    std::thread::sleep(hold);
    let mut up = Vec::new();
    if with_shift {
        up.push(key_input(VK_SHIFT, true));
    }
    up.push(key_input(VK_CONTROL, true));
    send(&up)
}

pub fn send_enter() -> Result<(), String> {
    send(&[key_input(VK_RETURN, false), key_input(VK_RETURN, true)])
}

/// Digita testo Unicode (usato dalle prove sulla finestra di prova).
pub fn type_unicode(text: &str) -> Result<(), String> {
    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        for up in [false, true] {
            let mut flags = KEYEVENTF_UNICODE;
            if up {
                flags |= KEYEVENTF_KEYUP;
            }
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: INJECT_TAG,
                    },
                },
            });
        }
    }
    send(&inputs)
}
