use std::collections::BTreeMap;
#[cfg(feature = "server")]
use uuid::Uuid;

use crate::document::{Identifier, Relationship, RelationshipData, Request, ResourceResponse, Response, ID};
use crate::error::Error;

pub trait Resource {
    type Attributes;
    type Relations;

    fn type_name() -> &'static str;

    fn into_response(self) -> Response<Self::Attributes, Self::Relations>;
}

pub trait FromID
where
    Self: Sized,
{
    fn from_id(id: ID) -> Result<Self, Error>;
}

impl FromID for String {
    fn from_id(id: ID) -> Result<Self, Error> {
        Ok(id.0)
    }
}

impl FromID for usize {
    fn from_id(id: ID) -> Result<Self, Error> {
        id.0.parse().or(Err(Error::new_bad_request(&format!(
            "invalid value for unsigned id value: {}",
            id
        ))))
    }
}

impl FromID for isize {
    fn from_id(id: ID) -> Result<Self, Error> {
        id.0.parse().or(Err(Error::new_bad_request(&format!(
            "invalid value for integer id value: {}",
            id
        ))))
    }
}

#[cfg(feature = "server")]
impl FromID for Uuid {
    fn from_id(id: ID) -> Result<Self, Error> {
        Uuid::parse_str(&id.0).map_err(|err| {
            Error::new_bad_request(&format!(
                "invalid value for UUID id value: {}",
                err.to_string()
            ))
        })
    }
}

impl FromID for ID {
    fn from_id(id: ID) -> Result<Self, Error> {
        Ok(id)
    }
}

// IntoResponse is used to create _successful_ jsonapi responses from a resource struct
// it is not used to create error responses (return a jsonapi::Error::into() for that)
pub trait IntoResponse {
    type Attributes;

    fn into_response(self) -> ResourceResponse<Self::Attributes>;
}

pub trait FromRequest
where
    Self: Sized,
{
    type Attributes;
    fn from_request(req: Request<Self::Attributes>) -> Result<Self, Error>;
}

pub trait IntoRelationships {
    fn into_relationships(self) -> Option<BTreeMap<String, RelationshipData>>;
}

pub trait FromRelationships
where
    Self: Sized,
{
    fn from_relationships(rels: Option<BTreeMap<String, RelationshipData>>) -> Result<Self, Error>;
}

impl IntoRelationships for () {
    fn into_relationships(self) -> Option<BTreeMap<String, RelationshipData>> {
        None
    }
}

impl FromRelationships for () {
    fn from_relationships(rels: Option<BTreeMap<String, RelationshipData>>) -> Result<(), Error> {
        match rels {
            None => Ok(()),
            Some(map) => {
                if map.len() == 0 {
                    Ok(())
                } else {
                    Err(Error::new_bad_request(
                        "unexpected relationships for this resource type",
                    ))
                }
            }
        }
    }
}

pub trait IntoRelationship {
    fn into_relationship(self, resource_name: &str) -> Relationship;
}

pub trait FromRelationship
where
    Self: Sized,
{
    fn from_relationship(r: Relationship) -> Result<Self, Error>;
}

impl<I: FromID> FromRelationship for I {
    fn from_relationship(r: Relationship) -> Result<Self, Error> {
        match r {
            Relationship::ToOne(one) => Ok(I::from_id(one.id)?),
            _ => Err(Error::new_bad_request(
                "invalid relationship: expected a to-one, got to-many",
            )),
        }
    }
}

impl<I: FromID> FromRelationship for Vec<I> {
    fn from_relationship(r: Relationship) -> Result<Vec<I>, Error> {
        match r {
            Relationship::ToMany(many) => {
                let mut results = Vec::with_capacity(many.len());
                for each in many.into_iter() {
                    results.push(I::from_id(each.id)?);
                }
                Ok(results)
            }
            _ => Err(Error::new_bad_request(
                "invalid relationship: expected a to-many, got to-one",
            )),
        }
    }
}

impl<I> IntoRelationship for I
where
    ID: From<I>,
{
    fn into_relationship(self, resource_name: &str) -> Relationship {
        Relationship::ToOne(Identifier {
            id: self.into(),
            typ: resource_name.to_string(),
        })
    }
}

impl<I> IntoRelationship for Vec<I>
where
    ID: From<I>,
{
    fn into_relationship(self, resource_name: &str) -> Relationship {
        Relationship::ToMany(
            self.into_iter()
                .map(|each| Identifier {
                    id: each.into(),
                    typ: resource_name.to_string(),
                })
                .collect(),
        )
    }
}
