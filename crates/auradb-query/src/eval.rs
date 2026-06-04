//! Filter evaluation and value comparison.

use std::cmp::Ordering;

use auradb_core::{Record, Value};

use crate::ir::{CompareOp, Filter};

/// Evaluate a filter against a record.
pub fn matches(record: &Record, filter: &Filter) -> bool {
    match filter {
        Filter::And { filters } => filters.iter().all(|f| matches(record, f)),
        Filter::Or { filters } => filters.iter().any(|f| matches(record, f)),
        Filter::Not { filter } => !matches(record, filter),
        Filter::Exists { field } => record
            .get_path(field)
            .map(|v| !v.is_null())
            .unwrap_or(false),
        Filter::Contains { field, substring } => record
            .get_path(field)
            .and_then(Value::as_text)
            .map(|s| s.contains(substring.as_str()))
            .unwrap_or(false),
        Filter::Compare { field, op, value } => match record.get_path(field) {
            Some(actual) => compare(actual, *op, value),
            None => false,
        },
        Filter::ContainsText { field, query } => {
            match record.get_path(field).and_then(Value::as_text) {
                Some(text) => {
                    let mut terms = auradb_index::tokenize(query);
                    terms.sort();
                    terms.dedup();
                    if terms.is_empty() {
                        return false;
                    }
                    let doc: std::collections::HashSet<String> =
                        auradb_index::tokenize(text).into_iter().collect();
                    terms.iter().all(|t| doc.contains(t))
                }
                None => false,
            }
        }
    }
}

/// Compare a record's field value against a query value with an operator.
fn compare(actual: &Value, op: CompareOp, expected: &Value) -> bool {
    match op {
        CompareOp::Eq => values_equal(actual, expected),
        CompareOp::Ne => !values_equal(actual, expected),
        CompareOp::In => match expected {
            Value::Array(items) => items.iter().any(|item| values_equal(actual, item)),
            _ => false,
        },
        CompareOp::Lt => order(actual, expected) == Some(Ordering::Less),
        CompareOp::Lte => matches!(
            order(actual, expected),
            Some(Ordering::Less | Ordering::Equal)
        ),
        CompareOp::Gt => order(actual, expected) == Some(Ordering::Greater),
        CompareOp::Gte => {
            matches!(
                order(actual, expected),
                Some(Ordering::Greater | Ordering::Equal)
            )
        }
    }
}

/// Equality with numeric coercion (int vs float) and timestamp/int coercion.
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            }
        }
        (Value::Timestamp(x), Value::Int(y)) | (Value::Int(y), Value::Timestamp(x)) => x == y,
        _ => a == b,
    }
}

/// Total-ish ordering for comparable value pairs. Returns `None` for
/// non-comparable pairs (which makes ordering comparisons fail to match).
pub fn order(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            a.as_f64()?.partial_cmp(&b.as_f64()?)
        }
        (Value::Timestamp(x), Value::Timestamp(y)) => x.partial_cmp(y),
        (Value::Timestamp(x), Value::Int(y)) | (Value::Int(x), Value::Timestamp(y)) => {
            x.partial_cmp(y)
        }
        (Value::Text(x), Value::Text(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_core::{CollectionId, Document, RecordId};

    fn rec() -> Record {
        let mut meta = Document::new();
        meta.insert("status".into(), Value::Text("published".into()));
        let mut f = Document::new();
        f.insert("title".into(), Value::Text("Refund policy".into()));
        f.insert("views".into(), Value::Int(42));
        f.insert("metadata".into(), Value::Object(meta));
        Record::new(RecordId::from_u128(1), CollectionId::new("Doc"), f)
    }

    #[test]
    fn eq_and_numeric_coercion() {
        let r = rec();
        assert!(matches(
            &r,
            &Filter::Compare {
                field: "views".into(),
                op: CompareOp::Eq,
                value: Value::Float(42.0),
            }
        ));
    }

    #[test]
    fn comparison_operators() {
        let r = rec();
        assert!(matches(
            &r,
            &Filter::Compare {
                field: "views".into(),
                op: CompareOp::Gt,
                value: Value::Int(10),
            }
        ));
        assert!(!matches(
            &r,
            &Filter::Compare {
                field: "views".into(),
                op: CompareOp::Lt,
                value: Value::Int(10),
            }
        ));
    }

    #[test]
    fn contains_and_exists_and_paths() {
        let r = rec();
        assert!(matches(
            &r,
            &Filter::Contains {
                field: "title".into(),
                substring: "Refund".into(),
            }
        ));
        assert!(matches(
            &r,
            &Filter::Exists {
                field: "metadata.status".into(),
            }
        ));
        assert!(matches(
            &r,
            &Filter::Compare {
                field: "metadata.status".into(),
                op: CompareOp::Eq,
                value: Value::Text("published".into()),
            }
        ));
    }

    #[test]
    fn and_or_not() {
        let r = rec();
        let f = Filter::And {
            filters: vec![
                Filter::Compare {
                    field: "views".into(),
                    op: CompareOp::Gte,
                    value: Value::Int(42),
                },
                Filter::Or {
                    filters: vec![
                        Filter::Not {
                            filter: Box::new(Filter::Exists {
                                field: "missing".into(),
                            }),
                        },
                        Filter::Exists {
                            field: "title".into(),
                        },
                    ],
                },
            ],
        };
        assert!(matches(&r, &f));
    }

    #[test]
    fn in_operator() {
        let r = rec();
        assert!(matches(
            &r,
            &Filter::Compare {
                field: "title".into(),
                op: CompareOp::In,
                value: Value::Array(vec![
                    Value::Text("Other".into()),
                    Value::Text("Refund policy".into()),
                ]),
            }
        ));
    }
}
