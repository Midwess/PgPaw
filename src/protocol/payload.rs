use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SqlRequest {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<Value>,
    pub bearer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SqlReply {
    pub command: String,
    pub rows: Value,
    pub rows_affected: u64,
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
    use super::{LiveEvent, SqlReply};
    use serde_json::json;

    #[test]
    fn sql_reply_is_camel_case_json() {
        let value = serde_json::to_value(SqlReply {
            command: "SELECT".to_string(),
            rows: json!([]),
            rows_affected: 0,
        })
        .unwrap();
        assert_eq!(
            value,
            json!({"command": "SELECT", "rows": [], "rowsAffected": 0})
        );
    }

    #[test]
    fn live_events_are_typed() {
        let value = serde_json::to_value(LiveEvent::UpToDate { txid: 9 }).unwrap();
        assert_eq!(value, json!({"type": "up-to-date", "txid": 9}));
    }
}
