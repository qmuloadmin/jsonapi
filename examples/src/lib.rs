use jsonapi_resource_derive::{FromRequest, IntoRelationships, IntoResponse};
use uuid::Uuid;

#[derive(FromRequest)]
struct SimpleRequest {
	id: Uuid,
	attributes: SimpleAttributes
}

#[derive(Clone)]
struct SimpleAttributes {
	foo: String,
	bar: Option<isize>
}

#[derive(IntoResponse)]
struct SimpleResponse {
	id: Uuid,
	attributes: SimpleAttributes,
}

#[derive(IntoResponse)]
struct FakeResponse {
	id: usize,
	relations: FakeRelations
}

#[derive(IntoRelationships)]
struct FakeRelations {
	simple: Option<Uuid>
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jsonapi::{FromRequest, Identifier, Relationship, RelationshipData, Request, ResourceRequest};

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
	}
}
