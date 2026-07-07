//! Capability-based operation traits and route mounting for actix-web.
//!
//! A resource's data-access layer is a *store* (a plain struct held in
//! `web::Data<S>`) implementing [`Store`] plus whichever of [`Show`],
//! [`List`], [`Create`], [`Update`], [`Delete`] it supports. Each operation
//! trait is a plain `async fn` taking already-parsed, typed inputs and
//! returning domain types — what happens inside (Diesel, transactions,
//! external API calls) is entirely the implementor's business. This crate
//! owns everything from the socket to the store call: routing, extraction,
//! query parsing, envelope building, pagination links, content types, error
//! rendering. See `DESIGN.md` for the full rationale.
//!
//! Mount a store's routes with [`resource`]:
//!
//! ```ignore
//! App::new()
//!     .app_data(Data::new(design_store))
//!     .service(
//!         jsonapi::actix::resource::<DesignStore>()
//!             .show().list().create().update().delete()
//!     )
//! ```
//!
//! Each builder method only compiles if the store implements the matching
//! trait: capability is an implementation, there is no runtime "not
//! supported" path.

use std::future::Future;
use std::marker::PhantomData;

use actix_web::{
    dev::{AppService, HttpServiceFactory},
    http::StatusCode,
    web, HttpRequest, HttpResponse, HttpResponseBuilder, Route,
};
use serde::{de::DeserializeOwned, Serialize};

use crate::actix::extract::{error_document_response, JsonApi, MEDIA_TYPE};
use crate::{
    pagination_links, parse_query, CursorPage, EstimatedTotal, Error, FromID, IntoResponse,
    ListQuery, PageMeta, ResourceResponse, ResourceType, Response, ResponseMeta, ResponseType,
    ShowQuery, Total, WithIncluded, ID,
};

/// Convenient alias for the id type of a store's resource.
pub type IdOf<S> = <<S as Store>::Resource as ResourceType>::Id;

/// A handle to the data layer for one resource type, held in `web::Data<S>`.
///
/// What happens inside the operation methods — Diesel, transactions, calls to
/// other services — is entirely the implementor's business; this crate owns
/// everything between the socket and these methods.
pub trait Store: 'static {
    /// The resource type this store serves, identifying the wire type name
    /// and id parsing.
    type Resource: ResourceType;
    /// Per-request context, e.g. `auth::Session<MySession>`; `()` when none.
    /// Its extraction error is rendered by its own `ResponseError` impl (for
    /// `auth::Session<T>`, a missing-middleware 500 or, more commonly, the
    /// 401/403 produced by the session-extracting middleware upstream).
    type Ctx: actix_web::FromRequest;
}

/// Capability: `GET /{type}/{id}`.
pub trait Show: Store {
    /// The domain type returned by a successful `show`, converted to a
    /// JSON:API resource document via [`IntoResponse`].
    type Shown: IntoResponse;
    /// The type of resources this store can sideload into `included`.
    /// Typically a hand- or derive-built `enum` when more than one resource
    /// type can be sideloaded (`#[derive(IntoResponse)]` supports enums for
    /// exactly this — each variant becomes a possible included resource
    /// type); [`crate::NoIncluded`] for stores that never sideload anything.
    type Included: IntoResponse;

    /// Fetch a single resource by id.
    ///
    /// `ctx` is the per-request context (e.g. session); `q` carries the
    /// parsed `include` query parameter. Authorization decisions (e.g.
    /// "does `ctx` have access to this resource") are this method's
    /// responsibility — the crate only guarantees `ctx` is present and typed.
    ///
    /// Relationship linkage (the `relationships` member's resource
    /// identifiers) should always be returned regardless of `q.include`;
    /// `q.include` only gates whether the *full* sideloaded resources are
    /// also returned via [`WithIncluded::including`] (established downstream
    /// convention: linkage is cheap and always useful, sideloaded resources
    /// cost an extra fetch and are opt-in).
    fn show(
        &self,
        ctx: Self::Ctx,
        id: IdOf<Self>,
        q: ShowQuery,
    ) -> impl Future<Output = Result<WithIncluded<Self::Shown, Self::Included>, Error>>;
}

/// Capability: `GET /{type}/`.
pub trait List: Store {
    /// Resource-specific filter type, deserialized FLAT from the top-level
    /// query string by platform convention (e.g. `is_hidden=false`,
    /// `name[contains]=shirt`) — see [`crate::ListQuery::parse`]. The names
    /// `page`, `sort`, and `include` are reserved (parsed separately into
    /// [`ListQuery`]'s other fields) and must not be used as filter field
    /// names. Spec-style nesting under `filter[...]` is still available,
    /// opt-in, by giving your filter type a single `filter: Inner` field.
    /// A filter field absent from the query string falls back to its own
    /// serde default, so make filter fields `Option`/`#[serde(default)]`
    /// unless you want their absence to be a 400.
    type Filter: DeserializeOwned;
    /// Resource-specific sort key enum, deserialized from `sort=...`. Use
    /// [`crate::Unsorted`] for resources that don't support sorting.
    type SortKey: DeserializeOwned;
    /// The domain type of one list item, converted to a JSON:API resource
    /// document via [`IntoResponse`].
    type Item: IntoResponse;
    /// The type of resources this store can sideload into `included`.
    /// Typically a hand- or derive-built `enum` when more than one resource
    /// type can be sideloaded (`#[derive(IntoResponse)]` supports enums for
    /// exactly this); [`crate::NoIncluded`] for stores that never sideload
    /// anything.
    type Included: IntoResponse;

    /// Fetch one page of results.
    ///
    /// `q` carries the parsed filter/page/sort/include query parameters. The
    /// returned [`CursorPage`] should be built with [`CursorPage::from_probe`]
    /// (the "fetch `size + 1` rows" idiom) so `has_more` is known without a
    /// separate count query; attach a [`Total`] via
    /// [`CursorPage::with_total`] if the store can cheaply produce one.
    ///
    /// Relationship linkage should always be returned regardless of
    /// `q.include`; `q.include` only gates whether the full sideloaded
    /// resources are also returned via [`WithIncluded::including`]
    /// (established downstream convention).
    fn list(
        &self,
        ctx: Self::Ctx,
        q: ListQuery<Self::Filter, Self::SortKey>,
    ) -> impl Future<Output = Result<WithIncluded<CursorPage<Self::Item>, Self::Included>, Error>>;

    /// Cursor stamped on each item (`meta.page.cursor`) and used for the
    /// `page[after]` next link. Defaults to the item's resource id.
    fn item_cursor(item: &ResourceResponse<<Self::Item as IntoResponse>::Attributes>) -> String {
        item.id.id.0.clone()
    }
}

/// Capability: `POST /{type}/`.
pub trait Create: Store {
    /// The request body shape, parsed from a JSON:API document via
    /// [`crate::FromRequest`].
    type Draft: crate::FromRequest;
    /// The domain type returned by a successful `create`, converted to a
    /// JSON:API resource document via [`IntoResponse`].
    type Created: IntoResponse;

    /// Create a resource from a validated draft.
    fn create(
        &self,
        ctx: Self::Ctx,
        draft: Self::Draft,
    ) -> impl Future<Output = Result<Self::Created, Error>>;
}

/// Capability: `PATCH /{type}/{id}`.
pub trait Update: Store {
    /// The request body shape, parsed from a JSON:API document via
    /// [`crate::FromRequest`].
    type Patch: crate::FromRequest;
    /// The domain type returned by a successful `update`, converted to a
    /// JSON:API resource document via [`IntoResponse`].
    type Updated: IntoResponse;

    /// Apply a patch to an existing resource.
    fn update(
        &self,
        ctx: Self::Ctx,
        id: IdOf<Self>,
        patch: Self::Patch,
    ) -> impl Future<Output = Result<Self::Updated, Error>>;
}

/// Capability: `DELETE /{type}/{id}`.
pub trait Delete: Store {
    /// Delete a resource by id.
    fn delete(&self, ctx: Self::Ctx, id: IdOf<Self>) -> impl Future<Output = Result<(), Error>>;
}

/// Serialize `body` into a `200`-family JSON:API document response; falls
/// back to a JSON:API 500 error document if serialization itself fails
/// (which should not happen for well-formed `Attributes` types).
fn document<T: Serialize>(status: StatusCode, body: &T) -> HttpResponse {
    match serde_json::to_string(body) {
        Ok(s) => HttpResponseBuilder::new(status)
            .content_type(MEDIA_TYPE)
            .body(s),
        Err(_) => error_document_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &Response::from(Error::new_internal_error(
                "failed to serialize response document",
            )),
        ),
    }
}

async fn show_handler<S>(
    store: web::Data<S>,
    ctx: S::Ctx,
    path: web::Path<String>,
    req: HttpRequest,
) -> Result<HttpResponse, Error>
where
    S: Show,
    <S::Shown as IntoResponse>::Attributes: Serialize,
    <S::Included as IntoResponse>::Attributes: Serialize,
{
    let id = <IdOf<S> as FromID>::from_id(ID(path.into_inner()))?;
    let q: ShowQuery = parse_query(req.query_string())?;
    let WithIncluded { primary, included } = store.show(ctx, id, q).await?;
    let response: Response<_, <S::Included as IntoResponse>::Attributes> =
        Response::from(primary);
    let response = if included.is_empty() {
        response
    } else {
        response.include_many(included)
    };
    Ok(document(StatusCode::OK, &response))
}

async fn list_handler<S>(
    store: web::Data<S>,
    ctx: S::Ctx,
    req: HttpRequest,
) -> Result<HttpResponse, Error>
where
    S: List,
    <S::Item as IntoResponse>::Attributes: Serialize,
    <S::Included as IntoResponse>::Attributes: Serialize,
{
    let q = ListQuery::<S::Filter, S::SortKey>::parse(req.query_string())?;
    let WithIncluded {
        primary: page,
        included,
    } = store.list(ctx, q).await?;
    let has_more = page.has_more;
    let total = page.total;

    let response: Response<_, <S::Included as IntoResponse>::Attributes> =
        Response::from(page.items).with_item_cursors(S::item_cursor);
    let response = if included.is_empty() {
        response
    } else {
        response.include_many(included)
    };

    let last_cursor = match &response.primary {
        ResponseType::Ok(items) => items
            .last()
            .and_then(|item| item.meta.as_ref())
            .map(|m| m.page.cursor.clone()),
        ResponseType::Error(_) => None,
    };

    let links = pagination_links(req.path(), req.query_string(), last_cursor.as_deref(), has_more);
    let meta = match total {
        Some(Total::Exact(n)) => Some(ResponseMeta {
            page: PageMeta {
                total: Some(n),
                estimated_total: None,
                range_truncated: None,
            },
        }),
        Some(Total::Estimated(n)) => Some(ResponseMeta {
            page: PageMeta {
                total: None,
                estimated_total: Some(EstimatedTotal { best_guess: n }),
                range_truncated: None,
            },
        }),
        None => None,
    };

    let response = response.paginate(links, meta);
    Ok(document(StatusCode::OK, &response))
}

async fn create_handler<S>(
    store: web::Data<S>,
    ctx: S::Ctx,
    draft: JsonApi<S::Draft>,
) -> Result<HttpResponse, Error>
where
    S: Create,
    <S::Draft as crate::FromRequest>::Attributes: DeserializeOwned,
    <S::Created as IntoResponse>::Attributes: Serialize,
{
    let created = store.create(ctx, draft.into_inner()).await?;
    Ok(document(StatusCode::CREATED, &Response::<_, ()>::from(created)))
}

async fn update_handler<S>(
    store: web::Data<S>,
    ctx: S::Ctx,
    path: web::Path<String>,
    patch: JsonApi<S::Patch>,
) -> Result<HttpResponse, Error>
where
    S: Update,
    <S::Patch as crate::FromRequest>::Attributes: DeserializeOwned,
    <S::Updated as IntoResponse>::Attributes: Serialize,
{
    let id = <IdOf<S> as FromID>::from_id(ID(path.into_inner()))?;
    let updated = store.update(ctx, id, patch.into_inner()).await?;
    Ok(document(StatusCode::OK, &Response::<_, ()>::from(updated)))
}

async fn delete_handler<S>(
    store: web::Data<S>,
    ctx: S::Ctx,
    path: web::Path<String>,
) -> Result<HttpResponse, Error>
where
    S: Delete,
{
    let id = <IdOf<S> as FromID>::from_id(ID(path.into_inner()))?;
    store.delete(ctx, id).await?;
    Ok(HttpResponseBuilder::new(StatusCode::NO_CONTENT).finish())
}

/// Route bundle for one resource type, mounted at `/{TYPE_NAME}`.
///
/// Built with [`resource`]; enable operations with [`ResourceScope::show`],
/// [`ResourceScope::list`], [`ResourceScope::create`], [`ResourceScope::update`],
/// [`ResourceScope::delete`]. Only operations whose trait the store implements
/// can be enabled — a capability is an implementation, there is no runtime
/// "not supported" path. Mount with `App::service`/`Scope::service`.
pub struct ResourceScope<S> {
    scope: actix_web::Scope,
    _store: PhantomData<fn() -> S>,
}

/// Begin a [`ResourceScope`] for `S`, mounted at `/{S::Resource::TYPE_NAME}`.
/// Chain builder methods (`.show()`, `.list()`, ...) to enable operations.
pub fn resource<S: Store>() -> ResourceScope<S> {
    ResourceScope {
        scope: actix_web::web::scope(&format!("/{}", <S::Resource as ResourceType>::TYPE_NAME)),
        _store: PhantomData,
    }
}

impl<S: Store> ResourceScope<S> {
    /// Mount `GET /{id}` to `S::show`.
    pub fn show(mut self) -> Self
    where
        S: Show,
        <S::Shown as IntoResponse>::Attributes: Serialize,
        <S::Included as IntoResponse>::Attributes: Serialize,
    {
        self.scope = self.scope.route("/{id}", web::get().to(show_handler::<S>));
        self
    }

    /// Mount `GET ""` and `GET "/"` to `S::list`.
    pub fn list(mut self) -> Self
    where
        S: List,
        <S::Item as IntoResponse>::Attributes: Serialize,
        <S::Included as IntoResponse>::Attributes: Serialize,
    {
        self.scope = self
            .scope
            .route("", web::get().to(list_handler::<S>))
            .route("/", web::get().to(list_handler::<S>));
        self
    }

    /// Mount a custom route inside this resource's scope, at a path relative
    /// to the scope's prefix (e.g. `"/{id}/actions/verify"` for the actions
    /// pattern).
    ///
    /// Exists because actix does not fall through between two scopes sharing
    /// a prefix: a hand-written route for this resource (an actions
    /// endpoint, a denormalized read) has to be mounted inside the *same*
    /// scope as the generated routes, or it will never be reached whenever
    /// the generated scope also matches the prefix.
    pub fn route(mut self, path: &str, route: Route) -> Self {
        self.scope = self.scope.route(path, route);
        self
    }

    /// Mount any [`HttpServiceFactory`] inside this resource's scope, for
    /// when a single `.route(...)` isn't enough (e.g. a sub-scope of related
    /// hand-written endpoints). See [`ResourceScope::route`] for why this
    /// needs to share the scope rather than being mounted alongside it.
    pub fn service(mut self, factory: impl HttpServiceFactory + 'static) -> Self {
        self.scope = self.scope.service(factory);
        self
    }

    /// Mount `POST ""` and `POST "/"` to `S::create`, responding `201 Created`.
    pub fn create(mut self) -> Self
    where
        S: Create,
        <S::Draft as crate::FromRequest>::Attributes: DeserializeOwned,
        <S::Created as IntoResponse>::Attributes: Serialize,
    {
        self.scope = self
            .scope
            .route("", web::post().to(create_handler::<S>))
            .route("/", web::post().to(create_handler::<S>));
        self
    }

    /// Mount `PATCH /{id}` to `S::update`.
    pub fn update(mut self) -> Self
    where
        S: Update,
        <S::Patch as crate::FromRequest>::Attributes: DeserializeOwned,
        <S::Updated as IntoResponse>::Attributes: Serialize,
    {
        self.scope = self
            .scope
            .route("/{id}", web::patch().to(update_handler::<S>));
        self
    }

    /// Mount `DELETE /{id}` to `S::delete`, responding `204 No Content`.
    pub fn delete(mut self) -> Self
    where
        S: Delete,
    {
        self.scope = self
            .scope
            .route("/{id}", web::delete().to(delete_handler::<S>));
        self
    }
}

impl<S: Store> HttpServiceFactory for ResourceScope<S> {
    fn register(self, config: &mut AppService) {
        self.scope.register(config)
    }
}

#[cfg(test)]
mod tests;
