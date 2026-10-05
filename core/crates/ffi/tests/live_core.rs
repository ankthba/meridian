//! Live-network smoke test of the app's composition root with no
//! credentials (ignored by default):
//! `cargo test -p meridian-ffi --test live_core -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use meridian_ffi::{CoreConfigFfi, CoreEventFfi, CoreEvents, Core, DataModeFfi, KeyValue, ScreenStatusFfi, SecretSource};

#[derive(Debug)]
struct NoSecrets;
impl SecretSource for NoSecrets {
    fn secret(&self, _provider: String, _field: String) -> Option<String> {
        None
    }
}

#[derive(Debug)]
struct Quiet;
impl CoreEvents for Quiet {
    fn on_event(&self, _event: CoreEventFfi) {}
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live network: keyless sources only"]
async fn keyless_screens_load_promptly() {
    let config = CoreConfigFfi {
        mode: DataModeFfi::Live,
        data_dir: std::env::temp_dir().join("meridian-live-test").to_string_lossy().into_owned(),
        in_memory: true,
        fixed_clock_ns: None,
        mock_seed: 0,
        mock_extra_symbols: 0,
        mock_update_rate: 0.0,
    };
    let core = Core::new(config, Arc::new(NoSecrets), Arc::new(Quiet)).expect("core");
    core.start().expect("start");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    for (f, args) in [("N", vec![("scope", "press")]), ("TOP", vec![]), ("CRYP", vec![]), ("FXC", vec![])] {
        let t = Instant::now();
        let args = args.into_iter().map(|(k, v)| KeyValue { key: k.into(), value: v.into() }).collect();
        let s = tokio::time::timeout(Duration::from_secs(30), core.screen(f.into(), None, args)).await;
        let took = t.elapsed();
        match s {
            Ok(Ok(s)) => println!("{f}: {took:?} {:?}", match &s.status { ScreenStatusFfi::Ok => "ok".to_string(), other => format!("{other:?}") }),
            Ok(Err(e)) => println!("{f}: {took:?} error {e}"),
            Err(_) => println!("{f}: TIMED OUT after {took:?}"),
        }
    }
    core.shutdown().expect("shutdown");
}
