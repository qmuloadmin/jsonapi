//! Optional relational mapping descriptors.
//!
//! JSON:API describes a graph of resources; that graph usually lives in a
//! relational database. This crate performs **no** database access itself,
//! but a resource can optionally declare enough enrichment — its table name
//! and how each of its relationships is physically represented — for an
//! adapter (e.g. a future diesel helper crate) to build the graph queries
//! (joins, foreign key lookups, etc.) without the resource author having to
//! hand-write that plumbing per resource.
//!
//! Everything here is `const`-friendly (`&'static` data only) so a
//! [`ResourceMapping`] can be declared as an associated `const` on
//! [`MappedResource`].

use crate::resource::ResourceType;

/// How one relationship of a resource maps onto relational storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationshipMapping {
    /// A to-one relationship backed by a foreign-key column on this
    /// resource's own table.
    ToOne {
        /// The relationship name as it appears in the JSON:API document.
        name: &'static str,
        /// The column on this resource's table holding the foreign key.
        fk_column: &'static str,
        /// The `TYPE_NAME` of the related resource.
        resource_type: &'static str,
    },
    /// A to-many relationship backed by a join (lookup) table.
    ToMany {
        /// The relationship name as it appears in the JSON:API document.
        name: &'static str,
        /// The join/lookup table connecting this resource to the related one.
        join_table: &'static str,
        /// The column on `join_table` referencing this resource.
        local_key: &'static str,
        /// The column on `join_table` referencing the related resource.
        foreign_key: &'static str,
        /// The `TYPE_NAME` of the related resource.
        resource_type: &'static str,
    },
}

impl RelationshipMapping {
    /// The relationship name, common to both variants.
    pub fn name(&self) -> &'static str {
        match self {
            RelationshipMapping::ToOne { name, .. } => name,
            RelationshipMapping::ToMany { name, .. } => name,
        }
    }

    /// The `TYPE_NAME` of the related resource, common to both variants.
    pub fn resource_type(&self) -> &'static str {
        match self {
            RelationshipMapping::ToOne { resource_type, .. } => resource_type,
            RelationshipMapping::ToMany { resource_type, .. } => resource_type,
        }
    }
}

/// The relational mapping for a resource: its table and how its
/// relationships are represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMapping {
    /// The table this resource's rows live in.
    pub table: &'static str,
    /// The primary key column for this resource's table. Conventionally
    /// `"id"`.
    pub id_column: &'static str,
    /// The mapping for each relationship this resource declares.
    pub relationships: &'static [RelationshipMapping],
}

impl ResourceMapping {
    /// Look up a relationship mapping by its JSON:API relationship name.
    pub fn relationship(&self, name: &str) -> Option<&'static RelationshipMapping> {
        self.relationships.iter().find(|r| r.name() == name)
    }
}

/// A resource that declares its relational mapping, so a DB adapter can
/// build graph queries (joins, foreign key lookups) generically.
pub trait MappedResource: ResourceType {
    /// The relational mapping for this resource.
    const MAPPING: ResourceMapping;
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAPPING: ResourceMapping = ResourceMapping {
        table: "designs",
        id_column: "id",
        relationships: &[
            RelationshipMapping::ToOne {
                name: "author",
                fk_column: "author_id",
                resource_type: "users",
            },
            RelationshipMapping::ToMany {
                name: "tags",
                join_table: "designs_tags",
                local_key: "design_id",
                foreign_key: "tag_id",
                resource_type: "tags",
            },
        ],
    };

    #[test]
    fn to_one_accessors() {
        let rel = MAPPING.relationship("author").unwrap();
        assert_eq!(rel.name(), "author");
        assert_eq!(rel.resource_type(), "users");
        match rel {
            RelationshipMapping::ToOne { fk_column, .. } => assert_eq!(*fk_column, "author_id"),
            _ => panic!("expected ToOne"),
        }
    }

    #[test]
    fn to_many_accessors() {
        let rel = MAPPING.relationship("tags").unwrap();
        assert_eq!(rel.name(), "tags");
        assert_eq!(rel.resource_type(), "tags");
        match rel {
            RelationshipMapping::ToMany {
                join_table,
                local_key,
                foreign_key,
                ..
            } => {
                assert_eq!(*join_table, "designs_tags");
                assert_eq!(*local_key, "design_id");
                assert_eq!(*foreign_key, "tag_id");
            }
            _ => panic!("expected ToMany"),
        }
    }

    #[test]
    fn missing_relationship_is_none() {
        assert!(MAPPING.relationship("nonexistent").is_none());
    }

    #[test]
    fn table_and_id_column() {
        assert_eq!(MAPPING.table, "designs");
        assert_eq!(MAPPING.id_column, "id");
    }
}
