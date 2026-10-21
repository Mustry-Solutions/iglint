//! Dense index types for the arenas of the workspace and of the models.

macro_rules! id_type {
    ($(#[$doc:meta] $name:ident),* $(,)?) => {
        $(
            #[$doc]
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
            pub struct $name(pub u32);

            impl $name {
                /// The index into the owning arena.
                pub const fn index(self) -> usize {
                    self.0 as usize
                }

                pub(crate) fn from_index(index: usize) -> Self {
                    $name(u32::try_from(index).expect("arena index fits in u32"))
                }
            }
        )*
    };
}

id_type!(
    /// A file of the [`crate::Workspace`].
    FileId,
    /// A resource of the [`crate::Workspace`].
    ResourceId,
    /// A project of the [`crate::Workspace`].
    ProjectId,
    /// A gateway configuration collection of the [`crate::Workspace`].
    CollectionId,
    /// A component of a [`crate::View`]. The root component is `ComponentId(0)`.
    ComponentId,
    /// An entry of a model's `scripts` arena.
    ScriptId,
    /// An entry of a model's `expressions` arena.
    ExprId,
    /// An entry of a model's `bindings` arena.
    BindingId,
    /// An entry of a model's `prop_configs` arena.
    PropConfigId,
    /// A tag in a [`crate::TagFolder`].
    TagId,
);
