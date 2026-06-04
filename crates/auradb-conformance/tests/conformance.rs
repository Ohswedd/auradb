//! End-to-end conformance: start a real AuraDB server on an ephemeral port and
//! run the full scenario suite over TCP.

use std::sync::Arc;

use auradb_conformance::run_all;
use auradb_server::{Config, Server};
use tokio::net::TcpListener;
use tokio::sync::Notify;

async fn start_server(dir: &std::path::Path, page_size: usize) -> (String, Arc<Notify>) {
    let config = Config {
        data_dir: dir.to_path_buf(),
        page_size,
        // Bind handled by the provided listener; address here is informational.
        ..Config::default()
    };
    let server = Server::open(config).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let shutdown = Arc::new(Notify::new());
    let shutdown_for_task = shutdown.clone();
    tokio::spawn(async move {
        let _ = server
            .run_on(listener, async move {
                shutdown_for_task.notified().await;
            })
            .await;
    });
    // Give the accept loop a moment to start.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (addr, shutdown)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_conformance_suite_passes() {
    let dir = tempfile::tempdir().unwrap();
    // page_size = 2 forces cursor paging so the streaming path is exercised.
    let (addr, shutdown) = start_server(dir.path(), 2).await;

    let report = run_all(&addr).await.expect("conformance run");
    eprintln!("{}", report.summary());
    assert!(
        report.all_passed(),
        "conformance failures:\n{}",
        report.summary()
    );

    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_survives_server_restart() {
    use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
    use auradb_conformance::Client;

    let dir = tempfile::tempdir().unwrap();

    // First server: write data.
    let (addr, shutdown) = start_server(dir.path(), 100).await;
    {
        let mut client = Client::connect(&addr).await.unwrap();
        client
            .create_schema(&CollectionSchema::new("K").with_field(FieldDef {
                name: "id".into(),
                field_type: FieldType::Uuid,
                primary_key: true,
                unique: true,
                nullable: false,
                indexed: false,
            }))
            .await
            .unwrap();
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("survivor".into()));
        client.insert("K", f).await.unwrap();
    }
    shutdown.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Second server on the same data dir: data is still there.
    let (addr2, shutdown2) = start_server(dir.path(), 100).await;
    {
        use auradb::query::CountQuery;
        let mut client = Client::connect(&addr2).await.unwrap();
        let count = client
            .count(&CountQuery {
                collection: "K".into(),
                filter: None,
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
    shutdown2.notify_one();
}
