use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FactType {
    #[default]
    Durable,
    State,
    Event,
}

impl fmt::Display for FactType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FactType::Durable => write!(f, "durable"),
            FactType::State => write!(f, "state"),
            FactType::Event => write!(f, "event"),
        }
    }
}
