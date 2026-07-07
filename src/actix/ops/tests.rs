//! Integration tests for the operation traits + route mounting, against an
//! in-memory `WidgetStore`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use actix_web::{
    test::{call_service, init_service, read_body, read_body_json, TestRequest},
    web::Data,
    App,
};
use serde_derive::{Deserialize, Serialize};
use serde_json::json;

use super::*;
use crate::actix::extract::MEDIA_TYPE;
use crate::auth::{Session, UserSessionFactory, UserSessionMiddleware};
use crate::{
    CursorPage, Direction, Error, FromRelationships, FromRequest, Identifier, ListQuery, NoIncluded,
    Range, ResourceResponse, ResourceType, Response, ShowQuery, StringMatch, Total, WithIncluded, ID,
};

// ---- domain type ----------------------------------------------------------

#[derive(Debug, Clone)]
struct Widget {
    id: usize,
    name: String,
    weight: u32,
}

struct WidgetResource;

impl ResourceType for WidgetResource {
    const TYPE_NAME: &'static str = "widgets";
    type Id = usize;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct WidgetAttributes {
    name: String,
    weight: u32,
}

/// Wraps a `Widget` for `IntoResponse`.
struct WidgetOut(Widget);

impl IntoResponse for WidgetOut {
    type Attributes = WidgetAttributes;

    fn into_response(self) -> ResourceResponse<Self::Attributes> {
        ResourceResponse {
            id: Identifier {
                id: ID(self.0.id.to_string()),
                typ: WidgetResource::TYPE_NAME.to_owned(),
            },
            attributes: WidgetAttributes {
                name: self.0.name,
                weight: self.0.weight,
            },
            relationships: None,
            meta: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct WidgetDraftAttributes {
    name: String,
    weight: u32,
}

struct WidgetDraft {
    name: String,
    weight: u32,
}

impl FromRequest for WidgetDraft {
    type Attributes = WidgetDraftAttributes;

    fn from_request(req: crate::Request<Self::Attributes>) -> Result<Self, Error> {
        <() as FromRelationships>::from_relationships(req.data.relationships)?;
        Ok(WidgetDraft {
            name: req.data.attributes.name,
            weight: req.data.attributes.weight,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct WidgetPatchAttributes {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    weight: Option<u32>,
}

struct WidgetPatch {
    name: Option<String>,
    weight: Option<u32>,
}

impl FromRequest for WidgetPatch {
    type Attributes = WidgetPatchAttributes;

    fn from_request(req: crate::Request<Self::Attributes>) -> Result<Self, Error> {
        <() as FromRelationships>::from_relationships(req.data.relationships)?;
        Ok(WidgetPatch {
            name: req.data.attributes.name,
            weight: req.data.attributes.weight,
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct WidgetFilter {
    name: Option<StringMatch>,
    weight: Range<u32>,
}

fn matches_filter(w: &Widget, f: &WidgetFilter) -> bool {
    if let Some(name) = &f.name {
        let matches = match name {
            StringMatch::Eq(v) => &w.name == v,
            StringMatch::Contains(v) => w.name.contains(v.as_str()),
        };
        if !matches {
            return false;
        }
    }
    if let Some(eq) = f.weight.eq {
        if w.weight != eq {
            return false;
        }
    }
    if let Some(gt) = f.weight.gt {
        if w.weight <= gt {
            return false;
        }
    }
    if let Some(gte) = f.weight.gte {
        if w.weight < gte {
            return false;
        }
    }
    if let Some(lt) = f.weight.lt {
        if w.weight >= lt {
            return false;
        }
    }
    if let Some(lte) = f.weight.lte {
        if w.weight > lte {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WidgetSortKey {
    Name,
    Weight,
}

// ---- store ------------------------------------------------------------

struct WidgetStore {
    inner: RwLock<BTreeMap<usize, Widget>>,
    next_id: AtomicUsize,
}

impl WidgetStore {
    fn new() -> Self {
        WidgetStore {
            inner: RwLock::new(BTreeMap::new()),
            next_id: AtomicUsize::new(1),
        }
    }

    fn seeded(widgets: impl IntoIterator<Item = (usize, &'static str, u32)>) -> Self {
        let store = Self::new();
        let mut max_id = 0usize;
        {
            let mut map = store.inner.write().unwrap();
            for (id, name, weight) in widgets {
                map.insert(
                    id,
                    Widget {
                        id,
                        name: name.to_owned(),
                        weight,
                    },
                );
                max_id = max_id.max(id);
            }
        }
        store.next_id.store(max_id + 1, Ordering::SeqCst);
        store
    }
}

impl Store for WidgetStore {
    type Resource = WidgetResource;
    type Ctx = ();
}

impl Show for WidgetStore {
    type Shown = WidgetOut;
    type Included = NoIncluded;

    async fn show(
        &self,
        _ctx: (),
        id: IdOf<Self>,
        _q: ShowQuery,
    ) -> Result<WithIncluded<Self::Shown, Self::Included>, Error> {
        self.inner
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .map(WidgetOut)
            .map(Into::into)
            .ok_or_else(|| Error::new_not_found("widget not found"))
    }
}

impl List for WidgetStore {
    type Filter = WidgetFilter;
    type SortKey = WidgetSortKey;
    type Item = WidgetOut;
    type Included = NoIncluded;

    async fn list(
        &self,
        _ctx: (),
        q: ListQuery<Self::Filter, Self::SortKey>,
    ) -> Result<WithIncluded<CursorPage<Self::Item>, Self::Included>, Error> {
        let map = self.inner.read().unwrap();
        let mut items: Vec<Widget> = map
            .values()
            .filter(|w| matches_filter(w, &q.filter))
            .cloned()
            .collect();
        drop(map);

        // Stable multi-key sort: apply least-significant key first so the
        // most-significant key (first in the parsed spec) dominates.
        for (key, dir) in q.sort.iter().collect::<Vec<_>>().into_iter().rev() {
            items.sort_by(|a, b| {
                let ord = match key {
                    WidgetSortKey::Name => a.name.cmp(&b.name),
                    WidgetSortKey::Weight => a.weight.cmp(&b.weight),
                };
                match dir {
                    Direction::Asc => ord,
                    Direction::Desc => ord.reverse(),
                }
            });
        }

        let total = items.len();

        let after_index = q
            .page
            .after
            .as_deref()
            .and_then(|cursor| {
                let after_id: usize = cursor.parse().ok()?;
                items.iter().position(|w| w.id == after_id)
            })
            .map(|pos| pos + 1)
            .unwrap_or(0);

        let remaining: Vec<Widget> = if after_index < items.len() {
            items.split_off(after_index)
        } else {
            Vec::new()
        };

        let size = q.page.size.unwrap_or(10) as usize;
        let probe: Vec<Widget> = remaining.into_iter().take(size + 1).collect();

        Ok(CursorPage::from_probe(probe, size)
            .map(WidgetOut)
            .with_total(Total::Exact(total))
            .into())
    }
}

impl Create for WidgetStore {
    type Draft = WidgetDraft;
    type Created = WidgetOut;

    async fn create(&self, _ctx: (), draft: Self::Draft) -> Result<Self::Created, Error> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let widget = Widget {
            id,
            name: draft.name,
            weight: draft.weight,
        };
        self.inner.write().unwrap().insert(id, widget.clone());
        Ok(WidgetOut(widget))
    }
}

impl Update for WidgetStore {
    type Patch = WidgetPatch;
    type Updated = WidgetOut;

    async fn update(
        &self,
        _ctx: (),
        id: IdOf<Self>,
        patch: Self::Patch,
    ) -> Result<Self::Updated, Error> {
        let mut map = self.inner.write().unwrap();
        let widget = map
            .get_mut(&id)
            .ok_or_else(|| Error::new_not_found("widget not found"))?;
        if let Some(name) = patch.name {
            widget.name = name;
        }
        if let Some(weight) = patch.weight {
            widget.weight = weight;
        }
        Ok(WidgetOut(widget.clone()))
    }
}

impl Delete for WidgetStore {
    async fn delete(&self, _ctx: (), id: IdOf<Self>) -> Result<(), Error> {
        self.inner
            .write()
            .unwrap()
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| Error::new_not_found("widget not found"))
    }
}

/// Builds a full-capability test app for a fresh `WidgetStore`. A macro
/// (rather than a helper fn) because the concrete `init_service` return type
/// is unnameable (nested opaque types) as a fn return type.
macro_rules! full_app {
    ($store:expr) => {
        init_service(
            App::new().app_data(Data::new($store)).service(
                resource::<WidgetStore>()
                    .show()
                    .list()
                    .create()
                    .update()
                    .delete(),
            ),
        )
        .await
    };
}

fn content_type<B: actix_web::body::MessageBody>(resp: &actix_web::dev::ServiceResponse<B>) -> String {
    resp.headers()
        .get("content-type")
        .expect("expected a content-type header")
        .to_str()
        .unwrap()
        .to_owned()
}

// ---- list -------------------------------------------------------------

#[actix_web::test]
async fn list_both_paths_have_expected_shape() {
    let store = WidgetStore::seeded([(1, "Alice", 10), (2, "Bob", 20)]);
    let app = full_app!(store);

    for path in ["/widgets", "/widgets/"] {
        let req = TestRequest::get().uri(path).to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(content_type(&resp), MEDIA_TYPE);
        let body: serde_json::Value = read_body_json(resp).await;
        let data = body["data"].as_array().expect("data array");
        assert_eq!(data.len(), 2, "path {path}");
        for item in data {
            assert!(item["meta"]["page"]["cursor"].is_string());
        }
        assert!(body["links"]["prev"].is_null());
        assert!(body["links"]["next"].is_null());
        assert_eq!(body["meta"]["page"]["total"], 2);
    }
}

#[actix_web::test]
async fn filter_narrows_results() {
    let store = WidgetStore::seeded([
        (1, "Alice Bolt", 10),
        (2, "Bob Nut", 20),
        (3, "Alice Screw", 30),
    ]);
    let app = full_app!(store);

    let req = TestRequest::get()
        .uri("/widgets/?name[contains]=Alice")
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    for item in data {
        assert!(item["attributes"]["name"]
            .as_str()
            .unwrap()
            .contains("Alice"));
    }

    // The `next` link preserves the flat filter param alongside the cursor
    // (established convention: pagination links must not drop filters).
    let req = TestRequest::get()
        .uri("/widgets/?name[contains]=Alice&page[size]=1")
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    let next = body["links"]["next"]
        .as_str()
        .expect("expected a next link (2 Alice widgets, page size 1)");
    assert!(next.contains("name[contains]=Alice"), "next link: {next}");
}

#[actix_web::test]
async fn sort_orders_results_descending() {
    let store = WidgetStore::seeded([(1, "A", 5), (2, "B", 15), (3, "C", 10)]);
    let app = full_app!(store);

    let req = TestRequest::get().uri("/widgets/?sort=-weight").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    let weights: Vec<u64> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["attributes"]["weight"].as_u64().unwrap())
        .collect();
    assert_eq!(weights, vec![15, 10, 5]);
}

#[actix_web::test]
async fn pagination_walks_to_exhaustion() {
    let store = WidgetStore::seeded((1usize..=5).map(|i| (i, "widget", i as u32 * 10)));
    let app = full_app!(store);

    let mut seen = Vec::new();
    let mut next: Option<String> = Some("/widgets/?page[size]=2".to_owned());
    let mut pages = 0;
    while let Some(uri) = next.take() {
        pages += 1;
        assert!(pages <= 10, "pagination did not terminate");
        let req = TestRequest::get().uri(&uri).to_request();
        let resp = call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = read_body_json(resp).await;
        let data = body["data"].as_array().unwrap();
        assert!(data.len() <= 2);
        for item in data {
            seen.push(item["id"].as_str().unwrap().to_owned());
        }
        next = body["links"]["next"].as_str().map(|s| s.to_owned());
    }
    assert_eq!(seen, vec!["1", "2", "3", "4", "5"]);
}

#[actix_web::test]
async fn bad_sort_key_is_400_envelope() {
    let store = WidgetStore::seeded([(1, "A", 5)]);
    let app = full_app!(store);

    let req = TestRequest::get().uri("/widgets/?sort=bogus").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["errors"][0]["status"].as_str(), Some("400"));
}

// ---- show ---------------------------------------------------------------

#[actix_web::test]
async fn show_returns_resource_document() {
    let store = WidgetStore::seeded([(1, "Alice", 10)]);
    let app = full_app!(store);

    let req = TestRequest::get().uri("/widgets/1").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(content_type(&resp), MEDIA_TYPE);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["data"][0]["id"], "1");
    assert_eq!(body["data"][0]["type"], "widgets");
    assert_eq!(body["data"][0]["attributes"]["name"], "Alice");
}

#[actix_web::test]
async fn show_unknown_id_is_404() {
    let store = WidgetStore::seeded([(1, "Alice", 10)]);
    let app = full_app!(store);

    let req = TestRequest::get().uri("/widgets/999").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["errors"][0]["status"].as_str(), Some("404"));
}

#[actix_web::test]
async fn show_non_numeric_id_is_400() {
    let store = WidgetStore::seeded([(1, "Alice", 10)]);
    let app = full_app!(store);

    let req = TestRequest::get().uri("/widgets/abc").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---- create / update / delete -------------------------------------------

#[actix_web::test]
async fn create_responds_201_and_is_retrievable() {
    let store = WidgetStore::new();
    let app = full_app!(store);

    let payload = json!({
        "data": {
            "id": null,
            "type": "widgets",
            "attributes": { "name": "Gadget", "weight": 42 },
            "relationships": null
        }
    });
    let req = TestRequest::post()
        .uri("/widgets/")
        .set_json(&payload)
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(content_type(&resp), MEDIA_TYPE);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["data"][0]["attributes"]["name"], "Gadget");
    let id = body["data"][0]["id"].as_str().unwrap().to_owned();

    let req = TestRequest::get()
        .uri(&format!("/widgets/{id}"))
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["data"][0]["attributes"]["name"], "Gadget");
}

#[actix_web::test]
async fn create_malformed_body_is_400() {
    let store = WidgetStore::new();
    let app = full_app!(store);

    let req = TestRequest::post()
        .uri("/widgets/")
        .insert_header(("content-type", "application/json"))
        .set_payload("not json")
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[actix_web::test]
async fn update_then_delete_then_404() {
    let store = WidgetStore::seeded([(1, "Alice", 10)]);
    let app = full_app!(store);

    let payload = json!({
        "data": {
            "id": "1",
            "type": "widgets",
            "attributes": { "name": "Alice Updated", "weight": null },
            "relationships": null
        }
    });
    let req = TestRequest::patch()
        .uri("/widgets/1")
        .set_json(&payload)
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(content_type(&resp), MEDIA_TYPE);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["data"][0]["attributes"]["name"], "Alice Updated");
    assert_eq!(body["data"][0]["attributes"]["weight"], 10);

    let req = TestRequest::delete().uri("/widgets/1").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let bytes = read_body(resp).await;
    assert!(bytes.is_empty());

    let req = TestRequest::get().uri("/widgets/1").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---- Ctx plumbing via session middleware --------------------------------

#[derive(Clone, Debug, PartialEq)]
struct TestSession {
    user_id: String,
}

struct HeaderSessionFactory;

impl UserSessionFactory for HeaderSessionFactory {
    type Session = TestSession;

    fn extract(&self, req: &actix_web::dev::ServiceRequest) -> Result<Self::Session, Error> {
        req.headers()
            .get("x-user-id")
            .and_then(|v| v.to_str().ok())
            .map(|v| TestSession {
                user_id: v.to_owned(),
            })
            .ok_or_else(|| Error::new_unauthorized("missing x-user-id header"))
    }
}

struct SecuredWidgetStore {
    inner: Arc<WidgetStore>,
}

impl Store for SecuredWidgetStore {
    type Resource = WidgetResource;
    type Ctx = Session<TestSession>;
}

impl Show for SecuredWidgetStore {
    type Shown = WidgetOut;
    type Included = NoIncluded;

    async fn show(
        &self,
        _ctx: Session<TestSession>,
        id: IdOf<Self>,
        q: ShowQuery,
    ) -> Result<WithIncluded<Self::Shown, Self::Included>, Error> {
        Show::show(&*self.inner, (), id, q).await
    }
}

#[actix_web::test]
async fn ctx_plumbing_composes_with_session_middleware() {
    let base = WidgetStore::seeded([(1, "Alice", 10)]);
    let store = SecuredWidgetStore {
        inner: Arc::new(base),
    };

    let app = init_service(
        App::new()
            .app_data(Data::new(store))
            .wrap(UserSessionMiddleware::new(HeaderSessionFactory))
            .service(resource::<SecuredWidgetStore>().show()),
    )
    .await;

    let req = TestRequest::get().uri("/widgets/1").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["errors"][0]["status"].as_str(), Some("401"));

    let req = TestRequest::get()
        .uri("/widgets/1")
        .insert_header(("x-user-id", "alice"))
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(content_type(&resp), MEDIA_TYPE);
}

// ---- include support (compound documents) --------------------------------

#[derive(Debug, Clone)]
struct Owner {
    id: usize,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct OwnerAttributes {
    name: String,
}

struct OwnerOut(Owner);

impl IntoResponse for OwnerOut {
    type Attributes = OwnerAttributes;

    fn into_response(self) -> ResourceResponse<Self::Attributes> {
        ResourceResponse {
            id: Identifier {
                id: ID(self.0.id.to_string()),
                typ: "owners".to_owned(),
            },
            attributes: OwnerAttributes { name: self.0.name },
            relationships: None,
            meta: None,
        }
    }
}

/// Mirrors the real downstream pattern: an enum of every resource type a
/// widget can sideload, gated per-variant on `q.include`. In a consuming
/// crate this would be `#[derive(IntoResponse)]` on an enum (the derive
/// supports enums for exactly this); hand-implemented here since the derive
/// crate isn't a dependency of `jsonapi` itself.
enum WidgetIncluded {
    Owner(OwnerOut),
}

#[derive(Serialize)]
#[serde(untagged)]
enum WidgetIncludedAttributes {
    Owner(OwnerAttributes),
}

impl IntoResponse for WidgetIncluded {
    type Attributes = WidgetIncludedAttributes;

    fn into_response(self) -> ResourceResponse<Self::Attributes> {
        match self {
            WidgetIncluded::Owner(owner) => {
                let ResourceResponse {
                    id,
                    attributes,
                    relationships,
                    meta,
                } = owner.into_response();
                ResourceResponse {
                    id,
                    attributes: WidgetIncludedAttributes::Owner(attributes),
                    relationships,
                    meta,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct OwnedWidget {
    id: usize,
    name: String,
    weight: u32,
    owner: Owner,
}

/// A second store (distinct from `WidgetStore`) whose `Show`/`List` sideload
/// the widget's owner when `include=owner` is requested.
struct WidgetWithOwnerStore {
    inner: RwLock<BTreeMap<usize, OwnedWidget>>,
}

impl WidgetWithOwnerStore {
    fn seeded(
        widgets: impl IntoIterator<Item = (usize, &'static str, u32, usize, &'static str)>,
    ) -> Self {
        let mut map = BTreeMap::new();
        for (id, name, weight, owner_id, owner_name) in widgets {
            map.insert(
                id,
                OwnedWidget {
                    id,
                    name: name.to_owned(),
                    weight,
                    owner: Owner {
                        id: owner_id,
                        name: owner_name.to_owned(),
                    },
                },
            );
        }
        WidgetWithOwnerStore {
            inner: RwLock::new(map),
        }
    }
}

impl Store for WidgetWithOwnerStore {
    type Resource = WidgetResource;
    type Ctx = ();
}

impl Show for WidgetWithOwnerStore {
    type Shown = WidgetOut;
    type Included = WidgetIncluded;

    async fn show(
        &self,
        _ctx: (),
        id: IdOf<Self>,
        q: ShowQuery,
    ) -> Result<WithIncluded<Self::Shown, Self::Included>, Error> {
        let map = self.inner.read().unwrap();
        let widget = map
            .get(&id)
            .ok_or_else(|| Error::new_not_found("widget not found"))?;
        let shown = WidgetOut(Widget {
            id: widget.id,
            name: widget.name.clone(),
            weight: widget.weight,
        });
        let result = WithIncluded::new(shown);
        Ok(if q.include.contains("owner") {
            result.including(vec![WidgetIncluded::Owner(OwnerOut(widget.owner.clone()))])
        } else {
            result
        })
    }
}

impl List for WidgetWithOwnerStore {
    type Filter = WidgetFilter;
    type SortKey = WidgetSortKey;
    type Item = WidgetOut;
    type Included = WidgetIncluded;

    async fn list(
        &self,
        _ctx: (),
        q: ListQuery<Self::Filter, Self::SortKey>,
    ) -> Result<WithIncluded<CursorPage<Self::Item>, Self::Included>, Error> {
        let map = self.inner.read().unwrap();
        let items: Vec<OwnedWidget> = map.values().cloned().collect();
        drop(map);

        let size = q.page.size.unwrap_or(10) as usize;
        let probe: Vec<OwnedWidget> = items.into_iter().take(size + 1).collect();
        let page = CursorPage::from_probe(probe, size);

        let included = if q.include.contains("owner") {
            page.items
                .iter()
                .map(|w| WidgetIncluded::Owner(OwnerOut(w.owner.clone())))
                .collect()
        } else {
            Vec::new()
        };

        let out_page = page.map(|w| {
            WidgetOut(Widget {
                id: w.id,
                name: w.name,
                weight: w.weight,
            })
        });

        Ok(WithIncluded {
            primary: out_page,
            included,
        })
    }
}

macro_rules! owner_app {
    ($store:expr) => {
        init_service(
            App::new()
                .app_data(Data::new($store))
                .service(resource::<WidgetWithOwnerStore>().show().list()),
        )
        .await
    };
}

#[actix_web::test]
async fn show_with_include_owner_sideloads_owner() {
    let store = WidgetWithOwnerStore::seeded([(1, "Gadget", 5, 10, "Alice")]);
    let app = owner_app!(store);

    let req = TestRequest::get()
        .uri("/widgets/1?include=owner")
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    let included = body["included"].as_array().expect("included array present");
    assert_eq!(included.len(), 1);
    assert_eq!(included[0]["type"], "owners");
    assert_eq!(included[0]["id"], "10");
    assert_eq!(included[0]["attributes"]["name"], "Alice");
}

#[actix_web::test]
async fn show_without_include_omits_included_member() {
    let store = WidgetWithOwnerStore::seeded([(1, "Gadget", 5, 10, "Alice")]);
    let app = owner_app!(store);

    let req = TestRequest::get().uri("/widgets/1").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    assert!(
        body.get("included").is_none(),
        "included member should be absent (not null), got: {body}"
    );
}

#[actix_web::test]
async fn list_with_include_owner_sideloads_alongside_pagination() {
    let store = WidgetWithOwnerStore::seeded([
        (1, "Gadget", 5, 10, "Alice"),
        (2, "Sprocket", 6, 11, "Bob"),
    ]);
    let app = owner_app!(store);

    let req = TestRequest::get()
        .uri("/widgets/?include=owner&page[size]=1")
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;

    let data = body["data"].as_array().expect("data array");
    assert_eq!(data.len(), 1);

    let included = body["included"].as_array().expect("included array present");
    assert_eq!(included.len(), 1);
    assert_eq!(included[0]["type"], "owners");
    assert_eq!(included[0]["attributes"]["name"], "Alice");

    // Pagination links/cursors are unaffected by include.
    assert!(body["links"]["next"].as_str().is_some());
    assert!(data[0]["meta"]["page"]["cursor"].is_string());
}

#[actix_web::test]
async fn list_without_include_omits_included_member() {
    let store = WidgetWithOwnerStore::seeded([(1, "Gadget", 5, 10, "Alice")]);
    let app = owner_app!(store);

    let req = TestRequest::get().uri("/widgets/").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    assert!(body.get("included").is_none());
}

// ---- WithIncluded unit tests ----------------------------------------------

#[test]
fn with_included_from_wraps_primary_with_empty_included() {
    let wi: WithIncluded<i32, &str> = 5.into();
    assert_eq!(wi.primary, 5);
    assert!(wi.included.is_empty());
}

#[test]
fn with_included_new_has_empty_included() {
    let wi: WithIncluded<i32, &str> = WithIncluded::new(5);
    assert_eq!(wi.primary, 5);
    assert!(wi.included.is_empty());
}

#[test]
fn with_included_including_appends_across_calls() {
    let wi = WithIncluded::new(5).including(vec!["a", "b"]).including(vec!["c"]);
    assert_eq!(wi.primary, 5);
    assert_eq!(wi.included, vec!["a", "b", "c"]);
}

// ---- ResourceScope::route / ::service passthrough -------------------------

async fn promote_handler(store: Data<WidgetStore>, path: web::Path<usize>) -> HttpResponse {
    let id = path.into_inner();
    let exists = store.inner.read().unwrap().contains_key(&id);
    if exists {
        HttpResponse::Ok().json(json!({ "promoted": id }))
    } else {
        HttpResponse::NotFound().finish()
    }
}

#[actix_web::test]
async fn custom_route_coexists_with_generated_show() {
    let store = WidgetStore::seeded([(1, "Alice", 10)]);
    let app = init_service(
        App::new().app_data(Data::new(store)).service(
            resource::<WidgetStore>()
                .show()
                .route("/{id}/actions/promote", web::post().to(promote_handler)),
        ),
    )
    .await;

    let req = TestRequest::post()
        .uri("/widgets/1/actions/promote")
        .to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["promoted"], 1);

    // The generated show route still works alongside the custom one.
    let req = TestRequest::get().uri("/widgets/1").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["data"][0]["attributes"]["name"], "Alice");
}

async fn ping_handler() -> &'static str {
    "pong"
}

#[actix_web::test]
async fn custom_service_mounts_inside_scope() {
    let store = WidgetStore::seeded([(1, "Alice", 10)]);
    let app = init_service(
        App::new().app_data(Data::new(store)).service(
            resource::<WidgetStore>()
                .show()
                .service(web::resource("/{id}/ping").route(web::get().to(ping_handler))),
        ),
    )
    .await;

    let req = TestRequest::get().uri("/widgets/1/ping").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_body(resp).await;
    assert_eq!(body, "pong");
}

// ---- Responder impl -------------------------------------------------------

async fn plain_handler() -> Response<WidgetAttributes, ()> {
    Response::from(WidgetOut(Widget {
        id: 42,
        name: "Direct".to_owned(),
        weight: 7,
    }))
}

#[actix_web::test]
async fn responder_impl_returns_document_directly() {
    let app = init_service(
        App::new().route("/direct", actix_web::web::get().to(plain_handler)),
    )
    .await;

    let req = TestRequest::get().uri("/direct").to_request();
    let resp = call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(content_type(&resp), MEDIA_TYPE);
    let body: serde_json::Value = read_body_json(resp).await;
    assert_eq!(body["data"][0]["attributes"]["name"], "Direct");
}
