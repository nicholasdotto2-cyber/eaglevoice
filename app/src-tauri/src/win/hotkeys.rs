//! Scorciatoie della dettatura.
//!
//! Il tasto lo legge già `shortcut\handy_keys.rs` (crate handy-keys: hook
//! `WH_KEYBOARD_LL` su thread dedicato, combinazioni di soli modificatori come
//! Ctrl+Win, blocco della combinazione e mascheramento di Win con il tasto
//! neutro 0xE8, reinstallazione dopo sblocco e ripresa; più il watchdog in
//! `shortcut\hook_watchdog.rs`). Questo file trasforma gli eventi press e
//! release dei binding in intenzioni della dettatura:
//!
//! - `transcribe` (Ctrl+Win, push-to-talk): premi avvia (subito, nessuna
//!   attesa), rilasci dopo `double_tap_ms` ferma.
//! - doppio tocco: un rilascio prima di `double_tap_ms` lascia la
//!   registrazione aperta e arma un timer di `double_tap_ms` dalla prima
//!   pressione. Una seconda pressione entro il timer blocca le mani libere
//!   (`LockHandsFree`; il rilascio del secondo tocco si ignora); timer scaduto
//!   senza seconda pressione: `DiscardTap`, annullo silenzioso senza incolla.
//! - `hands_free` (Ctrl+Win+Spazio): una pressione avvia, la seconda ferma.
//!   Se arriva mentre il PTT è tenuto, blocca la dettatura in hands-free e il
//!   rilascio del PTT non la ferma più.
//! - un PTT durante l'hands-free lo chiude.
//! - `cancel` (Esc) annulla la dettatura in corso, con qualunque modificatore
//!   tenuto (varianti registrate in `shortcut\handy_keys.rs`).
//! - `paste_last` (Shift+Alt+Z) e i Transforms a riposo: slot 1 sul binding
//!   `polish` (Alt+Win+1), slot 2-9 su `transform_2`..`transform_9`.
//! - debounce: una nuova dettatura non parte prima di `debounce_ms` dalla fine
//!   della precedente. Il tocco singolo scartato non conta come fine.
//!
//! Collegamento: l'avvio dell'app chiama `install_full(sink)` (intenti completi,
//! `DictationIntent`) oppure `install(sink)` (intenti di prima, `HotkeyIntent`:
//! `DiscardTap` arriva come `Cancel`, lo slot 1 come `Polish`, gli slot 2-9 si
//! perdono). Senza nessuno dei due il comportamento di Handy resta invariato.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::AppHandle;

pub const BINDING_PTT: &str = "transcribe";
pub const BINDING_HANDS_FREE: &str = "hands_free";
pub const BINDING_CANCEL: &str = "cancel";
pub const BINDING_PASTE_LAST: &str = "paste_last";
/// Slot 1 dei Transforms (Polish): binding già esistente, riusato.
pub const BINDING_POLISH: &str = "polish";
/// Slot 2-9 dei Transforms: `transform_2` .. `transform_9`.
pub const BINDING_TRANSFORM_PREFIX: &str = "transform_";
pub const TRANSFORM_SLOTS: u8 = 9;

/// Scorciatoie predefinite, in sintassi handy-keys.
pub const DEFAULT_BINDINGS: &[(&str, &str, &str)] = &[
    (BINDING_PTT, "ctrl+super", "Push-to-talk"),
    (BINDING_HANDS_FREE, "ctrl+super+space", "Hands-free"),
    (BINDING_CANCEL, "escape", "Annulla"),
    (BINDING_PASTE_LAST, "shift+alt+z", "Incolla l'ultima dettatura"),
    (BINDING_POLISH, "alt+super+1", "Polish"),
];

/// Scorciatoie dei Transforms: (binding, combinazione predefinita, nome).
/// Gli slot 4-9 non hanno scorciatoia predefinita ma sono assegnabili.
pub const DEFAULT_TRANSFORM_BINDINGS: &[(&str, Option<&str>, &str)] = &[
    (BINDING_POLISH, Some("alt+super+1"), "Polish"),
    ("transform_2", Some("alt+super+2"), "Prompt Builder"),
    ("transform_3", Some("alt+super+3"), "Riscrittura personalizzata 1"),
    ("transform_4", None, "Riscrittura personalizzata 2"),
    ("transform_5", None, "Riscrittura personalizzata 3"),
    ("transform_6", None, "Riscrittura personalizzata 4"),
    ("transform_7", None, "Riscrittura personalizzata 5"),
    ("transform_8", None, "Riscrittura personalizzata 6"),
    ("transform_9", None, "Riscrittura personalizzata 7"),
];

/// Slot del Transform legato a un binding (1 per `polish`, 2-9 per `transform_N`).
pub fn transform_slot(binding_id: &str) -> Option<u8> {
    if binding_id == BINDING_POLISH {
        return Some(1);
    }
    let n: u8 = binding_id.strip_prefix(BINDING_TRANSFORM_PREFIX)?.parse().ok()?;
    (2..=TRANSFORM_SLOTS).contains(&n).then_some(n)
}

/// Binding dello slot `slot` (1-9).
pub fn transform_binding(slot: u8) -> Option<String> {
    match slot {
        1 => Some(BINDING_POLISH.to_string()),
        2..=TRANSFORM_SLOTS => Some(format!("{BINDING_TRANSFORM_PREFIX}{slot}")),
        _ => None,
    }
}

/// Intenti di prima (li consuma `flow::handle_intent`). Restano invariati
/// finché l'app non passa a `DictationIntent` con `install_full`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HotkeyIntent {
    /// Avvia una dettatura (cattura del contesto e microfono).
    Start { hands_free: bool },
    /// La dettatura in corso passa da push-to-talk a hands-free.
    LockHandsFree,
    /// Ferma la registrazione e procedi con trascrizione e incolla.
    Stop,
    /// Scarta la dettatura in corso.
    Cancel,
    PasteLast,
    Polish,
}

/// Intenti completi della macchina a stati.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DictationIntent {
    /// Avvia una dettatura (cattura del contesto e microfono). Parte alla
    /// pressione, anche quando poi si rivela un tocco breve.
    Start { hands_free: bool },
    /// La dettatura in corso passa in mani libere: Ctrl+Win+Spazio durante il
    /// PTT, oppure seconda pressione del doppio tocco.
    LockHandsFree,
    /// Ferma la registrazione e procedi con trascrizione e incolla.
    Stop,
    /// Scarta la dettatura in corso (Esc).
    Cancel,
    /// Tocco singolo senza seconda pressione entro `double_tap_ms` dalla
    /// prima: scarta la registrazione senza incollare, senza suono d'errore.
    DiscardTap,
    PasteLast,
    /// Transform dello slot 1-9 (1 Polish, 2 Prompt Builder, 3-9 personalizzati).
    Transform { slot: u8 },
}

impl DictationIntent {
    /// Traduzione verso gli intenti di prima (sink installato con `install`).
    pub fn to_legacy(self) -> Option<HotkeyIntent> {
        Some(match self {
            DictationIntent::Start { hands_free } => HotkeyIntent::Start { hands_free },
            DictationIntent::LockHandsFree => HotkeyIntent::LockHandsFree,
            DictationIntent::Stop => HotkeyIntent::Stop,
            DictationIntent::Cancel | DictationIntent::DiscardTap => HotkeyIntent::Cancel,
            DictationIntent::PasteLast => HotkeyIntent::PasteLast,
            DictationIntent::Transform { slot: 1 } => HotkeyIntent::Polish,
            DictationIntent::Transform { .. } => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    /// PTT tenuto (registra); `since` è la pressione.
    Held { since: Instant },
    /// Primo tocco breve rilasciato: registrazione aperta, si aspetta la
    /// seconda pressione fino a `deadline`.
    AwaitSecond { deadline: Instant },
    HandsFree,
}

/// Macchina a stati pura (testabile senza tastiera).
#[derive(Debug)]
pub struct HotkeyMachine {
    mode: Mode,
    held: HashSet<String>,
    /// Dopo uno stop dato da una pressione, ignora le pressioni finché tutti i
    /// binding sono rilasciati (es. lo Spazio di Ctrl+Win+Spazio che ferma).
    swallow_until_release: bool,
    last_end: Option<Instant>,
    debounce: Duration,
    /// Soglia del tocco breve e finestra della seconda pressione (0 = niente doppio tocco).
    double_tap: Duration,
    /// Numero del timer valido: ogni uscita da `AwaitSecond` lo invalida.
    timer_gen: u64,
    /// Timer da armare (scadenza, numero), letto da `take_timer_request`.
    pending_timer: Option<(Instant, u64)>,
}

impl HotkeyMachine {
    pub fn new(debounce: Duration) -> Self {
        Self::with_double_tap(debounce, Duration::from_millis(DEFAULT_DOUBLE_TAP_MS))
    }

    pub fn with_double_tap(debounce: Duration, double_tap: Duration) -> Self {
        Self {
            mode: Mode::Idle,
            held: HashSet::new(),
            swallow_until_release: false,
            last_end: None,
            debounce,
            double_tap,
            timer_gen: 0,
            pending_timer: None,
        }
    }

    pub fn set_debounce(&mut self, debounce: Duration) {
        self.debounce = debounce;
    }

    pub fn set_double_tap(&mut self, double_tap: Duration) {
        self.double_tap = double_tap;
    }

    pub fn is_handled(binding_id: &str) -> bool {
        matches!(
            binding_id,
            BINDING_PTT | BINDING_HANDS_FREE | BINDING_CANCEL | BINDING_PASTE_LAST
        ) || transform_slot(binding_id).is_some()
    }

    fn debounced(&self, now: Instant) -> bool {
        self.last_end
            .map(|t| now.saturating_duration_since(t) < self.debounce)
            .unwrap_or(false)
    }

    fn set_mode(&mut self, mode: Mode) {
        if matches!(self.mode, Mode::AwaitSecond { .. }) {
            self.timer_gen += 1; // il timer armato non vale più
            self.pending_timer = None;
        }
        self.mode = mode;
    }

    fn end(&mut self, now: Instant) {
        self.set_mode(Mode::Idle);
        self.last_end = Some(now);
    }

    /// La dettatura è finita per altre vie (errore, stop dalla barra, limite di durata).
    pub fn dictation_ended(&mut self, now: Instant) {
        if self.mode != Mode::Idle {
            self.end(now);
        }
    }

    /// Dimentica un binding tolto dalla registrazione: il suo rilascio non
    /// arriverà più, quindi non deve restare fra i tasti premuti.
    pub fn forget_binding(&mut self, binding_id: &str) {
        self.held.remove(binding_id);
        if self.held.is_empty() {
            self.swallow_until_release = false;
        }
    }

    /// Timer del doppio tocco da armare dopo l'ultimo evento: (scadenza, numero).
    pub fn take_timer_request(&mut self) -> Option<(Instant, u64)> {
        self.pending_timer.take()
    }

    /// Scadenza del timer `gen`: se si aspetta ancora la seconda pressione, il
    /// tocco era singolo e la registrazione si scarta.
    pub fn on_timer(&mut self, gen: u64, now: Instant) -> Option<DictationIntent> {
        match self.mode {
            Mode::AwaitSecond { deadline } if gen == self.timer_gen && now >= deadline => {
                // Nessuna dettatura conclusa: il debounce non parte.
                self.set_mode(Mode::Idle);
                Some(DictationIntent::DiscardTap)
            }
            _ => None,
        }
    }

    pub fn on_event(&mut self, binding_id: &str, pressed: bool, now: Instant) -> Option<DictationIntent> {
        use DictationIntent::*;
        if !pressed {
            self.held.remove(binding_id);
            if self.held.is_empty() {
                self.swallow_until_release = false;
            }
            if binding_id == BINDING_PTT {
                if let Mode::Held { since } = self.mode {
                    let deadline = since + self.double_tap;
                    if self.double_tap.is_zero() || now >= deadline {
                        self.end(now);
                        return Some(Stop);
                    }
                    // Tocco breve: la registrazione resta aperta.
                    self.mode = Mode::AwaitSecond { deadline };
                    self.timer_gen += 1;
                    self.pending_timer = Some((deadline, self.timer_gen));
                }
            }
            return None;
        }
        // Annulla non entra in `held`: dopo l'annullamento `flow::cancel` toglie
        // la registrazione di Esc e il suo rilascio non arriva più. Se restasse
        // in `held`, ogni pressione successiva verrebbe scartata.
        if binding_id != BINDING_CANCEL && !self.held.insert(binding_id.to_string()) {
            return None; // ripetizione automatica: conta solo il fronte dopo un rilascio
        }
        if self.swallow_until_release {
            return None;
        }
        match (binding_id, self.mode) {
            (BINDING_PTT, Mode::Idle) => {
                if self.debounced(now) {
                    return None;
                }
                self.mode = Mode::Held { since: now };
                Some(Start { hands_free: false })
            }
            (BINDING_PTT | BINDING_HANDS_FREE, Mode::AwaitSecond { .. }) => {
                // Secondo tocco: mani libere. Il suo rilascio (e lo Spazio, se
                // c'è) si ignora fino al rilascio di tutti i binding.
                self.set_mode(Mode::HandsFree);
                self.swallow_until_release = true;
                Some(LockHandsFree)
            }
            (BINDING_PTT, Mode::HandsFree) => {
                self.end(now);
                self.swallow_until_release = true;
                Some(Stop)
            }
            (BINDING_HANDS_FREE, Mode::Idle) => {
                if self.debounced(now) {
                    return None;
                }
                self.mode = Mode::HandsFree;
                Some(Start { hands_free: true })
            }
            (BINDING_HANDS_FREE, Mode::Held { .. }) => {
                self.mode = Mode::HandsFree;
                Some(LockHandsFree)
            }
            (BINDING_HANDS_FREE, Mode::HandsFree) => {
                self.end(now);
                self.swallow_until_release = true;
                Some(Stop)
            }
            (BINDING_CANCEL, Mode::Held { .. } | Mode::AwaitSecond { .. } | Mode::HandsFree) => {
                self.end(now);
                // Ignora il resto solo finché restano premuti altri binding
                // (es. Ctrl+Win del push-to-talk): il loro rilascio arriva.
                self.swallow_until_release = !self.held.is_empty();
                Some(Cancel)
            }
            (BINDING_PASTE_LAST, Mode::Idle) => Some(PasteLast),
            (id, Mode::Idle) => transform_slot(id).map(|slot| Transform { slot }),
            _ => None,
        }
    }
}

pub const DEFAULT_DOUBLE_TAP_MS: u64 = 500;

type Sink = Box<dyn Fn(&AppHandle, HotkeyIntent) + Send + Sync>;
type FullSink = Box<dyn Fn(&AppHandle, DictationIntent) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
static FULL_SINK: OnceLock<FullSink> = OnceLock::new();
static DEBOUNCE_MS: AtomicU64 = AtomicU64::new(500);
static DOUBLE_TAP_MS: AtomicU64 = AtomicU64::new(DEFAULT_DOUBLE_TAP_MS);

/// Numero di dettature avviate: l'osservazione dopo l'incolla si chiude
/// (`next_dictation_started`) quando cambia.
pub static DICTATION_STARTS: AtomicU64 = AtomicU64::new(0);

/// Da chiamare se una dettatura parte senza scorciatoia (barra, vassoio):
/// chiude l'osservazione in corso. Le partenze da tastiera lo fanno da sole.
pub fn notify_dictation_started() {
    DICTATION_STARTS.fetch_add(1, Ordering::SeqCst);
}

fn machine() -> &'static Mutex<HotkeyMachine> {
    static M: OnceLock<Mutex<HotkeyMachine>> = OnceLock::new();
    M.get_or_init(|| {
        Mutex::new(HotkeyMachine::with_double_tap(
            Duration::from_millis(DEBOUNCE_MS.load(Ordering::Relaxed)),
            Duration::from_millis(DOUBLE_TAP_MS.load(Ordering::Relaxed)),
        ))
    })
}

pub fn set_debounce_ms(ms: u64) {
    DEBOUNCE_MS.store(ms, Ordering::Relaxed);
    if let Ok(mut m) = machine().lock() {
        m.set_debounce(Duration::from_millis(ms));
    }
}

pub fn set_double_tap_ms(ms: u64) {
    DOUBLE_TAP_MS.store(ms, Ordering::Relaxed);
    if let Ok(mut m) = machine().lock() {
        m.set_double_tap(Duration::from_millis(ms));
    }
}

/// Collega le scorciatoie alla catena della dettatura con gli intenti di
/// prima (`HotkeyIntent`). Il sink gira sul thread delle scorciatoie (o del
/// timer del doppio tocco): deve ritornare subito (spawn del lavoro).
pub fn install(sink: impl Fn(&AppHandle, HotkeyIntent) + Send + Sync + 'static) {
    if SINK.set(Box::new(sink)).is_err() {
        log::warn!("[win] hotkeys::install chiamata più volte: tengo il primo sink");
        return;
    }
    crate::shortcut::set_shortcut_interceptor(intercept);
}

/// Come `install`, con gli intenti completi (`DiscardTap`, `Transform`).
/// Se installato, ha la precedenza sul sink di `install`.
pub fn install_full(sink: impl Fn(&AppHandle, DictationIntent) + Send + Sync + 'static) {
    if FULL_SINK.set(Box::new(sink)).is_err() {
        log::warn!("[win] hotkeys::install_full chiamata più volte: tengo il primo sink");
        return;
    }
    crate::shortcut::set_shortcut_interceptor(intercept);
}

fn dispatch(app: &AppHandle, intent: DictationIntent) {
    if matches!(intent, DictationIntent::Start { .. }) {
        notify_dictation_started();
    }
    if let Some(full) = FULL_SINK.get() {
        full(app, intent);
    } else if let Some(sink) = SINK.get() {
        match intent.to_legacy() {
            Some(legacy) => sink(app, legacy),
            None => log::info!("[win] {intent:?} senza sink completo (install_full): ignorato"),
        }
    }
}

/// Da chiamare quando un binding viene tolto dalla registrazione (il suo
/// rilascio non arriverà): lo toglie dai tasti premuti della macchina.
pub fn forget_binding(binding_id: &str) {
    if let Ok(mut m) = machine().lock() {
        m.forget_binding(binding_id);
    }
}

/// Da chiamare quando la dettatura finisce per vie diverse dalle scorciatoie.
pub fn notify_dictation_ended() {
    if let Ok(mut m) = machine().lock() {
        m.dictation_ended(Instant::now());
    }
}

fn arm_timer(app: &AppHandle, deadline: Instant, gen: u64) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("win-double-tap".into())
        .spawn(move || {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            let intent = match machine().lock() {
                Ok(mut m) => m.on_timer(gen, Instant::now().max(deadline)),
                Err(_) => None,
            };
            if let Some(intent) = intent {
                log::debug!("[win] doppio tocco: nessuna seconda pressione -> {intent:?}");
                dispatch(&app, intent);
            }
        });
    if spawned.is_err() {
        log::warn!("[win] timer del doppio tocco non avviato");
    }
}

/// Intercettore registrato in `shortcut::handler`: `true` se l'evento è gestito qui.
fn intercept(app: &AppHandle, binding_id: &str, _hotkey: &str, pressed: bool) -> bool {
    if SINK.get().is_none() && FULL_SINK.get().is_none() {
        return false;
    }
    if !HotkeyMachine::is_handled(binding_id) {
        return false;
    }
    let (intent, timer) = match machine().lock() {
        Ok(mut m) => {
            let i = m.on_event(binding_id, pressed, Instant::now());
            (i, m.take_timer_request())
        }
        Err(_) => (None, None),
    };
    if let Some((deadline, gen)) = timer {
        arm_timer(app, deadline, gen);
    }
    if let Some(intent) = intent {
        log::debug!("[win] scorciatoia {binding_id} pressed={pressed} -> {intent:?}");
        dispatch(app, intent);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use DictationIntent::*;

    fn ms(base: Instant, n: u64) -> Instant {
        base + Duration::from_millis(n)
    }

    fn m500() -> HotkeyMachine {
        HotkeyMachine::new(Duration::from_millis(500))
    }

    #[test]
    fn push_to_talk_premi_e_rilascia() {
        let t = Instant::now();
        let mut m = m500();
        assert_eq!(m.on_event(BINDING_PTT, true, t), Some(Start { hands_free: false }));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 30)), None, "ripetizione");
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 2000)), Some(Stop));
        assert!(m.take_timer_request().is_none(), "tenuto oltre la soglia: niente timer");
    }

    #[test]
    fn hands_free_da_ptt_e_seconda_pressione_ferma() {
        let t = Instant::now();
        let mut m = m500();
        // Ctrl+Win (PTT) poi Spazio: blocco in hands-free; il rilascio non ferma.
        assert_eq!(m.on_event(BINDING_PTT, true, t), Some(Start { hands_free: false }));
        assert_eq!(m.on_event(BINDING_HANDS_FREE, true, ms(t, 80)), Some(LockHandsFree));
        assert_eq!(m.on_event(BINDING_HANDS_FREE, false, ms(t, 150)), None);
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 200)), None);
        assert!(m.take_timer_request().is_none(), "Ctrl+Win+Spazio breve non è un tocco");
        // Seconda Ctrl+Win+Spazio: il PTT chiude, lo Spazio che segue non riavvia.
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 5000)), Some(Stop));
        assert_eq!(m.on_event(BINDING_HANDS_FREE, true, ms(t, 5050)), None);
        assert_eq!(m.on_event(BINDING_HANDS_FREE, false, ms(t, 5100)), None);
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 5150)), None);
    }

    #[test]
    fn ptt_durante_hands_free_lo_chiude_e_annulla() {
        let t = Instant::now();
        let mut m = m500();
        assert_eq!(m.on_event(BINDING_HANDS_FREE, true, t), Some(Start { hands_free: true }));
        m.on_event(BINDING_HANDS_FREE, false, ms(t, 50));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 3000)), Some(Stop));
        m.on_event(BINDING_PTT, false, ms(t, 3100));
        // Esc annulla solo durante una dettatura.
        assert_eq!(m.on_event(BINDING_CANCEL, true, ms(t, 4000)), None);
        m.on_event(BINDING_CANCEL, false, ms(t, 4010));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 5000)), Some(Start { hands_free: false }));
        assert_eq!(m.on_event(BINDING_CANCEL, true, ms(t, 5500)), Some(Cancel));
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 5600)), None, "già annullata");
    }

    #[test]
    fn debounce_500_ms_fra_dettature() {
        let t = Instant::now();
        let mut m = m500();
        m.on_event(BINDING_PTT, true, t);
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 1000)), Some(Stop));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 1200)), None, "dentro il debounce");
        m.on_event(BINDING_PTT, false, ms(t, 1250));
        assert_eq!(
            m.on_event(BINDING_PTT, true, ms(t, 1600)),
            Some(Start { hands_free: false })
        );
    }

    #[test]
    fn incolla_ultima_e_transforms_a_riposo() {
        let t = Instant::now();
        let mut m = m500();
        assert_eq!(m.on_event(BINDING_PASTE_LAST, true, t), Some(PasteLast));
        m.on_event(BINDING_PASTE_LAST, false, ms(t, 50));
        assert_eq!(m.on_event(BINDING_POLISH, true, ms(t, 100)), Some(Transform { slot: 1 }));
        m.on_event(BINDING_POLISH, false, ms(t, 150));
        assert_eq!(m.on_event("transform_2", true, ms(t, 200)), Some(Transform { slot: 2 }));
        m.on_event("transform_2", false, ms(t, 250));
        assert_eq!(m.on_event("transform_9", true, ms(t, 300)), Some(Transform { slot: 9 }));
        m.on_event("transform_9", false, ms(t, 350));
        assert!(!HotkeyMachine::is_handled("transform_10"));
        assert!(!HotkeyMachine::is_handled("transform_1"), "lo slot 1 è il binding polish");
        assert!(!HotkeyMachine::is_handled("transcribe_with_post_process"));
        // Durante una dettatura i Transforms non partono.
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 1000)), Some(Start { hands_free: false }));
        assert_eq!(m.on_event("transform_3", true, ms(t, 1100)), None);
    }

    #[test]
    fn slot_dei_transforms_e_scorciatoie_predefinite() {
        assert_eq!(transform_slot("polish"), Some(1));
        assert_eq!(transform_slot("transform_2"), Some(2));
        assert_eq!(transform_slot("transform_x"), None);
        assert_eq!(transform_binding(3).as_deref(), Some("transform_3"));
        assert_eq!(transform_binding(0), None);
        let with_default: Vec<_> = DEFAULT_TRANSFORM_BINDINGS
            .iter()
            .filter_map(|(id, combo, _)| combo.map(|c| (*id, c)))
            .collect();
        assert_eq!(
            with_default,
            vec![("polish", "alt+super+1"), ("transform_2", "alt+super+2"), ("transform_3", "alt+super+3")]
        );
        assert_eq!(DEFAULT_TRANSFORM_BINDINGS.len(), 9);
        for (id, _, _) in DEFAULT_TRANSFORM_BINDINGS {
            assert!(HotkeyMachine::is_handled(id), "{id}");
        }
        // Traduzione verso gli intenti di prima.
        assert_eq!(Transform { slot: 1 }.to_legacy(), Some(HotkeyIntent::Polish));
        assert_eq!(Transform { slot: 2 }.to_legacy(), None);
        assert_eq!(DiscardTap.to_legacy(), Some(HotkeyIntent::Cancel));
    }

    #[test]
    fn annulla_in_push_to_talk_senza_rilascio_di_esc_non_blocca() {
        let t = Instant::now();
        let mut m = m500();
        assert_eq!(m.on_event(BINDING_PTT, true, t), Some(Start { hands_free: false }));
        assert_eq!(m.on_event(BINDING_CANCEL, true, ms(t, 1000)), Some(Cancel));
        // Esc viene tolto dalla registrazione: il suo rilascio non arriva.
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 1100)), None, "già annullata");
        assert_eq!(
            m.on_event(BINDING_PTT, true, ms(t, 2000)),
            Some(Start { hands_free: false }),
            "dopo l'annullamento una nuova pressione deve partire"
        );
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 3000)), Some(Stop));
    }

    #[test]
    fn annulla_in_hands_free_senza_rilascio_di_esc_non_blocca() {
        let t = Instant::now();
        let mut m = m500();
        assert_eq!(m.on_event(BINDING_HANDS_FREE, true, t), Some(Start { hands_free: true }));
        m.on_event(BINDING_HANDS_FREE, false, ms(t, 50));
        assert_eq!(m.on_event(BINDING_CANCEL, true, ms(t, 2000)), Some(Cancel));
        // nessun rilascio di Esc
        assert_eq!(
            m.on_event(BINDING_HANDS_FREE, true, ms(t, 3000)),
            Some(Start { hands_free: true })
        );
        m.on_event(BINDING_HANDS_FREE, false, ms(t, 3050));
        assert_eq!(m.on_event(BINDING_HANDS_FREE, true, ms(t, 4000)), Some(Stop));
        m.on_event(BINDING_HANDS_FREE, false, ms(t, 4050));
        assert_eq!(m.on_event(BINDING_PASTE_LAST, true, ms(t, 5000)), Some(PasteLast));
    }

    #[test]
    fn binding_tolto_dalla_registrazione_non_resta_premuto() {
        let t = Instant::now();
        let mut m = m500();
        assert_eq!(m.on_event(BINDING_PASTE_LAST, true, t), Some(PasteLast));
        // il binding viene registrato di nuovo su un altro tasto: il rilascio si perde
        m.forget_binding(BINDING_PASTE_LAST);
        assert_eq!(m.on_event(BINDING_PASTE_LAST, true, ms(t, 1000)), Some(PasteLast));
    }

    // ------------------------------------------------------ doppio tocco

    #[test]
    fn doppio_tocco_blocca_mani_libere_e_la_pressione_dopo_chiude() {
        let t = Instant::now();
        let mut m = m500();
        assert_eq!(m.on_event(BINDING_PTT, true, t), Some(Start { hands_free: false }));
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 120)), None, "tocco breve: niente stop");
        let (deadline, gen) = m.take_timer_request().expect("timer armato");
        assert_eq!(deadline, ms(t, 500), "timer dalla prima pressione");
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 300)), Some(LockHandsFree));
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 380)), None, "rilascio del secondo tocco ignorato");
        assert_eq!(m.on_timer(gen, ms(t, 500)), None, "timer superato dal blocco");
        // Bloccato: la pressione dopo chiude e incolla, il suo rilascio non riavvia.
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 6000)), Some(Stop));
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 6100)), None);
        assert!(m.take_timer_request().is_none());
    }

    #[test]
    fn doppio_tocco_bloccato_esc_annulla() {
        let t = Instant::now();
        let mut m = m500();
        m.on_event(BINDING_PTT, true, t);
        m.on_event(BINDING_PTT, false, ms(t, 100));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 250)), Some(LockHandsFree));
        m.on_event(BINDING_PTT, false, ms(t, 330));
        assert_eq!(m.on_event(BINDING_CANCEL, true, ms(t, 4000)), Some(Cancel));
        // Dopo l'annullamento si riparte normalmente (debounce dalla fine).
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 4600)), Some(Start { hands_free: false }));
    }

    #[test]
    fn tocco_singolo_scade_in_annullo_silenzioso() {
        let t = Instant::now();
        let mut m = m500();
        m.on_event(BINDING_PTT, true, t);
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 90)), None);
        let (deadline, gen) = m.take_timer_request().unwrap();
        assert_eq!(m.on_timer(gen + 1, deadline), None, "timer vecchio ignorato");
        assert_eq!(m.on_timer(gen, deadline), Some(DiscardTap));
        assert_eq!(m.on_timer(gen, ms(t, 900)), None, "una volta sola");
        // Esc durante l'attesa del secondo tocco: annulla.
        m.on_event(BINDING_PTT, true, ms(t, 2000));
        m.on_event(BINDING_PTT, false, ms(t, 2100));
        let (_, gen2) = m.take_timer_request().unwrap();
        assert_eq!(m.on_event(BINDING_CANCEL, true, ms(t, 2200)), Some(Cancel));
        assert_eq!(m.on_timer(gen2, ms(t, 2500)), None);
    }

    #[test]
    fn auto_ripetizione_non_conta_come_secondo_tocco() {
        let t = Instant::now();
        let mut m = m500();
        assert_eq!(m.on_event(BINDING_PTT, true, t), Some(Start { hands_free: false }));
        // Il tasto tenuto ripete la pressione senza rilascio vero.
        for i in 1..=8 {
            assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 30 * i)), None);
        }
        // Rilascio prima della soglia: attesa del secondo tocco, non blocco.
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 260)), None);
        let (deadline, gen) = m.take_timer_request().expect("attesa del secondo tocco");
        assert_eq!(m.on_timer(gen, deadline), Some(DiscardTap));
        // Tenuto a lungo con ripetizioni: stop al rilascio, come oggi.
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 1000)), Some(Start { hands_free: false }));
        for i in 1..=30 {
            assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 1000 + 33 * i)), None);
        }
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 2500)), Some(Stop));
    }

    #[test]
    fn debounce_non_mangia_il_secondo_tocco_ne_la_pressione_dopo_lo_scarto() {
        let t = Instant::now();
        // Debounce lungo: se il tocco breve fosse contato come fine dettatura,
        // la seconda pressione verrebbe scartata.
        let mut m = HotkeyMachine::with_double_tap(Duration::from_millis(2000), Duration::from_millis(500));
        m.on_event(BINDING_PTT, true, t);
        m.on_event(BINDING_PTT, false, ms(t, 100));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 200)), Some(LockHandsFree));
        m.on_event(BINDING_PTT, false, ms(t, 260));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 5000)), Some(Stop));
        m.on_event(BINDING_PTT, false, ms(t, 5050));
        // Tocco singolo scartato a 9500: non fa partire il debounce.
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 9000)), Some(Start { hands_free: false }));
        m.on_event(BINDING_PTT, false, ms(t, 9100));
        let (d, g) = m.take_timer_request().unwrap();
        assert_eq!(m.on_timer(g, d), Some(DiscardTap));
        assert_eq!(
            m.on_event(BINDING_PTT, true, ms(t, 9600)),
            Some(Start { hands_free: false }),
            "subito dopo lo scarto si riparte"
        );
    }

    #[test]
    fn terzo_tasto_durante_l_accordo() {
        let t = Instant::now();
        let mut m = m500();
        // Ctrl+Win+Spazio breve (rilascio entro la soglia, in entrambi gli
        // ordini): resta mani libere, nessun timer, nessuno scarto.
        for (k, ptt_first) in [(0u64, true), (10_000, false)] {
            assert_eq!(m.on_event(BINDING_PTT, true, ms(t, k)), Some(Start { hands_free: false }));
            assert_eq!(m.on_event(BINDING_HANDS_FREE, true, ms(t, k + 40)), Some(LockHandsFree));
            if ptt_first {
                assert_eq!(m.on_event(BINDING_PTT, false, ms(t, k + 120)), None);
                assert_eq!(m.on_event(BINDING_HANDS_FREE, false, ms(t, k + 130)), None);
            } else {
                assert_eq!(m.on_event(BINDING_HANDS_FREE, false, ms(t, k + 120)), None);
                assert_eq!(m.on_event(BINDING_PTT, false, ms(t, k + 130)), None);
            }
            assert!(m.take_timer_request().is_none());
            assert_eq!(m.on_event(BINDING_PTT, true, ms(t, k + 3000)), Some(Stop));
            m.on_event(BINDING_PTT, false, ms(t, k + 3100));
        }
        // Secondo tocco fatto come Ctrl+Win+Spazio: blocca una volta, lo Spazio si ignora.
        m.on_event(BINDING_PTT, true, ms(t, 20_000));
        m.on_event(BINDING_PTT, false, ms(t, 20_100));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 20_250)), Some(LockHandsFree));
        assert_eq!(m.on_event(BINDING_HANDS_FREE, true, ms(t, 20_280)), None);
        m.on_event(BINDING_HANDS_FREE, false, ms(t, 20_320));
        m.on_event(BINDING_PTT, false, ms(t, 20_340));
        // Un Transform premuto con l'accordo non parte e non tocca lo stato.
        assert_eq!(m.on_event(BINDING_POLISH, true, ms(t, 21_000)), None);
        m.on_event(BINDING_POLISH, false, ms(t, 21_050));
        assert_eq!(m.on_event(BINDING_PTT, true, ms(t, 25_000)), Some(Stop));
    }

    #[test]
    fn soglia_zero_disattiva_il_doppio_tocco() {
        let t = Instant::now();
        let mut m = HotkeyMachine::with_double_tap(Duration::from_millis(500), Duration::ZERO);
        m.on_event(BINDING_PTT, true, t);
        assert_eq!(m.on_event(BINDING_PTT, false, ms(t, 80)), Some(Stop));
    }

    #[test]
    fn fine_dettatura_da_fuori_durante_l_attesa_invalida_il_timer() {
        let t = Instant::now();
        let mut m = m500();
        m.on_event(BINDING_PTT, true, t);
        m.on_event(BINDING_PTT, false, ms(t, 100));
        let (d, g) = m.take_timer_request().unwrap();
        m.dictation_ended(ms(t, 200)); // es. errore del microfono
        assert_eq!(m.on_timer(g, d), None);
    }
}
