#[cfg(feature = "actixweb")]
use actix_web::{
    error::JsonPayloadError,
    http::StatusCode,
    web::{Json, JsonBody},
    FromRequest as FromWebRequest, HttpResponse, HttpResponseBuilder, ResponseError,
};
#[cfg(feature = "actixweb")]
use core::future::Future;
#[cfg(feature = "actixweb")]
use futures_core::ready;
#[cfg(feature = "actixweb")]
use serde::de::DeserializeOwned;
use serde_derive::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Display, ops};
#[cfg(feature = "actixweb")]
use std::{pin::Pin, task::Poll};
#[cfg(feature = "server")]
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
pub struct ResourceResponse<D> {
    #[serde(flatten)]
    pub id: Identifier,
    pub attributes: D,
    pub relationships: Option<BTreeMap<String, RelationshipData>>,
}

pub trait Resource {
    type Attributes;
    type Relations;

    fn type_name() -> &'static str;

    fn into_response(self) -> Response<Self::Attributes, Self::Relations>;
}

#[derive(Serialize, Deserialize, Clone)]
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

#[cfg(feature = "server")]
impl From<Uuid> for ID {
    fn from(id: Uuid) -> ID {
        ID(id.to_string())
    }
}

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

#[cfg(feature = "server")]
impl FromID for Uuid {
    fn from_id(id: ID) -> Result<Self, Error> {
        Uuid::parse_str(&id.0).map_err(|err| {
            Error::new_bad_request(&format!(
                "invalid value for UUID id value: {}",
                err.to_string()
            ))
        })
    }
}

impl FromID for ID {
    fn from_id(id: ID) -> Result<Self, Error> {
        Ok(id)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RelationshipData {
    pub data: Relationship,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Identifier {
    pub id: ID,
    #[serde(rename = "type")]
    pub typ: String,
}

#[derive(Serialize, Deserialize)]
pub struct ResourceRequest<D> {
    pub id: Option<ID>,
    #[serde(rename = "type")]
    pub typ: String,
    pub attributes: D,
    pub relationships: Option<BTreeMap<String, RelationshipData>>,
}

impl<T: Clone> Clone for Request<T> {
    fn clone(&self) -> Self {
        Request {
            data: ResourceRequest {
                id: self.data.id.clone(),
                typ: self.data.typ.clone(),
                attributes: self.data.attributes.clone(),
                relationships: match &self.data.relationships {
                    Some(x) => Some(x.clone()),
                    None => None,
                },
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Request<D> {
    pub data: ResourceRequest<D>,
}

#[derive(Serialize, Deserialize)]
pub struct Response<P, I> {
    #[serde(flatten)]
    pub primary: ResponseType<P>,
    pub included: Option<Vec<ResourceResponse<I>>>,
}

impl<P, I> Response<P, I> {
    pub fn include<Ex>(mut self, resource: Ex) -> Self
    where
        Ex: IntoResponse<Attributes = I>,
    {
        if self.included.is_none() {
            self.included = Some(vec![resource.into_response()])
        } else {
            self.included
                .as_mut()
                .unwrap()
                .push(resource.into_response())
        }
        self
    }

    pub fn include_many<Ex>(mut self, resources: Vec<Ex>) -> Self
    where
        Ex: IntoResponse<Attributes = I>,
    {
        if self.included.is_none() {
            self.included = Some(
                resources
                    .into_iter()
                    .map(|res| res.into_response())
                    .collect(),
            )
        } else {
            self.included.as_mut().unwrap().append(
                &mut resources
                    .into_iter()
                    .map(|res| res.into_response())
                    .collect(),
            )
        }
        self
    }
}

impl<P> Response<P, Option<()>> {
    pub fn finish(self) -> Self {
        self
    }
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

// IntoResponse is used to create _successful_ jsonapi responses from a resource struct
// it is not used to create error responses (return a jsonapi::Error::into() for that)
pub trait IntoResponse {
    type Attributes;

    fn into_response(self) -> ResourceResponse<Self::Attributes>;
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

impl<R: IntoResponse, I> From<R> for Response<R::Attributes, I> {
    fn from(r: R) -> Self {
        Response {
            primary: ResponseType::Ok(vec![r.into_response()]),
            included: None,
        }
    }
}

impl<R: IntoResponse, I> From<Vec<R>> for Response<R::Attributes, I> {
    fn from(v: Vec<R>) -> Self {
        let data = v.into_iter().map(|each| each.into_response()).collect();
        Response {
            primary: ResponseType::Ok(data),
            included: None,
        }
    }
}

impl From<Error> for Response<(), ()> {
    fn from(e: Error) -> Self {
        Response {
            primary: ResponseType::Error(vec![e]),
            included: None,
        }
    }
}

impl From<Vec<Error>> for Response<(), ()> {
    fn from(v: Vec<Error>) -> Self {
        Response {
            primary: ResponseType::Error(v),
            included: None,
        }
    }
}

// Stuff that should be moved into a jsonapi-actixweb crate at a later date
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

#[cfg(feature = "actixweb")]
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

#[cfg(feature = "actixweb")]
pub struct JsonApiExtractFut<T: FromRequest> {
    fut: JsonBody<Request<T::Attributes>>,
}

#[cfg(feature = "actixweb")]
impl From<JsonPayloadError> for Error {
    fn from(err: JsonPayloadError) -> Error {
        Error::new_bad_request(&err.to_string())
    }
}

#[cfg(feature = "actixweb")]
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

#[cfg(feature = "actixweb")]
impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        (&self.status).into()
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        HttpResponseBuilder::new(self.status_code()).json(Response::from(self.clone()))
    }
}

#[cfg(feature = "actixweb")]
impl Into<HttpResponse> for Error {
    fn into(self) -> HttpResponse {
        HttpResponseBuilder::new(self.status_code()).json(Response::from(self))
    }
}

#[cfg(feature = "actixweb")]
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use uuid::Uuid;

    use crate::{
        FromID, FromRelationships, FromRequest, Identifier, IntoResponse, Relationship,
        RelationshipData, Request, ResourceRequest, ResourceResponse, Response,
    };

    // A simple request with no relationships
    struct SimpleRequest {
        id: Uuid,
        attributes: SimpleAttributes,
    }

    #[derive(Clone)]
    struct SimpleAttributes {
        foo: String,
        bar: Option<isize>,
    }

    impl FromRequest for SimpleRequest {
        type Attributes = SimpleAttributes;

        fn from_request(req: Request<Self::Attributes>) -> Result<Self, crate::Error> {
            // ensure no relationships were passed (this implicitly has a "relationships" of unit struct)
            FromRelationships::from_relationships(req.data.relationships)?;
            Ok(SimpleRequest {
                id: FromID::from_id(req.data.id.unwrap())?,
                attributes: req.data.attributes,
            })
        }
    }

    #[test]
    fn test_simple_request() {
        let id = Uuid::new_v4();
        let mut req = Request {
            data: ResourceRequest {
                id: Some(id.clone().into()),
                typ: "simple".into(),
                attributes: SimpleAttributes {
                    foo: "testing".into(),
                    bar: Some(123),
                },
                relationships: None,
            },
        };
        assert!(SimpleRequest::from_request(req.clone()).is_ok());
        req.data.id = Some("foobarbaz".into()); // invalid UUID format
        assert!(SimpleRequest::from_request(req.clone()).is_err());
        req.data.id = Some(id.clone().into());
        let mut relations = BTreeMap::new();
        relations.insert(
            "fake".to_owned(),
            RelationshipData {
                data: Relationship::ToOne(Identifier {
                    id: "test".into(),
                    typ: "fake".into(),
                }),
            },
        );
        req.data.relationships = Some(relations);
        assert!(SimpleRequest::from_request(req.clone()).is_err());
    }

    struct SimpleResponse {
        id: Uuid,
        attributes: SimpleAttributes,
    }

    impl IntoResponse for SimpleResponse {
        type Attributes = SimpleAttributes;

        fn into_response(self) -> ResourceResponse<Self::Attributes> {
            ResourceResponse {
                id: Identifier {
                    id: self.id.into(),
                    typ: "simple".into(),
                },
                attributes: self.attributes,
                relationships: None,
            }
        }
    }

    #[test]
    fn test_simple_response() {
        let attrs = SimpleAttributes {
            foo: "foo".into(),
            bar: None,
        };
        let id = Uuid::new_v4();
        let response = SimpleResponse {
            id,
            attributes: attrs,
        };
        // finish with no included resources.
        // finish is essentially a more readable way to provided types for responses
        // with no included resources. There is likely a better way to do this but for
        // now this is the approach we're taking.
        Response::from(response).finish();
    }
}
