use serde_derive::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Display};
#[cfg(feature = "server")]
use uuid::Uuid;

use crate::error::Error;
use crate::resource::IntoResponse;

/// Profile URI for the JSON:API cursor-based pagination profile.
pub const CURSOR_PAGINATION_PROFILE: &str =
    "http://jsonapi.org/profiles/ethanresnick/cursor-pagination/";

#[derive(Serialize, Deserialize)]
pub struct ResourceResponse<D> {
    #[serde(flatten)]
    pub id: Identifier,
    pub attributes: D,
    pub relationships: Option<BTreeMap<String, RelationshipData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<ResourceMeta>,
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

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// Pagination links for cursor-based pagination.
///
/// `prev` and `next` are required by the profile but may be `None` (serialized as `null`)
/// when there is no previous or next page. `first` and `last` are optional.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PaginationLinks {
    pub prev: Option<String>,
    pub next: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

/// Estimated total count with a best-guess value.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EstimatedTotal {
    #[serde(rename = "bestGuess")]
    pub best_guess: usize,
}

/// Page-level metadata for cursor-based pagination, placed at `meta.page` in the response.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PageMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "estimatedTotal")]
    pub estimated_total: Option<EstimatedTotal>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "rangeTruncated")]
    pub range_truncated: Option<bool>,
}

/// Top-level `meta` object wrapping the `page` pagination metadata.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResponseMeta {
    pub page: PageMeta,
}

/// Per-item pagination metadata containing the cursor for a single resource.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ItemPageMeta {
    pub cursor: String,
}

/// Per-item `meta` object wrapping cursor pagination info.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResourceMeta {
    pub page: ItemPageMeta,
}

#[derive(Serialize, Deserialize)]
pub struct Response<P, I> {
    #[serde(flatten)]
    pub primary: ResponseType<P>,
    pub included: Option<Vec<ResourceResponse<I>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<PaginationLinks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<ResponseMeta>,
}

impl<P, I> Response<P, I> {
    pub fn paginate(mut self, links: PaginationLinks, meta: Option<ResponseMeta>) -> Self {
        self.links = Some(links);
        self.meta = meta;
        self
    }

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

    /// Stamp each primary-data item's per-item pagination meta (`meta.page.cursor`)
    /// using the given function. No-op on error responses.
    pub fn with_item_cursors(mut self, f: impl Fn(&ResourceResponse<P>) -> String) -> Self {
        if let ResponseType::Ok(ref mut items) = self.primary {
            for item in items.iter_mut() {
                let cursor = f(item);
                item.meta = Some(ResourceMeta {
                    page: ItemPageMeta { cursor },
                });
            }
        }
        self
    }

    /// Default cursor: the item's resource id.
    pub fn with_id_cursors(self) -> Self {
        self.with_item_cursors(|item| item.id.id.0.clone())
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

impl<R: IntoResponse, I> From<R> for Response<R::Attributes, I> {
    fn from(r: R) -> Self {
        Response {
            primary: ResponseType::Ok(vec![r.into_response()]),
            included: None,
            links: None,
            meta: None,
        }
    }
}

impl<R: IntoResponse, I> From<Vec<R>> for Response<R::Attributes, I> {
    fn from(v: Vec<R>) -> Self {
        let data = v.into_iter().map(|each| each.into_response()).collect();
        Response {
            primary: ResponseType::Ok(data),
            included: None,
            links: None,
            meta: None,
        }
    }
}

impl From<Error> for Response<(), ()> {
    fn from(e: Error) -> Self {
        Response {
            primary: ResponseType::Error(vec![e]),
            included: None,
            links: None,
            meta: None,
        }
    }
}

impl From<Vec<Error>> for Response<(), ()> {
    fn from(v: Vec<Error>) -> Self {
        Response {
            primary: ResponseType::Error(v),
            included: None,
            links: None,
            meta: None,
        }
    }
}
