use thiserror::Error;

/// Failure of an analytics computation that cannot produce a meaningful number.
///
/// Functions that return plain `f64`/`Vec<f64>` signal "undefined" with `NaN`
/// instead (for example the Sharpe ratio of a constant series); this error is
/// for calls whose inputs are structurally unusable.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AnalyticsError {
    /// Not enough observations (after dropping non-finite values) for the method.
    #[error("insufficient data: needed at least {needed} observations, got {got}")]
    InsufficientData { needed: usize, got: usize },
    /// A matrix that must be invertible / positive (semi-)definite is not.
    #[error("matrix is singular or not positive semi-definite")]
    Singular,
    /// Input slices that must have equal lengths do not.
    #[error("length mismatch: expected {expected}, got {got}")]
    LengthMismatch { expected: usize, got: usize },
    /// Any other invalid argument (out-of-range parameter, non-finite value, ...).
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AnalyticsError>;
