pub mod causal;
pub mod config;
pub mod fact_type;
pub mod frontmatter;
pub mod hash;
pub mod ontology;
pub mod temporal;
pub mod validation;

pub use causal::{
    align_up, dequantize_weight, expected_file_size, quantize_weight, validate_causal_header,
    CausalBuildReport, CausalEdge, CausalHeader, CausalNode, CausalValidationWarning,
    CAUSAL_MAGIC, CAUSAL_VERSION, NULL_NODE,
};
pub use config::{load_workspace_config, AccessTrackingConfig, CompileConfig, OntologyConfig, Tier3Config, WorkspaceConfig, CAUSAL_MAX_HOPS_CAP};
pub use fact_type::FactType;
pub use frontmatter::{FactRecord, RawFrontmatter};
pub use hash::fnv1a_u64;
pub use ontology::{OntologyError, OntologyIndex, TagValidation};
pub use validation::{validate, CompileWarning, ValidationError};

#[cfg(test)]
mod tests;
