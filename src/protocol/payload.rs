use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadRequest {
    pub sql: String,
    pub bearer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum ReadResponse {
    Public { hash: String, version: u64 },
    Private { rows: Value, version: u64 },
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CursorRequest {
    pub hash: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CursorResponse {
    pub etag: String,
    pub rows: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LiveRequest {
    pub sql: String,
    pub bearer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LiveEvent {
    Snapshot {
        rows: Option<Value>,
        hash: Option<String>,
        version: u64,
    },
    Insert {
        key: String,
        row: Value,
        txid: u32,
    },
    Update {
        key: String,
        row: Value,
        txid: u32,
    },
    Delete {
        key: String,
        row: Value,
        txid: u32,
    },
    UpToDate {
        txid: u32,
    },
    Reset,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WireError {
    pub name: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::{LiveEvent, ReadResponse};
    use serde_json::json;

    #[test]
    fn read_response_has_explicit_scope() {
        let value = serde_json::to_value(ReadResponse::Public {
            hash: "abc".to_string(),
            version: 4,
        })
        .unwrap();
        assert_eq!(
            value,
            json!({"scope": "public", "hash": "abc", "version": 4})
        );
    }

    #[test]
    fn live_events_are_typed() {
        let value = serde_json::to_value(LiveEvent::UpToDate { txid: 9 }).unwrap();
        assert_eq!(value, json!({"type": "up-to-date", "txid": 9}));
    }
}
