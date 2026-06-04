# Server-Side Cursors

`auradb-server` provides real server-side cursors for paging query results.

## Lifecycle

1. A `Query`/`find` returns the first page (up to `page_size` rows). If more
   rows match, the response carries a `cursor_id`.
2. The client sends `CursorFetch { cursor_id, limit }` to retrieve subsequent
   pages. Each page indicates whether more remain.
3. When the result is exhausted the cursor is closed automatically; the client
   may also close early with `CursorClose { cursor_id }`.

## Bounded memory

A cursor stores only the **ordered record ids** of the planned result (plus any
vector scores), not materialized rows. Rows are materialized per page on demand
from the engine. Memory is therefore bounded by the id-list size rather than the
full payload.

## Timeouts and cleanup

Cursors have an idle timeout (`cursor_timeout_secs`). A background reaper removes
cursors idle past the timeout and updates the `active_cursors` gauge. Cursors
are also closed when their owning connection disconnects, so a dropped client
never leaks cursors.

## Honesty

The planned id list itself is materialized up front (bounded by the matched-row
count, or by `limit` when set). True streaming of an unbounded result set
without holding all ids is future work; until then, bound large queries with a
`limit`.

## Tests

Paging through a cursor, explicit close, timeout reaping (unit), and end-to-end
streaming over the wire with a small page size in the conformance suite.
