//! Legacy descriptor reads backed by the common committed public replay.
//! Recording capture uses the same replay, but freezes bodies into `capture::CaptureDto`.

pub use crate::committed_replay::{
    PublicMaterialContent, PublicMaterialDescriptor, PublicMaterialError, PublicMaterialIdentity,
    PublicMaterialLimits, PublicMaterialOrigin, PublicMaterialUse, PublicRunBoundary,
    PublicRunOutcome, SessionPublicMaterialReader,
};
