#[cfg(feature = "actix-web")]
use actix_web::{
    error::JsonPayloadError,
    http::StatusCode,
    web::{Json, JsonBody},
    FromRequest as FromWebRequest, HttpResponse, HttpResponseBuilder, ResponseError,
};
#[cfg(feature = "actix-web")]
use core::future::Future;
#[cfg(feature = "actix-web")]
use futures_core::ready;
#[cfg(feature = "actix-web")]
use serde::de::DeserializeOwned;
use serde_derive::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Display, ops, pin::Pin, task::Poll};

// TODO: ALl the Deserialize derives should be broken out into a separate feature
// for clients as its a lot of code that doesn't need to exist for servers
// check the commit associated with this comment to get a list of types
// that don't need Deserialize for a server
#[derive(Serialize, Deserialize)]
pub struct ResourceResponse<D> {
    #[serde(flatten)]
    pub id: Identifier,
    pub attributes: D,
    pub relationships: Option<BTreeMap<String, RelationshipData>>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Relationship {
    ToOne(Identifier),
    ToMany(Vec<Identifier>),
}

impl Into<RelationshipData> for Relationship {
    fn into(self) -> RelationshipData {
        RelationshipData { data: self }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Hash)]
pub struct ID(pub String);

impl From<String> for ID {
    fn from(s: String) -> ID {
        ID(s)
    }
}

impl From<&str> for ID {
    fn from(s: &str) -> ID {
        ID(s.into())
    }
}

impl From<usize> for ID {
    fn from(u: usize) -> ID {
        ID(u.to_string())
    }
}

impl From<isize> for ID {
    fn from(i: isize) -> ID {
        ID(i.to_string())
    }
}

impl Display for ID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Ord for ID {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

pub trait FromID
where
    Self: Sized,
{
    fn from_id(id: ID) -> Result<Self, Error>;
}

impl FromID for String {
    fn from_id(id: ID) -> Result<Self, Error> {
        Ok(id.0)
    }
}

impl FromID for usize {
    fn from_id(id: ID) -> Result<Self, Error> {
        id.0.parse().or(Err(Error::new_bad_request(&format!(
            "invalid value for unsigned id value: {}",
            id
        ))))
    }
}

impl FromID for isize {
    fn from_id(id: ID) -> Result<Self, Error> {
        id.0.parse().or(Err(Error::new_bad_request(&format!(
            "invalid value for integer id value: {}",
            id
        ))))
    }
}

impl FromID for ID {
    fn from_id(id: ID) -> Result<Self, Error> {
        Ok(id)
    }
}

#[derive(Serialize, Deserialize)]
pub struct RelationshipData {
    pub data: Relationship,
}

#[derive(Serialize, Deserialize)]
pub struct Identifier {
    pub id: ID,
    #[serde(rename = "type")]
    pub typ: String,
}

#[derive(Deserialize, Serialize)]
pub struct ResourceRequest<D> {
    pub id: Option<ID>,
    #[serde(rename = "type")]
    pub typ: String,
    pub attributes: D,
    pub relationships: Option<BTreeMap<String, RelationshipData>>,
}

#[derive(Deserialize, Serialize)]
pub struct Request<D> {
    pub data: ResourceRequest<D>,
}

#[derive(Serialize, Deserialize)]
pub struct Response<D> {
    #[serde(flatten)]
    pub primary: ResponseType<D>,
}

#[derive(Serialize, Deserialize)]
pub enum ResponseType<D> {
    #[serde(rename = "data")]
    Ok(Vec<ResourceResponse<D>>),
    #[serde(rename = "errors")]
    Error(Vec<Error>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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

pub trait Responder {
    type Attributes;
    type Relations;

    fn name() -> String;
    fn id(&self) -> ID;
    fn attributes(&self) -> Self::Attributes;
    fn relations(&self) -> Self::Relations;
}

pub trait FromRequest
where
    Self: Sized,
{
    type Attributes;
    fn from_request(req: Request<Self::Attributes>) -> Result<Self, Error>;
}

pub trait IntoRelationships {
    fn into_relationships(self) -> Option<BTreeMap<String, RelationshipData>>;
}

pub trait FromRelationships
where
    Self: Sized,
{
    fn from_relationships(rels: Option<BTreeMap<String, RelationshipData>>) -> Result<Self, Error>;
}

impl IntoRelationships for () {
    fn into_relationships(self) -> Option<BTreeMap<String, RelationshipData>> {
        None
    }
}

impl FromRelationships for () {
    fn from_relationships(rels: Option<BTreeMap<String, RelationshipData>>) -> Result<(), Error> {
        match rels {
            None => Ok(()),
            Some(map) => {
                if map.len() == 0 {
                    Ok(())
                } else {
                    Err(Error::new_bad_request(
                        "unexpected relationships for this resource type",
                    ))
                }
            }
        }
    }
}

pub trait IntoRelationship {
    fn into_relationship(self, resource_name: &str) -> Relationship;
}

pub trait FromRelationship
where
    Self: Sized,
{
    fn from_relationship(r: Relationship) -> Result<Self, Error>;
}

impl<I: FromID> FromRelationship for I {
    fn from_relationship(r: Relationship) -> Result<Self, Error> {
        match r {
            Relationship::ToOne(one) => Ok(I::from_id(one.id)?),
            _ => Err(Error::new_bad_request(
                "invalid relationship: expected a to-one, got to-many",
            )),
        }
    }
}

impl<I: FromID> FromRelationship for Vec<I> {
    fn from_relationship(r: Relationship) -> Result<Vec<I>, Error> {
        match r {
            Relationship::ToMany(many) => {
                let mut results = Vec::with_capacity(many.len());
                for each in many.into_iter() {
                    results.push(I::from_id(each.id)?);
                }
                Ok(results)
            }
            _ => Err(Error::new_bad_request(
                "invalid relationship: expected a to-many, got to-one",
            )),
        }
    }
}

impl<I> IntoRelationship for I
where
    ID: From<I>,
{
    fn into_relationship(self, resource_name: &str) -> Relationship {
        Relationship::ToOne(Identifier {
            id: self.into(),
            typ: resource_name.to_string(),
        })
    }
}

impl<I> IntoRelationship for Vec<I>
where
    ID: From<I>,
{
    fn into_relationship(self, resource_name: &str) -> Relationship {
        Relationship::ToMany(
            self.into_iter()
                .map(|each| Identifier {
                    id: each.into(),
                    typ: resource_name.to_string(),
                })
                .collect(),
        )
    }
}

impl<R: Responder> From<R> for Response<R::Attributes>
where
    R::Relations: IntoRelationships,
{
    fn from(r: R) -> Self {
        Response {
            primary: ResponseType::Ok(vec![ResourceResponse {
                id: Identifier {
                    id: r.id(),
                    typ: R::name(),
                },
                attributes: r.attributes(),
                relationships: r.relations().into_relationships(),
            }]),
        }
    }
}

impl<R: Responder> From<Vec<R>> for Response<R::Attributes>
where
    R::Relations: IntoRelationships,
{
    fn from(v: Vec<R>) -> Self {
        let data = v
            .into_iter()
            .map(|each| ResourceResponse {
                id: Identifier {
                    id: each.id(),
                    typ: R::name(),
                },
                attributes: each.attributes(),
                relationships: each.relations().into_relationships(),
            })
            .collect();
        Response {
            primary: ResponseType::Ok(data),
        }
    }
}

impl From<Error> for Response<()> {
    fn from(e: Error) -> Self {
        Response {
            primary: ResponseType::Error(vec![e]),
        }
    }
}

impl From<Vec<Error>> for Response<()> {
    fn from(v: Vec<Error>) -> Self {
        Response {
            primary: ResponseType::Error(v),
        }
    }
}

// Stuff that should be moved into a jsonapi-actix-web crate at a later date
pub struct JsonApi<R>(R);

impl<R> JsonApi<R> {
    pub fn into_inner(self) -> R {
        self.0
    }
}

impl<R> ops::Deref for JsonApi<R> {
    type Target = R;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(feature = "actix-web")]
impl<R: FromRequest> FromWebRequest for JsonApi<R>
where
    R::Attributes: DeserializeOwned,
{
    type Error = Error;

    type Future = JsonApiExtractFut<R>;

    fn from_request(
        req: &actix_web::HttpRequest,
        payload: &mut actix_web::dev::Payload,
    ) -> Self::Future {
        JsonApiExtractFut {
            fut: JsonBody::new(req, payload, None, true),
        }
    }
}

#[cfg(feature = "actix-web")]
pub struct JsonApiExtractFut<T: FromRequest> {
    fut: JsonBody<Request<T::Attributes>>,
}

#[cfg(feature = "actix-web")]
impl From<JsonPayloadError> for Error {
    fn from(err: JsonPayloadError) -> Error {
        Error::new_bad_request(&err.to_string())
    }
}

#[cfg(feature = "actix-web")]
impl<T: FromRequest> Future for JsonApiExtractFut<T>
where
    T::Attributes: DeserializeOwned,
{
    type Output = Result<JsonApi<T>, Error>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.get_mut();

        let res = ready!(Pin::new(&mut this.fut).poll(cx));

        let res = match res {
            Err(err) => Err(err.into()),
            Ok(data) => Ok(Json(data)),
        };

        Poll::Ready(match res {
            Err(err) => Err(err),
            Ok(json_req) => match T::from_request(json_req.into_inner()) {
                Ok(inner) => Ok(JsonApi(inner)),
                Err(err) => Err(err),
            },
        })
    }
}

#[cfg(feature = "actix-web")]
impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        (&self.status).into()
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        HttpResponseBuilder::new(self.status_code()).json(Response::from(self.clone()))
    }
}

#[cfg(feature = "actix-web")]
impl Into<HttpResponse> for Error {
    fn into(self) -> HttpResponse {
        HttpResponseBuilder::new(self.status_code()).json(Response::from(self))
    }
}

#[cfg(feature = "actix-web")]
impl Into<StatusCode> for &ErrorStatus {
    fn into(self) -> StatusCode {
        match self {
            ErrorStatus::BadRequest => StatusCode::BAD_REQUEST,
            ErrorStatus::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorStatus::Forbidden => StatusCode::FORBIDDEN,
            ErrorStatus::NotFound => StatusCode::NOT_FOUND,
            ErrorStatus::Conflict => StatusCode::CONFLICT,
            ErrorStatus::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
