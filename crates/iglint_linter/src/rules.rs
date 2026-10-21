//! All rules known to iglint.

use crate::registry::{Registry, RuleMeta};

/// Every rule, in code order.
pub static ALL: &[&RuleMeta] = &[];

pub static REGISTRY: Registry = Registry::new(ALL);
