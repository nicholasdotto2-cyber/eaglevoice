//! Contesto alla pressione del tasto:
//! finestra e processo in primo piano, URL se è un browser, testo del campo
//! col fuoco via UI Automation (prima, selezione, dopo), categoria, nomi propri.
//! Tetto di tempo complessivo; ciò che non arriva in tempo resta vuoto.

use std::time::{Duration, Instant};

use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW,
    GetWindowThreadProcessId,
};

use super::category::{classify, friendly_app_name, is_browser, is_terminal};
use super::text::{
    extract_proper_nouns, truncate_head_on_word, truncate_tail_on_word, FIELD_PIECE_MAX_CHARS,
};
use super::uia::{take_variant_bool, variant_i32, FIELD_WORKER, URL_WORKER};
use super::{AppContext, FieldText, WinSettings};

/// Caratteri letti per ciascun pezzo prima del taglio su confine di parola.
const READ_CHARS: i32 = 600;
/// Tetto per la lettura della barra degli indirizzi.
pub const URL_TIMEOUT: Duration = Duration::from_millis(50);
pub const MAX_PROPER_NOUNS: usize = 30;

/// Dati della finestra in primo piano (solo Win32, nessuna chiamata UIA).
#[derive(Debug, Clone, Default)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub pid: u32,
    pub exe_path: String,
    pub class: String,
    pub title: String,
}

fn wide_to_string(buf: &[u16], len: i32) -> String {
    let len = len.max(0) as usize;
    String::from_utf16_lossy(&buf[..len.min(buf.len())])
}

pub(crate) fn hwnd_from(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

fn process_path(pid: u32) -> String {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if ok {
            String::from_utf16_lossy(&buf[..size as usize])
        } else {
            String::new()
        }
    }
}

/// Informazioni su una finestra qualsiasi.
pub fn window_info(hwnd_raw: isize) -> WindowInfo {
    if hwnd_raw == 0 {
        return WindowInfo::default();
    }
    let hwnd = hwnd_from(hwnd_raw);
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let mut class = [0u16; 256];
        let class_len = GetClassNameW(hwnd, &mut class);
        let mut title = [0u16; 512];
        let title_len = GetWindowTextW(hwnd, &mut title);
        let mut info = WindowInfo {
            hwnd: hwnd_raw,
            pid,
            exe_path: process_path(pid),
            class: wide_to_string(&class, class_len),
            title: wide_to_string(&title, title_len),
        };
        // App UWP: la finestra in primo piano è di ApplicationFrameHost, l'app
        // vera è una finestra figlia di un altro processo.
        if info.exe_path.to_lowercase().ends_with("applicationframehost.exe") {
            if let Some(child_pid) = uwp_child_pid(hwnd, pid) {
                info.pid = child_pid;
                info.exe_path = process_path(child_pid);
            }
        }
        info
    }
}

unsafe extern "system" fn enum_child(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let data = &mut *(lparam.0 as *mut (u32, u32));
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid != 0 && pid != data.0 {
        data.1 = pid;
        return BOOL(0);
    }
    BOOL(1)
}

fn uwp_child_pid(hwnd: HWND, host_pid: u32) -> Option<u32> {
    let mut data: (u32, u32) = (host_pid, 0);
    unsafe {
        let _ = EnumChildWindows(
            Some(hwnd),
            Some(enum_child),
            LPARAM(&mut data as *mut (u32, u32) as isize),
        );
    }
    (data.1 != 0).then_some(data.1)
}

pub fn foreground_info() -> WindowInfo {
    let hwnd = unsafe { GetForegroundWindow() };
    window_info(hwnd.0 as isize)
}

/// Testo del campo letto via UIA, prima del taglio a 500 caratteri.
#[derive(Debug, Clone, Default)]
pub(crate) struct RawField {
    pub before: String,
    pub selected: String,
    pub after: String,
    pub is_editable: bool,
    pub source: &'static str,
}

unsafe fn range_text(r: &IUIAutomationTextRange, max: i32) -> String {
    r.GetText(max).map(|b| b.to_string()).unwrap_or_default()
}

unsafe fn is_degenerate(r: &IUIAutomationTextRange) -> bool {
    r.CompareEndpoints(TextPatternRangeEndpoint_Start, r, TextPatternRangeEndpoint_End)
        .map(|c| c == 0)
        .unwrap_or(true)
}

/// Errore di lettura quando i confini del campo non si possono usare.
const FIELD_BOUNDS_ERR: &str = "field_bounds";

/// Riporta gli estremi di `r` dentro `bounds` (il DocumentRange del campo).
/// Errore se un confronto fallisce: in Chromium, al primo fuoco su un campo
/// appena attivato, il DocumentRange può essere già scaduto (0x80040201,
/// 0x80131509) e senza confini la lettura prenderebbe il testo della pagina.
unsafe fn clamp_to(
    r: &IUIAutomationTextRange,
    bounds: &IUIAutomationTextRange,
) -> Result<(), &'static str> {
    use TextPatternRangeEndpoint_End as END;
    use TextPatternRangeEndpoint_Start as START;
    let start = r.CompareEndpoints(START, bounds, START).map_err(|_| FIELD_BOUNDS_ERR)?;
    if start < 0 {
        r.MoveEndpointByRange(START, bounds, START).map_err(|_| FIELD_BOUNDS_ERR)?;
    }
    let end = r.CompareEndpoints(END, bounds, END).map_err(|_| FIELD_BOUNDS_ERR)?;
    if end > 0 {
        r.MoveEndpointByRange(END, bounds, END).map_err(|_| FIELD_BOUNDS_ERR)?;
    }
    Ok(())
}

unsafe fn element_editable(el: &IUIAutomationElement, range: Option<&IUIAutomationTextRange>) -> bool {
    if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
        if let Ok(ro) = vp.CurrentIsReadOnly() {
            return !ro.as_bool();
        }
    }
    if let Some(r) = range {
        if let Ok(v) = r.GetAttributeValue(UIA_IsReadOnlyAttributeId) {
            if let Some(ro) = take_variant_bool(v) {
                return !ro;
            }
        }
    }
    matches!(el.CurrentControlType(), Ok(t) if t == UIA_EditControlTypeId)
}

/// Oltre questa lunghezza il campo non viene esplorato in cerca di testo
/// generato: i segnaposto compaiono solo nei campi vuoti.
const GENERATED_SCAN_MAX: i32 = 300;

/// Testo generato dentro il campo (segnaposto disegnato col CSS `::before`,
/// come in ProseMirror e tiptap): Chromium lo espone come testo statico con
/// `readonly=true` nelle AriaProperties, mentre il testo scritto nel campo
/// non ce l'ha. Intervalli in ordine di inizio.
unsafe fn generated_ranges(
    a: &IUIAutomation,
    el: &IUIAutomationElement,
    tp: &IUIAutomationTextPattern,
    bounds: &IUIAutomationTextRange,
) -> Vec<IUIAutomationTextRange> {
    let mut out = Vec::new();
    if range_text(bounds, GENERATED_SCAN_MAX + 1).chars().count() > GENERATED_SCAN_MAX as usize {
        return out;
    }
    let Ok(cond) =
        a.CreatePropertyCondition(UIA_ControlTypePropertyId, &variant_i32(UIA_TextControlTypeId.0))
    else {
        return out;
    };
    let Ok(all) = el.FindAll(TreeScope_Descendants, &cond) else {
        return out;
    };
    for i in 0..all.Length().unwrap_or(0) {
        let Ok(t) = all.GetElement(i) else { continue };
        let aria = t.CurrentAriaProperties().map(|b| b.to_string()).unwrap_or_default();
        if aria.split(';').any(|p| p.trim() == "readonly=true") {
            if let Ok(r) = tp.RangeFromChild(&t) {
                out.push(r);
            }
        }
    }
    out.sort_by(|x, y| {
        x.CompareEndpoints(TextPatternRangeEndpoint_Start, y, TextPatternRangeEndpoint_Start)
            .unwrap_or(0)
            .cmp(&0)
    });
    out
}

/// Testo di `r` senza le parti che cadono negli intervalli `excluded`.
unsafe fn text_excluding(
    r: &IUIAutomationTextRange,
    excluded: &[IUIAutomationTextRange],
    limit: i32,
) -> String {
    use TextPatternRangeEndpoint_End as END;
    use TextPatternRangeEndpoint_Start as START;
    if excluded.is_empty() {
        return range_text(r, limit);
    }
    let Ok(cur) = r.Clone() else {
        return range_text(r, limit);
    };
    let mut out = String::new();
    for g in excluded {
        let ends_before = g.CompareEndpoints(END, &cur, START).map(|c| c <= 0).unwrap_or(true);
        let starts_after = g.CompareEndpoints(START, &cur, END).map(|c| c >= 0).unwrap_or(true);
        if ends_before || starts_after {
            continue;
        }
        if g.CompareEndpoints(START, &cur, START).map(|c| c > 0).unwrap_or(false) {
            if let Ok(seg) = cur.Clone() {
                let _ = seg.MoveEndpointByRange(END, g, START);
                out.push_str(&range_text(&seg, limit));
            }
        }
        let _ = cur.MoveEndpointByRange(START, g, END);
    }
    out.push_str(&range_text(&cur, limit));
    out
}

/// Ripulitura comune a ogni lettura. Il carattere U+FFFC (oggetto incorporato,
/// che Chromium mette al posto del contenuto di textarea e input) non è testo.
/// Un campo il cui testo intero coincide con il segnaposto dichiarato
/// (HelpText, dove Chromium mette `placeholder` e `aria-placeholder`) è vuoto.
/// Un campo fatto solo di spazi e a capo è vuoto.
fn clean_field(mut raw: RawField, placeholder: &str) -> RawField {
    for piece in [&mut raw.before, &mut raw.selected, &mut raw.after] {
        if piece.contains('\u{FFFC}') {
            *piece = piece.replace('\u{FFFC}', "");
        }
    }
    let whole = format!("{}{}{}", raw.before, raw.selected, raw.after);
    let whole = whole.trim();
    let placeholder = placeholder.trim();
    if whole.is_empty() || (!placeholder.is_empty() && whole == placeholder) {
        raw.before.clear();
        raw.selected.clear();
        raw.after.clear();
    }
    raw
}

/// Legge prima, selezione e dopo il cursore di un elemento, limitati al campo
/// e senza segnaposto. TextPattern2 (caret) → TextPattern (selezione) →
/// ValuePattern (valore intero).
pub(crate) unsafe fn read_field(
    a: &IUIAutomation,
    el: &IUIAutomationElement,
    limit: i32,
) -> Result<RawField, &'static str> {
    // Confini scaduti al primo fuoco: una seconda lettura con pattern nuovi
    // di solito basta; se fallisce ancora il campo resta sconosciuto.
    let raw = match read_field_raw(a, el, limit) {
        Err(FIELD_BOUNDS_ERR) => read_field_raw(a, el, limit)?,
        other => other?,
    };
    let placeholder = el.CurrentHelpText().map(|b| b.to_string()).unwrap_or_default();
    Ok(clean_field(raw, &placeholder))
}

unsafe fn read_field_raw(
    a: &IUIAutomation,
    el: &IUIAutomationElement,
    limit: i32,
) -> Result<RawField, &'static str> {
    if el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false) {
        return Err("password");
    }
    let tp2 = el
        .GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
        .ok();
    let tp: Option<IUIAutomationTextPattern> = match &tp2 {
        Some(p) => p.cast().ok(),
        None => el
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .ok(),
    };
    if let Some(tp) = tp {
        let sel = tp.GetSelection().ok().and_then(|arr| {
            if arr.Length().unwrap_or(0) > 0 {
                arr.GetElement(0).ok()
            } else {
                None
            }
        });
        let has_selection = sel.as_ref().map(|r| !is_degenerate(r)).unwrap_or(false);
        let mut source = "text_pattern";
        let base = if has_selection {
            sel
        } else if let Some(p2) = &tp2 {
            let mut active = BOOL(0);
            match p2.GetCaretRange(&mut active) {
                Ok(r) => {
                    source = "text_pattern2";
                    Some(r)
                }
                Err(_) => sel,
            }
        } else {
            sel
        };
        // Confini del campo: in Chromium (Claude, Code, ChatGPT, Comet, Edge)
        // un intervallo spostato a caratteri esce dal campo e prende il testo
        // della pagina intorno (prova `live_campo_vuoto_chromium`).
        let bounds = tp.DocumentRange().map_err(|_| FIELD_BOUNDS_ERR)?;
        let generated = generated_ranges(a, el, &tp, &bounds);
        if let Some(base) = base {
            clamp_to(&base, &bounds)?;
            let selected = if has_selection {
                text_excluding(&base, &generated, limit)
            } else {
                String::new()
            };
            let before = match base.Clone() {
                Ok(r) => {
                    let _ = r.MoveEndpointByRange(
                        TextPatternRangeEndpoint_End,
                        &base,
                        TextPatternRangeEndpoint_Start,
                    );
                    let _ = r.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -limit);
                    clamp_to(&r, &bounds)?;
                    text_excluding(&r, &generated, limit)
                }
                Err(_) => String::new(),
            };
            let after = match base.Clone() {
                Ok(r) => {
                    let _ = r.MoveEndpointByRange(
                        TextPatternRangeEndpoint_Start,
                        &base,
                        TextPatternRangeEndpoint_End,
                    );
                    let _ = r.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, limit);
                    clamp_to(&r, &bounds)?;
                    text_excluding(&r, &generated, limit)
                }
                Err(_) => String::new(),
            };
            return Ok(RawField {
                before,
                selected,
                after,
                is_editable: element_editable(el, Some(&base)),
                source,
            });
        }
        // Nessun cursore né selezione: tutto il documento come "prima".
        return Ok(RawField {
            before: text_excluding(&bounds, &generated, limit),
            is_editable: element_editable(el, Some(&bounds)),
            source,
            ..Default::default()
        });
    }
    if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
        let value = vp.CurrentValue().map(|b| b.to_string()).unwrap_or_default();
        let read_only = vp.CurrentIsReadOnly().map(|b| b.as_bool()).unwrap_or(true);
        return Ok(RawField {
            before: value,
            is_editable: !read_only,
            source: "value_pattern",
            ..Default::default()
        });
    }
    Err("no_text_pattern")
}

/// Testo intero di un elemento (per l'osservazione dopo l'incolla).
pub(crate) unsafe fn read_full_text(el: &IUIAutomationElement, max: i32) -> Option<String> {
    if let Ok(tp) = el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
        if let Ok(doc) = tp.DocumentRange() {
            if let Ok(t) = doc.GetText(max) {
                return Some(t.to_string());
            }
        }
    }
    if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
        if let Ok(v) = vp.CurrentValue() {
            return Some(v.to_string());
        }
    }
    None
}

/// Elemento col fuoco, solo se appartiene al processo atteso.
pub(crate) unsafe fn focused_element(
    a: &IUIAutomation,
    expected_pid: u32,
) -> Result<IUIAutomationElement, &'static str> {
    let el = a.GetFocusedElement().map_err(|_| "no_focus")?;
    let pid = el.CurrentProcessId().unwrap_or(0) as u32;
    if expected_pid != 0 && pid != expected_pid {
        return Err("focus_other_process");
    }
    Ok(el)
}

fn looks_like_url(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty()
        && !v.contains(char::is_whitespace)
        && (v.contains("://") || v.starts_with("about:") || v.contains('.'))
}

/// URL dalla barra degli indirizzi: primo campo Edit della finestra con un
/// ValuePattern che sembra un indirizzo.
pub(crate) unsafe fn address_bar_url(a: &IUIAutomation, hwnd_raw: isize) -> Option<String> {
    let root = a.ElementFromHandle(hwnd_from(hwnd_raw)).ok()?;
    let cond = a
        .CreatePropertyCondition(UIA_ControlTypePropertyId, &variant_i32(UIA_EditControlTypeId.0))
        .ok()?;
    let edit = root.FindFirst(TreeScope_Descendants, &cond).ok()?;
    let vp = edit
        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        .ok()?;
    let value = vp.CurrentValue().ok()?.to_string();
    looks_like_url(&value).then(|| value.trim().to_string())
}

fn to_field_text(raw: &RawField) -> FieldText {
    FieldText {
        before: truncate_tail_on_word(&raw.before, FIELD_PIECE_MAX_CHARS),
        selected: truncate_head_on_word(&raw.selected, FIELD_PIECE_MAX_CHARS),
        after: truncate_head_on_word(&raw.after, FIELD_PIECE_MAX_CHARS),
        is_editable: raw.is_editable,
    }
}

fn base_context(info: &WindowInfo, settings: &WinSettings) -> AppContext {
    AppContext {
        hwnd: info.hwnd,
        process_exe: super::category::exe_file_name(&info.exe_path).to_string(),
        app_name: friendly_app_name(&info.exe_path),
        window_title: info.title.clone(),
        url: None,
        category: classify(&settings.category_table, &info.exe_path, None),
        field: None,
        proper_nouns: Vec::new(),
        captured_in_ms: 0,
        pid: info.pid,
        window_class: info.class.clone(),
        is_terminal: is_terminal(&info.class, &info.exe_path, &settings.terminal_processes),
        field_source: None,
    }
}

fn finish(ctx: &mut AppContext, settings: &WinSettings, t0: Instant) {
    ctx.category = classify(&settings.category_table, &ctx.process_exe, ctx.url.as_deref());
    let title = ctx.window_title.clone();
    let mut texts: Vec<&str> = Vec::new();
    if let Some(f) = &ctx.field {
        texts.extend([f.before.as_str(), f.selected.as_str(), f.after.as_str()]);
    }
    texts.push(&title);
    ctx.proper_nouns = extract_proper_nouns(&texts, MAX_PROPER_NOUNS);
    ctx.captured_in_ms = t0.elapsed().as_millis() as u64;
}

/// Contesto della finestra in primo piano, entro `timeout` (default 150 ms).
/// Da chiamare alla pressione del tasto, in un thread a parte.
pub fn capture_context(timeout: Duration) -> AppContext {
    let t0 = Instant::now();
    let settings = super::current_settings();
    let info = foreground_info();
    let mut ctx = base_context(&info, &settings);
    if info.hwnd == 0 {
        ctx.field_source = Some("no_foreground".into());
        finish(&mut ctx, &settings, t0);
        return ctx;
    }

    // URL in parallelo al campo, con un tetto suo di 50 ms.
    let url_rx = if is_browser(&info.exe_path) {
        let (tx, rx) = std::sync::mpsc::channel();
        let hwnd = info.hwnd;
        let budget = URL_TIMEOUT.min(timeout);
        std::thread::spawn(move || {
            let url = URL_WORKER
                .run(budget, move |a| unsafe { address_bar_url(a, hwnd) })
                .flatten();
            let _ = tx.send(url);
        });
        Some(rx)
    } else {
        None
    };

    if ctx.is_terminal {
        // Privacy: nei terminali il testo (comandi, output) non si legge.
        ctx.field_source = Some("terminal".into());
    } else {
        let pid = info.pid;
        let remaining = timeout.saturating_sub(t0.elapsed());
        let read = FIELD_WORKER.run(remaining, move |a| unsafe {
            let el = focused_element(a, pid)?;
            read_field(a, &el, READ_CHARS)
        });
        match read {
            Some(Ok(raw)) => {
                ctx.field = Some(to_field_text(&raw));
                ctx.field_source = Some(raw.source.into());
            }
            Some(Err(reason)) => ctx.field_source = Some(reason.into()),
            None => ctx.field_source = Some("timeout".into()),
        }
    }

    if let Some(rx) = url_rx {
        let remaining = timeout.saturating_sub(t0.elapsed()).max(Duration::from_millis(1));
        ctx.url = rx.recv_timeout(remaining).ok().flatten();
    }
    finish(&mut ctx, &settings, t0);
    ctx
}

/// Diagnostica in sola lettura su una finestra che non è in primo piano:
/// processo, URL e primo campo di testo (Edit o Document) esposto da UIA.
/// Non sposta il fuoco e non invia tasti. Il testo del campo finisce in
/// `field.before` (tagliato): chi lo usa nei report ne stampa solo la lunghezza.
pub fn inspect_window(hwnd_raw: isize, timeout: Duration) -> AppContext {
    let t0 = Instant::now();
    let settings = super::current_settings();
    let info = window_info(hwnd_raw);
    let mut ctx = base_context(&info, &settings);
    if is_browser(&info.exe_path) {
        ctx.url = URL_WORKER
            .run(URL_TIMEOUT.max(timeout / 2), move |a| unsafe {
                address_bar_url(a, hwnd_raw)
            })
            .flatten();
    }
    if ctx.is_terminal {
        ctx.field_source = Some("terminal".into());
    } else {
        let remaining = timeout.saturating_sub(t0.elapsed()).max(Duration::from_millis(10));
        let read = FIELD_WORKER.run(remaining, move |a| unsafe {
            let root = a.ElementFromHandle(hwnd_from(hwnd_raw)).map_err(|_| "no_element")?;
            let edit = a
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &variant_i32(UIA_EditControlTypeId.0))
                .map_err(|_| "condition")?;
            let doc = a
                .CreatePropertyCondition(
                    UIA_ControlTypePropertyId,
                    &variant_i32(UIA_DocumentControlTypeId.0),
                )
                .map_err(|_| "condition")?;
            let either = a.CreateOrCondition(&edit, &doc).map_err(|_| "condition")?;
            let el = root
                .FindFirst(TreeScope_Descendants, &either)
                .map_err(|_| "no_text_element")?;
            let full = read_full_text(&el, 4000).ok_or("no_text_pattern")?;
            Ok::<_, &'static str>(RawField {
                before: full,
                is_editable: element_editable(&el, None),
                source: "inspect",
                ..Default::default()
            })
        });
        match read {
            Some(Ok(raw)) => {
                ctx.field = Some(FieldText {
                    before: raw.before,
                    is_editable: raw.is_editable,
                    ..Default::default()
                });
                ctx.field_source = Some(raw.source.into());
            }
            Some(Err(reason)) => ctx.field_source = Some(reason.into()),
            None => ctx.field_source = Some("timeout".into()),
        }
    }
    finish(&mut ctx, &settings, t0);
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(before: &str, selected: &str, after: &str) -> RawField {
        RawField {
            before: before.into(),
            selected: selected.into(),
            after: after.into(),
            is_editable: true,
            source: "text_pattern",
        }
    }

    #[test]
    fn campo_con_solo_oggetto_incorporato_o_a_capo_e_vuoto() {
        let f = clean_field(raw("", "", "\u{FFFC}"), "");
        assert_eq!((f.before.as_str(), f.after.as_str()), ("", ""));
        let f = clean_field(raw("\n", "", ""), "");
        assert_eq!(f.before, "");
        assert!(f.is_editable);
    }

    #[test]
    fn campo_uguale_al_segnaposto_dichiarato_e_vuoto() {
        let f = clean_field(raw("", "", "Scrivi qui"), "Scrivi qui");
        assert_eq!((f.before.as_str(), f.after.as_str()), ("", ""));
    }

    #[test]
    fn testo_vero_resta_anche_con_segnaposto() {
        let f = clean_field(raw("Testo prova, ", "", "fine prova\u{FFFC}"), "Scrivi qui");
        assert_eq!(f.before, "Testo prova, ");
        assert_eq!(f.after, "fine prova");
    }
}
