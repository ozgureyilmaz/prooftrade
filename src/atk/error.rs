use thiserror::Error;

use super::RequiredCapability;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum McpError {
    #[error("MCP transport I/O failed: {0}")]
    Io(String),
    #[error("MCP child process exited")]
    ProcessExited,
    #[error("MCP response timed out for {method}")]
    RequestTimeout { method: String },
    #[error("MCP returned malformed JSON: {0}")]
    MalformedJson(String),
    #[error("MCP response has an invalid envelope: {0}")]
    InvalidEnvelope(String),
    #[error("MCP JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String },
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AtkError {
    #[error("invalid ATK configuration: {0}")]
    InvalidConfig(String),
    #[error("ATK process could not be launched: {0}")]
    LaunchFailed(String),
    #[error("ATK integration is unavailable: {0}")]
    Unavailable(String),
    #[error("ATK integration is not ready")]
    NotReady,
    #[error("MCP transport error: {0}")]
    Transport(#[from] McpError),
    #[error("MCP protocol response is invalid: {0}")]
    ProtocolResponseInvalid(String),
    #[error("MCP JSON-RPC error {code}: {message}")]
    ProtocolError { code: i64, message: String },
    #[error("ATK tool response is invalid: {0}")]
    ResponseInvalid(String),
    #[error("ATK tool {tool} rejected the request ({code}): {message}")]
    ToolRejected {
        tool: String,
        code: String,
        message: String,
    },
    #[error("required ATK capability is unavailable: {0:?}")]
    CapabilityUnavailable(RequiredCapability),
    #[error("ATK tool schema is invalid for {tool}: {reason}")]
    ToolSchemaInvalid { tool: String, reason: String },
    #[error(
        "ATK demo/live mode mismatch: expected demo={expected_demo}, actual demo={actual_demo}"
    )]
    ModeMismatch {
        expected_demo: bool,
        actual_demo: bool,
    },
    #[error("ATK server version mismatch: expected {expected}, actual {actual}")]
    VersionMismatch { expected: String, actual: String },
    #[error(
        "ATK read-only mode mismatch: expected read_only={expected}, actual read_only={actual}"
    )]
    ReadOnlyMismatch { expected: bool, actual: bool },
    #[error("ATK site mismatch: expected {expected}, actual {actual}")]
    SiteMismatch { expected: String, actual: String },
    #[error("invalid spot order request: {0}")]
    InvalidOrderRequest(String),
    #[error("numeric field {field} is invalid: {value}")]
    InvalidNumber { field: String, value: String },
}
