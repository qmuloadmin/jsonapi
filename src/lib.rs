pub mod actix;
pub mod auth;
mod document;
mod error;
mod mapping;
mod query;
mod resource;

#[cfg(feature = "diesel")]
mod diesel_support;

pub use document::*;
pub use error::*;
pub use mapping::*;
pub use query::*;
pub use resource::*;

#[cfg(feature = "actixweb")]
pub use actix::extract::{JsonApi, JsonApiExtractFut, MEDIA_TYPE};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use uuid::Uuid;

    use crate::{
        FromID, FromRelationships, FromRequest, Identifier, IntoResponse, Relationship,
        RelationshipData, Request, ResourceRequest, ResourceResponse, Response,
    };

    // A simple request with no relationships
    struct SimpleRequest {
        id: Uuid,
        attributes: SimpleAttributes,
    }

    #[derive(Clone)]
    struct SimpleAttributes {
        foo: String,
        bar: Option<isize>,
    }

    impl FromRequest for SimpleRequest {
        type Attributes = SimpleAttributes;

        fn from_request(req: Request<Self::Attributes>) -> Result<Self, crate::Error> {
            // ensure no relationships were passed (this implicitly has a "relationships" of unit struct)
            <() as FromRelationships>::from_relationships(req.data.relationships)?;
            Ok(SimpleRequest {
                id: FromID::from_id(req.data.id.unwrap())?,
                attributes: req.data.attributes,
            })
        }
    }

    #[test]
    fn test_simple_request() {
        let id = Uuid::new_v4();
        let mut req = Request {
            data: ResourceRequest {
                id: Some(id.clone().into()),
                typ: "simple".into(),
                attributes: SimpleAttributes {
                    foo: "testing".into(),
                    bar: Some(123),
                },
                relationships: None,
            },
        };
        assert!(SimpleRequest::from_request(req.clone()).is_ok());
        req.data.id = Some("foobarbaz".into()); // invalid UUID format
        assert!(SimpleRequest::from_request(req.clone()).is_err());
        req.data.id = Some(id.clone().into());
        let mut relations = BTreeMap::new();
        relations.insert(
            "fake".to_owned(),
            RelationshipData {
                data: Relationship::ToOne(Identifier {
                    id: "test".into(),
                    typ: "fake".into(),
                }),
            },
        );
        req.data.relationships = Some(relations);
        assert!(SimpleRequest::from_request(req.clone()).is_err());
    }

    struct SimpleResponse {
        id: Uuid,
        attributes: SimpleAttributes,
    }

    impl IntoResponse for SimpleResponse {
        type Attributes = SimpleAttributes;

        fn into_response(self) -> ResourceResponse<Self::Attributes> {
            ResourceResponse {
                id: Identifier {
                    id: self.id.into(),
                    typ: "simple".into(),
                },
                attributes: self.attributes,
                relationships: None,
                meta: None,
            }
        }
    }

    #[test]
    fn test_simple_response() {
        let attrs = SimpleAttributes {
            foo: "foo".into(),
            bar: None,
        };
        let id = Uuid::new_v4();
        let response = SimpleResponse {
            id,
            attributes: attrs,
        };
        // finish with no included resources.
        // finish is essentially a more readable way to provide types for responses
        // with no included resources. There is likely a better way to do this but for
        // now this is the approach we're taking.
        Response::from(response).finish();
    }
}
