#![cfg(feature = "actixweb")]

use actix_web::body::{EitherBody, MessageBody};
use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{Error as WebError, HttpMessage, ResponseError};
use std::future::{ready, Future, Ready};
use std::pin::Pin;
use std::rc::Rc;

/// A factory trait for extracting a per-request user session from an actix-web
/// `ServiceRequest`. This is a deliberately stripped-down alternative to wiring up
/// `actix_web::dev::Transform` + `Service` by hand: implementors provide a single
/// `extract` method that inspects the request and returns either the session value
/// or a [`crate::Error`].
///
/// The session value is stored on the request's extensions map so downstream
/// handlers can recover it through `actix_web::FromRequest`. On error, the JSON:API
/// `ResponseError` impl renders a standards-compliant error document with the
/// status derived from the `Error`'s [`crate::ErrorStatus`].
///
/// ```ignore
/// struct JwtFactory { key: ES256kPublicKey }
///
/// #[derive(Clone)]
/// struct Session { user: Uuid, role: String }
///
/// impl UserSessionFactory for JwtFactory {
///     type Session = Session;
///     fn extract(&self, req: &ServiceRequest) -> Result<Session, jsonapi::Error> {
///         let token = req.headers().get("authorization")
///             .and_then(|v| v.to_str().ok())
///             .and_then(|v| v.strip_prefix("Bearer "))
///             .ok_or_else(|| jsonapi::Error::new_unauthorized("missing bearer token"))?;
///         // ...parse token, build Session...
///     }
/// }
///
/// App::new().wrap(UserSessionMiddleware::new(JwtFactory { key }))
/// ```
pub trait UserSessionFactory: 'static {
    /// The session type produced by `extract`. Must be `Clone` because the value is
    /// inserted into the request's extension map and then handed out by-value via
    /// `FromRequest` to handlers further down the chain.
    type Session: Clone + 'static;

    /// Inspect the request and produce either a session or a JSON:API error. When
    /// this returns `Err`, the inner service is never called; the error is rendered
    /// as a JSON:API error response and returned to the client.
    fn extract(&self, req: &ServiceRequest) -> Result<Self::Session, crate::Error>;
}

/// Extractor recovering the session stored by [`UserSessionMiddleware`] from the
/// request extensions. `T` is the [`UserSessionFactory::Session`] type. Handlers
/// simply take `Session<MySession>` as an argument:
///
/// ```ignore
/// async fn whoami(sess: Session<MySession>) -> HttpResponse {
///     HttpResponse::Ok().body(sess.user_id.clone())
/// }
/// ```
///
/// If the middleware was not installed on the route (so no session of type `T`
/// exists in the extensions), extraction fails with a 500 JSON:API error rather
/// than a 401: a missing session at this point is a server wiring bug, not a
/// client authentication failure (the middleware itself rejects unauthenticated
/// requests before the handler runs).
pub struct Session<T>(pub T);

impl<T> Session<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::ops::Deref for Session<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Clone + 'static> actix_web::FromRequest for Session<T> {
    type Error = crate::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(
        req: &actix_web::HttpRequest,
        _: &mut actix_web::dev::Payload,
    ) -> Self::Future {
        ready(match req.extensions().get::<T>().cloned() {
            Some(session) => Ok(Session(session)),
            None => Err(crate::Error::new_internal_error(
                "session middleware was not installed on this route",
            )),
        })
    }
}

/// Actix-web middleware factory wrapping a [`UserSessionFactory`]. Pass to
/// `.wrap()` on an `App` or `Scope`. The factory is shared (via `Rc`) across all
/// requests handled by a single worker.
pub struct UserSessionMiddleware<F> {
    factory: Rc<F>,
}

impl<F> UserSessionMiddleware<F> {
    pub fn new(factory: F) -> Self {
        Self {
            factory: Rc::new(factory),
        }
    }
}

impl<S, B, F> Transform<S, ServiceRequest> for UserSessionMiddleware<F>
where
    F: UserSessionFactory,
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = WebError> + 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = WebError;
    type InitError = ();
    type Transform = UserSessionService<S, F>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, srv: S) -> Self::Future {
        ready(Ok(UserSessionService {
            srv,
            factory: self.factory.clone(),
        }))
    }
}

pub struct UserSessionService<S, F> {
    srv: S,
    factory: Rc<F>,
}

impl<S, B, F> Service<ServiceRequest> for UserSessionService<S, F>
where
    F: UserSessionFactory,
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = WebError> + 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = WebError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, WebError>>>>;

    forward_ready!(srv);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        match self.factory.extract(&req) {
            Ok(session) => {
                req.extensions_mut().insert(session);
                let fut = self.srv.call(req);
                Box::pin(async move { Ok(fut.await?.map_into_left_body()) })
            }
            Err(err) => {
                let response = err.error_response();
                let (http_req, _) = req.into_parts();
                let sr = ServiceResponse::new(http_req, response).map_into_right_body();
                Box::pin(async move { Ok(sr) })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::StatusCode;
    use actix_web::test::{call_service, init_service, read_body, read_body_json, TestRequest};
    use actix_web::{get, App, HttpRequest, HttpResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Clone, Debug, PartialEq)]
    struct TestSession {
        user_id: String,
    }

    struct HeaderSessionFactory {
        calls: Arc<AtomicUsize>,
    }

    impl UserSessionFactory for HeaderSessionFactory {
        type Session = TestSession;

        fn extract(&self, req: &ServiceRequest) -> Result<Self::Session, crate::Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let user_id = req
                .headers()
                .get("x-user-id")
                .ok_or_else(|| crate::Error::new_unauthorized("missing x-user-id header"))?
                .to_str()
                .map_err(|_| crate::Error::new_bad_request("invalid x-user-id header"))?
                .to_owned();
            if user_id == "forbidden" {
                return Err(crate::Error::new_forbidden("user is forbidden"));
            }
            Ok(TestSession { user_id })
        }
    }

    #[get("/whoami")]
    async fn whoami(sess: Session<TestSession>) -> HttpResponse {
        HttpResponse::Ok().body(sess.user_id.clone())
    }

    fn factory() -> (HeaderSessionFactory, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            HeaderSessionFactory {
                calls: calls.clone(),
            },
            calls,
        )
    }

    #[actix_web::test]
    async fn injects_session_when_header_present() {
        let (f, calls) = factory();
        let app = init_service(
            App::new()
                .wrap(UserSessionMiddleware::new(f))
                .service(whoami),
        )
        .await;

        let req = TestRequest::get()
            .uri("/whoami")
            .insert_header(("x-user-id", "alice"))
            .to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(read_body(resp).await.as_ref(), b"alice");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[actix_web::test]
    async fn renders_jsonapi_unauthorized_when_header_missing() {
        let (f, _) = factory();
        let app = init_service(
            App::new()
                .wrap(UserSessionMiddleware::new(f))
                .service(whoami),
        )
        .await;

        let req = TestRequest::get().uri("/whoami").to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let body: serde_json::Value = read_body_json(resp).await;
        let errors = body
            .get("errors")
            .and_then(|v| v.as_array())
            .expect("expected JSON:API errors envelope");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].get("status").and_then(|v| v.as_str()), Some("401"));
    }

    #[actix_web::test]
    async fn maps_factory_error_status_through_to_response() {
        let (f, _) = factory();
        let app = init_service(
            App::new()
                .wrap(UserSessionMiddleware::new(f))
                .service(whoami),
        )
        .await;

        let req = TestRequest::get()
            .uri("/whoami")
            .insert_header(("x-user-id", "forbidden"))
            .to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let body: serde_json::Value = read_body_json(resp).await;
        assert_eq!(
            body["errors"][0]["status"].as_str(),
            Some("403"),
            "expected forbidden status in JSON:API envelope: {body}"
        );
    }

    #[actix_web::test]
    async fn session_extraction_without_middleware_is_a_server_error() {
        // No UserSessionMiddleware wrapped: Session<T> extraction must fail
        // as a 500 (wiring bug), not a 401.
        let app = init_service(App::new().service(whoami)).await;

        let req = TestRequest::get().uri("/whoami").to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body: serde_json::Value = read_body_json(resp).await;
        assert_eq!(body["errors"][0]["status"].as_str(), Some("500"));
    }

    #[actix_web::test]
    async fn skips_handler_on_extraction_failure() {
        // Verify that when extract returns Err the inner handler is never called.
        // We do this by mounting a handler that would *panic* if it ran without
        // a session being present, then sending a request with no auth header.
        #[get("/strict")]
        async fn strict(sess: Session<TestSession>) -> HttpResponse {
            // Reaching this handler with no session would mean the middleware
            // failed to short-circuit on an Err extraction.
            panic!(
                "handler should not run when extract returns Err; got {}",
                sess.user_id
            );
        }

        let (f, calls) = factory();
        let app = init_service(
            App::new()
                .wrap(UserSessionMiddleware::new(f))
                .service(strict),
        )
        .await;

        let req = TestRequest::get().uri("/strict").to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // extract was invoked exactly once; handler never ran.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
