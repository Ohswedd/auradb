# Relationships

AuraDB models typed relationships (links) between collections as first-class
schema concepts.

## Declaring a relationship

```json
{
  "name": "owner",
  "target": "User",
  "cardinality": "to_one",
  "on_delete": "restrict"
}
```

- `cardinality` is `to_one` or `to_many`.
- `on_delete` is `restrict` (reject deletion while referenced) or `set_null`
  (allow deletion; the link becomes unresolved on read).

A record stores the link as the **target's primary-key value**: a string for
`to_one`, an array of strings for `to_many`.

## Resolution

Internally records are addressed by a derived `RecordId` (a stable hash of the
collection name and primary-key value). The engine resolves a link by deriving
the target id from the stored key - so clients link by natural primary-key value
rather than by internal id.

## Referential integrity

- **On write**, every relationship value must reference an existing target
  record, or the write is rejected with a schema violation. Insert parents
  before children.
- **On delete**, a `restrict` relationship that points at the record being
  deleted blocks the deletion with a conflict error. `set_null` allows it.

## Include (hydration)

A find can hydrate related records into each row:

```json
{"collection": "Doc", "includes": ["owner"]}
```

Each returned row gains `includes["owner"]`, an array of the related records'
fields (one element for `to_one`, many for `to_many`).

## Limitations

Reverse-link queries, multi-hop graph traversal, and adjacency indexes are
**not implemented**; the engine provides forward links, hydration, and
referential integrity. Physical-pointer link optimization is future work; see
[ROADMAP](ROADMAP.md).
