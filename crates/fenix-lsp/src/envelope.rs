use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const JSONRPC_VERSION: &str = "2.0";

fn jsonrpc_version() -> String {
    JSONRPC_VERSION.to_string()
}

/// One JSON-RPC 2.0 message, read off (or about to be written to) the
/// wire -- request, response, or notification, distinguished the same
/// way the spec itself does rather than as separate types: a request or
/// notification carries `method`; a response doesn't. A request (but
/// not a notification) also carries `id`; a response always does.
/// `result`/`error` are mutually exclusive on a response, per spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RawMessage {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl RawMessage {
    pub(crate) fn request(id: i64, method: &str, params: Value) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), id: Some(Value::from(id)), method: Some(method.to_string()), params: Some(params), result: None, error: None }
    }

    pub(crate) fn notification(method: &str, params: Value) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), id: None, method: Some(method.to_string()), params: Some(params), result: None, error: None }
    }

    pub(crate) fn response(id: Value, result: Result<Value, ResponseError>) -> Self {
        let (result, error) = match result {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        Self { jsonrpc: JSONRPC_VERSION.to_string(), id: Some(id), method: None, params: None, result, error }
    }
}

/// A JSON-RPC 2.0 error object (the `error` field of a failed response).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_omits_result_and_error() {
        let msg = RawMessage::request(1, "textDocument/hover", Value::Null);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["id"], Value::from(1));
        assert_eq!(json["method"], Value::from("textDocument/hover"));
        assert!(json.get("result").is_none());
        assert!(json.get("error").is_none());
    }

    #[test]
    fn notification_omits_id() {
        let msg = RawMessage::notification("textDocument/didOpen", Value::Null);
        let json = serde_json::to_value(&msg).unwrap();
        assert!(json.get("id").is_none());
    }

    #[test]
    fn a_successful_response_carries_result_not_error() {
        let msg = RawMessage::response(Value::from(2), Ok(Value::from("ok")));
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["result"], Value::from("ok"));
        assert!(json.get("error").is_none());
        assert!(json.get("method").is_none());
    }

    #[test]
    fn a_failed_response_carries_error_not_result() {
        let msg = RawMessage::response(Value::from(2), Err(ResponseError { code: -32600, message: "bad".to_string(), data: None }));
        let json = serde_json::to_value(&msg).unwrap();
        assert!(json.get("result").is_none());
        assert_eq!(json["error"]["code"], Value::from(-32600));
    }

    #[test]
    fn deserializes_a_server_response_missing_the_jsonrpc_field() {
        // Real servers always send it, but this client shouldn't hard
        // fail on a message that happens to omit it.
        let raw = br#"{"id":1,"result":{}}"#;
        let msg: RawMessage = serde_json::from_slice(raw).unwrap();
        assert_eq!(msg.jsonrpc, JSONRPC_VERSION);
    }
}
