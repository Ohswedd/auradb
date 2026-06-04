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

## Limitations

Document **path indexes** (a secondary index on a nested path such as
`metadata.status`) are not built; nested-field filters are evaluated by scanning
candidate records. Declaring an index on a nested path is future work.
