pub mod fact_type;
pub mod frontmatter;
pub mod validation;

pub use fact_type::FactType;
pub use frontmatter::{FactRecord, RawFrontmatter};
pub use validation::{validate, CompileWarning, ValidationError};

#[cfg(test)]
mod tests;
