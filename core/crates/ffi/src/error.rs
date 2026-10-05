use meridian_engine::EngineError;

/// Every FFI error. Maps to a Swift `Error` enum.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{reason}")]
    NotAvailable { reason: String },
    #[error("{message}")]
    InvalidInput { message: String },
    #[error("{what} not found")]
    NotFound { what: String },
    #[error("{message}")]
    Provider { message: String },
    #[error("{message}")]
    Storage { message: String },
    #[error("cancelled")]
    Cancelled,
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl From<EngineError> for CoreError {
    fn from(e: EngineError) -> Self {
        let message = e.user_message();
        match e {
            _ if e.is_unavailable() => CoreError::NotAvailable { reason: message },
            EngineError::InvalidInput(m) => CoreError::InvalidInput { message: m },
            EngineError::NotFound(w) => CoreError::NotFound { what: w },
            EngineError::Provider(_) => CoreError::Provider { message },
            EngineError::Store(_) => CoreError::Storage { message },
            EngineError::Cancelled => CoreError::Cancelled,
            EngineError::NotAvailable { .. } | EngineError::Internal(_) => CoreError::Internal { message },
        }
    }
}

impl From<meridian_store::StoreError> for CoreError {
    fn from(e: meridian_store::StoreError) -> Self {
        CoreError::Storage { message: e.to_string() }
    }
}

impl From<uniffi::UnexpectedUniFFICallbackError> for CoreError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        CoreError::Internal { message: format!("callback failed: {}", e.reason) }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
