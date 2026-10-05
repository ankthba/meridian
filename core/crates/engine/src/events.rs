use meridian_alerts::FiredAlert;
use serde::{Deserialize, Serialize};

/// Low-frequency push events to the UI. Implementations must return quickly
/// and must not call back into the engine synchronously.
pub trait EngineEvents: Send + Sync {
    fn on_event(&self, event: EngineEvent);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EngineEvent {
    /// A streaming feed connected or disconnected.
    FeedStatus { provider: String, connected: bool, message: String },
    AlertFired(FiredAlert),
    /// Suggestion index (re)built; count of instruments.
    UniverseLoaded { instruments: u64 },
    /// Free-form log line for the status bar.
    Status { message: String },
    /// ASK asked to display a function in another panel.
    Show { function: String, security: Option<String>, args: Vec<(String, String)> },
}

/// Discards events (tests).
pub struct NullEvents;

impl EngineEvents for NullEvents {
    fn on_event(&self, _event: EngineEvent) {}
}
