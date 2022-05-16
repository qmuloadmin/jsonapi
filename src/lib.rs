use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct ResourceResponse<D> {
    #[serde(flatten)]
    pub id: Identifier,
    pub attributes: D,
    pub relationships: Option<BTreeMap<String, RelationshipData>>,
}

pub type ID = String;

#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
pub struct RelationshipData {
    data: Relationship,
}

#[derive(Serialize, Deserialize)]
pub struct Identifier {
    pub id: ID,
    #[serde(rename = "type")]
    pub typ: String,
}

#[derive(Deserialize)]
pub struct ResourceRequest<D> {
    pub id: Option<ID>,
    #[serde(rename = "type")]
    pub typ: String,
    pub attributes: D,
}

#[derive(Deserialize)]
pub struct Request<D> {
    pub data: ResourceRequest<D>,
}

#[derive(Serialize)]
pub struct Response<D> {
    #[serde(flatten)]
    primary: ResponseType<D>,
}

#[derive(Serialize)]
pub enum ResponseType<D> {
    #[serde(rename = "data")]
    Ok(Vec<ResourceResponse<D>>),
    #[serde(rename = "errors")]
    Error(Vec<Error>),
}

#[derive(Serialize)]
pub struct Error {
    // should be numeric but represented as a string
    pub status: String,
    // this is a human readable code, not a numeric code (that is status, above)
    pub code: Option<String>,
    pub title: String,
    pub detail: Option<String>,
}

impl Error {
    pub fn new_not_found(title: &str) -> Self {
        Error {
            status: "404".to_owned(),
            code: Some("Not Found".to_owned()),
            title: title.to_owned(),
            detail: None,
        }
    }
    pub fn new_bad_request(title: &str) -> Self {
        Error {
            status: "400".to_owned(),
            code: Some("Bad Request".to_owned()),
            title: title.to_owned(),
            detail: None,
        }
    }
}

pub trait Resource {
    type Attributes;
    type Relations;

    fn name(&self) -> String;
    fn id(&self) -> ID;
    fn attributes(&self) -> Self::Attributes;
    fn relations(&self) -> Self::Relations;
}

pub trait Relations {
    fn relationships(&self) -> Option<BTreeMap<String, RelationshipData>>;
}

impl Relations for () {
    fn relationships(&self) -> Option<BTreeMap<String, RelationshipData>> {
        None
    }
}

pub trait Relatable {
    fn into_relation(&self, resource_name: &str) -> Relationship;
}

impl Relatable for ID {
    fn into_relation(&self, resource_name: &str) -> Relationship {
        Relationship::ToOne(Identifier {
            id: self.clone(),
            typ: resource_name.to_string(),
        })
    }
}

impl Relatable for Vec<ID> {
    fn into_relation(&self, resource_name: &str) -> Relationship {
        Relationship::ToMany(
            self.into_iter()
                .map(|each| Identifier {
                    id: each.clone(),
                    typ: resource_name.to_string(),
                })
                .collect(),
        )
    }
}

impl<R: Resource> From<R> for Response<R::Attributes>
where
    R::Relations: Relations,
{
    fn from(r: R) -> Self {
        Response {
            primary: ResponseType::Ok(vec![ResourceResponse {
                id: Identifier {
                    id: r.id(),
                    typ: r.name(),
                },
                attributes: r.attributes(),
                relationships: r.relations().relationships(),
            }]),
        }
    }
}

impl<R: Resource> From<Vec<R>> for Response<R::Attributes>
where
    R::Relations: Relations,
{
    fn from(v: Vec<R>) -> Self {
        let data = v
            .into_iter()
            .map(|each| ResourceResponse {
                id: Identifier {
                    id: each.id(),
                    typ: each.name(),
                },
                attributes: each.attributes(),
                relationships: each.relations().relationships(),
            })
            .collect();
        Response {
            primary: ResponseType::Ok(data),
        }
    }
}

impl From<Error> for Response<()> {
    fn from(e: Error) -> Self {
        Response {
            primary: ResponseType::Error(vec![e]),
        }
    }
}

impl From<Vec<Error>> for Response<()> {
    fn from(v: Vec<Error>) -> Self {
        Response {
            primary: ResponseType::Error(v),
        }
    }
}
