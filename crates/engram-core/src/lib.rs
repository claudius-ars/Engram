pub mod causal;
pub mod config;
pub mod fact_type;
pub mod frontmatter;
pub mod temporal;
pub mod validation;

pub use causal::{
    align_up, dequantize_weight, expected_file_size, quantize_weight, validate_causal_header,
    CausalBuildReport, CausalEdge, CausalHeader, CausalNode, CausalValidationWarning,
    CAUSAL_MAGIC, CAUSAL_VERSION, NULL_NODE,
};
pub use config::{load_workspace_config, CompileConfig, WorkspaceConfig, CAUSAL_MAX_HOPS_CAP};
pub use fact_type::FactType;
pub use frontmatter::{FactRecord, RawFrontmatter};
pub use validation::{validate, CompileWarning, ValidationError};

#[cfg(test)]
mod tests;
