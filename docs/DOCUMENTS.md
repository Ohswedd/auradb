# Documents

AuraDB records are documents: ordered maps of field name to `Value`. The value
model is a superset of JSON.

## Value model

`Value` is `Null`, `Bool`, `Int (i64)`, `Float (f64)`, `Text`, `Bytes`,
`Timestamp (epoch ms)`, `Vector`, `Array`, or `Object` (a nested document).

Plain scalars, arrays, and objects map to natural JSON so the wire format stays
Connector-compatible. Extension types use reserved `$`-prefixed keys:

- `{"$vector": [ ... ]}`
- `{"$timestamp": 1717459200000}`
- `{"$bytes": [ ... ]}`

## Nested documents

A field of type `document` stores arbitrary nested objects and arrays. Nested
values are validated, persisted, and queryable.

## Path access

Filters, ordering, and projections accept **dotted paths** to reach into nested
documents, e.g. `metadata.status` or `metadata.source`:

```json
{"type": "compare", "field": "metadata.status", "op": "eq", "value": "published"}
{"type": "exists", "field": "metadata.classification"}
```

`Value::get_path` and `Record::get_path` resolve these paths.

## Validation

When a field is declared `document`, the engine validates that the stored value
is an object. Untyped nested structure inside a document is accepted as-is.

## Document-path indexes

A document-path index accelerates equality filters on a nested document value
addressed by a dotted path. Declare it in a schema via an `indexes` array:

```json
{ "indexes": [ { "path": "profile.company", "kind": "document_path" } ] }
```

- The query planner uses the index for equality filters on that path and reports
  it in EXPLAIN as `strategy: index_lookup` with `used_index: "profile.company"`.
- Records that do not contain the path are simply not matched.
- Updates and deletes maintain the index, and it is persisted and restored across
  restart (see [INDEXING.md](INDEXING.md) and [STORAGE_ENGINE.md](STORAGE_ENGINE.md)).
- Schema validation rejects an index whose path root is not a declared field, and
  a multi-segment `document_path` whose root is not a `document` field.

### Nested-array caveat

A document-path index indexes the single value found at the exact path. Indexing
the individual elements of an array located at a path is **not** supported; if the
value at the path is an array, the array as a whole is what the index sees, not
its members.
