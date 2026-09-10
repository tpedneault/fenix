//! Opt-in latency tracing: launch with FENIX_PROFILE=1 and capture stderr.
use std::time::Instant;

pub struct Scope(Option<(&'static str, Instant)>);

impl Scope {
    pub fn mark(&self, stage: &'static str) {
        if let Some((name, start)) = self.0 {
            eprintln!("fenix-profile {name}/{stage}: {:.3} ms cumulative", start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    pub fn new(name: &'static str) -> Self {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        Self(if *ENABLED.get_or_init(|| std::env::var_os("FENIX_PROFILE").is_some()) {
            Some((name, Instant::now()))
        } else {
            None
        })
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some((name, start)) = self.0 {
            eprintln!("fenix-profile {name}: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
