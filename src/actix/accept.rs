//! Content-type negotiation for plain-JSON clients.
//!
//! This crate renders every JSON:API document — success and error alike —
//! with `Content-Type: application/vnd.api+json`. That's correct per spec,
//! but some HTTP clients (simple fetch wrappers, API testing tools, generic
//! REST tooling) are strict about `Accept: application/json` and choke on an
//! unexpected media type even though the body is perfectly valid JSON. The
//! wire format never changes — only the label on it does.

use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::{from_comma_delimited, Accept, HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE};
use actix_web::mime::{self, Mime};
use actix_web::{body::MessageBody, http::header::QualityItem, Error as WebError};
use std::future::{ready, Future, Ready};
use std::pin::Pin;

use crate::actix::extract::MEDIA_TYPE;

/// Middleware: when the request's `Accept` header explicitly prefers
/// `application/json` over `application/vnd.api+json`, rewrite outgoing
/// JSON:API responses' `Content-Type` to `application/json`. The document
/// body is unchanged — JSON:API documents are plain JSON, so relabeling the
/// content type is all a strict plain-JSON client needs. Wrap an `App` or
/// `Scope`:
///
/// ```ignore
/// App::new().wrap(NegotiateContentType).service(resource::<S>()...)
/// ```
pub struct NegotiateContentType;

/// Decide whether the client's `Accept` header explicitly prefers plain JSON
/// over JSON:API's `application/vnd.api+json`.
///
/// The `Accept` header is parsed into its ranked media ranges (quality, then
/// specificity — see [`Accept::ranked`]) and walked in client-preference
/// order. The **first** range that matches one of our two candidate media
/// types decides the outcome:
///
/// - `application/vnd.api+json` (exact) → `false` — the client asked for
///   JSON:API by name, so leave it alone.
/// - `application/json` (exact, not a wildcard) → `true` — the client
///   explicitly wants plain JSON.
/// - `*/*` or `application/*` → `false` — a wildcard is already satisfied by
///   our default `application/vnd.api+json`, so there's no reason to rewrite.
///
/// If no range in the header matches either candidate, if the header is
/// missing, or if it fails to parse, the answer is `false`: this crate
/// deliberately does not treat a mismatched `Accept` header as reason to
/// return `406 Not Acceptable`, so the safe default is to leave the response
/// as JSON:API.
fn prefers_plain_json(headers: &HeaderMap) -> bool {
    let ranges: Vec<QualityItem<Mime>> = match from_comma_delimited(headers.get_all(ACCEPT)) {
        Ok(ranges) => ranges,
        Err(_) => return false,
    };
    let accept = Accept(ranges);

    for range in accept.ranked() {
        let essence = range.essence_str();
        if essence == MEDIA_TYPE {
            return false;
        }
        if essence == mime::APPLICATION_JSON.essence_str() {
            return true;
        }
        if range.type_() == "*" || (range.type_() == "application" && range.subtype() == "*") {
            return false;
        }
    }

    false
}

impl<S, B> Transform<S, ServiceRequest> for NegotiateContentType
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = WebError> + 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = WebError;
    type InitError = ();
    type Transform = NegotiateContentTypeService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, srv: S) -> Self::Future {
        ready(Ok(NegotiateContentTypeService { srv }))
    }
}

pub struct NegotiateContentTypeService<S> {
    srv: S,
}

impl<S, B> Service<ServiceRequest> for NegotiateContentTypeService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = WebError> + 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = WebError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, WebError>>>>;

    forward_ready!(srv);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let plain = prefers_plain_json(req.headers());
        let fut = self.srv.call(req);

        Box::pin(async move {
            let mut res = fut.await?;

            if plain {
                let is_jsonapi = res
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.starts_with(MEDIA_TYPE))
                    .unwrap_or(false);

                if is_jsonapi {
                    res.headers_mut()
                        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                }
            }

            Ok(res)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, IntoResponse, ResourceType};
    use actix_web::test::{call_service, init_service, read_body, read_body_json, TestRequest};
    use actix_web::{http::StatusCode, web, App};

    // ---- prefers_plain_json decision table ---------------------------------

    fn headers(accept: Option<&str>) -> HeaderMap {
        let mut req = TestRequest::default();
        if let Some(accept) = accept {
            req = req.insert_header((ACCEPT, accept));
        }
        let req = req.to_srv_request();
        req.headers().clone()
    }

    #[test]
    fn no_accept_header_defaults_to_false() {
        assert!(!prefers_plain_json(&headers(None)));
    }

    #[test]
    fn plain_json_accept_is_true() {
        assert!(prefers_plain_json(&headers(Some("application/json"))));
    }

    #[test]
    fn jsonapi_accept_is_false() {
        assert!(!prefers_plain_json(&headers(Some(MEDIA_TYPE))));
    }

    #[test]
    fn json_before_vnd_prefers_json() {
        assert!(prefers_plain_json(&headers(Some(&format!(
            "application/json, {MEDIA_TYPE}"
        )))));
    }

    #[test]
    fn vnd_before_json_prefers_vnd() {
        assert!(!prefers_plain_json(&headers(Some(&format!(
            "{MEDIA_TYPE}, application/json"
        )))));
    }

    #[test]
    fn quality_values_reorder_preference() {
        // json is explicitly deprioritized; vnd (implicit q=1) wins the ranking.
        assert!(!prefers_plain_json(&headers(Some(&format!(
            "application/json;q=0.5, {MEDIA_TYPE}"
        )))));
    }

    #[test]
    fn star_star_is_false() {
        assert!(!prefers_plain_json(&headers(Some("*/*"))));
    }

    #[test]
    fn application_star_is_false() {
        assert!(!prefers_plain_json(&headers(Some("application/*"))));
    }

    #[test]
    fn unrelated_media_type_is_false() {
        assert!(!prefers_plain_json(&headers(Some("text/html"))));
    }

    #[test]
    fn garbage_header_value_is_false() {
        assert!(!prefers_plain_json(&headers(Some("not a media type"))));
    }

    // ---- integration: generated-handler path -------------------------------

    struct Widget {
        id: usize,
        name: String,
    }

    struct WidgetResource;

    impl crate::ResourceType for WidgetResource {
        const TYPE_NAME: &'static str = "widgets";
        type Id = usize;
    }

    #[derive(serde_derive::Serialize, Clone)]
    struct WidgetAttributes {
        name: String,
    }

    struct WidgetOut(Widget);

    impl IntoResponse for WidgetOut {
        type Attributes = WidgetAttributes;

        fn into_response(self) -> crate::ResourceResponse<Self::Attributes> {
            crate::ResourceResponse {
                id: crate::Identifier {
                    id: crate::ID(self.0.id.to_string()),
                    typ: WidgetResource::TYPE_NAME.to_owned(),
                },
                attributes: WidgetAttributes { name: self.0.name },
                relationships: None,
                meta: None,
            }
        }
    }

    struct WidgetStore;

    impl crate::actix::ops::Store for WidgetStore {
        type Resource = WidgetResource;
        type Ctx = ();
    }

    impl crate::actix::ops::Show for WidgetStore {
        type Shown = WidgetOut;
        type Included = crate::NoIncluded;

        async fn show(
            &self,
            _ctx: (),
            id: usize,
            _q: crate::ShowQuery,
        ) -> Result<crate::WithIncluded<Self::Shown, Self::Included>, Error> {
            if id == 1 {
                Ok(WidgetOut(Widget {
                    id: 1,
                    name: "Alice".to_owned(),
                })
                .into())
            } else {
                Err(Error::new_not_found("widget not found"))
            }
        }
    }

    impl crate::actix::ops::Delete for WidgetStore {
        async fn delete(&self, _ctx: (), id: usize) -> Result<(), Error> {
            if id == 1 {
                Ok(())
            } else {
                Err(Error::new_not_found("widget not found"))
            }
        }
    }

    fn content_type<B: MessageBody>(resp: &ServiceResponse<B>) -> Option<String> {
        resp.headers()
            .get(CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned())
    }

    // Macros (rather than helper fns) because `init_service`'s return type is
    // an unnameable nested opaque type — same idiom as `ops::tests::full_app!`.
    macro_rules! negotiated_app {
        () => {
            init_service(
                App::new()
                    .app_data(web::Data::new(WidgetStore))
                    .wrap(NegotiateContentType)
                    .service(crate::actix::resource::<WidgetStore>().show().delete()),
            )
            .await
        };
    }

    macro_rules! plain_app {
        () => {
            init_service(
                App::new()
                    .app_data(web::Data::new(WidgetStore))
                    .service(crate::actix::resource::<WidgetStore>().show().delete()),
            )
            .await
        };
    }

    #[actix_web::test]
    async fn show_with_plain_json_accept_rewrites_content_type() {
        let app = negotiated_app!();

        let req = TestRequest::get()
            .uri("/widgets/1")
            .insert_header((ACCEPT, "application/json"))
            .to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(content_type(&resp).as_deref(), Some("application/json"));

        let body: serde_json::Value = read_body_json(resp).await;
        assert!(body.get("data").is_some(), "expected a `data` member: {body}");
    }

    #[actix_web::test]
    async fn show_with_vnd_accept_stays_jsonapi() {
        let app = negotiated_app!();

        let req = TestRequest::get()
            .uri("/widgets/1")
            .insert_header((ACCEPT, MEDIA_TYPE))
            .to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(content_type(&resp).as_deref(), Some(MEDIA_TYPE));
    }

    #[actix_web::test]
    async fn show_with_no_accept_header_stays_jsonapi() {
        let app = negotiated_app!();

        let req = TestRequest::get().uri("/widgets/1").to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(content_type(&resp).as_deref(), Some(MEDIA_TYPE));
    }

    #[actix_web::test]
    async fn error_document_with_plain_json_accept_rewrites_content_type() {
        let app = negotiated_app!();

        let req = TestRequest::get()
            .uri("/widgets/999")
            .insert_header((ACCEPT, "application/json"))
            .to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(content_type(&resp).as_deref(), Some("application/json"));

        let body: serde_json::Value = read_body_json(resp).await;
        assert_eq!(body["errors"][0]["status"].as_str(), Some("404"));
    }

    #[actix_web::test]
    async fn delete_204_passes_through_unharmed() {
        let app = negotiated_app!();

        let req = TestRequest::delete()
            .uri("/widgets/1")
            .insert_header((ACCEPT, "application/json"))
            .to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(content_type(&resp).is_none());
        assert!(read_body(resp).await.is_empty());
    }

    #[actix_web::test]
    async fn without_middleware_plain_json_accept_is_ignored() {
        let app = plain_app!();

        let req = TestRequest::get()
            .uri("/widgets/1")
            .insert_header((ACCEPT, "application/json"))
            .to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(content_type(&resp).as_deref(), Some(MEDIA_TYPE));
    }
}
