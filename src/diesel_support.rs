//! `diesel::result::Error` → [`crate::Error`] mapping.
//!
//! This crate does no database access itself, but every consumer that uses
//! diesel ends up redefining the same handful of translations from a
//! `diesel::result::Error` into a JSON:API error document. This module
//! provides that mapping once, behind the optional `diesel` feature, so it
//! doesn't leak database internals into error responses (the generic
//! "database error" arm intentionally carries no `detail`).
#![cfg(feature = "diesel")]

use diesel::result::{DatabaseErrorKind, Error as DieselError};

use crate::error::Error;

impl From<DieselError> for Error {
    fn from(err: DieselError) -> Self {
        match err {
            DieselError::NotFound => Error::new_not_found("resource not found"),
            DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, info) => {
                let mut e = Error::new_conflict("resource already exists");
                e.detail = Some(info.message().to_owned());
                e
            }
            DieselError::DatabaseError(DatabaseErrorKind::ForeignKeyViolation, info) => {
                let mut e = Error::new_bad_request("referenced resource does not exist");
                e.detail = Some(info.message().to_owned());
                e
            }
            DieselError::DatabaseError(DatabaseErrorKind::CheckViolation, info) => {
                let mut e = Error::new_bad_request("invalid value");
                e.detail = Some(info.message().to_owned());
                e
            }
            // Anything else (including other DatabaseErrorKind variants):
            // don't leak internals into the response.
            _ => Error::new_internal_error("database error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::result::DatabaseErrorInformation;

    #[derive(Debug)]
    struct TestDbErrorInfo {
        message: String,
    }

    impl DatabaseErrorInformation for TestDbErrorInfo {
        fn message(&self) -> &str {
            &self.message
        }
        fn details(&self) -> Option<&str> {
            None
        }
        fn hint(&self) -> Option<&str> {
            None
        }
        fn table_name(&self) -> Option<&str> {
            None
        }
        fn column_name(&self) -> Option<&str> {
            None
        }
        fn constraint_name(&self) -> Option<&str> {
            None
        }
        fn statement_position(&self) -> Option<i32> {
            None
        }
    }

    fn info(message: &str) -> Box<dyn DatabaseErrorInformation + Send + Sync> {
        Box::new(TestDbErrorInfo {
            message: message.to_owned(),
        })
    }

    #[test]
    fn not_found_maps_to_404() {
        let err: Error = DieselError::NotFound.into();
        assert_eq!(err.status, crate::error::ErrorStatus::NotFound);
        assert_eq!(err.detail, None);
    }

    #[test]
    fn unique_violation_maps_to_409_with_detail() {
        let diesel_err = DieselError::DatabaseError(
            DatabaseErrorKind::UniqueViolation,
            info("duplicate key value"),
        );
        let err: Error = diesel_err.into();
        assert_eq!(err.status, crate::error::ErrorStatus::Conflict);
        assert_eq!(err.title, "resource already exists");
        assert_eq!(err.detail, Some("duplicate key value".to_owned()));
    }

    #[test]
    fn foreign_key_violation_maps_to_400_with_detail() {
        let diesel_err = DieselError::DatabaseError(
            DatabaseErrorKind::ForeignKeyViolation,
            info("violates foreign key constraint"),
        );
        let err: Error = diesel_err.into();
        assert_eq!(err.status, crate::error::ErrorStatus::BadRequest);
        assert_eq!(err.title, "referenced resource does not exist");
        assert_eq!(
            err.detail,
            Some("violates foreign key constraint".to_owned())
        );
    }

    #[test]
    fn check_violation_maps_to_400_with_detail() {
        let diesel_err =
            DieselError::DatabaseError(DatabaseErrorKind::CheckViolation, info("check failed"));
        let err: Error = diesel_err.into();
        assert_eq!(err.status, crate::error::ErrorStatus::BadRequest);
        assert_eq!(err.title, "invalid value");
        assert_eq!(err.detail, Some("check failed".to_owned()));
    }

    #[test]
    fn other_errors_map_to_500_with_no_detail() {
        let diesel_err = DieselError::AlreadyInTransaction;
        let err: Error = diesel_err.into();
        assert_eq!(err.status, crate::error::ErrorStatus::InternalError);
        assert_eq!(err.title, "database error");
        assert_eq!(err.detail, None);

        // A DatabaseError with an unmapped kind should also not leak detail.
        let diesel_err = DieselError::DatabaseError(
            DatabaseErrorKind::UnableToSendCommand,
            info("some internal detail"),
        );
        let err: Error = diesel_err.into();
        assert_eq!(err.status, crate::error::ErrorStatus::InternalError);
        assert_eq!(err.detail, None);
    }
}
