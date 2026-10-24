//! Machine-measured Perspective facts generated inside the Ignition 8.3.3
//! image by `stubs-gen/gen_perspective_fields.py` (run
//! `stubs-gen/perspective-fields.sh`). See [`crate::perspective_fields`]
//! and `docs/internal/fix-safety.md` ("Machine-measured facts").

#[rustfmt::skip]
mod perspective_fields;

pub(crate) use perspective_fields::{COMPONENT_NAME_MEMBERS, CONFIG_FIELDS};

#[cfg(test)]
pub(crate) use perspective_fields::GENERATOR_HASH;
