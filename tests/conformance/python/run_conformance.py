#!/usr/bin/env python3
"""AuraDB conformance harness (Python).

A real, runnable Aura Wire Protocol client implemented in pure Python (standard
library only). It connects to a running AuraDB server and exercises every
v0.2.0 capability over the wire, optionally authenticating with a static token
and connecting over TLS.

Usage:
    # Start a server first, e.g.:
    #   auradb server --data-dir .local/auradb --bind 127.0.0.1 --port 7171
    python run_conformance.py --addr 127.0.0.1:7171
    python run_conformance.py --addr 127.0.0.1:7171 --auth-token dev-secret
    python run_conformance.py --addr 127.0.0.1:7171 \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost --auth-token dev-secret

The published Aura Connector drives the same server through its native AuraDB
backend (aura-connector >= 0.3.0); see docs/AURA_CONNECTOR_COMPATIBILITY.md.

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

import argparse
import json
import socket
import ssl
import struct
import sys
import zlib

MAGIC = b"AURA"
VERSION = 1
HEADER_LEN = 44
FLAG_PAYLOAD_CHECKSUM = 0x0001

# Opcodes (must match crates/auradb-protocol/src/opcode.rs).
OP = {
    "Hello": 0x01,
    "Ping": 0x02,
    "Health": 0x03,
    "Auth": 0x04,
    "SchemaCreate": 0x10,
    "SchemaGet": 0x12,
    "SchemaList": 0x13,
    "Query": 0x20,
    "Mutate": 0x21,
    "CursorFetch": 0x22,
    "CursorClose": 0x23,
    "Explain": 0x24,
    "MigrationEstimate": 0x25,
    "TxnBegin": 0x30,
    "TxnCommit": 0x31,
    "TxnRollback": 0x32,
    "HelloAck": 0x81,
    "Pong": 0x82,
    "HealthResult": 0x83,
    "AuthResult": 0x84,
    "Ok": 0x90,
    "QueryResult": 0x91,
    "Error": 0xFF,
}
NAME = {v: k for k, v in OP.items()}


class AuraError(Exception):
    pass


class Client:
    def __init__(self, host, port, auth_token=None, tls_ca=None, server_name="localhost"):
        raw = socket.create_connection((host, port))
        if tls_ca:
            ctx = ssl.create_default_context(cafile=tls_ca)
            self.sock = ctx.wrap_socket(raw, server_hostname=server_name)
        else:
            self.sock = raw
        self.req_id = 0
        self.auth_token = auth_token
        self.hello()

    def _encode(self, opcode, txn_id, payload):
        self.req_id += 1
        header = bytearray()
        header += MAGIC
        header.append(VERSION)
        header.append(opcode)
        header += struct.pack(">H", FLAG_PAYLOAD_CHECKSUM)
        header += struct.pack(">H", HEADER_LEN)
        header.append(0)  # compression
        header.append(0)  # reserved
        header += struct.pack(">I", len(payload))
        header += self.req_id.to_bytes(16, "big")
        header += struct.pack(">Q", txn_id)
        header += struct.pack(">I", zlib.crc32(bytes(header)) & 0xFFFFFFFF)
        frame = bytes(header) + payload
        frame += struct.pack(">I", zlib.crc32(payload) & 0xFFFFFFFF)
        return frame

    def _recv_exact(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise AuraError("connection closed")
            buf += chunk
        return buf

    def _read(self):
        header = self._recv_exact(HEADER_LEN)
        opcode = header[5]
        flags = struct.unpack(">H", header[6:8])[0]
        payload_len = struct.unpack(">I", header[12:16])[0]
        trailer = 4 if flags & FLAG_PAYLOAD_CHECKSUM else 0
        rest = self._recv_exact(payload_len + trailer)
        payload = rest[:payload_len]
        return opcode, payload

    def call(self, op_name, payload_obj=None, txn_id=0):
        payload = b"" if payload_obj is None else json.dumps(payload_obj).encode()
        self.sock.sendall(self._encode(OP[op_name], txn_id, payload))
        opcode, resp = self._read()
        if opcode == OP["Error"]:
            err = json.loads(resp)
            raise AuraError(f"{err.get('code')}: {err.get('message')}")
        return opcode, (json.loads(resp) if resp else None)

    def hello(self):
        payload = {"client_version": "py", "protocol_version": 1}
        if self.auth_token is not None:
            payload["auth_token"] = self.auth_token
        return self.call("Hello", payload)[1]

    def authenticate(self, token):
        return self.call("Auth", {"token": token})[1]

    def ping(self):
        op, _ = self.call("Ping", None)
        assert op == OP["Pong"], "expected PONG"

    def health(self):
        return self.call("Health")[1]

    def create_schema(self, schema):
        self.call("SchemaCreate", schema)

    def list_schemas(self):
        return self.call("SchemaList")[1]

    def mutate(self, mutation, txn_id=0):
        return self.call("Mutate", mutation, txn_id=txn_id)[1]

    def find_all(self, query, txn_id=0):
        op, page = self.call("Query", {"query": "find", **query}, txn_id=txn_id)
        rows = page["rows"]
        while page.get("cursor_id") is not None:
            op, page = self.call(
                "CursorFetch", {"cursor_id": page["cursor_id"], "limit": 100}, txn_id=txn_id
            )
            rows += page["rows"]
        return rows

    def count(self, collection, flt=None, txn_id=0):
        _, v = self.call(
            "Query", {"query": "count", "collection": collection, "filter": flt}, txn_id=txn_id
        )
        return v["count"]

    def exists(self, collection, flt=None, txn_id=0):
        _, v = self.call(
            "Query", {"query": "exists", "collection": collection, "filter": flt}, txn_id=txn_id
        )
        return v["exists"]

    def explain(self, query):
        return self.call("Explain", query)[1]

    def migration_estimate(self, schema):
        return self.call("MigrationEstimate", schema)[1]

    def begin(self):
        return self.call("TxnBegin")[1]["txn_id"]

    def commit(self, txn_id):
        self.call("TxnCommit", None, txn_id=txn_id)

    def rollback(self, txn_id):
        self.call("TxnRollback", None, txn_id=txn_id)


def field(name, ftype, **kw):
    f = {"name": name, "field_type": {"kind": ftype}}
    f.update(kw)
    return f


def vector_field(name, dim):
    return {"name": name, "field_type": {"kind": "vector", "dim": dim}}


USER_SCHEMA = {
    "name": "User",
    "fields": [field("id", "uuid", primary_key=True, unique=True, nullable=False)],
    "relationships": [],
}
DOC_SCHEMA = {
    "name": "Doc",
    "fields": [
        field("id", "uuid", primary_key=True, unique=True, nullable=False),
        field("status", "string", indexed=True),
        field("title", "string"),
        field("body", "string"),
        field("views", "int"),
        field("metadata", "document"),
        vector_field("embedding", 3),
    ],
    "relationships": [
        {"name": "owner", "target": "User", "cardinality": "to_one", "on_delete": "restrict"}
    ],
    "indexes": [
        {"path": "metadata.source", "kind": "document_path"},
        {"path": "body", "kind": "full_text"},
    ],
}


def doc(id, status, views, emb):
    return {
        "id": id,
        "status": status,
        "title": f"Title {id}",
        "body": f"alpha document number {id}",
        "views": views,
        "owner": "u1",
        "embedding": {"$vector": emb},
        "metadata": {"source": "import"},
    }


SCENARIOS = []


def scenario(fn):
    SCENARIOS.append(fn)
    return fn


@scenario
def ping(c):
    c.ping()


@scenario
def health(c):
    assert c.health()["ready"] is True


@scenario
def cluster_health_shape(c):
    # The cluster health section is additive and present only in cluster mode.
    # When present (single-node cluster server), validate its honest shape; when
    # absent (the recommended non-cluster server), the scenario is a no-op. This
    # keeps the suite green against both deployment modes while exercising the
    # additive field over the wire.
    report = c.health()
    cluster = report.get("cluster")
    if cluster is None:
        return
    assert cluster["enabled"] is True
    assert cluster["role"] in ("leader", "follower", "candidate")
    assert isinstance(cluster["term"], int)
    assert isinstance(cluster["commit_index"], int)
    assert isinstance(cluster["applied_index"], int)
    # A single-node cluster has no peers and its applied index never exceeds
    # what has been committed.
    assert cluster["applied_index"] <= cluster["commit_index"]


@scenario
def schema_create(c):
    c.create_schema(USER_SCHEMA)
    c.create_schema(DOC_SCHEMA)
    assert len(c.list_schemas()) >= 2


@scenario
def insert(c):
    c.mutate({"mutation": "insert", "collection": "User", "fields": {"id": "u1"}})
    for d in (doc("d1", "published", 10, [1.0, 0.0, 0.0]),
              doc("d2", "draft", 5, [0.0, 1.0, 0.0]),
              doc("d3", "published", 20, [0.9, 0.1, 0.0])):
        c.mutate({"mutation": "insert", "collection": "Doc", "fields": d})


@scenario
def find(c):
    assert len(c.find_all({"collection": "Doc"})) == 3


@scenario
def filter_scenario(c):
    rows = c.find_all({"collection": "Doc", "filter": {"type": "compare", "field": "status", "op": "eq", "value": "published"}})
    assert len(rows) == 2


@scenario
def document_field(c):
    rows = c.find_all({"collection": "Doc", "filter": {"type": "compare", "field": "metadata.source", "op": "eq", "value": "import"}})
    assert len(rows) == 3


@scenario
def document_path_index(c):
    flt = {"type": "compare", "field": "metadata.source", "op": "eq", "value": "import"}
    plan = c.explain({"collection": "Doc", "filter": flt})
    assert plan["used_index"] == "metadata.source", plan
    assert plan["strategy"] == "index_lookup", plan
    assert len(c.find_all({"collection": "Doc", "filter": flt})) == 3


@scenario
def full_text_search(c):
    common = {"type": "contains_text", "field": "body", "query": "document"}
    assert len(c.find_all({"collection": "Doc", "filter": common})) == 3
    unique = {"type": "contains_text", "field": "body", "query": "d1"}
    assert len(c.find_all({"collection": "Doc", "filter": unique})) == 1
    plan = c.explain({"collection": "Doc", "filter": common})
    assert plan["strategy"] == "full_text_scan", plan
    assert plan["used_index"] == "body", plan


@scenario
def relationship_include(c):
    rows = c.find_all({"collection": "Doc", "includes": ["owner"], "limit": 1})
    assert rows and len(rows[0]["includes"]["owner"]) == 1


@scenario
def vector_nearest(c):
    rows = c.find_all({"collection": "Doc", "vector": {"field": "embedding", "query": [1.0, 0.0, 0.0], "k": 2, "metric": "cosine"}})
    assert len(rows) == 2 and rows[0]["fields"]["id"] == "d1"


@scenario
def explain(c):
    plan = c.explain({"collection": "Doc", "filter": {"type": "compare", "field": "status", "op": "eq", "value": "published"}})
    assert plan["used_index"] == "status"


@scenario
def count_and_exists(c):
    assert c.count("Doc") == 3
    assert c.exists("Doc", {"type": "compare", "field": "id", "op": "eq", "value": "d1"})


@scenario
def migration_estimate(c):
    target = json.loads(json.dumps(DOC_SCHEMA))
    target["fields"].append(field("category", "string", indexed=True))
    est = c.migration_estimate(target)
    assert est["exists"] and "category" in est["new_indexes"]


@scenario
def update_upsert_delete(c):
    r = c.mutate({"mutation": "update", "collection": "Doc",
                  "filter": {"type": "compare", "field": "id", "op": "eq", "value": "d2"},
                  "set": {"status": "published"}})
    assert r["updated"] == 1
    r = c.mutate({"mutation": "upsert", "collection": "Doc", "fields": doc("d1", "archived", 99, [1.0, 0.0, 0.0])})
    assert r["updated"] == 1
    r = c.mutate({"mutation": "delete", "collection": "Doc",
                  "filter": {"type": "compare", "field": "id", "op": "eq", "value": "d3"}})
    assert r["deleted"] == 1


@scenario
def transaction(c):
    txn = c.begin()
    c.mutate({"mutation": "insert", "collection": "Doc", "fields": doc("d4", "draft", 1, [0.0, 0.0, 1.0])}, txn_id=txn)
    c.commit(txn)
    assert c.exists("Doc", {"type": "compare", "field": "id", "op": "eq", "value": "d4"})
    txn = c.begin()
    c.mutate({"mutation": "insert", "collection": "Doc", "fields": doc("d5", "draft", 1, [0.0, 0.0, 1.0])}, txn_id=txn)
    c.rollback(txn)
    assert not c.exists("Doc", {"type": "compare", "field": "id", "op": "eq", "value": "d5"})


@scenario
def transaction_scoped_reads(c):
    id_eq = {"type": "compare", "field": "id", "op": "eq", "value": "d6"}
    txn = c.begin()
    c.mutate({"mutation": "insert", "collection": "Doc", "fields": doc("d6", "draft", 1, [0.0, 0.0, 1.0])}, txn_id=txn)
    # Read-your-writes: visible to the transaction's own reads...
    assert c.exists("Doc", id_eq, txn_id=txn), "staged write must be visible within the transaction"
    assert len(c.find_all({"collection": "Doc", "filter": id_eq}, txn_id=txn)) == 1
    # ...but not to a non-transactional read until commit.
    assert not c.exists("Doc", id_eq), "staged write must be invisible to non-transactional reads"
    c.commit(txn)
    assert c.exists("Doc", id_eq), "committed write must be visible after commit"


def main():
    parser = argparse.ArgumentParser(description="AuraDB Python conformance harness")
    parser.add_argument("--addr", default="127.0.0.1:7171")
    parser.add_argument("--auth-token", default=None, help="static token for an auth-enabled server")
    parser.add_argument("--tls-ca", default=None, help="PEM CA bundle to trust (enables TLS)")
    parser.add_argument("--tls-server-name", default="localhost")
    args = parser.parse_args()
    host, port = args.addr.rsplit(":", 1)
    client = Client(
        host,
        int(port),
        auth_token=args.auth_token,
        tls_ca=args.tls_ca,
        server_name=args.tls_server_name,
    )

    passed, failed = 0, 0
    for fn in SCENARIOS:
        name = fn.__name__
        try:
            fn(client)
            print(f"  [PASS] {name}")
            passed += 1
        except Exception as exc:  # noqa: BLE001 - report any failure
            print(f"  [FAIL] {name}: {exc}")
            failed += 1

    print(f"\nConformance: {passed}/{passed + failed} scenarios passed")
    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()
