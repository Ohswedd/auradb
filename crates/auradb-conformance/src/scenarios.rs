//! The Aura Connector conformance scenario suite, run over the wire protocol.

use auradb::core::{
    Cardinality, CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, OnDelete,
    Relationship, Value,
};
use auradb::core::{Error, Result};
use auradb::query::{
    AggregateMetric, AggregateOp, AggregateQuery, AnnParams, CompareOp, CountQuery, ExistsQuery,
    FacetRequest, Filter, FindQuery, FusionMode, HybridSearch, HybridWeights, Mutation,
    TextOperator, TextRank, TextSearch, VectorSearch,
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
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("views", FieldType::Int))
        .with_field(FieldDef::new("metadata", FieldType::Document))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
        .with_relationship(Relationship {
            name: "owner".into(),
            target: "User".into(),
            cardinality: Cardinality::ToOne,
            on_delete: OnDelete::Restrict,
        })
}

/// Deterministic full-text body for a seeded document.
fn doc_body(id: &str) -> &'static str {
    match id {
        "d1" => "raft consensus raft",
        "d2" => "the raft module coordinates replicas across nodes",
        _ => "storage compaction and flushing",
    }
}

fn doc(id: &str, status: &str, views: i64, embedding: Vec<f32>) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("status".into(), Value::Text(status.into()));
    m.insert("title".into(), Value::Text(format!("Title {id}")));
    m.insert("body".into(), Value::Text(doc_body(id).into()));
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
        "text_search_bm25",
        async {
            let mut q = FindQuery::new("Doc");
            q.text_search = Some(Box::new(TextSearch {
                field: "body".into(),
                query: "raft".into(),
                operator: TextOperator::Or,
                rank: TextRank::Bm25,
                k1: None,
                b: None,
                analyzer: None,
            }));
            let rows = client.find_all(&q).await?;
            check(rows.len() == 2, "two BM25 matches")?;
            check(
                rows[0].fields.get("id") == Some(&Value::Text("d1".into())),
                "dense doc ranks first",
            )?;
            check(rows[0].rank == Some(1), "1-based rank present")
        }
        .await,
    );

    report.record(
        "hybrid_search",
        async {
            let mut q = FindQuery::new("Doc");
            q.hybrid = Some(Box::new(HybridSearch {
                text_field: "body".into(),
                text_query: "raft".into(),
                vector_field: "embedding".into(),
                vector: vec![1.0, 0.0, 0.0],
                top_k: 3,
                metric: None,
                weights: HybridWeights::default(),
                fusion: FusionMode::WeightedSum,
                operator: TextOperator::Or,
                k1: None,
                b: None,
                analyzer: None,
            }));
            let rows = client.find_all(&q).await?;
            check(!rows.is_empty(), "hybrid returns rows")?;
            check(rows[0].score.is_some(), "fused score present")
        }
        .await,
    );

    report.record(
        "search_explain_analyze",
        async {
            let mut q = FindQuery::new("Doc");
            q.text_search = Some(Box::new(TextSearch {
                field: "body".into(),
                query: "raft".into(),
                operator: TextOperator::Or,
                rank: TextRank::Bm25,
                k1: None,
                b: None,
                analyzer: None,
            }));
            let plan = client.explain_analyze(&q).await?;
            check(plan.text_search.is_some(), "text_search summary present")?;
            check(plan.analysis.is_some(), "analyze metrics present")
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

    report.record(
        "transaction_scoped_reads",
        async {
            let id_eq = |id: &str| ExistsQuery {
                collection: "Doc".into(),
                filter: Some(Filter::Compare {
                    field: "id".into(),
                    op: CompareOp::Eq,
                    value: Value::Text(id.into()),
                }),
            };
            let txn = client.begin().await?;
            client
                .mutate(
                    txn,
                    &Mutation::Insert {
                        collection: "Doc".into(),
                        fields: doc("d6", "draft", 1, vec![0.0, 0.0, 1.0]),
                    },
                )
                .await?;

            // Read-your-writes: the transaction sees its staged insert.
            let in_txn = client.exists_in_txn(txn, &id_eq("d6")).await?;
            // Isolation: a non-transactional read does not, before commit.
            let out_txn = client.exists(&id_eq("d6")).await?;
            // The transaction's find also returns the staged row.
            let mut q = FindQuery::new("Doc");
            q.filter = Some(Filter::Compare {
                field: "id".into(),
                op: CompareOp::Eq,
                value: Value::Text("d6".into()),
            });
            let txn_rows = client.find_all_in_txn(txn, &q).await?;

            client.commit(txn).await?;
            let after_commit = client.exists(&id_eq("d6")).await?;

            check(
                in_txn && !out_txn && txn_rows.len() == 1 && after_commit,
                "transaction-scoped read sees staged write before commit; \
                 non-transactional read does not until commit",
            )
        }
        .await,
    );

    let id_eq = |id: &str| Filter::Compare {
        field: "id".into(),
        op: CompareOp::Eq,
        value: Value::Text(id.into()),
    };
    let set_status = |status: &str| {
        let mut s = Document::new();
        s.insert("status".into(), Value::Text(status.into()));
        s
    };

    report.record(
        "snapshot_isolation_later_commit_invisible",
        async {
            client
                .mutate(
                    0,
                    &Mutation::Insert {
                        collection: "Doc".into(),
                        fields: doc("snap", "v1", 1, vec![0.0, 0.0, 1.0]),
                    },
                )
                .await?;
            let txn = client.begin().await?;
            // A concurrent auto-commit updates the record after the snapshot pins.
            client
                .mutate(
                    0,
                    &Mutation::Update {
                        collection: "Doc".into(),
                        filter: Some(id_eq("snap")),
                        set: set_status("v2"),
                    },
                )
                .await?;
            let mut q = FindQuery::new("Doc");
            q.filter = Some(id_eq("snap"));
            let snap_rows = client.find_all_in_txn(txn, &q).await?;
            let latest_rows = client.find_all(&q).await?;
            client.rollback(txn).await?;
            check(
                snap_rows[0].fields.get("status") == Some(&Value::Text("v1".into())),
                "transaction sees its begin-time snapshot",
            )?;
            check(
                latest_rows[0].fields.get("status") == Some(&Value::Text("v2".into())),
                "non-transactional read sees the latest commit",
            )
        }
        .await,
    );

    report.record(
        "write_conflict_rejected",
        async {
            client
                .mutate(
                    0,
                    &Mutation::Insert {
                        collection: "Doc".into(),
                        fields: doc("conf", "a", 1, vec![0.0, 1.0, 0.0]),
                    },
                )
                .await?;
            let a = client.begin().await?;
            let b = client.begin().await?;
            client
                .mutate(
                    a,
                    &Mutation::Update {
                        collection: "Doc".into(),
                        filter: Some(id_eq("conf")),
                        set: set_status("from-a"),
                    },
                )
                .await?;
            client
                .mutate(
                    b,
                    &Mutation::Update {
                        collection: "Doc".into(),
                        filter: Some(id_eq("conf")),
                        set: set_status("from-b"),
                    },
                )
                .await?;
            client.commit(a).await?;
            // The second committer conflicts (first-committer-wins).
            let b_result = client.commit(b).await;
            check(
                b_result.is_err(),
                "second concurrent writer is rejected on commit",
            )
        }
        .await,
    );

    report.record(
        "explain_analyze_shape",
        async {
            let mut q = FindQuery::new("Doc");
            q.filter = Some(Filter::Compare {
                field: "status".into(),
                op: CompareOp::Eq,
                value: Value::Text("published".into()),
            });
            let plan = client.explain_analyze(&q).await?;
            let analysis = plan
                .analysis
                .ok_or_else(|| auradb_core::Error::Internal("missing analysis".into()))?;
            check(plan.plan_tree.is_some(), "plan tree present")?;
            check(
                analysis.returned_rows == analysis.matched_rows,
                "analyze reports matched/returned rows",
            )
        }
        .await,
    );

    report.record(
        "planner_uses_index",
        async {
            let mut q = FindQuery::new("Doc");
            q.filter = Some(id_eq("d1"));
            let plan = client.explain(&q).await?;
            check(plan.used_index.as_deref() == Some("id"), "id index used")?;
            check(plan.plan_tree.is_some(), "plan tree present")
        }
        .await,
    );

    // ----- v1.2.0 query ergonomics -----

    report.record(
        "aggregate_count_min_max",
        async {
            // Derive the expected values from the live collection so the scenario
            // is robust to whatever earlier scenarios inserted.
            let rows = client.find_all(&FindQuery::new("Doc")).await?;
            let views: Vec<i64> = rows
                .iter()
                .filter_map(|r| match r.fields.get("views") {
                    Some(Value::Int(v)) => Some(*v),
                    _ => None,
                })
                .collect();
            let expect_count = rows.len() as i64;
            let expect_min = *views.iter().min().unwrap_or(&0);
            let expect_max = *views.iter().max().unwrap_or(&0);

            let mut q = AggregateQuery::new("Doc");
            q.metrics = vec![
                AggregateMetric {
                    op: AggregateOp::Count,
                    field: None,
                },
                AggregateMetric {
                    op: AggregateOp::Min,
                    field: Some("views".into()),
                },
                AggregateMetric {
                    op: AggregateOp::Max,
                    field: Some("views".into()),
                },
            ];
            let r = client.aggregate(&q).await?;
            check(r.matched as i64 == expect_count, "count matches find_all")?;
            check(
                r.metrics[0].value == Value::Int(expect_count),
                "count metric",
            )?;
            check(r.metrics[1].value == Value::Int(expect_min), "min views")?;
            check(r.metrics[2].value == Value::Int(expect_max), "max views")
        }
        .await,
    );

    report.record(
        "terms_facet_index_backed",
        async {
            // Expected buckets derived from the live data: count desc, value asc.
            let rows = client.find_all(&FindQuery::new("Doc")).await?;
            let mut counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for row in &rows {
                if let Some(Value::Text(s)) = row.fields.get("status") {
                    *counts.entry(s.clone()).or_insert(0) += 1;
                }
            }
            let mut expected: Vec<(String, usize)> = counts.into_iter().collect();
            expected.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

            let mut q = AggregateQuery::new("Doc");
            q.facets = vec![FacetRequest {
                field: "status".into(),
                limit: None,
            }];
            let r = client.aggregate(&q).await?;
            let facet = &r.facets[0];
            check(facet.used_index, "status equality index serves the facet")?;
            let got: Vec<(String, usize)> = facet
                .buckets
                .iter()
                .filter_map(|b| match &b.value {
                    Value::Text(s) => Some((s.clone(), b.count)),
                    _ => None,
                })
                .collect();
            check(got == expected, "facet buckets match grouped data, ordered")
        }
        .await,
    );

    report.record(
        "search_facet_bm25",
        async {
            // Facet the "raft" BM25 candidate set (d1, d2) by status.
            let mut q = AggregateQuery::new("Doc");
            q.text_search = Some(Box::new(TextSearch {
                field: "body".into(),
                query: "raft".into(),
                operator: TextOperator::Or,
                rank: TextRank::Bm25,
                k1: None,
                b: None,
                analyzer: None,
            }));
            q.facets = vec![FacetRequest {
                field: "status".into(),
                limit: None,
            }];
            let r = client.aggregate(&q).await?;
            check(r.search_scoped, "scoped to the BM25 candidate set")?;
            check(r.matched == 2, "two 'raft' docs")
        }
        .await,
    );

    report.record(
        "vector_ann_preview",
        async {
            // The approximate (HNSW) preview engages only above a minimum-dataset
            // threshold (below it, exact is both correct and cheaper). Use a
            // dedicated collection seeded above that threshold so the preview is
            // genuinely exercised, not exact-fallback.
            let ann_schema = CollectionSchema::new("AnnDoc")
                .with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::Uuid,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                })
                .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }));
            client.create_schema(&ann_schema).await?;
            for i in 0..24u32 {
                let mut m = Document::new();
                m.insert("id".into(), Value::Text(format!("a{i}")));
                m.insert(
                    "embedding".into(),
                    Value::Vector(vec![1.0 - (i as f32) * 0.01, (i as f32) * 0.01, 0.0]),
                );
                client.insert("AnnDoc", m).await?;
            }

            let mut q = FindQuery::new("AnnDoc");
            q.vector = Some(VectorSearch {
                field: "embedding".into(),
                query: vec![1.0, 0.0, 0.0],
                k: 2,
                metric: "cosine".into(),
            });
            q.vector_ann = Some(AnnParams::default());
            let rows = client.find_all(&q).await?;
            check(rows.len() == 2, "two approximate nearest")?;
            let plan = client.explain(&q).await?;
            let v = plan
                .vector
                .ok_or_else(|| Error::Internal("missing vector plan".into()))?;
            check(v.approximate, "EXPLAIN reports the approximate preview")?;
            check(
                v.vector_mode.as_deref() == Some("ann_preview"),
                "EXPLAIN vector_mode is ann_preview",
            )
        }
        .await,
    );

    report.record(
        "ranked_pagination_search_page",
        async {
            // Page a BM25 search over the wire by cursor token; reconstruct the
            // full ranked order with no duplicates.
            let mut find = FindQuery::new("Doc");
            find.text_search = Some(Box::new(TextSearch {
                field: "body".into(),
                query: "raft".into(),
                operator: TextOperator::Or,
                rank: TextRank::Bm25,
                k1: None,
                b: None,
                analyzer: None,
            }));
            let reference: Vec<String> = client
                .find_all(&find)
                .await?
                .iter()
                .filter_map(|r| match r.fields.get("id") {
                    Some(Value::Text(s)) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            let mut paged: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let page = client.search_page(&find, 1, cursor.clone()).await?;
                for row in &page.rows {
                    if let Some(Value::Text(s)) = row.fields.get("id") {
                        paged.push(s.clone());
                    }
                }
                match page.next_cursor {
                    Some(t) => cursor = Some(t),
                    None => break,
                }
            }
            check(!reference.is_empty(), "BM25 matched some docs")?;
            check(paged == reference, "paging reconstructs the ranked order")
        }
        .await,
    );

    Ok(report)
}
