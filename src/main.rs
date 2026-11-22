#![cfg_attr(test, feature(test))]

use std::sync::Arc;

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tower_http::services::ServeDir;

#[derive(Debug)]
struct AppError(StatusCode, String);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, format!("{}\n", self.1)).into_response()
    }
}

/// =======BEGIN ROUTES========
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Entry {
    blud: String,
    lol: String,
    time: String,
}

#[derive(Deserialize, Default)]
struct GetR {
    #[serde(default)]
    since: Option<String>,
}

async fn get_messages(
    State(state): State<Arc<AppState>>,
    body: Body,
) -> Result<Json<Vec<Entry>>, AppError> {
    let bytes = to_bytes(body, 4096)
        .await
        .map_err(|_| AppError(StatusCode::BAD_REQUEST, "Invalid body".into()))?;

    let input: GetR = if bytes.is_empty() {
        GetR::default()
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|_| AppError(StatusCode::BAD_REQUEST, "Invalid JSON".into()))?
    };
    let since = input.since.unwrap_or_default();

    let rows = if since.is_empty() {
        sqlx::query_as::<_, (String, String, i64)>("SELECT blud, lol, created_at FROM chat")
            .fetch_all(&state.db)
            .await
            .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        let since: i64 = since
            .parse()
            .map_err(|_| AppError(StatusCode::BAD_REQUEST, "Invalid since".into()))?;
        sqlx::query_as::<_, (String, String, i64)>(
            "SELECT blud, lol, created_at FROM chat WHERE created_at > ?",
        )
        .bind(since)
        .fetch_all(&state.db)
        .await
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    let entries = rows
        .into_iter()
        .map(|(blud, lol, time)| Entry {
            blud,
            lol,
            time: time.to_string(),
        })
        .collect();

    Ok(Json(entries))
}

#[derive(Default, Deserialize)]
struct NewEntry {
    #[serde(default)]
    blud: Option<String>,
    #[serde(default)]
    lol: Option<String>,
}

async fn post_message(
    State(state): State<Arc<AppState>>,
    body: Body,
) -> Result<Response, AppError> {
    let bytes = to_bytes(body, 4096)
        .await
        .map_err(|_| AppError(StatusCode::BAD_REQUEST, "Invalid JSON".into()))?;

    let input: Option<NewEntry> = serde_json::from_slice(&bytes)
        .map_err(|_| AppError(StatusCode::BAD_REQUEST, "Invalid JSON".into()))?;
    let input = input.unwrap_or_default();

    let blud = input.blud.unwrap_or_default();
    let lol = input.lol.unwrap_or_default();

    if lol.is_empty() {
        return Err(AppError(StatusCode::BAD_REQUEST, "uh nuh".into()));
    }

    if blud.len() > 1000 || lol.len() > 1000 {
        return Err(AppError(StatusCode::BAD_REQUEST, "too long".into()));
    }

    let nsecs = calculate_nsecs(&state.http)
        .await
        .map_err(|_| AppError(StatusCode::BAD_GATEWAY, "its-site unavailable".into()))?;

    sqlx::query("INSERT INTO chat (blud, lol, created_at) VALUES (?, ?, ?)")
        .bind(&blud)
        .bind(&lol)
        .bind(nsecs)
        .execute(&state.db)
        .await
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let ws_payload = serde_json::to_string(&Entry {
        blud: blud.clone(),
        lol: lol.clone(),
        time: nsecs.to_string(),
    })
    .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let _ = state.tx.send(Utf8Bytes::from(ws_payload));

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "status": "success" })),
    )
        .into_response())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.tx.subscribe();

    let mut send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(text) => {
                    if sender.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    eprintln!("ws client lagged, skipped {skipped} messages");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Close(_) = msg {
                break;
            }
        }
    });
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
}

#[derive(Deserialize)]
struct NsecsPayload {
    nsecs: String,
}

async fn calculate_nsecs(client: &reqwest::Client) -> Result<i64, ()> {
    let link = std::env::var("OFFSET_LINK").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let url = format!("{}/api/nsecs", link.trim_end_matches('/'));

    let response = client.get(url).send().await.map_err(|_| ())?;
    let payload: NsecsPayload = response.json().await.map_err(|_| ())?;

    payload.nsecs.parse::<i64>().map_err(|_| ())
}

async fn method_not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, "Method not allowed\n").into_response()
}

struct AppState {
    db: SqlitePool,
    http: reqwest::Client,
    tx: broadcast::Sender<Utf8Bytes>,
}

/// ========BEGIN MAIN==========
#[tokio::main]
async fn main() {
    let (tx, _rx) = broadcast::channel(1024);
    let options = SqliteConnectOptions::new()
        .filename("AC.db")
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);

    let db = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .expect("Fucked up to open db");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS chat (
            blud TEXT NOT NULL,
            lol TEXT NOT NULL,
            created_at BIGINT NOT NULL
        );",
    )
    .execute(&db)
    .await
    .expect("Fucked up to create table");

    println!("DB initialized");

    let state = Arc::new(AppState {
        db,
        http: reqwest::Client::new(),
        tx,
    });

    let app = Router::new()
        .route("/api/diddy_chat", get(get_messages).post(post_message))
        .route("/ws", get(ws_handler))
        .method_not_allowed_fallback(method_not_allowed)
        .fallback_service(ServeDir::new("static"))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:1060")
        .await
        .expect("Fucked up to bind");

    println!("Listening on http://127.0.0.1:1060");

    axum::serve(listener, app).await.expect("server error");
}

/// ========BEGIN TESTS=========
#[cfg(test)]
mod tests {
    extern crate test;

    use super::*;
    use axum::body::Body;
    use sqlx::SqlitePool;
    use test::Bencher;

    async fn test_state() -> Arc<AppState> {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory db");
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS chat (
                blud TEXT NOT NULL,
                lol TEXT NOT NULL,
                created_at BIGINT NOT NULL
            );",
        )
        .execute(&db)
        .await
        .expect("failed to create table");
        Arc::new(AppState {
            db,
            http: reqwest::Client::new(),
            tx: tokio::sync::broadcast::channel(1024).0,
        })
    }

    #[test]
    fn entry_serializes_fields() {
        let entry = Entry {
            blud: "b".into(),
            lol: "l".into(),
            time: "42".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"blud\":\"b\""));
        assert!(json.contains("\"lol\":\"l\""));
        assert!(json.contains("\"time\":\"42\""));
    }

    #[test]
    fn entry_deserializes_from_json() {
        let json = r#"{"blud":"x","lol":"y","time":"7"}"#;
        let entry: Entry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.blud, "x");
        assert_eq!(entry.lol, "y");
        assert_eq!(entry.time, "7");
    }

    #[test]
    fn getr_defaults_to_none() {
        let r: GetR = serde_json::from_str("{}").unwrap();
        assert!(r.since.is_none());
        assert!(r.since.unwrap_or_default().is_empty());
    }

    #[test]
    fn getr_parses_since() {
        let r: GetR = serde_json::from_str(r#"{"since":"99"}"#).unwrap();
        assert_eq!(r.since.unwrap(), "99");
    }

    #[test]
    fn newentry_defaults_are_none() {
        let n = NewEntry::default();
        assert!(n.blud.is_none());
        assert!(n.lol.is_none());
    }

    #[test]
    fn newentry_deserializes_partial_fields() {
        let n: NewEntry = serde_json::from_str(r#"{"lol":"hi"}"#).unwrap();
        assert_eq!(n.lol.as_deref(), Some("hi"));
        assert!(n.blud.is_none());
    }

    #[tokio::test]
    async fn get_messages_returns_empty_for_empty_db() {
        let state = test_state().await;
        let resp = get_messages(State(state), Body::empty()).await.unwrap();
        assert!(resp.0.is_empty());
    }

    #[tokio::test]
    async fn get_messages_maps_rows() {
        let state = test_state().await;
        sqlx::query("INSERT INTO chat (blud, lol, created_at) VALUES (?, ?, ?)")
            .bind("blud")
            .bind("lol")
            .bind(5_i64)
            .execute(&state.db)
            .await
            .unwrap();

        let resp = get_messages(State(state), Body::empty()).await.unwrap();
        assert_eq!(resp.0.len(), 1);
        assert_eq!(resp.0[0].blud, "blud");
        assert_eq!(resp.0[0].lol, "lol");
        assert_eq!(resp.0[0].time, "5");
    }

    #[tokio::test]
    async fn get_messages_rejects_invalid_since() {
        let state = test_state().await;
        let body = Body::from(r#"{"since":"not-a-number"}"#);
        let err = get_messages(State(state), body).await.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_messages_filters_by_since() {
        let state = test_state().await;
        sqlx::query("INSERT INTO chat (blud, lol, created_at) VALUES (?, ?, ?)")
            .bind("old")
            .bind("l")
            .bind(1_i64)
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO chat (blud, lol, created_at) VALUES (?, ?, ?)")
            .bind("new")
            .bind("l")
            .bind(10_i64)
            .execute(&state.db)
            .await
            .unwrap();

        let body = Body::from(r#"{"since":"5"}"#);
        let resp = get_messages(State(state), body).await.unwrap();
        assert_eq!(resp.0.len(), 1);
        assert_eq!(resp.0[0].blud, "new");
    }

    #[tokio::test]
    async fn post_message_rejects_empty_body() {
        let state = test_state().await;
        let err = post_message(State(state), Body::empty()).await.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_message_rejects_empty_lol() {
        let state = test_state().await;
        let body = Body::from(r#"{"blud":"b","lol":""}"#);
        let err = post_message(State(state), body).await.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_message_rejects_too_long_fields() {
        let state = test_state().await;
        let long = "a".repeat(1001);
        let body = Body::from(format!(r#"{{"blud":"{}","lol":"hi"}}"#, long));
        let err = post_message(State(state), body).await.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_message_fails_without_nsecs_service() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("OFFSET_LINK", "http://127.0.0.1:1");
        }
        let state = test_state().await;
        let body = Body::from(r#"{"blud":"b","lol":"l"}"#);
        let err = post_message(State(state), body).await.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn calculate_nsecs_url_trims_trailing_slash() {
        let link = "http://127.0.0.1:8080/";
        let url = format!("{}/api/nsecs", link.trim_end_matches('/'));
        assert_eq!(url, "http://127.0.0.1:8080/api/nsecs");
    }

    #[bench]
    fn bench_entry_serialize(b: &mut Bencher) {
        let entry = Entry {
            blud: "blud".into(),
            lol: "lol".into(),
            time: "123456".into(),
        };
        b.iter(|| serde_json::to_string(&entry).unwrap());
    }

    #[bench]
    fn bench_entry_deserialize(b: &mut Bencher) {
        let json = r#"{"blud":"blud","lol":"lol","time":"123456"}"#;
        b.iter(|| serde_json::from_str::<Entry>(json).unwrap());
    }

    #[bench]
    fn bench_getr_deserialize(b: &mut Bencher) {
        let json = r#"{"since":"123456"}"#;
        b.iter(|| serde_json::from_str::<GetR>(json).unwrap());
    }

    #[bench]
    fn bench_newentry_deserialize(b: &mut Bencher) {
        let json = r#"{"blud":"b","lol":"l"}"#;
        b.iter(|| serde_json::from_str::<NewEntry>(json).unwrap());
    }

    #[bench]
    fn bench_json_vec_serialize(b: &mut Bencher) {
        let entries = vec![
            Entry {
                blud: "a".into(),
                lol: "b".into(),
                time: "1".into(),
            },
            Entry {
                blud: "c".into(),
                lol: "d".into(),
                time: "2".into(),
            },
        ];
        b.iter(|| serde_json::to_string(&entries).unwrap());
    }

    #[bench]
    fn bench_select_where_created_at_gt(b: &mut Bencher) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pool = rt.block_on(async {
            let pool = SqlitePool::connect("sqlite::memory:")
                .await
                .expect("failed to open in-memory db");
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS chat (
                    blud TEXT NOT NULL,
                    lol TEXT NOT NULL,
                    created_at BIGINT NOT NULL
                );",
            )
            .execute(&pool)
            .await
            .expect("failed to create table");
            for i in 0..1000_i64 {
                sqlx::query("INSERT INTO chat (blud, lol, created_at) VALUES (?, ?, ?)")
                    .bind("blud")
                    .bind("lol")
                    .bind(i)
                    .execute(&pool)
                    .await
                    .expect("failed to insert row");
            }
            pool
        });

        let threshold: i64 = 500;
        b.iter(|| {
            rt.block_on(async {
                let rows = sqlx::query_as::<_, (String, String, i64)>(
                    "SELECT blud, lol, created_at FROM chat WHERE created_at > ?",
                )
                .bind(threshold)
                .fetch_all(&pool)
                .await
                .expect("query failed");
                test::black_box(rows);
            })
        });
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    use futures_util::StreamExt;
    use tokio::net::TcpListener;
    use tokio_tungstenite::connect_async;

    async fn start_ws_server() -> (u16, Arc<AppState>) {
        let state = test_state().await;
        let app = Router::new()
            .route("/ws", get(ws_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (port, state)
    }

    #[tokio::test]
    async fn ws_client_receives_broadcast() {
        let (port, state) = start_ws_server().await;
        let url = format!("ws://127.0.0.1:{}/ws", port);
        let (mut ws_stream, _) = connect_async(url).await.unwrap();
        let payload = r#"{"blud":"b","lol":"l","time":"42"}"#;
        state
            .tx
            .send(axum::extract::ws::Utf8Bytes::from(payload.to_string()))
            .unwrap();
        let msg = ws_stream.next().await.unwrap().unwrap();
        assert!(msg.is_text());
        assert_eq!(msg.to_text().unwrap(), payload);
    }

    #[tokio::test]
    async fn ws_client_receives_posted_message() {
        let _lock = ENV_LOCK.lock().unwrap();
        let mock_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_port = mock_listener.local_addr().unwrap().port();
        let mock_app = Router::new().route(
            "/api/nsecs",
            get(|| async { Json(serde_json::json!({"nsecs":"42"})) }),
        );
        tokio::spawn(async move {
            axum::serve(mock_listener, mock_app).await.unwrap();
        });

        unsafe {
            std::env::set_var("OFFSET_LINK", format!("http://127.0.0.1:{}", mock_port));
        }

        let (port, state) = start_ws_server().await;
        let url = format!("ws://127.0.0.1:{}/ws", port);
        let (mut ws_stream, _) = connect_async(url).await.unwrap();

        let body = Body::from(r#"{"blud":"b","lol":"l"}"#);
        let resp = post_message(State(state), body).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let msg = ws_stream.next().await.unwrap().unwrap();
        assert!(msg.is_text());
        let value: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        assert_eq!(value["blud"], "b");
        assert_eq!(value["lol"], "l");
        assert_eq!(value["time"], "42");
    }

    #[tokio::test]
    async fn post_message_broadcasts_payload() {
        let _lock = ENV_LOCK.lock().unwrap();
        let mock_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_port = mock_listener.local_addr().unwrap().port();
        let mock_app = Router::new().route(
            "/api/nsecs",
            get(|| async { Json(serde_json::json!({"nsecs":"42"})) }),
        );
        tokio::spawn(async move {
            axum::serve(mock_listener, mock_app).await.unwrap();
        });

        unsafe {
            std::env::set_var("OFFSET_LINK", format!("http://127.0.0.1:{}", mock_port));
        }

        let state = test_state().await;
        let mut rx = state.tx.subscribe();
        let body = Body::from(r#"{"blud":"b","lol":"l"}"#);
        let resp = post_message(State(state), body).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let msg = rx.recv().await.unwrap();
        let value: serde_json::Value = serde_json::from_str(msg.as_str()).unwrap();
        assert_eq!(value["blud"], "b");
        assert_eq!(value["lol"], "l");
        assert_eq!(value["time"], "42");
    }
}
