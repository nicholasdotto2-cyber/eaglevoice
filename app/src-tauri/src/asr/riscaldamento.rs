//! Riscaldamento della GPU dopo una pausa lunga: dopo ore di inattività la
//! prima richiesta a whisper-server costa qualche secondo invece di una
//! frazione di secondo.
//!
//! Il riscaldamento manda 1 s di rumore con il VAD SPENTO: con il VAD acceso
//! il Silero scarta il rumore (0 segmenti) e l'encoder sulla GPU non gira,
//! quindi non scalda nulla.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 1 s di rumore a 16 kHz, uguale a quello dell'avvio.
pub(crate) fn noise() -> Vec<f32> {
    (0..16_000).map(|i| ((i * 7919) % 97) as f32 / 9700.0 - 0.005).collect()
}

/// Lascia partire un solo riscaldamento alla volta.
#[derive(Default)]
pub(crate) struct RewarmGate {
    running: Arc<AtomicBool>,
}

impl RewarmGate {
    /// Lancia `job` in un thread se nessun riscaldamento è in corso; `false`
    /// se ne gira già uno (o se il thread non parte). Non blocca.
    pub(crate) fn try_spawn<F: FnOnce() + Send + 'static>(&self, job: F) -> bool {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let running = Arc::clone(&self.running);
        let spawned = std::thread::Builder::new()
            .name("asr-riscaldamento".into())
            .spawn(move || {
                // Il flag torna libero anche se `job` va in panic.
                struct Release(Arc<AtomicBool>);
                impl Drop for Release {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::Release);
                    }
                }
                let _release = Release(running);
                job();
            });
        if spawned.is_err() {
            self.running.store(false, Ordering::Release);
            return false;
        }
        true
    }

    #[cfg(test)]
    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn un_solo_riscaldamento_alla_volta() {
        let gate = RewarmGate::default();
        let (tx, rx) = mpsc::channel::<()>();
        assert!(gate.try_spawn(move || {
            let _ = rx.recv();
        }));
        // Il primo è fermo sul canale: il secondo e il terzo vengono saltati.
        assert!(!gate.try_spawn(|| panic!("non doveva partire")));
        assert!(!gate.try_spawn(|| panic!("non doveva partire")));
        tx.send(()).unwrap();
        let t0 = Instant::now();
        while gate.is_running() {
            assert!(t0.elapsed() < Duration::from_secs(5), "flag mai rilasciato");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Finito il primo, uno nuovo riparte.
        let (tx2, rx2) = mpsc::channel::<()>();
        assert!(gate.try_spawn(move || tx2.send(()).unwrap()));
        rx2.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
