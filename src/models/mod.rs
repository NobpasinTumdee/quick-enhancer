//! AI model management: the catalog (`models.json`), install state, install plans, uninstall,
//! the custom models folder and "does it fit this PC" checks. Downloads use the same installer
//! as the external tools (`net::tools`).

pub mod catalog;
pub mod custom;
pub mod fit;
pub mod install;

pub use catalog::{Backend, Catalog, Group, ModelEntry, RuntimeNeed};
pub use custom::{CustomKind, CustomModel};
pub use fit::Fit;
pub use install::{InstallSnapshot, ModelState, RuntimeKind};
