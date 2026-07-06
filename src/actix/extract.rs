use actix_web::{
    body::BoxBody,
    error::JsonPayloadError,
    http::StatusCode,
    web::{Json, JsonBody},
    FromRequest as FromWebRequest, HttpRequest, HttpResponse, HttpResponseBuilder, Responder,
    ResponseError,
};
use core::future::Future;
use futures_core::ready;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::ops;
use std::{pin::Pin, task::Poll};

use crate::document::{Request, Response};
use crate::error::{Error, ErrorStatus};
use crate::resource::FromRequest;

/// The JSON:API media type, used as the `Content-Type` for every response
/// this crate renders (documents and error documents alike).
pub const MEDIA_TYPE: &str = "application/vnd.api+json";

/// Actix-web body extractor for JSON:API request documents: deserializes the
/// payload as a [`Request`] and converts it into `R` via [`FromRequest`].
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

/// Serialize an error document, falling back to a plain-text 500 body if
/// serialization itself somehow fails (it shouldn't, but this keeps the error
/// path panic-free).
pub(crate) fn error_document_response(status: StatusCode, response: &Response<(), ()>) -> HttpResponse<BoxBody> {
    match serde_json::to_string(response) {
        Ok(body) => HttpResponseBuilder::new(status)
            .content_type(MEDIA_TYPE)
            .body(body),
        Err(_) => HttpResponseBuilder::new(StatusCode::INTERNAL_SERVER_ERROR)
            .content_type("text/plain; charset=utf-8")
            .body("failed to serialize error document"),
    }
}

impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        (&self.status).into()
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        error_document_response(self.status_code(), &Response::from(self.clone()))
    }
}

impl Into<HttpResponse> for Error {
    fn into(self) -> HttpResponse {
        let status = self.status_code();
        error_document_response(status, &Response::from(self))
    }
}

impl<P: Serialize, I: Serialize> Responder for Response<P, I> {
    type Body = BoxBody;

    fn respond_to(self, _req: &HttpRequest) -> HttpResponse<BoxBody> {
        match serde_json::to_string(&self) {
            Ok(body) => HttpResponseBuilder::new(StatusCode::OK)
                .content_type(MEDIA_TYPE)
                .body(body),
            Err(_) => error_document_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &Response::from(Error::new_internal_error(
                    "failed to serialize response document",
                )),
            ),
        }
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
