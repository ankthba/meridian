//! Live-network checks of the news path through the engine (ignored by
//! default): `cargo test -p meridian-engine --test live_news -- --ignored`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use meridian_engine::screen::ScreenStatus;
use meridian_engine::screens::ScreenRequest;
use meridian_engine::{DataMode, Engine, EngineConfig, NullEvents};
use meridian_provider::Provider;
use meridian_provider_rss::{RssConfig, RssProvider};

fn live_engine() -> Arc<Engine> {
    let rss: Arc<dyn Provider> = Arc::new(RssProvider::new(RssConfig::default()).expect("rss"));
    let mut config = EngineConfig::test(DataMode::Live, 0);
    config.fixed_clock = None;
    Engine::new(&config, vec![rss], Arc::new(NullEvents)).expect("engine")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live network: fetches press-release feeds"]
async fn press_release_screen_loads_promptly() {
    let engine = live_engine();
    engine.start();
    let t = Instant::now();
    let req = ScreenRequest { function: "N".into(), security: None, args: vec![("scope".into(), "press".into())] };
    let screen = tokio::time::timeout(Duration::from_secs(20), engine.screen(req)).await.expect("screen timed out");
    let took = t.elapsed();
    println!("N press: {took:?}, status {:?}", screen.status);
    assert!(matches!(screen.status, ScreenStatus::Ok), "{:?}", screen.status);
    assert!(took < Duration::from_secs(5), "took {took:?}");
    engine.shutdown();
}
