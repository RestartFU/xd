use serde_json::{Map, Value};
use thiserror::Error;

pub const UNAUTHENTICATED_FRAME_LIMIT: usize = 64 * 1024;
pub const AUTHENTICATED_FRAME_LIMIT: usize = 96 * 1024 * 1024;
const REQUEST_ID: &str = "_xd_request";

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Event {
        name: String,
        body: Map<String, Value>,
    },
    Reply {
        request_id: Option<u64>,
        body: Map<String, Value>,
    },
}

#[derive(Debug, Error, PartialEq)]
pub enum ProtocolError {
    #[error("request must be a JSON object")]
    RequestNotObject,
    #[error("request must contain a non-empty string op")]
    MissingOperation,
    #[error("frame exceeds the configured limit")]
    FrameTooLarge,
    #[error("frame is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("frame is neither an event nor a reply")]
    UnknownFrame,
}

#[derive(Debug)]
pub struct ProtocolCodec {
    next_request_id: u64,
}

impl Default for ProtocolCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolCodec {
    pub fn new() -> Self {
        Self { next_request_id: 1 }
    }

    pub fn encode_request(&mut self, request: Value) -> Result<(u64, Vec<u8>), ProtocolError> {
        let mut body = request
            .as_object()
            .cloned()
            .ok_or(ProtocolError::RequestNotObject)?;
        let valid_operation = body
            .get("op")
            .and_then(Value::as_str)
            .is_some_and(|operation| !operation.is_empty());
        if !valid_operation {
            return Err(ProtocolError::MissingOperation);
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        body.insert(REQUEST_ID.into(), Value::from(request_id));

        let mut encoded = serde_json::to_vec(&body)
            .map_err(|error| ProtocolError::InvalidJson(error.to_string()))?;
        encoded.push(b'\n');
        Ok((request_id, encoded))
    }

    pub fn decode_line(line: &[u8], limit: usize) -> Result<Option<Frame>, ProtocolError> {
        if line.len() > limit {
            return Err(ProtocolError::FrameTooLarge);
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }

        let value: Value = serde_json::from_slice(line)
            .map_err(|error| ProtocolError::InvalidJson(error.to_string()))?;
        let body = value
            .as_object()
            .cloned()
            .ok_or(ProtocolError::UnknownFrame)?;

        if let Some(name) = body.get("event").and_then(Value::as_str) {
            return Ok(Some(Frame::Event {
                name: name.to_owned(),
                body,
            }));
        }
        if body.get("ok").and_then(Value::as_bool).is_some() {
            let request_id = body.get(REQUEST_ID).and_then(Value::as_u64);
            return Ok(Some(Frame::Reply { request_id, body }));
        }

        Err(ProtocolError::UnknownFrame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assigns_monotonic_request_ids_and_newline_frames() {
        let mut codec = ProtocolCodec::new();
        let (first, encoded) = codec.encode_request(json!({"op": "tree"})).unwrap();
        let (second, _) = codec
            .encode_request(json!({"op": "chat", "chat": "chat-1"}))
            .unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert!(encoded.ends_with(b"\n"));
        assert_eq!(
            serde_json::from_slice::<Value>(&encoded).unwrap()[REQUEST_ID],
            1
        );
    }

    #[test]
    fn separates_events_from_correlated_replies() {
        let event = ProtocolCodec::decode_line(
            br#"{"event":"queued","chat":"chat-1","queue":["next"]}"#,
            AUTHENTICATED_FRAME_LIMIT,
        )
        .unwrap()
        .unwrap();
        let reply = ProtocolCodec::decode_line(
            br#"{"ok":true,"_xd_request":42}"#,
            AUTHENTICATED_FRAME_LIMIT,
        )
        .unwrap()
        .unwrap();

        assert!(matches!(event, Frame::Event { name, .. } if name == "queued"));
        assert!(matches!(
            reply,
            Frame::Reply {
                request_id: Some(42),
                ..
            }
        ));
    }

    #[test]
    fn enforces_the_transport_frame_budget() {
        let oversized = vec![b'x'; UNAUTHENTICATED_FRAME_LIMIT + 1];
        assert_eq!(
            ProtocolCodec::decode_line(&oversized, UNAUTHENTICATED_FRAME_LIMIT),
            Err(ProtocolError::FrameTooLarge)
        );
    }
}
