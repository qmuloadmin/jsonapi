use jsonapi_resource_derive::{FromRequest, IntoRelationships, IntoResponse};
use serde_derive::Serialize;
use uuid::Uuid;

#[derive(FromRequest)]
struct SimpleRequest {
    id: Uuid,
    attributes: SimpleAttributes,
}

#[derive(Clone, Serialize)]
struct SimpleAttributes {
    foo: String,
    bar: Option<isize>,
}

#[derive(IntoResponse)]
#[jsonapi(name = "simples")]
struct SimpleResponse {
    id: Uuid,
    attributes: SimpleAttributes,
}

#[derive(IntoResponse, Clone)]
#[jsonapi(name = "fakes")]
struct FakeResponse {
    id: usize,
    relations: FakeRelations,
}

#[derive(IntoRelationships, Clone)]
struct FakeRelations {
    simple: Option<Uuid>,
}

#[derive(IntoResponse)]
// All the types that can be included in the response of FakeResponse
enum Included {
	// TODO need to fix the Option<()> type and use a different type. See macro crate
    #[jsonapi(attr_name = "Option<()>")]
    Fake(FakeResponse),
	// TODO we shouldn't need to make this a string literal
    #[jsonapi(attr_name = "SimpleAttributes")]
    Simple(SimpleResponse),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jsonapi::{
        FromRequest, Identifier, Relationship, RelationshipData, Request, ResourceRequest, Response,
    };

    use super::*;

    #[test]
    fn test_from_request() {
        let id = Uuid::new_v4();
        let mut req = Request {
            data: ResourceRequest {
                id: Some(id.clone().into()),
                typ: "simple".into(),
                attributes: SimpleAttributes {
                    foo: "test".into(),
                    bar: Some(4),
                },
                relationships: None,
            },
        };
        assert!(SimpleRequest::from_request(req.clone()).is_ok());
        req.data.id = Some("foobar".into());
        assert!(SimpleRequest::from_request(req.clone()).is_err());
        let mut relations = BTreeMap::new();
        relations.insert(
            "foo".into(),
            RelationshipData {
                data: Relationship::ToOne(Identifier {
                    id: "fake".into(),
                    typ: "fakes".into(),
                }),
            },
        );
        req.data.relationships = Some(relations);
        req.data.id = Some(id.into());
        assert!(SimpleRequest::from_request(req.clone()).is_err());
    }
    #[test]
    fn test_responder() {
        // this isn't purposeful, yet. If it compiles, then it works. There's no
        // way a conversion into a response can fail at the moment
        let id = Uuid::new_v4();
        let res = FakeResponse {
            id: 5,
            relations: FakeRelations {
                simple: Some(id.clone()),
            },
        };
        let simple = SimpleResponse {
            id: id,
            attributes: SimpleAttributes {
                foo: "bar".into(),
                bar: Some(3),
            },
        };
        let res = Response::from(res.clone())
            .include(Included::Simple(simple))
            .include(Included::Fake(res));
		println!("{}", serde_json::to_string(&res).unwrap());
    }
}
