use actix_web::{
    error::JsonPayloadError,
    http::StatusCode,
    web::{Json, JsonBody},
    FromRequest as FromWebRequest, HttpResponse, HttpResponseBuilder, ResponseError,
};
use core::future::Future;
use futures_core::ready;
use serde::de::DeserializeOwned;
use std::ops;
use std::{pin::Pin, task::Poll};

use crate::document::{Request, Response};
use crate::error::{Error, ErrorStatus};
use crate::resource::FromRequest;

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

pub struct JsonApiExtractFut<T: FromRequest> {
    fut: JsonBody<Request<T::Attributes>>,
}

impl From<JsonPayloadError> for Error {
    fn from(err: JsonPayloadError) -> Error {
        Error::new_bad_request(&err.to_string())
    }
}

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

impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        (&self.status).into()
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        HttpResponseBuilder::new(self.status_code()).json(Response::from(self.clone()))
    }
}

impl Into<HttpResponse> for Error {
    fn into(self) -> HttpResponse {
        HttpResponseBuilder::new(self.status_code()).json(Response::from(self))
    }
}

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
