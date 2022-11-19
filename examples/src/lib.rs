use jsonapi::{Identifier, IntoResponse, IntoRelationships};
use jsonapi_resource_derive::{FromRequest, IntoRelationships, IntoResponse};
use serde_derive::Serialize;
use uuid::Uuid;

#[derive(FromRequest)]
struct SimpleRequest {
	id: Uuid,
	attributes: SimpleAttributes
}

#[derive(Clone, Serialize)]
struct SimpleAttributes {
	foo: String,
	bar: Option<isize>
}

#[derive(IntoResponse)]
struct SimpleResponse {
	id: Uuid,
	attributes: SimpleAttributes,
}

#[derive(IntoResponse, Clone)]
struct FakeResponse {
	id: usize,
	relations: FakeRelations
}

#[derive(IntoRelationships, Clone)]
struct FakeRelations {
	simple: Option<Uuid>
}

enum Included {
	Fake(FakeResponse),
	Simple(SimpleResponse)
}

#[derive(Serialize)]
#[serde(untagged)]
enum IncludedAttrs {
	Simple(SimpleAttributes),
	Fake(())
}

impl IntoResponse for Included {
	type Attributes = IncludedAttrs;
	fn into_response(self) -> jsonapi::ResourceResponse<Self::Attributes> {
		match self {
			Included::Fake(res) => jsonapi::ResourceResponse{
				id: Identifier{id: res.id.into(), typ: "fakes".into()},
				attributes: IncludedAttrs::Fake(()),
				relationships: res.relations.into_relationships()
			},
			Included::Simple(res) => jsonapi::ResourceResponse{
				id: Identifier{id: res.id.into(), typ: "simples".into()},
				attributes: IncludedAttrs::Simple(res.attributes),
				relationships: None,
			}
		}
	}
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jsonapi::{FromRequest, Identifier, Relationship, RelationshipData, Request, ResourceRequest, Response};

    use super::*;

    #[test]
    fn test_from_request() {
        let id = Uuid::new_v4();
		let mut req = Request{
			data: ResourceRequest{
				id: Some(id.clone().into()),
				typ: "simple".into(),
				attributes: SimpleAttributes{
					foo: "test".into(),
					bar: Some(4),
				},
				relationships: None
			}
		};
		assert!(SimpleRequest::from_request(req.clone()).is_ok());
		req.data.id = Some("foobar".into());
		assert!(SimpleRequest::from_request(req.clone()).is_err());
		let mut relations = BTreeMap::new();
		relations.insert("foo".into(), RelationshipData{data: Relationship::ToOne(
			Identifier{id: "fake".into(), typ: "fakes".into()}
		)});
		req.data.relationships = Some(relations);
		req.data.id = Some(id.into());
		assert!(SimpleRequest::from_request(req.clone()).is_err());
    }
	#[test]
	fn test_responder() {
		// this isn't purposeful, yet. If it compiles, then it works. There's no
		// way a conversion into a response can fail at the moment
		let id = Uuid::new_v4();
		let res = FakeResponse{
			id: 5,
			relations: FakeRelations{
				simple: Some(id.clone())
			}
		};
		let simple = SimpleResponse{
			id: id,
			attributes: SimpleAttributes{
				foo: "bar".into(),
				bar: Some(3)
			}
		};
		let res = Response::from(res.clone()).include(Included::Simple(simple)).include(Included::Fake(res));
	}
}
