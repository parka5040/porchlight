//! Permission-checked item service. One domain savepoint and WriteScope per request.
// Imports retained only for validate.rs, owned by the concurrent workstream.
use crate::{
    error::{ApiError, FieldError},
    extract::Caller,
};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use porchlight_core::{access::Visibility, items as core};
use porchlight_db::items::{ItemWrite, Relationships};
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

mod acl;
#[cfg(test)]
mod acl_tests;
pub mod ics_import;
mod occurrence_write;
mod read;
mod revert;
pub use revert::revert;
mod validate;
mod write;

pub(crate) use acl::mask as mask_conflicts;
use acl::occurrence_row;
pub(crate) use acl::restrict_feed;
pub use acl::{attach_current, domain_error, filter_capabilities, resource_capabilities, response};
pub(crate) use acl::{authorize, load};
pub use read::{list, occurrences, occurrences_with_zone, read_snapshot};
pub(crate) use validate::{field_decode, reference_error};
pub use write::{Operation, mutate};
