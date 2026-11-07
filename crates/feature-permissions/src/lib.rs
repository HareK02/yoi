#![doc = include_str!("../README.md")]

pub mod types;
pub use types::*;

mod registry;
pub use registry::{
    FeatureHandle, JobGrant, LimitedGrant, NamespaceHandle, PermissionError, PermissionHost,
    ResolvedPermissions,
};
