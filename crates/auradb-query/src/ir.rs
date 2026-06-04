//! The Query Intermediate Representation (IR).
//!
//! This is the Aura-Connector-compatible query model. It is intentionally a
//! transparent JSON shape (see `docs/QUERY_ENGINE.md`); a follow-up task pins
//! golden IR fixtures from the real connector. Reads use [`FindQuery`] /
//! [`CountQuery`] / [`ExistsQuery`]; writes use [`Mutation`].

use auradb_core::{Document, Value};
use serde::{Deserialize, Serialize};

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Membership: the field value equals one of an array of values.
    In,
}

/// A filter predicate tree. Field names may be dotted document paths
/// (e.g. `metadata.status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Filter {
    /// Logical conjunction.
    And {
        /// Sub-filters that must all match.
        filters: Vec<Filter>,
    },
    /// Logical disjunction.
    Or {
        /// Sub-filters of which at least one must match.
        filters: Vec<Filter>,
    },
    /// Logical negation.
    Not {
        /// The negated sub-filter.
        filter: Box<Filter>,
    },
    /// A field comparison.
    Compare {
        /// The field (dotted path supported).
        field: String,
        /// The comparison operator.
        op: CompareOp,
        /// The right-hand value.
        value: Value,
    },
    /// Case-sensitive substring containment on a string field.
    Contains {
        /// The field (dotted path supported).
        field: String,
        /// The substring to find.
        substring: String,
    },
    /// The field exists and is non-null.
    Exists {
        /// The field (dotted path supported).
        field: String,
    },
}

/// An ordering key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderKey {
    /// The field to order by (dotted path supported).
    pub field: String,
    /// Descending if true, ascending otherwise.
    #[serde(default)]
    pub desc: bool,
}

/// An exact vector nearest-neighbour clause.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorSearch {
    /// The vector field to search.
    pub field: String,
    /// The query vector.
    pub query: Vec<f32>,
    /// The number of nearest neighbours to consider.
    pub k: usize,
    /// The metric name (`cosine`, `euclidean`, `dot_product`).
    pub metric: String,
}

/// A read query returning matching rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindQuery {
    /// The collection to query.
    pub collection: String,
    /// Optional filter predicate.
    #[serde(default)]
    pub filter: Option<Filter>,
    /// Optional vector nearest-neighbour clause (applied before ordering).
    #[serde(default)]
    pub vector: Option<VectorSearch>,
    /// Ordering keys (ignored when a vector clause is present, which orders by
    /// similarity).
    #[serde(default)]
    pub order_by: Vec<OrderKey>,
    /// Maximum number of rows to return.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Number of leading rows to skip.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Optional projection: only these fields are returned.
    #[serde(default)]
    pub projection: Option<Vec<String>>,
    /// Relationship field names to hydrate into each row.
    #[serde(default)]
    pub includes: Vec<String>,
}

impl FindQuery {
    /// Construct a minimal find over a collection.
    pub fn new(collection: impl Into<String>) -> Self {
        FindQuery {
            collection: collection.into(),
            filter: None,
            vector: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            projection: None,
            includes: Vec::new(),
        }
    }
}

/// A count query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CountQuery {
    /// The collection.
    pub collection: String,
    /// Optional filter.
    #[serde(default)]
    pub filter: Option<Filter>,
}

/// An existence query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExistsQuery {
    /// The collection.
    pub collection: String,
    /// Optional filter.
    #[serde(default)]
    pub filter: Option<Filter>,
}

/// A read request opcode payload (`Opcode::Query`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "snake_case")]
pub enum ReadRequest {
    /// Find matching rows.
    Find(FindQuery),
    /// Count matching rows.
    Count(CountQuery),
    /// Test whether any row matches.
    Exists(ExistsQuery),
}

/// A mutation request payload (`Opcode::Mutate`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mutation", rename_all = "snake_case")]
pub enum Mutation {
    /// Insert a single record. Fails if the primary key already exists.
    Insert {
        /// Target collection.
        collection: String,
        /// The record fields (must include the primary key).
        fields: Document,
    },
    /// Insert many records atomically.
    BulkInsert {
        /// Target collection.
        collection: String,
        /// The records' fields.
        records: Vec<Document>,
    },
    /// Update fields of all records matching a filter.
    Update {
        /// Target collection.
        collection: String,
        /// Which records to update.
        filter: Option<Filter>,
        /// Field assignments to merge into matched records.
        set: Document,
    },
    /// Delete all records matching a filter.
    Delete {
        /// Target collection.
        collection: String,
        /// Which records to delete.
        filter: Option<Filter>,
    },
    /// Insert a record, or replace it if the primary key already exists.
    Upsert {
        /// Target collection.
        collection: String,
        /// The record fields (must include the primary key).
        fields: Document,
    },
}

impl Mutation {
    /// The collection this mutation targets.
    pub fn collection(&self) -> &str {
        match self {
            Mutation::Insert { collection, .. }
            | Mutation::BulkInsert { collection, .. }
            | Mutation::Update { collection, .. }
            | Mutation::Delete { collection, .. }
            | Mutation::Upsert { collection, .. } => collection,
        }
    }
}

/// The result of a mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationResult {
    /// Number of records inserted.
    #[serde(default)]
    pub inserted: usize,
    /// Number of records updated.
    #[serde(default)]
    pub updated: usize,
    /// Number of records deleted.
    #[serde(default)]
    pub deleted: usize,
    /// The ids affected (as hex strings).
    #[serde(default)]
    pub ids: Vec<String>,
}

impl MutationResult {
    /// An empty result.
    pub fn empty() -> Self {
        MutationResult {
            inserted: 0,
            updated: 0,
            deleted: 0,
            ids: Vec::new(),
        }
    }
}

/// One result row from a [`FindQuery`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// The record id (hex string).
    pub id: String,
    /// The (possibly projected) fields.
    pub fields: Document,
    /// Vector similarity score, when the query had a vector clause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// Hydrated related records keyed by relationship name.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub includes: std::collections::BTreeMap<String, Vec<Document>>,
}

/// A page of query results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResultPage {
    /// The rows in this page.
    pub rows: Vec<Row>,
    /// A cursor id to fetch the next page, if more rows remain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_id: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_query_roundtrips() {
        let q = FindQuery {
            collection: "Doc".into(),
            filter: Some(Filter::And {
                filters: vec![
                    Filter::Compare {
                        field: "status".into(),
                        op: CompareOp::Eq,
                        value: Value::Text("published".into()),
                    },
                    Filter::Exists {
                        field: "metadata.source".into(),
                    },
                ],
            }),
            vector: None,
            order_by: vec![OrderKey {
                field: "created_at".into(),
                desc: true,
            }],
            limit: Some(10),
            offset: Some(5),
            projection: Some(vec!["id".into(), "title".into()]),
            includes: vec!["owner".into()],
        };
        let json = serde_json::to_string(&q).unwrap();
        let back: FindQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(q, back);
    }

    #[test]
    fn read_request_tagged() {
        let r = ReadRequest::Count(CountQuery {
            collection: "C".into(),
            filter: None,
        });
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["query"], "count");
    }

    #[test]
    fn mutation_roundtrips() {
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text("x".into()));
        let m = Mutation::Upsert {
            collection: "C".into(),
            fields,
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Mutation = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
        assert_eq!(back.collection(), "C");
    }
}
