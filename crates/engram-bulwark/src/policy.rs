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
    pub domain_tags: Vec<String>,
    pub fact_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PolicyDecision {
    Allow,
    Deny {
        reason: String,
        rule_name: Option<String>,
    },
}
