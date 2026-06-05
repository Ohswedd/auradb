//! An async AuraDB protocol client - the stand-in for Aura Connector used by
//! the conformance suite. It implements the client side of AWP independently of
//! the server crate.

use auradb::core::{CollectionSchema, Document, Error, ErrorCode, Result};
use auradb::query::{
    CountQuery, ExistsQuery, ExplainPlan, FindQuery, MigrationEstimate, Mutation, MutationResult,
    QueryResultPage, ReadRequest, Row,
};
use auradb_protocol::{
    AuthRequest, AuthResult, CursorCloseRequest, CursorFetchRequest, ErrorPayload, Frame,
    HealthReport, HelloAck, HelloRequest, Opcode, RequestId, DEFAULT_MAX_PAYLOAD,
    FLAG_PAYLOAD_CHECKSUM, HEADER_LEN, PROTOCOL_VERSION,
};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// Client TLS settings: a CA bundle to trust and the server name to verify.
#[derive(Debug, Clone)]
pub struct ClientTls {
    /// PEM CA bundle used to verify the server certificate.
    pub ca_cert_path: PathBuf,
    /// The server name verified against the certificate SAN.
    pub server_name: String,
}

/// Options controlling how a [`Client`] connects.
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    /// A static token presented at handshake time.
    pub auth_token: Option<String>,
    /// TLS settings; `None` connects over plain TCP.
    pub tls: Option<ClientTls>,
}

/// A connection that is either plain TCP or a TLS session over TCP.
enum Conn {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Conn::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Conn::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_flush(cx),
            Conn::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Conn::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

fn build_tls_connector(tls: &ClientTls) -> Result<TlsConnector> {
    let bytes = std::fs::read(&tls.ca_cert_path).map_err(Error::Io)?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &bytes[..]) {
        let cert = cert.map_err(|e| Error::Config(format!("parsing CA bundle: {e}")))?;
        roots
            .add(cert)
            .map_err(|e| Error::Config(format!("adding CA certificate: {e}")))?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| Error::Config(format!("tls configuration: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

/// An async AuraDB client connection.
pub struct Client {
    stream: Conn,
    next_request_id: u128,
    max_payload: usize,
    auth_token: Option<String>,
}

impl Client {
    /// Connect to an AuraDB server over plain TCP and perform the HELLO
    /// handshake.
    pub async fn connect(addr: &str) -> Result<Client> {
        Client::connect_with(addr, ConnectOptions::default()).await
    }

    /// Connect over plain TCP and authenticate with a static token presented at
    /// handshake time.
    pub async fn connect_with_token(addr: &str, token: &str) -> Result<Client> {
        Client::connect_with(
            addr,
            ConnectOptions {
                auth_token: Some(token.to_string()),
                tls: None,
            },
        )
        .await
    }

    /// Connect with full control over authentication and TLS.
    pub async fn connect_with(addr: &str, options: ConnectOptions) -> Result<Client> {
        let tcp = TcpStream::connect(addr).await.map_err(Error::Io)?;
        let stream = match &options.tls {
            None => Conn::Plain(tcp),
            Some(tls) => {
                let connector = build_tls_connector(tls)?;
                let name = rustls::pki_types::ServerName::try_from(tls.server_name.clone())
                    .map_err(|e| Error::Config(format!("invalid TLS server name: {e}")))?;
                let session = connector.connect(name, tcp).await.map_err(Error::Io)?;
                Conn::Tls(Box::new(session))
            }
        };
        let mut client = Client {
            stream,
            next_request_id: 1,
            max_payload: DEFAULT_MAX_PAYLOAD,
            auth_token: options.auth_token,
        };
        client.hello().await?;
        Ok(client)
    }

    fn request_id(&mut self) -> RequestId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        RequestId(id)
    }

    async fn call(&mut self, opcode: Opcode, txn_id: u64, payload: Vec<u8>) -> Result<Frame> {
        let req = Frame::new(opcode, self.request_id(), txn_id, payload);
        self.write(&req).await?;
        let resp = self.read().await?;
        if resp.opcode == Opcode::Error {
            let payload: ErrorPayload = resp.decode_json()?;
            return Err(error_from_payload(payload));
        }
        Ok(resp)
    }

    async fn call_json<T: Serialize>(
        &mut self,
        opcode: Opcode,
        txn_id: u64,
        value: &T,
    ) -> Result<Frame> {
        let payload =
            serde_json::to_vec(value).map_err(|e| Error::Protocol(format!("serialize: {e}")))?;
        self.call(opcode, txn_id, payload).await
    }

    async fn write(&mut self, frame: &Frame) -> Result<()> {
        let bytes = frame.encode();
        self.stream.write_all(&bytes).await.map_err(Error::Io)?;
        self.stream.flush().await.map_err(Error::Io)?;
        Ok(())
    }

    async fn read(&mut self) -> Result<Frame> {
        let mut header = [0u8; HEADER_LEN];
        self.stream
            .read_exact(&mut header)
            .await
            .map_err(Error::Io)?;
        let payload_len =
            u32::from_be_bytes([header[12], header[13], header[14], header[15]]) as usize;
        let flags = u16::from_be_bytes([header[6], header[7]]);
        let trailer = if flags & FLAG_PAYLOAD_CHECKSUM != 0 {
            4
        } else {
            0
        };
        let mut full = Vec::with_capacity(HEADER_LEN + payload_len + trailer);
        full.extend_from_slice(&header);
        full.resize(HEADER_LEN + payload_len + trailer, 0);
        self.stream
            .read_exact(&mut full[HEADER_LEN..])
            .await
            .map_err(Error::Io)?;
        Frame::decode(&full, self.max_payload)?
            .map(|(f, _)| f)
            .ok_or_else(|| Error::Protocol("incomplete response frame".into()))
    }

    // ----- operations -----

    /// Perform the HELLO handshake, returning the server's capabilities.
    pub async fn hello(&mut self) -> Result<HelloAck> {
        let req = HelloRequest {
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            auth_token: self.auth_token.clone(),
        };
        let frame = self.call_json(Opcode::Hello, 0, &req).await?;
        frame.decode_json()
    }

    /// Authenticate the current connection with a static token via an `AUTH`
    /// frame. Returns the authentication result.
    pub async fn authenticate(&mut self, token: &str) -> Result<AuthResult> {
        let req = AuthRequest {
            token: token.to_string(),
        };
        let frame = self.call_json(Opcode::Auth, 0, &req).await?;
        frame.decode_json()
    }

    /// Liveness probe.
    pub async fn ping(&mut self) -> Result<()> {
        let frame = self.call(Opcode::Ping, 0, b"ping".to_vec()).await?;
        if frame.opcode == Opcode::Pong {
            Ok(())
        } else {
            Err(Error::Protocol("expected PONG".into()))
        }
    }

    /// Fetch the server health report.
    pub async fn health(&mut self) -> Result<HealthReport> {
        let frame = self.call(Opcode::Health, 0, Vec::new()).await?;
        frame.decode_json()
    }

    /// Register a collection schema.
    pub async fn create_schema(&mut self, schema: &CollectionSchema) -> Result<()> {
        self.call_json(Opcode::SchemaCreate, 0, schema)
            .await
            .map(|_| ())
    }

    /// Drop a collection schema.
    pub async fn drop_schema(&mut self, name: &str) -> Result<()> {
        self.call_json(Opcode::SchemaDrop, 0, &serde_json::json!({ "name": name }))
            .await
            .map(|_| ())
    }

    /// Fetch a collection schema.
    pub async fn get_schema(&mut self, name: &str) -> Result<CollectionSchema> {
        let frame = self
            .call_json(Opcode::SchemaGet, 0, &serde_json::json!({ "name": name }))
            .await?;
        frame.decode_json()
    }

    /// List all collection schemas.
    pub async fn list_schemas(&mut self) -> Result<Vec<CollectionSchema>> {
        let frame = self.call(Opcode::SchemaList, 0, Vec::new()).await?;
        frame.decode_json()
    }

    /// Apply a mutation (auto-commit, or within a transaction if `txn_id != 0`).
    pub async fn mutate(&mut self, txn_id: u64, mutation: &Mutation) -> Result<MutationResult> {
        let frame = self.call_json(Opcode::Mutate, txn_id, mutation).await?;
        frame.decode_json()
    }

    /// Insert a record (auto-commit).
    pub async fn insert(&mut self, collection: &str, fields: Document) -> Result<MutationResult> {
        self.mutate(
            0,
            &Mutation::Insert {
                collection: collection.to_string(),
                fields,
            },
        )
        .await
    }

    /// Run a find, returning the first page (with a cursor id if more remain).
    pub async fn find_page(&mut self, query: &FindQuery) -> Result<QueryResultPage> {
        self.find_page_in_txn(0, query).await
    }

    /// Run a find within a transaction (`txn_id != 0` reads the transaction
    /// view; `0` is an ordinary committed read).
    pub async fn find_page_in_txn(
        &mut self,
        txn_id: u64,
        query: &FindQuery,
    ) -> Result<QueryResultPage> {
        let frame = self
            .call_json(Opcode::Query, txn_id, &ReadRequest::Find(query.clone()))
            .await?;
        frame.decode_json()
    }

    /// Run a find and follow cursors to collect all rows.
    pub async fn find_all(&mut self, query: &FindQuery) -> Result<Vec<Row>> {
        self.find_all_in_txn(0, query).await
    }

    /// Run a find within a transaction, following cursors through the same
    /// transaction so staged writes stay visible across pages.
    pub async fn find_all_in_txn(&mut self, txn_id: u64, query: &FindQuery) -> Result<Vec<Row>> {
        let mut page = self.find_page_in_txn(txn_id, query).await?;
        let mut rows = page.rows;
        while let Some(cursor_id) = page.cursor_id {
            page = self.cursor_fetch_in_txn(txn_id, cursor_id, 100).await?;
            rows.extend(page.rows);
        }
        Ok(rows)
    }

    /// Fetch a page from a cursor.
    pub async fn cursor_fetch(&mut self, cursor_id: u64, limit: usize) -> Result<QueryResultPage> {
        self.cursor_fetch_in_txn(0, cursor_id, limit).await
    }

    /// Fetch a page from a cursor within a transaction.
    pub async fn cursor_fetch_in_txn(
        &mut self,
        txn_id: u64,
        cursor_id: u64,
        limit: usize,
    ) -> Result<QueryResultPage> {
        let frame = self
            .call_json(
                Opcode::CursorFetch,
                txn_id,
                &CursorFetchRequest { cursor_id, limit },
            )
            .await?;
        frame.decode_json()
    }

    /// Close a cursor.
    pub async fn cursor_close(&mut self, cursor_id: u64) -> Result<()> {
        self.call_json(Opcode::CursorClose, 0, &CursorCloseRequest { cursor_id })
            .await
            .map(|_| ())
    }

    /// Count matching records.
    pub async fn count(&mut self, query: &CountQuery) -> Result<usize> {
        self.count_in_txn(0, query).await
    }

    /// Count matching records within a transaction.
    pub async fn count_in_txn(&mut self, txn_id: u64, query: &CountQuery) -> Result<usize> {
        let frame = self
            .call_json(Opcode::Query, txn_id, &ReadRequest::Count(query.clone()))
            .await?;
        let v: serde_json::Value = frame.decode_json()?;
        Ok(v["count"].as_u64().unwrap_or(0) as usize)
    }

    /// Test whether any record matches.
    pub async fn exists(&mut self, query: &ExistsQuery) -> Result<bool> {
        self.exists_in_txn(0, query).await
    }

    /// Test whether any record matches within a transaction.
    pub async fn exists_in_txn(&mut self, txn_id: u64, query: &ExistsQuery) -> Result<bool> {
        let frame = self
            .call_json(Opcode::Query, txn_id, &ReadRequest::Exists(query.clone()))
            .await?;
        let v: serde_json::Value = frame.decode_json()?;
        Ok(v["exists"].as_bool().unwrap_or(false))
    }

    /// Produce an EXPLAIN plan.
    pub async fn explain(&mut self, query: &FindQuery) -> Result<ExplainPlan> {
        let frame = self.call_json(Opcode::Explain, 0, query).await?;
        frame.decode_json()
    }

    /// Produce an `EXPLAIN ANALYZE` plan: the analyze flag is carried as an
    /// optional sibling key in the raw Query IR, so this needs no protocol
    /// addition (older clients simply omit it).
    pub async fn explain_analyze(&mut self, query: &FindQuery) -> Result<ExplainPlan> {
        let mut value =
            serde_json::to_value(query).map_err(|e| auradb_core::Error::Internal(e.to_string()))?;
        if let serde_json::Value::Object(map) = &mut value {
            map.insert("analyze".into(), serde_json::Value::Bool(true));
        }
        let frame = self.call_json(Opcode::Explain, 0, &value).await?;
        frame.decode_json()
    }

    /// Estimate the impact of a schema migration.
    pub async fn migration_estimate(
        &mut self,
        target: &CollectionSchema,
    ) -> Result<MigrationEstimate> {
        let frame = self.call_json(Opcode::MigrationEstimate, 0, target).await?;
        frame.decode_json()
    }

    /// Begin a transaction, returning its id.
    pub async fn begin(&mut self) -> Result<u64> {
        let frame = self.call(Opcode::TxnBegin, 0, Vec::new()).await?;
        let v: serde_json::Value = frame.decode_json()?;
        v["txn_id"]
            .as_u64()
            .ok_or_else(|| Error::Protocol("missing txn_id".into()))
    }

    /// Commit a transaction.
    pub async fn commit(&mut self, txn_id: u64) -> Result<()> {
        self.call(Opcode::TxnCommit, txn_id, Vec::new())
            .await
            .map(|_| ())
    }

    /// Roll back a transaction.
    pub async fn rollback(&mut self, txn_id: u64) -> Result<()> {
        self.call(Opcode::TxnRollback, txn_id, Vec::new())
            .await
            .map(|_| ())
    }

    /// Send a raw frame and return the raw response (used to test malformed
    /// inputs and protocol-level behavior).
    pub async fn raw(&mut self, frame: &Frame) -> Result<Frame> {
        self.write(frame).await?;
        self.read().await
    }

    /// Allocate a request id without sending (for raw frames).
    pub fn next_request_id(&mut self) -> RequestId {
        self.request_id()
    }
}

fn error_from_payload(p: ErrorPayload) -> Error {
    match p.code {
        ErrorCode::Conflict => Error::Conflict(p.message),
        ErrorCode::TransactionTimeout => Error::TransactionTimeout(p.message),
        ErrorCode::UniqueViolation => Error::UniqueViolation(p.message),
        ErrorCode::NotFound => Error::NotFound(p.message),
        ErrorCode::SchemaViolation => Error::SchemaViolation(p.message),
        ErrorCode::Unsupported => Error::unsupported(p.message),
        ErrorCode::Unauthenticated => Error::Unauthenticated(p.message),
        ErrorCode::InvalidCredentials => Error::InvalidCredentials,
        ErrorCode::InvalidRequest => Error::InvalidRequest(p.message),
        ErrorCode::Protocol => Error::Protocol(p.message),
        ErrorCode::Corruption => Error::Corruption(p.message),
        ErrorCode::Storage => Error::Storage(p.message),
        ErrorCode::Config => Error::Config(p.message),
        ErrorCode::LimitExceeded => Error::LimitExceeded(p.message),
        ErrorCode::Io => Error::Internal(p.message),
        ErrorCode::Internal => Error::Internal(p.message),
    }
}
