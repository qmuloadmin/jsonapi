use serde_derive::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ErrorStatus {
    #[serde(rename = "400")]
    BadRequest,
    #[serde(rename = "401")]
    Unauthorized,
    #[serde(rename = "403")]
    Forbidden,
    #[serde(rename = "404")]
    NotFound,
    #[serde(rename = "409")]
    Conflict,
    #[serde(rename = "500")]
    InternalError,
}

impl std::fmt::Display for ErrorStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string::<ErrorStatus>(&self).unwrap()
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Error {
    pub status: ErrorStatus,
    // this is a human readable code, not a numeric code (that is status, above)
    pub code: Option<String>,
    pub title: String,
    pub detail: Option<String>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error {}: {}", self.status, self.title)
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn new_not_found(title: &str) -> Self {
        Error {
            status: ErrorStatus::NotFound,
            code: Some("Not Found".to_owned()),
            title: title.to_owned(),
            detail: None,
        }
    }
    pub fn new_bad_request(title: &str) -> Self {
        Error {
            status: ErrorStatus::BadRequest,
            code: Some("Bad Request".to_owned()),
            title: title.to_owned(),
            detail: None,
        }
    }
    pub fn new_internal_error(title: &str) -> Self {
        Error {
            status: ErrorStatus::InternalError,
            code: Some("Internal Server Error".to_owned()),
            title: title.to_owned(),
            detail: None,
        }
    }
    pub fn new_forbidden(title: &str) -> Self {
        Error {
            status: ErrorStatus::Forbidden,
            code: Some("Forbidden".into()),
            title: title.into(),
            detail: None,
        }
    }
    pub fn new_unauthorized(title: &str) -> Self {
        Error {
            status: ErrorStatus::Unauthorized,
            code: Some("Unauthorized".into()),
            title: title.into(),
            detail: None,
        }
    }
    pub fn new_conflict(title: &str) -> Self {
        Error {
            status: ErrorStatus::Conflict,
            code: Some("Confict".to_owned()),
            title: title.into(),
            detail: None,
        }
    }
}
