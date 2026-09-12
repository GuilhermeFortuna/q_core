// GENERATED FILE - DO NOT EDIT. Source schemas: schema/catalog/dataset-manifest.schema.json
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub arrow_schema: serde_json::Value,
    pub checksum_algorithm: String,
    pub dataset_id: String,
    pub files: Vec<serde_json::Value>,
    pub published_at: String,
    pub row_count: i64,
    pub state: String,
    pub subject: serde_json::Value,
    pub supersedes: Option<String>,
    pub time_range: serde_json::Value,
    pub tombstone: Option<serde_json::Value>,
    pub version: i64,
}
