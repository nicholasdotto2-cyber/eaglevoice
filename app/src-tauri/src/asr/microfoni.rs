//! Classifica dei microfoni: quando non c'è un microfono forzato si usa
//! il primo della classifica presente in quel momento, altrimenti il
//! predefinito di sistema. La scelta sta qui, separata dall'elenco reale dei
//! dispositivi, che legge `managers::audio`.

/// Primo nome di `ranked` presente fra `available` (confronto esatto, come
/// per il microfono forzato di Handy). `None`: predefinito di sistema.
pub fn first_ranked_present(ranked: &[String], available: &[String]) -> Option<String> {
    ranked
        .iter()
        .find(|name| available.iter().any(|a| a == *name))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn asr_primo_microfono_della_classifica_presente() {
        let ranked = v(&["Cuffie USB", "Webcam", "Microfono interno"]);
        // Il primo manca: vince il secondo, non l'ordine dell'elenco reale.
        let available = v(&["Microfono interno", "Webcam"]);
        assert_eq!(first_ranked_present(&ranked, &available).as_deref(), Some("Webcam"));
        // Nessuno presente, o classifica vuota: predefinito di sistema.
        assert_eq!(first_ranked_present(&ranked, &v(&["Altro"])), None);
        assert_eq!(first_ranked_present(&[], &available), None);
    }
}
