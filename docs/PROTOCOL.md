# Aura Wire Protocol (AWP)

`auradb-protocol` implements AWP: a binary, checksummed, versioned frame format
with opaque JSON payloads. The frame header layout is documented below.

> **AWP 1 is frozen for v1.** AuraDB v1.0.0 uses Aura Wire Protocol 1 (the
> `version` byte is `1`, `PROTOCOL_VERSION = 1`). AWP 1 is the stable v1 wire
> protocol. AuraDB v1.x will preserve AWP 1 compatibility unless a security or
> correctness issue requires a documented compatibility break. The recommended
> client is Aura Connector v0.4.1 (and compatible 0.4.x). See
> [COMPATIBILITY.md](COMPATIBILITY.md).

## Frame layout

```
offset  field
0..4    magic "AURA"
4       version (1)
5       opcode (frame type)
6..8    flags (u16 BE)
8..10   header length (u16 BE) - always 44
10      compression (0 = none)
11      reserved (0)
12..16  payload length (u32 BE)
16..32  request id (u128 BE)
32..40  transaction id (u64 BE)
40..44  header checksum (CRC32 of bytes 0..40, BE)
44..    payload
[payload checksum] CRC32 of payload (BE), present iff FLAG_PAYLOAD_CHECKSUM (0x0001)
```

Header length is fixed at 44 bytes. CRC32 (`crc32fast`) protects both the header
and, when the flag is set, the payload.

## Opcodes

Requests: `Hello (0x01)`, `Ping (0x02)`, `Health (0x03)`, `SchemaCreate (0x10)`,
`SchemaDrop (0x11)`, `SchemaGet (0x12)`, `SchemaList (0x13)`, `Query (0x20)`,
`Mutate (0x21)`, `CursorFetch (0x22)`, `CursorClose (0x23)`, `Explain (0x24)`,
`MigrationEstimate (0x25)`, `TxnBegin (0x30)`, `TxnCommit (0x31)`,
`TxnRollback (0x32)`.

Responses: `HelloAck (0x81)`, `Pong (0x82)`, `HealthResult (0x83)`, `Ok (0x90)`,
`QueryResult (0x91)`, `Error (0xFF)`.

Opcode byte values are part of the wire contract and are stable.

## Versioning and negotiation

The client sends its maximum protocol version in `HELLO`; the server replies
with `min(client, server)` and its capability list. Frames with version `0` or
greater than the server's maximum are rejected.

## Errors

Errors are `ERROR` frames carrying `{ "code": <stable code>, "message": <text> }`.
Codes are stable strings (`conflict`, `unique_violation`, `not_found`,
`unsupported`, …) defined in `auradb-core`.

## Validation and limits

Decoding validates magic, version, header length, header checksum, payload
length against the configured maximum, and (when present) the payload checksum.
Malformed frames yield a structured error; the server then closes the connection
because framing can no longer be trusted.

## Payloads and compatibility

Payloads are JSON (ADR-3) so the Query IR stays transparent and a Python or
future Connector client can interoperate against a documented schema. A
follow-up task pins golden frame and IR fixtures from the published Aura
Connector.

## Tests

Roundtrip, unknown magic, bad version, corrupt header/payload checksum,
oversized payload, unknown opcode, truncated frame, error-frame encoding, cursor
messages, and property/fuzz tests over arbitrary and corrupted bytes.
