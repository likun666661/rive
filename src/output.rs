use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Envelope<P: Serialize, D: Serialize> {
    pub protocol: P,
    pub display: D,
}

impl<P: Serialize, D: Serialize> Envelope<P, D> {
    pub fn new(protocol: P, display: D) -> Self {
        Self { protocol, display }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub protocol: ErrorProtocol,
    pub display: ErrorDisplay,
}

#[derive(Debug, Serialize)]
pub struct ErrorProtocol {
    pub ok: bool,
    pub code: &'static str,
    pub retryable: bool,
    pub expected_next_action: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ErrorDisplay {
    pub message: String,
}

impl ErrorEnvelope {
    pub fn new(
        code: &'static str,
        retryable: bool,
        expected_next_action: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            protocol: ErrorProtocol {
                ok: false,
                code,
                retryable,
                expected_next_action,
            },
            display: ErrorDisplay {
                message: message.into(),
            },
        }
    }
}
