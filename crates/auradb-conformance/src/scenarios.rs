//! The Aura Connector conformance scenario suite, run over the wire protocol.

use auradb::core::{
    Cardinality, CollectionSchema, Document, FieldDef, FieldType, OnDelete, Relationship, Value,
};
use auradb::core::{Error, Result};
use auradb::query::{
    CompareOp, CountQuery, ExistsQuery, Filter, FindQuery, Mutation, VectorSearch,
};

use crate::client::Client;

/// The outcome of one scenario.
#[derive(Debug, Clone)]
pub struct ScenarioOutcome {
    /// Scenario name.
    pub name: String,
    /// Whether it passed.
    pub passed: bool,
    /// Failure detail, if any.
    pub detail: String,
}

/// A full conformance report.
#[derive(Debug, Clone, Default)]
pub struct ConformanceReport {
    /// Per-scenario outcomes.
    pub outcomes: Vec<ScenarioOutcome>,
}

impl ConformanceReport {
    fn record(&mut self, name: &str, result: Result<()>) {
        let (passed, detail) = match result {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e.to_string()),
        };
        self.outcomes.push(ScenarioOutcome {
            name: name.to_string(),
            passed,
            detail,
        });
    }

    /// Whether every scenario passed.
    pub fn all_passed(&self) -> bool {
        self.outcomes.iter().all(|o| o.passed)
    }

    /// The number of passing scenarios.
    pub fn passed_count(&self) -> usize {
        self.outcomes.iter().filter(|o| o.passed).count()
    }

    /// A multi-line human-readable summary.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Conformance: {}/{} scenarios passed\n",
            self.passed_count(),
            self.outcomes.len()
        );
        for o in &self.outcomes {
            let mark = if o.passed { "PASS" } else { "FAIL" };
            s.push_str(&format!("  [{mark}] {}", o.name));
            if !o.passed {
                s.push_str(&format!("  -- {}", o.detail));
            }
            s.push('\n');
        }
        s
    }
}

fn check(cond: bool, msg: &str) -> Result<()> {
    if cond {
        Ok(())
    } else {
        Err(Error::Internal(format!("assertion failed: {msg}")))
    }
}

fn user_schema() -> CollectionSchema {
    CollectionSchema::new("User").with_field(FieldDef {
        name: "id".into(),
        field_type: FieldType::Uuid,
        primary_key: true,
        unique: true,
        nullable: false,
        indexed: false,
    })
}

fn doc_schema() -> CollectionSchema {
    CollectionSchema::new("Doc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef {
            name: "status".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: true,
            indexed: true,
        })
        .with_field(FieldDef::new("title", FieldType::String))
        .with_field(FieldDef::new("views", FieldType::Int))
        .with_field(FieldDef::new("metadata", FieldType::Document))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
        .with_relationship(Relationship {
            name: "owner".into(),
            target: "User".into(),
            cardinality: Cardinality::ToOne,
            on_delete: OnDelete::Restrict,
        })
}

fn doc(id: &str, status: &str, views: i64, embedding: Vec<f32>) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("status".into(), Value::Text(status.into()));
    m.insert("title".into(), Value::Text(format!("Title {id}")));
    m.insert("views".into(), Value::Int(views));
    m.insert("owner".into(), Value::Text("u1".into()));
    m.insert("embedding".into(), Value::Vector(embedding));
    let mut meta = Document::new();
    meta.insert("source".into(), Value::Text("import".into()));
    m.insert("metadata".into(), Value::Object(meta));
    m
}

/// Connect to `addr` and run the full conformance suite.
pub async fn run_all(addr: &str) -> Result<ConformanceReport> {
    let mut client = Client::connect(addr).await?;
    let mut report = ConformanceReport::default();

    // connect (implicit HELLO succeeded if we got here)
    report.record("connect", Ok(()));

    report.record("ping", client.ping().await.map(|_| ()));

    report.record(
        "health",
        async {
            let h = client.health().await?;
            check(h.ready, "server should be ready")
        }
        .await,
    );

    report.record(
        "schema_create",
        async {
            client.create_schema(&user_schema()).await?;
            client.create_schema(&doc_schema()).await?;
            Ok(())
        }
        .await,
    );

    report.record(
        "schema_get_and_list",
        async {
            let s = client.get_schema("Doc").await?;
            check(s.name == "Doc", "schema name")?;
            let all = client.list_schemas().await?;
            check(all.len() >= 2, "two schemas registered")
        }
        .await,
    );

    report.record(
        "insert",
        async {
            let mut u = Document::new();
            u.insert("id".into(), Value::Text("u1".into()));
            client.insert("User", u).await?;
            client
                .insert("Doc", doc("d1", "published", 10, vec![1.0, 0.0, 0.0]))
                .await?;
            client
                .insert("Doc", doc("d2", "draft", 5, vec![0.0, 1.0, 0.0]))
                .await?;
            client
                .insert("Doc", doc("d3", "published", 20, vec![0.9, 0.1, 0.0]))
                .await?;
            Ok(())
        }
        .await,
    );

    report.record(
        "find",
        async {
            let rows = client.find_all(&FindQuery::new("Doc")).await?;
            check(rows.len() == 3, "three docs")
        }
        .await,
    );

    report.record(
        "filter",
        async {
            let mut q = FindQuery::new("Doc");
            q.filter = Some(Filter::Compare {
                field: "status".into(),
                op: CompareOp::Eq,
                value: Value::Text("published".into()),
            });
            let rows = client.find_all(&q).await?;
            check(rows.len() == 2, "two published docs")
        }
        .await,
    );

    report.record(
        "document_field",
        async {
            let mut q = FindQuery::new("Doc");
            q.filter = Some(Filter::Compare {
                field: "metadata.source".into(),
                op: CompareOp::Eq,
                value: Value::Text("import".into()),
            });
            let rows = client.find_all(&q).await?;
            check(rows.len() == 3, "nested document filter")
        }
        .await,
    );

    report.record(
        "relationship_include",
        async {
            let mut q = FindQuery::new("Doc");
            q.includes = vec!["owner".into()];
            q.limit = Some(1);
            let rows = client.find_all(&q).await?;
            check(!rows.is_empty(), "at least one row")?;
            let owners = rows[0]
                .includes
                .get("owner")
                .ok_or_else(|| Error::Internal("missing owner include".into()))?;
            check(owners.len() == 1, "one owner hydrated")
        }
        .await,
    );

    report.record(
        "vector_nearest",
        async {
            let mut q = FindQuery::new("Doc");
            q.vector = Some(VectorSearch {
                field: "embedding".into(),
                query: vec![1.0, 0.0, 0.0],
                k: 2,
                metric: "cosine".into(),
            });
            let rows = client.find_all(&q).await?;
            check(rows.len() == 2, "two nearest")?;
            check(
                rows[0].fields.get("id") == Some(&Value::Text("d1".into())),
                "d1 is nearest",
            )?;
            check(rows[0].score.is_some(), "score present")
        }
        .await,
    );

    report.record(
        "explain",
        async {
            let mut q = FindQuery::new("Doc");
            q.filter = Some(Filter::Compare {
                field: "status".into(),
                op: CompareOp::Eq,
                value: Value::Text("published".into()),
            });
            let plan = client.explain(&q).await?;
            check(
                plan.used_index.as_deref() == Some("status"),
                "status index used",
            )
        }
        .await,
    );

    report.record(
        "count",
        async {
            let c = client
                .count(&CountQuery {
                    collection: "Doc".into(),
                    filter: None,
                })
                .await?;
            check(c == 3, "count is 3")
        }
        .await,
    );

    report.record(
        "exists",
        async {
            let e = client
                .exists(&ExistsQuery {
                    collection: "Doc".into(),
                    filter: Some(Filter::Compare {
                        field: "id".into(),
                        op: CompareOp::Eq,
                        value: Value::Text("d1".into()),
                    }),
                })
                .await?;
            check(e, "d1 exists")
        }
        .await,
    );

    report.record(
        "stream_cursor",
        async {
            // find_all follows the cursor through every page.
            let rows = client.find_all(&FindQuery::new("Doc")).await?;
            check(rows.len() == 3, "streamed all rows via cursor")
        }
        .await,
    );

    report.record(
        "migration_estimate",
        async {
            let target = doc_schema().with_field(FieldDef {
                name: "category".into(),
                field_type: FieldType::String,
                primary_key: false,
                unique: false,
                nullable: true,
                indexed: true,
            });
            let est = client.migration_estimate(&target).await?;
            check(est.exists, "collection exists")?;
            check(
                est.new_indexes.contains(&"category".to_string()),
                "new index detected",
            )
        }
        .await,
    );

    report.record(
        "update",
        async {
            let r = client
                .mutate(
                    0,
                    &Mutation::Update {
                        collection: "Doc".into(),
                        filter: Some(Filter::Compare {
                            field: "id".into(),
                            op: CompareOp::Eq,
                            value: Value::Text("d2".into()),
                        }),
                        set: {
                            let mut s = Document::new();
                            s.insert("status".into(), Value::Text("published".into()));
                            s
                        },
                    },
                )
                .await?;
            check(r.updated == 1, "one updated")
        }
        .await,
    );

    report.record(
        "upsert",
        async {
            let r = client
                .mutate(
                    0,
                    &Mutation::Upsert {
                        collection: "Doc".into(),
                        fields: doc("d1", "archived", 99, vec![1.0, 0.0, 0.0]),
                    },
                )
                .await?;
            check(r.updated == 1, "upsert replaced existing")
        }
        .await,
    );

    report.record(
        "delete",
        async {
            let r = client
                .mutate(
                    0,
                    &Mutation::Delete {
                        collection: "Doc".into(),
                        filter: Some(Filter::Compare {
                            field: "id".into(),
                            op: CompareOp::Eq,
                            value: Value::Text("d3".into()),
                        }),
                    },
                )
                .await?;
            check(r.deleted == 1, "one deleted")
        }
        .await,
    );

    report.record(
        "transaction_commit",
        async {
            let txn = client.begin().await?;
            client
                .mutate(
                    txn,
                    &Mutation::Insert {
                        collection: "Doc".into(),
                        fields: doc("d4", "draft", 1, vec![0.0, 0.0, 1.0]),
                    },
                )
                .await?;
            client.commit(txn).await?;
            let e = client
                .exists(&ExistsQuery {
                    collection: "Doc".into(),
                    filter: Some(Filter::Compare {
                        field: "id".into(),
                        op: CompareOp::Eq,
                        value: Value::Text("d4".into()),
                    }),
                })
                .await?;
            check(e, "committed record visible")
        }
        .await,
    );

    report.record(
        "transaction_rollback",
        async {
            let txn = client.begin().await?;
            client
                .mutate(
                    txn,
                    &Mutation::Insert {
                        collection: "Doc".into(),
                        fields: doc("d5", "draft", 1, vec![0.0, 0.0, 1.0]),
                    },
                )
                .await?;
            client.rollback(txn).await?;
            let e = client
                .exists(&ExistsQuery {
                    collection: "Doc".into(),
                    filter: Some(Filter::Compare {
                        field: "id".into(),
                        op: CompareOp::Eq,
                        value: Value::Text("d5".into()),
                    }),
                })
                .await?;
            check(!e, "rolled-back record not visible")
        }
        .await,
    );

    Ok(report)
}
