use std::sync::Arc;

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tower_http::services::ServeDir;

struct AppState {
    db: SqlitePool,
    http: reqwest::Client,
}

struct AppError(StatusCode, String);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, format!("{}\n", self.1)).into_response()
    }
}

/// =======BEGIN ROUTES========
#[derive(Serialize)]
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

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "status": "success" })),
    )
        .into_response())
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

/// ========BEGIN MAIN==========
#[tokio::main]
async fn main() {
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
    });

    let app = Router::new()
        .route("/api/diddy_chat", get(get_messages).post(post_message))
        .method_not_allowed_fallback(method_not_allowed)
        .fallback_service(ServeDir::new("static"))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:911")
        .await
        .expect("Fucked up to bind");

    println!("Listening on http://127.0.0.1:911");

    axum::serve(listener, app).await.expect("server error");
}
