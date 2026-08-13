#[derive(Debug, PartialEq, Clone, Copy)]
pub enum EditMode {
    Move,
    Scale,
    None,
}

/// The kind of editable object a selected collider belongs to.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum EditableObjectType {
    StaticPlatform,
    SpawnCloud,
    MovingPlatform,
}
