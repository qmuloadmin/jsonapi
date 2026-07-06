# Actix Web integration design

Goal: given a resource definition and a user-implemented *store*, generate
spec-compliant JSON:API CRUD endpoints under `/{type}/`, with typed query
parsing (filter/sort/cursor pagination/include), JSON:API-aware middleware
(session/auth), and a clean seam between the HTTP layer and data access.

## Lesson driving the design

A prior attempt at this problem (garment-service's `AutoImplRepo` /
`ActixWeb*Repository` family) generified the **data-access layer**: generic
traits over Diesel tables with blanket impls providing `create`/`read`/`update`.
It failed, observably:

- Trait-bound explosion: ~8 Diesel `where` bounds repeated 4× per operation;
  nobody dared add `Delete` or paginated search to the family.
- The sync, `web::block`-wrapped, one-changeset-in/one-row-out shape could not
  express multi-table writes, conditional cascades, external API calls
  (Stripe, other services), or transactions spanning them. Real resources
  bypassed the machinery entirely.
- Pagination was never modeled; every list endpoint hand-rolled the same
  keyset probe (`limit size+1` → `has_more` → per-item cursors → links).
- Meanwhile the HTTP layer — where the *actual* per-resource boilerplate lived
  (58 identical `match Ok/Err` handler tails, 27 identical `web::block` +
  span + error-flatten dances, per-resource pagination/link plumbing) — was
  never abstracted at all.

Conclusion: **generify the HTTP/document layer; leave data access free-form.**
The store trait methods are plain `async fn`s taking already-parsed, typed
inputs and returning domain types. What happens inside (Diesel, transactions,
Stripe calls, NATS) is entirely the implementor's business. This crate owns
everything from the socket to the store call: routing, extraction, query
parsing, envelope building, pagination links, content types, error rendering.

## Layering

```
actix route (generated)            jsonapi crate
  → extract Ctx (session), Id, Query/Document (typed)
  → call store method              user crate: Store (validation, authz rules,
                                     orchestration, transactions)
                                       → repository / Diesel / anything
  → build Response envelope         jsonapi crate (pagination links, cursors,
  → render (vnd.api+json, status)    error documents)
```

## Core vocabulary (feature-independent)

```rust
/// Identity of a resource type: its wire name and id type.
pub trait ResourceType {
    const TYPE_NAME: &'static str;          // e.g. "designs" → mounted at /designs/
    type Id: FromID;                        // path-segment parsing for /{id} routes
}
```

Pagination primitives (crate-owned versions of what every consumer hand-rolls):

```rust
/// Deserializes from `page[size]`, `page[after]`, `page[before]` (cursor profile).
pub struct PageParams { size: Option<u32>, after: Option<String>, before: Option<String> }

/// A page of items plus what's needed to build links/meta.
pub struct CursorPage<T> {
    items: Vec<T>,
    has_more: bool,
    total: Option<Total>,                   // exact or best-guess
}
impl<T> CursorPage<T> {
    /// The `limit size+1` probe idiom: pass in up-to-size+1 rows.
    pub fn from_probe(rows: Vec<T>, size: usize) -> Self;
}
```

Filter primitives (shared instead of reinvented per resource):

```rust
pub enum StringMatch { Eq(String), Contains(String) }   // name[eq]= / name[contains]=
pub struct Range<T> { eq/gte/lte/gt/lt: Option<T> }      // price[gte]=10&price[lte]=20
```

Sort: `sort=-created,name` parses into `SortSpec<K>` = ordered `Vec<(K, Direction)>`
where `K` is a user enum deriving `Deserialize` (unknown fields → 400).
`Unsorted` is provided for resources without sorting.

Include: `include=author,comments.author` parses into `IncludeSet` (path list).
Passed through to store methods; whether/how to honor it is the store's choice
(the existing `Response::include`/`include_many` builds the compound document).

## Operation traits (feature `actixweb`)

Capability-based: one trait per operation, implemented on the user's store
type (held in `web::Data<S>`). Native async-fn-in-trait — no `async_trait`,
no `Send` bounds needed (actix workers are single-threaded).

```rust
pub trait Store: 'static {
    type Resource: ResourceType;
    /// Per-request context (session). Any actix `FromRequest` whose error
    /// renders as a JSON:API error; `()` for unauthenticated APIs.
    /// `Session<T>` (below) is the standard choice.
    type Ctx: FromRequest;
}

pub trait Show: Store {
    type Shown: IntoResponse;
    async fn show(&self, ctx: Self::Ctx, id: Id<Self>, q: ShowQuery) -> Result<Self::Shown, Error>;
}

pub trait List: Store {
    type Filter: DeserializeOwned + Default;   // nested under filter[...]
    type SortKey: DeserializeOwned;            // or Unsorted
    type Item: IntoResponse;
    async fn list(&self, ctx: Self::Ctx, q: ListQuery<Self::Filter, Self::SortKey>)
        -> Result<CursorPage<Self::Item>, Error>;
    /// Cursor for one item; defaults to the resource id after conversion.
    fn cursor(item: &ResourceResponse<...>) -> String { /* id */ }
}

pub trait Create: Store {
    type Draft: FromRequest;                   // jsonapi::FromRequest (document body)
    type Created: IntoResponse;
    async fn create(&self, ctx: Self::Ctx, draft: Self::Draft) -> Result<Self::Created, Error>;
}

pub trait Update: Store {
    type Patch: FromRequest;
    type Updated: IntoResponse;
    async fn update(&self, ctx: Self::Ctx, id: Id<Self>, patch: Self::Patch) -> Result<Self::Updated, Error>;
}

pub trait Delete: Store {
    async fn delete(&self, ctx: Self::Ctx, id: Id<Self>) -> Result<(), Error>;
}
```

`ListQuery` = `{ filter, page: PageParams, sort: SortSpec<K>, include: IncludeSet }`,
parsed with `serde_qs` (form-encoding tolerant, so `%5B` from browser
`URLSearchParams` works). Filters are spec-compliant under `filter[...]`.

## Mounting

```rust
App::new()
    .app_data(Data::new(design_store))
    .service(
        jsonapi::actix::resource::<DesignStore>()   // scope "/designs"
            .show().list().create().update().delete()
    )
```

Each builder method only compiles if the store implements the matching trait
— capability = implementation, no runtime "not supported" paths. Routes:

- `GET    /{type}/`        → `List`  (200, links + per-item cursors + meta.page)
- `GET    /{type}/{id}`    → `Show`  (200)
- `POST   /{type}/`        → `Create` (201)
- `PATCH  /{type}/{id}`    → `Update` (200)
- `DELETE /{type}/{id}`    → `Delete` (204)

Trailing-slash collection routes (established convention downstream).
Responses use `application/vnd.api+json`. Errors — extraction, parse, or
store — all render as JSON:API error documents via the existing
`ResponseError` impl. Pagination `next` links are built from the request URI
with `page[after]` swapped in, preserving filter/sort params.

Escape hatch: everything the generated handlers use (extractors, `Documented`
responder below, link building) is public, so a hand-written route can live
alongside generated ones in the same scope (actions pattern,
`POST /{type}/{id}/actions/<verb>`, denormalized search endpoints).

`impl Responder for Response<P, I>` (200, vnd.api+json) so hand-written
handlers can `return Ok(Response::from(x).finish())` instead of the
`match … err.into()` tail.

## Sessions / middleware

`auth::UserSessionMiddleware` (already on this branch) extracts a session via
`UserSessionFactory` and stashes it in request extensions. New:

```rust
pub struct Session<T>(pub T);           // FromRequest: reads extensions,
                                        // 500 if middleware absent — the
                                        // per-service boilerplate, absorbed
```

`Store::Ctx = Session<MySession>` plumbs it into every operation method.
Authorization *decisions* stay in the store (role checks are domain logic);
the crate guarantees the session is present and typed. Additional middleware
(caching etc.) composes as ordinary actix middleware over the scope; it sees
JSON:API types via the same extension/extractor mechanisms.

## Relational mapping (optional descriptor layer)

JSON:API describes a graph; the graph usually lives in a relational DB. This
crate does **no** DB access, but a resource can optionally declare enough
enrichment for an adapter (e.g. a diesel helper crate) to build the graph:

```rust
pub trait MappedResource: ResourceType {
    const MAPPING: ResourceMapping;      // table + relationship mappings
}
pub struct ResourceMapping { table: &'static str, relationships: &'static [RelationshipMapping] }
pub enum RelationshipMapping {
    ToOne  { name: &'static str, fk_column: &'static str, resource_type: &'static str },
    ToMany { name: &'static str, join_table: &'static str, local_key: &'static str,
             foreign_key: &'static str, resource_type: &'static str },
}
```

Plus, under an optional `diesel` feature: the generic
`diesel::result::Error → jsonapi::Error` mapping (NotFound→404,
UniqueViolation→409, FK/Check violation→400, else 500) that every consumer
currently redefines.

## Derive support

`ResourceType` is derivable alongside the existing `IntoResponse`
(`#[jsonapi(name = "designs")]` already carries the type name). Mapping
descriptors get `#[jsonapi(table = "...", ...)]` attributes later.

## Staging

1. Module reorg (mechanical split of lib.rs; root paths preserved).
2. Query layer: `PageParams`, `CursorPage`, `SortSpec`, `IncludeSet`,
   `StringMatch`/`Range`, serde_qs wiring, link building. Unit tests.
3. Operation traits + `resource::<S>()` mounting + generated handlers +
   `Responder` impl. Actix integration tests.
4. `Session<T>` extractor + Ctx plumbing (auth.rs already has the middleware).
5. Mapping descriptors + optional diesel error mapping.
6. End-to-end example (in-memory store) + docs.
