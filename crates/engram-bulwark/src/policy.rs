#[derive(Debug, Clone, PartialEq)]
pub enum AccessType {
    Read,
    Write,
    LlmCall,
}

#[derive(Debug, Clone)]
pub struct PolicyRequest {
    pub access_type: AccessType,
    pub fact_id: Option<String>,
    pub agent_id: Option<String>,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
}
