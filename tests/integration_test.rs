use http_body_util::Empty;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

async fn mock_upstream_handler(_req: Request<Incoming>) -> Result<Response<String>, Infallible> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .body("upstream response".to_string())
        .unwrap())
}

async fn slow_upstream_handler(_req: Request<Incoming>) -> Result<Response<String>, Infallible> {
    // Simulate a slow upstream that takes longer than the timeout
    tokio::time::sleep(Duration::from_secs(10)).await;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .body("slow response".to_string())
        .unwrap())
}

async fn start_mock_upstream() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(mock_upstream_handler);
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    format!("http://127.0.0.1:{}", addr.port())
}

async fn start_slow_upstream() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(slow_upstream_handler);
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    format!("http://127.0.0.1:{}", addr.port())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_proxy_basic_request() {
    let upstream_addr = start_mock_upstream().await;
    let upstream_addrs = Arc::new(vec![upstream_addr]);
    let timeout = Duration::from_secs(30);

    let listener =
        rust_servicemesh::listener::Listener::bind("127.0.0.1:0", upstream_addrs, timeout)
            .await
            .unwrap();

    let proxy_addr = listener.local_addr();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    tokio::spawn(async move {
        let _ = listener.serve(shutdown_rx).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let uri = format!("http://{}/test", proxy_addr);

    let req = Request::builder()
        .uri(uri)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let response = client.request(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let _ = shutdown_tx.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_proxy_round_robin() {
    let upstream1 = start_mock_upstream().await;
    let upstream2 = start_mock_upstream().await;
    let upstream_addrs = Arc::new(vec![upstream1, upstream2]);
    let timeout = Duration::from_secs(30);

    let listener =
        rust_servicemesh::listener::Listener::bind("127.0.0.1:0", upstream_addrs, timeout)
            .await
            .unwrap();

    let proxy_addr = listener.local_addr();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    tokio::spawn(async move {
        let _ = listener.serve(shutdown_rx).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build_http();

    for _ in 0..5 {
        let uri = format!("http://{}/test", proxy_addr);
        let req = Request::builder()
            .uri(uri)
            .body(Empty::<Bytes>::new())
            .unwrap();
        let response = client.request(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let _ = shutdown_tx.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_proxy_timeout_enforcement() {
    // Start a slow upstream that takes 10 seconds to respond
    let slow_upstream = start_slow_upstream().await;
    let upstream_addrs = Arc::new(vec![slow_upstream]);

    // Set a short timeout (1 second)
    let timeout = Duration::from_secs(1);

    let listener =
        rust_servicemesh::listener::Listener::bind("127.0.0.1:0", upstream_addrs, timeout)
            .await
            .unwrap();

    let proxy_addr = listener.local_addr();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    tokio::spawn(async move {
        let _ = listener.serve(shutdown_rx).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let uri = format!("http://{}/test", proxy_addr);

    let start = std::time::Instant::now();
    let req = Request::builder()
        .uri(uri)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let response = client.request(req).await.unwrap();
    let elapsed = start.elapsed();

    // Should get 504 Gateway Timeout
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);

    // Should timeout in approximately 1 second, not 10
    assert!(
        elapsed < Duration::from_secs(2),
        "Request should timeout quickly"
    );
    assert!(
        elapsed >= Duration::from_secs(1),
        "Request should wait for timeout"
    );

    let _ = shutdown_tx.send(());
}
