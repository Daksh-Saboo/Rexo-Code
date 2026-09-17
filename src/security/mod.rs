//! Security layer: everything related to *should this be allowed to run*.
//!
//! - [`policies`] holds stateless, static rules: workspace boundary checks
//!   and terminal command risk classification.
//! - [`permissions`] holds the stateful approval flow that combines those
//!   policies with user config and interactive prompts.

pub mod permissions;
pub mod policies;
