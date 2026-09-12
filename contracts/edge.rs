// GENERATED FILE - DO NOT EDIT. Source schemas: schema/edge/common/error.schema.json, schema/edge/common/health.schema.json, schema/edge/execution/check-request.schema.json, schema/edge/execution/check-response.schema.json, schema/edge/execution/deal.schema.json, schema/edge/execution/deals-request.schema.json, schema/edge/execution/deals-response.schema.json, schema/edge/execution/lookup-outcome.schema.json, schema/edge/execution/lookup-request.schema.json, schema/edge/execution/order.schema.json, schema/edge/execution/position.schema.json, schema/edge/execution/positions-request.schema.json, schema/edge/execution/positions-response.schema.json, schema/edge/execution/quote-request.schema.json, schema/edge/execution/quote-response.schema.json, schema/edge/execution/submit-outcome.schema.json, schema/edge/execution/submit-request.schema.json
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckRequest {
    pub intent_id: String,
    pub order: ExecutionOrder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResponse {
    pub allowed: bool,
    pub margin: f64,
    pub reason: Option<String>,
    pub retcode: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DealsRequest {
    pub magic: Option<i64>,
    pub symbol: Option<String>,
    pub window_end: serde_json::Value,
    pub window_start: serde_json::Value,
}

pub type DealsResponse = Vec<ExecutionDeal>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeErrorResponse {
    pub code: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeHealthResponse {
    pub mt5_connected: bool,
    pub schema_version: String,
    pub status: String,
    pub terminal_build: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionDeal {
    pub comment: Option<String>,
    pub commission: Option<f64>,
    pub entry: Option<i64>,
    pub fee: Option<f64>,
    pub magic: Option<i64>,
    pub order_ticket: i64,
    pub price: f64,
    pub profit: Option<f64>,
    pub swap: Option<f64>,
    pub symbol: String,
    pub ticket: i64,
    pub time_msc: Option<i64>,
    #[serde(rename = "type")]
    pub r#type: Option<i64>,
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionOrder {
    pub comment: Option<String>,
    pub deviation: Option<i64>,
    pub magic: Option<i64>,
    pub price: Option<f64>,
    pub side: Option<String>,
    pub sl: Option<f64>,
    pub symbol: String,
    pub tp: Option<f64>,
    #[serde(rename = "type")]
    pub r#type: Option<i64>,
    pub type_filling: Option<i64>,
    pub type_time: Option<i64>,
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionPosition {
    pub comment: Option<String>,
    pub magic: Option<i64>,
    pub price_current: Option<f64>,
    pub price_open: f64,
    pub profit: Option<f64>,
    pub sl: Option<f64>,
    pub symbol: String,
    pub ticket: i64,
    pub time: Option<i64>,
    pub tp: Option<f64>,
    #[serde(rename = "type")]
    pub r#type: i64,
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LookupOutcome {
    Filled {
        pub closes_intent: String,
        pub deals: Vec<ExecutionDeal>,
    },
    Rejected {
        pub closes_intent: String,
        pub reason: Option<String>,
        pub retcode: i64,
    },
    NotFound {
        pub closes_intent: String,
    },
    Unavailable {
        pub closes_intent: String,
        pub reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupRequest {
    pub intent_id: String,
    pub magic: Option<i64>,
    pub window_end: serde_json::Value,
    pub window_start: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionsRequest {
    pub symbol: Option<String>,
}

pub type PositionsResponse = Vec<ExecutionPosition>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteRequest {
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteResponse {
    pub age_ms: i64,
    pub ask: f64,
    pub bid: f64,
    pub last: f64,
    pub symbol: String,
    pub time_msc: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SubmitOutcome {
    Accepted {
        pub order_ticket: i64,
        pub retcode: i64,
    },
    Rejected {
        pub reason: String,
        pub retcode: i64,
    },
    Indeterminate {
        pub reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitRequest {
    pub intent_id: String,
    pub order: ExecutionOrder,
}
