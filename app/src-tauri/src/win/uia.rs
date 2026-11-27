//! Thread UI Automation con COM in MTA e tetto di tempo.
//!
//! Le chiamate UIA vanno nel processo dell'app bersaglio e possono bloccarsi
//! se l'app è appesa. Ogni chiamata gira su un thread di lavoro persistente
//! (COM e `CUIAutomation` creati una volta); il chiamante aspetta al massimo
//! il suo timeout. Se il thread non risponde in tempo viene abbandonato e la
//! chiamata successiva ne crea uno nuovo. Due thread separati (campo e URL)
//! così le due letture corrono in parallelo.

use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use windows::core::Interface;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_BOOL, VT_I4};
use windows::Win32::UI::Accessibility::*;

type Job = Box<dyn FnOnce(&IUIAutomation) + Send>;

pub(crate) struct Worker {
    name: &'static str,
    slot: Mutex<Option<Sender<Job>>>,
}

pub(crate) static FIELD_WORKER: Worker = Worker::new("win-uia-field");
pub(crate) static URL_WORKER: Worker = Worker::new("win-uia-url");

/// Crea l'oggetto UI Automation con timeout di connessione e di transazione
/// brevi, così un'app appesa non blocca il thread per decine di secondi.
pub(crate) fn create_automation() -> windows::core::Result<IUIAutomation> {
    unsafe {
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))?;
        if let Ok(a2) = automation.cast::<IUIAutomation2>() {
            let _ = a2.SetConnectionTimeout(1000);
            let _ = a2.SetTransactionTimeout(1000);
        }
        Ok(automation)
    }
}

impl Worker {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            slot: Mutex::new(None),
        }
    }

    fn spawn(&self) -> Option<Sender<Job>> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<bool>();
        let spawned = thread::Builder::new()
            .name(self.name.to_string())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let automation = match create_automation() {
                    Ok(a) => {
                        let _ = ready_tx.send(true);
                        a
                    }
                    Err(e) => {
                        log::warn!("[win] CUIAutomation non disponibile: {e}");
                        let _ = ready_tx.send(false);
                        return;
                    }
                };
                while let Ok(job) = rx.recv() {
                    job(&automation);
                }
            });
        if spawned.is_err() {
            return None;
        }
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(true) => Some(tx),
            _ => None,
        }
    }

    fn sender(&self) -> Option<Sender<Job>> {
        let mut slot = self.slot.lock().ok()?;
        if slot.is_none() {
            *slot = self.spawn();
        }
        slot.clone()
    }

    fn discard(&self) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = None;
        }
    }

    pub(crate) fn warm_up(&self) {
        let _ = self.sender();
    }

    /// Esegue `f` sul thread UIA; `None` se scade il tempo o il thread non c'è.
    pub(crate) fn run<T: Send + 'static>(
        &self,
        timeout: Duration,
        f: impl FnOnce(&IUIAutomation) -> T + Send + 'static,
    ) -> Option<T> {
        let sender = self.sender()?;
        let (rtx, rrx) = mpsc::channel();
        let job: Job = Box::new(move |a| {
            let _ = rtx.send(f(a));
        });
        if sender.send(job).is_err() {
            self.discard();
            return None;
        }
        match rrx.recv_timeout(timeout) {
            Ok(v) => Some(v),
            Err(_) => {
                // Il thread è occupato su un'app lenta: lo abbandoniamo.
                log::debug!("[win] {} oltre {:?}: thread sostituito", self.name, timeout);
                self.discard();
                None
            }
        }
    }
}

pub(crate) fn warm_up() {
    FIELD_WORKER.warm_up();
    URL_WORKER.warm_up();
}

/// VARIANT intero (per le condizioni su ControlType).
pub(crate) fn variant_i32(value: i32) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        (*v.Anonymous.Anonymous).vt = VT_I4;
        (*v.Anonymous.Anonymous).Anonymous.lVal = value;
    }
    v
}

/// Legge un VARIANT booleano e lo libera. `None` se non è un booleano
/// (attributo misto o non supportato).
pub(crate) fn take_variant_bool(mut v: VARIANT) -> Option<bool> {
    let out = unsafe {
        if v.Anonymous.Anonymous.vt == VT_BOOL {
            Some(v.Anonymous.Anonymous.Anonymous.boolVal.0 != 0)
        } else {
            None
        }
    };
    unsafe {
        let _ = VariantClear(&mut v);
    }
    out
}
