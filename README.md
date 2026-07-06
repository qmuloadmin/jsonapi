# jsonapi

Serde types for [JSON:API](https://jsonapi.org) documents, plus (optionally) a
deep actix-web integration that turns a resource definition and a
hand-written store into spec-compliant CRUD endpoints — routing, query
parsing, envelope building, pagination links, content types, and error
rendering, all owned by this crate.

## Quickstart

Define a resource with the derive macros, implement whichever operation
traits your store supports, mount it. (Lifted from `examples/src/main.rs`,
which is a complete runnable server — see below.)

```rust
#[derive(IntoResponse, ResourceType)]
#[jsonapi(name = "todos")]
struct TodoResource { id: Uuid, attributes: TodoAttributes, relations: TodoRelations }

#[derive(Serialize, Clone)]
struct TodoAttributes { title: String, done: bool }

#[derive(IntoRelationships, FromRelationships, Clone)]
struct TodoRelations {
    #[jsonapi(resource_type = "users")]
    assignee: Option<Uuid>,
}

struct TodoStore { inner: RwLock<BTreeMap<Uuid, Todo>> }

impl Store for TodoStore {
    type Resource = TodoResource;
    type Ctx = Session<User>;   // per-request session, see auth::Session
}

impl Show for TodoStore {
    type Shown = TodoResource;
    // plain async fn: Diesel, an HTTP call, whatever — your business.
    async fn show(&self, ctx: Session<User>, id: Uuid, q: ShowQuery) -> Result<Self::Shown, Error> { ... }
}
// ...List, Create, Update, Delete similarly.

HttpServer::new(move || {
    App::new()
        .app_data(store.clone())
        .wrap(UserSessionMiddleware::new(DemoUserFactory))
        .service(resource::<TodoStore>().show().list().create().update().delete())
})
.bind(("127.0.0.1", 8080))?
.run()
.await
```

Each builder method (`.show()`, `.list()`, ...) only compiles if the store
implements the matching trait — capability is an implementation, there's no
runtime "not supported" path.

## Features

| Feature | Default | Adds |
|---|---|---|
| `server` | yes | `Uuid` as a valid resource id type (`FromID for Uuid`) |
| `actixweb` | no | Operation traits (`Store`/`Show`/`List`/`Create`/`Update`/`Delete`), route mounting (`actix::resource`), body extractors, and session middleware (`auth`) |
| `diesel` | no | `diesel::result::Error` → `jsonapi::Error` mapping (`NotFound`→404, unique violation→409, FK/check violation→400, else 500) |

## URL conventions handled for you

- `filter[name][eq]=x`, `filter[name][contains]=y` — via [`StringMatch`]
- `filter[price][gte]=10&filter[price][lte]=20` — via [`Range<T>`]
- `sort=-created,name` — parses into an ordered `Vec<(K, Direction)>`; unknown
  keys are a 400, not silently ignored
- `page[size]=25&page[after]=<cursor>` — the
  [cursor-pagination profile](http://jsonapi.org/profiles/ethanresnick/cursor-pagination/):
  response `links.next`/`links.prev` and per-item `meta.page.cursor`, built
  from the `CursorPage::from_probe` (`LIMIT size+1`) idiom
- `include=author,comments.author` — parsed into path segments; whether/how
  to honor it is the store's choice
- `application/vnd.api+json` on every response (documents and errors alike)

## Philosophy

A prior attempt at this problem generified the *data-access layer* (generic
traits over Diesel tables) and left the HTTP layer — where the actual
per-resource boilerplate lived — hand-rolled 50+ times over. This crate
inverts that: the HTTP/document layer is generic (routing, extraction, query
parsing, envelopes, pagination, errors), and the data layer is a plain
`async fn` per operation, free to do whatever it needs (Diesel, transactions,
external API calls) with no trait-bound explosion. See [`DESIGN.md`](DESIGN.md)
for the full rationale and staging.

## Examples

`examples/` is a small standalone crate with a complete "todos" JSON:API
server (`cargo run --bin todo-server`) wiring the derive macros, the
operation traits, cursor pagination, and session-middleware-based
authorization together end to end. Start there.
