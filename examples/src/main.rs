//! A small but complete JSON:API "todos" server, demonstrating the full
//! stack working together: derive macros (`IntoResponse`, `ResourceType`,
//! `IntoRelationships`, `FromRelationships`, `FromRequest`) producing types
//! that plug straight into the `actix` operation traits (`Store`, `Show`,
//! `List`, `Create`, `Update`, `Delete`) and `resource::<S>()` mounting, with
//! session middleware (`auth::UserSessionMiddleware`) plumbing a fake "logged
//! in user" into every store method.
//!
//! Run with `cargo run --bin todo-server` and see the printed curl commands.

use std::collections::BTreeMap;
use std::sync::RwLock;

use actix_web::{dev::ServiceRequest, web, App, HttpServer};
use jsonapi::actix::ops::{Create, Delete, IdOf, List, Show, Store, Update};
use jsonapi::actix::resource;
use jsonapi::auth::{Session, UserSessionFactory, UserSessionMiddleware};
use jsonapi::{CursorPage, Direction, Error, ListQuery, NoIncluded, ShowQuery, StringMatch, Total, WithIncluded};
use jsonapi_resource_derive::{FromRelationships, FromRequest, IntoRelationships, IntoResponse, ResourceType};
use serde_derive::{Deserialize, Serialize};
use uuid::Uuid;

// ---- domain type ------------------------------------------------------------

#[derive(Debug, Clone)]
struct Todo {
    id: Uuid,
    title: String,
    done: bool,
    assignee: Option<Uuid>,
}

// ---- wire types (derived) ----------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct TodoAttributes {
    title: String,
    done: bool,
}

/// Shared by both directions: `IntoRelationships` (building the `relationships`
/// object on a response) and `FromRelationships` (parsing it back out of a
/// request document for create/patch).
#[derive(Clone, IntoRelationships, FromRelationships)]
struct TodoRelations {
    #[jsonapi(resource_type = "users")]
    assignee: Option<Uuid>,
}

#[derive(IntoResponse, ResourceType)]
#[jsonapi(name = "todos")]
struct TodoResource {
    id: Uuid,
    attributes: TodoAttributes,
    relations: TodoRelations,
}

fn to_resource(t: Todo) -> TodoResource {
    TodoResource {
        id: t.id,
        attributes: TodoAttributes {
            title: t.title,
            done: t.done,
        },
        relations: TodoRelations { assignee: t.assignee },
    }
}

/// Create draft: deliberately has NO `id` field. The derived `FromRequest`
/// rejects any request document that carries an `id` when the target type
/// lacks one (clients don't get to pick ids for created todos here).
#[derive(FromRequest)]
struct TodoDraft {
    attributes: TodoDraftAttributes,
    relations: TodoRelations,
}

#[derive(Clone, Deserialize)]
struct TodoDraftAttributes {
    title: String,
    #[serde(default)]
    done: bool,
}

/// Patch: JSON:API PATCH documents carry the resource's `id`, so this type
/// has one; the derived `FromRequest` requires it to be present in the body
/// (the value itself isn't consulted — the URL segment is authoritative and
/// is what gets passed to `Update::update`).
#[derive(FromRequest)]
struct TodoPatch {
    // Required by the derived `FromRequest` (the body must carry an id), but
    // not otherwise consulted: the URL segment is authoritative.
    #[allow(dead_code)]
    id: Uuid,
    attributes: TodoPatchAttributes,
    relations: TodoRelations,
}

#[derive(Clone, Deserialize)]
struct TodoPatchAttributes {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    done: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct TodoFilter {
    done: Option<bool>,
    title: Option<StringMatch>,
}

fn matches_filter(t: &Todo, f: &TodoFilter) -> bool {
    if let Some(done) = f.done {
        if t.done != done {
            return false;
        }
    }
    if let Some(title) = &f.title {
        let matches = match title {
            StringMatch::Eq(v) => &t.title == v,
            StringMatch::Contains(v) => t.title.contains(v.as_str()),
        };
        if !matches {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TodoSortKey {
    Title,
}

// ---- fake auth (demo only) --------------------------------------------------

#[derive(Debug, Clone)]
struct User {
    id: Uuid,
    #[allow(dead_code)]
    name: String,
    is_admin: bool,
}

const ALICE_ID: Uuid = Uuid::from_u128(1);
const BOB_ID: Uuid = Uuid::from_u128(2);
const ADMIN_ID: Uuid = Uuid::from_u128(3);

/// DEMO-ONLY "authentication": trusts a plaintext `x-user` header naming one
/// of a fixed set of known users. A real service would verify a session
/// cookie, JWT, or similar in `extract`; this exists purely to demonstrate
/// `UserSessionFactory` wiring end to end.
struct DemoUserFactory;

impl UserSessionFactory for DemoUserFactory {
    type Session = User;

    fn extract(&self, req: &ServiceRequest) -> Result<User, Error> {
        let name = req
            .headers()
            .get("x-user")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Error::new_unauthorized("missing x-user header (demo auth only)"))?;
        match name {
            "alice" => Ok(User {
                id: ALICE_ID,
                name: "alice".to_owned(),
                is_admin: false,
            }),
            "bob" => Ok(User {
                id: BOB_ID,
                name: "bob".to_owned(),
                is_admin: false,
            }),
            "admin" => Ok(User {
                id: ADMIN_ID,
                name: "admin".to_owned(),
                is_admin: true,
            }),
            other => Err(Error::new_unauthorized(&format!(
                "unknown demo user '{other}': use alice, bob, or admin"
            ))),
        }
    }
}

// ---- store -------------------------------------------------------------------

struct TodoStore {
    inner: RwLock<BTreeMap<Uuid, Todo>>,
}

impl TodoStore {
    fn seeded() -> Self {
        let mut map = BTreeMap::new();
        for (id, title, done, assignee) in [
            (Uuid::from_u128(101), "Write proposal", false, Some(ALICE_ID)),
            (Uuid::from_u128(102), "Review PR", true, Some(BOB_ID)),
            (Uuid::from_u128(103), "Water plants", false, None),
        ] {
            map.insert(
                id,
                Todo {
                    id,
                    title: title.to_owned(),
                    done,
                    assignee,
                },
            );
        }
        TodoStore {
            inner: RwLock::new(map),
        }
    }
}

impl Store for TodoStore {
    type Resource = TodoResource;
    type Ctx = Session<User>;
}

impl Show for TodoStore {
    type Shown = TodoResource;
    type Included = NoIncluded;

    async fn show(
        &self,
        _ctx: Session<User>,
        id: IdOf<Self>,
        _q: ShowQuery,
    ) -> Result<WithIncluded<Self::Shown, Self::Included>, Error> {
        self.inner
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .map(to_resource)
            .map(Into::into)
            .ok_or_else(|| Error::new_not_found("todo not found"))
    }
}

impl List for TodoStore {
    type Filter = TodoFilter;
    type SortKey = TodoSortKey;
    type Item = TodoResource;
    type Included = NoIncluded;

    async fn list(
        &self,
        _ctx: Session<User>,
        q: ListQuery<Self::Filter, Self::SortKey>,
    ) -> Result<WithIncluded<CursorPage<Self::Item>, Self::Included>, Error> {
        let map = self.inner.read().unwrap();
        let mut items: Vec<Todo> = map
            .values()
            .filter(|t| matches_filter(t, &q.filter))
            .cloned()
            .collect();
        drop(map);

        // Stable multi-key sort: apply least-significant key first so the
        // most-significant key (first in the parsed spec) dominates.
        for (key, dir) in q.sort.iter().collect::<Vec<_>>().into_iter().rev() {
            items.sort_by(|a, b| {
                let ord = match key {
                    TodoSortKey::Title => a.title.cmp(&b.title),
                };
                match dir {
                    Direction::Asc => ord,
                    Direction::Desc => ord.reverse(),
                }
            });
        }

        let total = items.len();

        // `page[after]` is the last-seen item's id; find it in the
        // (post-filter, post-sort) list and take everything after it.
        let after_index = q
            .page
            .after
            .as_deref()
            .and_then(|cursor| {
                let after_id = Uuid::parse_str(cursor).ok()?;
                items.iter().position(|t| t.id == after_id)
            })
            .map(|pos| pos + 1)
            .unwrap_or(0);

        let remaining: Vec<Todo> = if after_index < items.len() {
            items.split_off(after_index)
        } else {
            Vec::new()
        };

        let size = q.page.size.unwrap_or(10) as usize;
        let probe: Vec<Todo> = remaining.into_iter().take(size + 1).collect();

        Ok(CursorPage::from_probe(probe, size)
            .map(to_resource)
            .with_total(Total::Exact(total))
            .into())
    }
}

impl Create for TodoStore {
    type Draft = TodoDraft;
    type Created = TodoResource;

    async fn create(&self, _ctx: Session<User>, draft: Self::Draft) -> Result<Self::Created, Error> {
        let id = Uuid::new_v4();
        let todo = Todo {
            id,
            title: draft.attributes.title,
            done: draft.attributes.done,
            assignee: draft.relations.assignee,
        };
        self.inner.write().unwrap().insert(id, todo.clone());
        Ok(to_resource(todo))
    }
}

impl Update for TodoStore {
    type Patch = TodoPatch;
    type Updated = TodoResource;

    async fn update(&self, _ctx: Session<User>, id: IdOf<Self>, patch: Self::Patch) -> Result<Self::Updated, Error> {
        let mut map = self.inner.write().unwrap();
        let todo = map
            .get_mut(&id)
            .ok_or_else(|| Error::new_not_found("todo not found"))?;
        if let Some(title) = patch.attributes.title {
            todo.title = title;
        }
        if let Some(done) = patch.attributes.done {
            todo.done = done;
        }
        if let Some(assignee) = patch.relations.assignee {
            todo.assignee = Some(assignee);
        }
        Ok(to_resource(todo.clone()))
    }
}

impl Delete for TodoStore {
    // Authorization lives here, in the store: only the todo's assignee or an
    // admin user may delete it. The crate only guarantees `ctx` is present
    // and typed; this decision is entirely domain logic.
    async fn delete(&self, ctx: Session<User>, id: IdOf<Self>) -> Result<(), Error> {
        let mut map = self.inner.write().unwrap();
        let todo = map
            .get(&id)
            .ok_or_else(|| Error::new_not_found("todo not found"))?;
        let user = ctx.into_inner();
        if !user.is_admin && todo.assignee != Some(user.id) {
            return Err(Error::new_forbidden(
                "only the assignee or an admin may delete this todo",
            ));
        }
        map.remove(&id);
        Ok(())
    }
}

// ---- main ---------------------------------------------------------------

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let store = web::Data::new(TodoStore::seeded());

    println!("todo-server listening on http://127.0.0.1:8080");
    println!();
    println!("Try (fake auth: x-user is one of alice, bob, admin):");
    println!("  curl -H 'x-user: alice' http://127.0.0.1:8080/todos/");
    println!("  curl -H 'x-user: alice' 'http://127.0.0.1:8080/todos/?filter[done]=false&sort=title'");
    println!("  curl -H 'x-user: alice' 'http://127.0.0.1:8080/todos/?page[size]=2'");
    println!("  curl -H 'x-user: alice' http://127.0.0.1:8080/todos/00000000-0000-0000-0000-000000000065");
    println!(
        "  curl -H 'x-user: alice' -H 'Content-Type: application/json' -X POST http://127.0.0.1:8080/todos/ \\\n       -d '{{\"data\":{{\"type\":\"todos\",\"attributes\":{{\"title\":\"Buy milk\"}}}}}}'"
    );
    println!(
        "  curl -H 'x-user: alice' -H 'Content-Type: application/json' -X PATCH http://127.0.0.1:8080/todos/<id> \\\n       -d '{{\"data\":{{\"id\":\"<id>\",\"type\":\"todos\",\"attributes\":{{\"done\":true}}}}}}'"
    );
    println!("  curl -H 'x-user: alice' -X DELETE http://127.0.0.1:8080/todos/<id>");
    println!("  curl http://127.0.0.1:8080/todos/            # no x-user -> 401 envelope");
    println!("  curl -H 'x-user: alice' 'http://127.0.0.1:8080/todos/?sort=bogus'  # -> 400 envelope");
    println!();

    HttpServer::new(move || {
        App::new()
            .app_data(store.clone())
            .wrap(UserSessionMiddleware::new(DemoUserFactory))
            .service(
                resource::<TodoStore>()
                    .show()
                    .list()
                    .create()
                    .update()
                    .delete(),
            )
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
