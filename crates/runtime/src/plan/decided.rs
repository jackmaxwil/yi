//! What the engine decides beside an op, journaled in the record's `extra` so the reducer
//! replays the decision instead of deciding it again.

use serde_json::{Map, Value};
use yi_types::plan::canonical::Digest;
use yi_types::plan::contract;
use yi_types::plan::doc::PlanId;
use yi_types::plan::op::Reaped;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decided {
    pub reaped: Vec<Reaped>,
    pub subplan: Option<PlanId>,
    /// How a `done` resolves; the engine decides it beside the committed verdict.
    pub resolution: Option<contract::Resolution>,
    /// The contract digest a `start` froze.
    pub contract_hash: Option<Digest>,
    /// The op's own todo gets one claim past the red ladder, judged live with the cap lever.
    pub readmit: bool,
}

impl Decided {
    pub fn into_extra(self) -> Result<Map<String, Value>, serde_json::Error> {
        let mut extra = Map::new();
        if !self.reaped.is_empty() {
            extra.insert("reaped".to_owned(), serde_json::to_value(&self.reaped)?);
        }
        if let Some(subplan) = self.subplan {
            extra.insert(
                "subplan".to_owned(),
                Value::String(subplan.as_str().to_owned()),
            );
        }
        if let Some(resolution) = self.resolution {
            extra.insert("resolution".to_owned(), serde_json::to_value(resolution)?);
        }
        if let Some(digest) = self.contract_hash {
            extra.insert(
                "contract_hash".to_owned(),
                Value::String(digest.to_string()),
            );
        }
        if self.readmit {
            extra.insert("readmit".to_owned(), Value::Bool(true));
        }
        Ok(extra)
    }

    pub(super) fn from_extra(extra: &Map<String, Value>) -> Result<Self, String> {
        let reaped = match extra.get("reaped") {
            None => Vec::new(),
            Some(value) => serde_json::from_value(value.clone()).map_err(|e| e.to_string())?,
        };
        let subplan = match extra.get("subplan") {
            None => None,
            Some(Value::String(id)) => Some(PlanId::new(id).map_err(|e| e.to_string())?),
            Some(other) => return Err(format!("subplan is not a string: {other}")),
        };
        let resolution = match extra.get("resolution") {
            None => None,
            Some(value) => serde_json::from_value(value.clone()).map_err(|e| e.to_string())?,
        };
        let contract_hash = match extra.get("contract_hash") {
            None => None,
            Some(Value::String(text)) => Some(Digest::parse(text).map_err(|e| e.to_string())?),
            Some(other) => return Err(format!("contract_hash is not a string: {other}")),
        };
        Ok(Self {
            reaped,
            subplan,
            resolution,
            contract_hash,
            readmit: extra.get("readmit") == Some(&Value::Bool(true)),
        })
    }
}
