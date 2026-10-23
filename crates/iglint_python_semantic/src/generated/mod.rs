//! Machine-measured facts generated inside the Ignition 8.3.3 image by
//! `stubs-gen/gen_vetted.py`. See that script and
//! `docs/internal/fix-safety.md` for what they mean and how to regenerate.

#[rustfmt::skip]
mod vetted_generated;

pub(crate) use vetted_generated::{
    JAVA_FILE_MEMBERS, JAVA_PACKAGE_CLASSES, JAVA_REFLECTIVE_MEMBERS, MODULE_ATTRS,
    MODULE_VETTED_ATTRS, PERSPECTIVE_DELEGATE_MEMBERS, PERSPECTIVE_MEMBERS,
    PERSPECTIVE_PROPERTY_TYPES,
};

#[cfg(test)]
pub(crate) use vetted_generated::{GENERATED_FROM, JAVA_FILE_CLASSES};
