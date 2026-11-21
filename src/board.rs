use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use temps_plugin_sdk::prelude::*;
use tokio::sync::Notify;

const SCHEMA: &str = include_str!("schema.sql");
const CATALOG: &str = include_str!("tools.json");

fn catalog() -> Vec<Value> {
    serde_json::from_str(CATALOG).expect("valid board catalog")
}

fn is_read(name: &str) -> bool {
    matches!(name, "list_threads" | "read_thread" | "list_channels")
}

pub fn ai_schema() -> utoipa::openapi::OpenApi {
    let mut paths = serde_json::Map::new();
    for tool in catalog() {
        let name = tool["name"].as_str().expect("tool name");
        let schema = &tool["inputSchema"];
        let mut operation = json!({
            "operationId":name, "summary":tool["description"], "x-temps-ai-tool":{},
            "responses":{"200":{"description":"Board operation result"}}
        });
        let method = if is_read(name) {
            let parameters: Vec<_> = schema["properties"].as_object().expect("properties").iter()
                .map(|(key, property)| json!({"name":key,"in":"query","schema":property,
                    "required":schema["required"].as_array().is_some_and(|keys| keys.iter().any(|item| item.as_str()==Some(key.as_str()))) }))
                .collect();
            operation["parameters"] = json!(parameters);
            "get"
        } else {
            operation["requestBody"] =
                json!({"required":true,"content":{"application/json":{"schema":schema}}});
            "post"
        };
        paths.insert(format!("/tools/{name}"), json!({(method):operation}));
    }
    serde_json::from_value(json!({"openapi":"3.1.0","info":{"title":"Agent Board","version":env!("CARGO_PKG_VERSION")},"paths":paths}))
        .expect("valid board OpenAPI schema")
}

fn authorized(caller: &AuthenticatedCaller, name: &str) -> bool {
    caller.is_admin()
        || caller.has_permission(if is_read(name) {
            PluginPermission::PROJECTS_READ
        } else {
            PluginPermission::PROJECTS_WRITE
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_schema_advertises_each_tool_once_with_its_real_http_method() {
        let spec = serde_json::to_value(ai_schema()).unwrap();
        let paths = spec["paths"].as_object().unwrap();
        assert_eq!(paths.len(), catalog().len());
        for tool in catalog() {
            let name = tool["name"].as_str().unwrap();
            let operations = paths[&format!("/tools/{name}")].as_object().unwrap();
            assert_eq!(operations.len(), 1);
            let method = if is_read(name) { "get" } else { "post" };
            let operation = &operations[method];
            assert_eq!(operation["operationId"], name);
            assert!(operation["x-temps-ai-tool"].is_object());
            if !is_read(name) {
                assert_eq!(
                    operation["requestBody"]["content"]["application/json"]["schema"]["properties"],
                    tool["inputSchema"]["properties"]
                );
            }
        }
    }
    #[test]
    fn shared_user_can_post_as_two_distinct_agents_or_as_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let store = BoardStore::open(dir.path().join("board.sqlite"), false).unwrap();
        let caller = AuthenticatedCaller::without_platform_delegation(
            1,
            "owner@example.test",
            PluginRole::User,
            ["projects:read", "projects:write"],
        );
        let mut headers = HeaderMap::new();
        headers.insert("x-temps-agent-name", "Coordinator".parse().unwrap());
        assert!(AgentSender::from_headers(&headers).is_none());
        headers.insert(
            temps_core::external_plugin::headers::CHAT_ID,
            "12".parse().unwrap(),
        );
        let coordinator = AgentSender::from_headers(&headers).unwrap();
        let id = store
            .call(
                &caller,
                "create_thread",
                &json!({"channel":"test","title":"Task","body":"One"}),
                Some(&coordinator),
            )
            .unwrap()["id"]
            .clone();
        headers.insert(
            temps_core::external_plugin::headers::CHAT_ID,
            "13".parse().unwrap(),
        );
        headers.insert("x-temps-agent-name", "Worker".parse().unwrap());
        let worker = AgentSender::from_headers(&headers).unwrap();
        store
            .call(
                &caller,
                "reply",
                &json!({"id":id,"body":"Two"}),
                Some(&worker),
            )
            .unwrap();
        store
            .call(
                &caller,
                "reply",
                &json!({"id":id,"body":"Three","sender":{"name":"Worker"}}),
                None,
            )
            .unwrap();
        let timeline = store
            .call(&caller, "list_threads", &json!({}), None)
            .unwrap();
        let messages = store
            .call(&caller, "read_thread", &json!({"id":id}), None)
            .unwrap();
        assert_eq!(timeline[0]["sender"], messages[0]["sender"]);
        assert_eq!(
            messages[0]["sender"],
            json!({"kind":"agent","chat_id":"12","name":"Coordinator"})
        );
        assert_eq!(
            messages[1]["sender"],
            json!({"kind":"agent","chat_id":"13","name":"Worker"})
        );
        assert_eq!(
            messages[2]["sender"],
            json!({"kind":"user","email":"owner@example.test"})
        );
        assert!(messages.as_array().unwrap().iter().all(|m| m["actor"] == 1));
    }

    #[tokio::test]
    async fn subject_check_uses_verified_read_access_and_exact_channel_id() {
        let temp = tempfile::tempdir().unwrap();
        let store = BoardStore::open(temp.path().join("board.sqlite"), false).unwrap();
        for (permission, source, subject, expected) in [
            ("projects:read", "message.created.v1", "1", Some(true)),
            ("projects:read", "message.created.v1", "2", Some(false)),
            ("projects:read", "message.created.v1", "01", Some(false)),
            ("projects:read", "other", "1", Some(false)),
            ("projects:write", "message.created.v1", "1", None),
        ] {
            let caller = AuthenticatedCaller::without_platform_delegation(
                1,
                "test",
                PluginRole::User,
                [permission],
            );
            let scope = WatchScope {
                source: source.into(),
                subject: subject.into(),
            };
            let response = watch_subject_access(State(store.clone()), caller, Query(scope)).await;
            if let Some(allowed) = expected {
                assert_eq!(response.status(), StatusCode::OK);
                let body = axum::body::to_bytes(response.into_body(), 1024)
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap()["allowed"],
                    json!(allowed)
                );
            } else {
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
            }
        }
    }
}

#[derive(Clone)]
pub struct BoardStore {
    path: PathBuf,
    pub(crate) pending: Arc<Notify>,
    publish_events: bool,
}

impl BoardStore {
    pub fn open(path: PathBuf, publish_events: bool) -> Result<Self, Error> {
        let mut db = Connection::open(&path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let legacy: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('channels') WHERE name='name')
             AND NOT EXISTS(SELECT 1 FROM pragma_table_info('channels') WHERE name='id')",
            [],
            |row| row.get(0),
        )?;
        if legacy {
            tx.execute_batch(
                "ALTER TABLE channels RENAME TO legacy_channels;
                CREATE TABLE channels (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL);
                INSERT INTO channels(id,name) SELECT rowid,name FROM legacy_channels;
                DROP TABLE legacy_channels;",
            )?;
        }
        tx.execute_batch(SCHEMA)?;
        let has_sender_email: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='sender_email')",
            [],
            |row| row.get(0),
        )?;
        if !has_sender_email {
            tx.execute_batch("ALTER TABLE messages ADD COLUMN sender_email TEXT")?;
        }
        tx.commit()?;
        Ok(Self {
            path,
            pending: Arc::new(Notify::new()),
            publish_events,
        })
    }

    pub(crate) fn call(
        &self,
        caller: &AuthenticatedCaller,
        name: &str,
        args: &Value,
        sender: Option<&AgentSender>,
    ) -> Result<Value, Error> {
        let result = run(self, caller, sender, name, args)?;
        if matches!(name, "create_thread" | "reply" | "complete_task") {
            self.pending.notify_one();
        }
        Ok(result)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn channel_name(&self, id: i64) -> rusqlite::Result<Option<String>> {
        Connection::open(self.path())?
            .query_row("SELECT name FROM channels WHERE id=?", [id], |row| {
                row.get(0)
            })
            .optional()
    }
}

async fn ai_call(
    store: BoardStore,
    caller: AuthenticatedCaller,
    name: String,
    args: Value,
    sender: Option<AgentSender>,
) -> Response {
    if !authorized(&caller, &name) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match tokio::task::spawn_blocking(move || store.call(&caller, &name, &args, sender.as_ref()))
        .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub fn router_with(store: BoardStore, endpoint: axum::routing::MethodRouter<BoardStore>) -> Router {
    let mut routes = Router::new();
    for tool in catalog() {
        let name = tool["name"].as_str().expect("tool name").to_owned();
        let route = format!("/tools/{name}");
        let handler = if is_read(&name) {
            get(
                move |State(path),
                      caller: AuthenticatedCaller,
                      Query(query): Query<HashMap<String, String>>| async move {
                    let mut args = json!(query);
                    for field in ["id", "message_id"] {
                        if let Some(raw) = args[field].as_str() {
                            let Ok(id) = raw.parse::<i64>() else {
                                return StatusCode::BAD_REQUEST.into_response();
                            };
                            args[field] = json!(id);
                        }
                    }
                    if let Some(raw) = args["include_ids"].as_str() {
                        let Ok(value) = raw.parse::<bool>() else {
                            return StatusCode::BAD_REQUEST.into_response();
                        };
                        args["include_ids"] = json!(value);
                    }
                    ai_call(path, caller, name, args, None).await
                },
            )
        } else {
            post(
                move |State(path), caller: AuthenticatedCaller, headers: HeaderMap, Json(args)| {
                    ai_call(
                        path,
                        caller,
                        name,
                        args,
                        AgentSender::from_headers(&headers),
                    )
                },
            )
        };
        routes = routes.route(&route, handler);
    }
    routes
        .route("/ui", get(ui))
        .route("/ui/", get(ui))
        .route(
            "/assets/board.css",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/css")],
                    include_str!("../web/dist/board.css"),
                )
            }),
        )
        .route(
            "/assets/board.js",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../web/dist/board.js"),
                )
            }),
        )
        .route("/watch-subjects", get(watch_subject_access))
        .route("/mcp", endpoint)
        .with_state(store)
}

#[derive(Deserialize)]
struct WatchScope {
    source: String,
    subject: String,
}

async fn watch_subject_access(
    State(store): State<BoardStore>,
    caller: AuthenticatedCaller,
    Query(scope): Query<WatchScope>,
) -> Response {
    if !authorized(&caller, "list_channels") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(channel_id) = scope.subject.parse::<i64>() else {
        return Json(json!({"allowed":false})).into_response();
    };
    if scope.source != "message.created.v1"
        || channel_id <= 0
        || scope.subject != channel_id.to_string()
    {
        return Json(json!({"allowed":false})).into_response();
    }
    match tokio::task::spawn_blocking(move || store.channel_name(channel_id)).await {
        Ok(Ok(Some(name))) => {
            Json(json!({"allowed":true, "label":format!("#{name}")})).into_response()
        }
        Ok(Ok(None)) => Json(json!({"allowed":false})).into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Debug)]
pub(crate) struct AgentSender {
    pub(crate) chat_id: String,
    pub(crate) name: Option<String>,
}

impl AgentSender {
    fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let chat_id = headers
            .get(temps_core::external_plugin::headers::CHAT_ID)?
            .to_str()
            .ok()?
            .trim();
        if chat_id.is_empty() {
            return None;
        }
        let name = headers
            .get("x-temps-agent-name")
            .and_then(|value| std::str::from_utf8(value.as_bytes()).ok())
            .map(str::trim)
            .filter(|name| !name.is_empty());
        Some(Self {
            chat_id: chat_id.to_owned(),
            name: name.map(str::to_owned),
        })
    }
}

// The numeric actor stays available to clients independently of display attribution.
fn attach_sender(
    message: &mut Value,
    row: &rusqlite::Row<'_>,
    chat_col: usize,
) -> rusqlite::Result<()> {
    if let Some(chat_id) = row.get::<_, Option<String>>(chat_col)? {
        let name = row
            .get::<_, Option<String>>(chat_col + 1)?
            .unwrap_or_else(|| format!("Chat agent {chat_id}"));
        message["sender"] = json!({"kind":"agent","chat_id":chat_id,"name":name});
    } else if let Some(email) = row.get::<_, Option<String>>(chat_col + 2)? {
        message["sender"] = json!({"kind":"user","email":email});
    }
    Ok(())
}

async fn ui() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}

pub async fn mcp(
    State(store): State<BoardStore>,
    caller: AuthenticatedCaller,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    let id = &request["id"];
    let fail = |code, message| {
        Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}))
            .into_response()
    };
    if request["jsonrpc"] != "2.0" || !request["method"].is_string() {
        return fail(-32600, "Invalid request");
    }
    if request.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let tools: Value = serde_json::from_str(CATALOG).expect("valid tool catalog");
    let result = match request["method"].as_str() {
        Some("initialize") => {
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"temps-agent-board","version":"0.1.0"}})
        }
        Some("ping") => json!({}),
        Some("tools/list") => json!({"tools":tools}),
        Some("tools/call") => {
            let name = request["params"]["name"].as_str().unwrap_or("").to_owned();
            if !tools
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name)
            {
                return fail(-32602, "Unknown tool");
            }
            if !authorized(&caller, &name) {
                return StatusCode::FORBIDDEN.into_response();
            }
            let args = request["params"]["arguments"].clone();
            let sender = AgentSender::from_headers(&headers);
            let outcome = tokio::task::spawn_blocking(move || {
                store.call(&caller, &name, &args, sender.as_ref())
            })
            .await;
            match outcome {
                Ok(Ok(value)) => json!({"content":[{"type":"text","text":value.to_string()}]}),
                Ok(Err(error)) => {
                    json!({"isError":true,"content":[{"type":"text","text":error.to_string()}]})
                }
                Err(_) => return fail(-32603, "Board worker failed"),
            }
        }
        _ => return fail(-32601, "Unknown method"),
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
fn field<'a>(args: &'a Value, key: &str) -> Result<&'a str, Error> {
    args[key]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 8192)
        .ok_or_else(|| format!("Invalid {key}").into())
}

fn append_message(
    tx: &rusqlite::Transaction<'_>,
    publish_events: bool,
    thread_id: i64,
    caller: &AuthenticatedCaller,
    body: &str,
    sender: Option<&AgentSender>,
) -> Result<(), Error> {
    let email = (sender.is_none() && !caller.email.trim().is_empty()).then(|| caller.email.trim());
    tx.execute(
        "INSERT INTO messages(thread,actor,body,sender_email) VALUES (?,?,?,?)",
        params![thread_id, caller.user_id, body, email],
    )?;
    let message_id = tx.last_insert_rowid();
    if let Some(agent) = sender {
        tx.execute(
            "INSERT INTO message_agents(message_id,chat_id,name) VALUES (?,?,?)",
            params![message_id, agent.chat_id, agent.name],
        )?;
    }
    if publish_events {
        tx.execute(
            "INSERT INTO pending_message_events(message_id,origin_chat_id) VALUES (?,?)",
            params![message_id, sender.map(|agent| &agent.chat_id)],
        )?;
    }
    Ok(())
}

fn run(
    store: &BoardStore,
    caller: &AuthenticatedCaller,
    sender: Option<&AgentSender>,
    name: &str,
    args: &Value,
) -> Result<Value, Error> {
    let actor = caller.user_id;
    let mut database = Connection::open(store.path())?;
    database.busy_timeout(std::time::Duration::from_secs(5))?;
    database.execute_batch("PRAGMA foreign_keys=ON")?;
    let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let thread = || {
        args["id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or("Expected a positive thread id")
    };
    let result = match name {
        "create_channel" => {
            let channel = field(args, "channel")?;
            if channel.starts_with('#')
                || channel.chars().count() > 80
                || channel.chars().any(char::is_control)
            {
                return Err("Use a channel name of up to 80 characters without a leading #".into());
            }
            if tx.execute("INSERT OR IGNORE INTO channels(name) VALUES (?)", [channel])? == 0 {
                return Err("A channel with that name already exists".into());
            }
            json!({"name":channel})
        }
        "list_channels" => {
            let mut statement =
                tx.prepare("SELECT name,id FROM channels ORDER BY name COLLATE NOCASE")?;
            let include_ids = match args.get("include_ids") {
                None => false,
                Some(value) => value.as_bool().ok_or("Expected boolean include_ids")?,
            };
            let rows = statement.query_map([], |row| {
                let name: String = row.get(0)?;
                Ok(if include_ids {
                    json!({"id":row.get::<_, i64>(1)?, "name":name})
                } else {
                    json!(name)
                })
            })?;
            json!(rows.collect::<Result<Vec<_>, _>>()?)
        }
        "create_thread" => {
            let dependency = match args.get("depends_on") {
                Some(value) => Some(value.as_i64().ok_or("Invalid dependency")?),
                None => None,
            };
            tx.execute(
                "INSERT OR IGNORE INTO channels(name) VALUES (?)",
                [field(args, "channel")?],
            )?;
            tx.execute(
                "INSERT INTO threads(channel,title,label,dependency) VALUES (?,?,?,?)",
                params![
                    field(args, "channel")?,
                    field(args, "title")?,
                    args["label"].as_str().unwrap_or(""),
                    dependency
                ],
            )?;
            let id = tx.last_insert_rowid();
            append_message(
                &tx,
                store.publish_events,
                id,
                caller,
                field(args, "body")?,
                sender,
            )?;
            json!({"id":id})
        }
        "list_threads" => {
            let mut statement = tx.prepare("SELECT t.id,channel,title,label,status,owner,dependency,m.actor,m.body,m.created,
                (SELECT count(*) FROM messages WHERE thread=t.id AND id>m.id), a.chat_id, a.name, m.sender_email
                FROM overview t JOIN messages m ON m.id=(SELECT min(id) FROM messages WHERE thread=t.id)
                LEFT JOIN message_agents a ON a.message_id=m.id
                WHERE (?1 IS NULL OR channel=?1) ORDER BY t.id DESC")?;
            let rows = statement.query_map([args["channel"].as_str()], |row| {
                let mut thread = json!({
                    "id":row.get::<_,i64>(0)?, "channel":row.get::<_,String>(1)?,
                    "title":row.get::<_,String>(2)?, "label":row.get::<_,String>(3)?,
                    "status":row.get::<_,String>(4)?, "owner":row.get::<_,Option<i32>>(5)?,
                    "depends_on":row.get::<_,Option<i64>>(6)?, "actor":row.get::<_,i32>(7)?,
                    "body":row.get::<_,String>(8)?, "created":row.get::<_,i64>(9)?,
                    "replies":row.get::<_,i64>(10)?
                });
                attach_sender(&mut thread, row, 11)?;
                Ok(thread)
            })?;
            json!(rows.collect::<Result<Vec<_>, _>>()?)
        }
        "read_thread" => {
            let message_id = args
                .get("message_id")
                .map(|id| id.as_i64().ok_or("Invalid message ID"))
                .transpose()?;
            let mut statement = tx.prepare(
                "SELECT m.id, m.actor, m.body, m.created, a.chat_id, a.name, m.sender_email FROM messages m
                 LEFT JOIN message_agents a ON a.message_id=m.id
                 WHERE m.thread=?1 AND (?2 IS NULL OR m.id=?2) ORDER BY m.id",
            )?;
            let rows = statement.query_map(params![thread()?, message_id], |row| {
                let mut message = json!({
                    "actor":row.get::<_,i32>(1)?,
                    "body":row.get::<_,String>(2)?,
                    "created":row.get::<_,i64>(3)?
                });
                attach_sender(&mut message, row, 4)?;
                if message_id.is_some() {
                    message["id"] = json!(row.get::<_, i64>(0)?);
                }
                Ok(message)
            })?;
            json!(rows.collect::<Result<Vec<_>, _>>()?)
        }
        "claim_task" => {
            let id:i64 = tx.query_row("SELECT id FROM overview WHERE channel=? AND status='ready' AND (?2 IS NULL OR label=?2) ORDER BY id LIMIT 1",params![field(args,"channel")?,args["label"].as_str()],|row| row.get(0)).map_err(|error| match error {rusqlite::Error::QueryReturnedNoRows => "No ready task".to_string(),other=>other.to_string()})?;
            tx.execute(
                "UPDATE threads SET owner=?,expires=unixepoch()+900 WHERE id=?",
                params![actor, id],
            )?;
            json!({"id":id,"lease_seconds":900})
        }
        "renew_task" | "complete_task" => {
            let id = thread()?;
            let done = name == "complete_task";
            if tx.execute("UPDATE threads SET done=?,expires=unixepoch()+900 WHERE id=? AND owner=? AND expires>unixepoch() AND done=0",params![done,id,actor])? == 0 { return Err("Active lease required".into()); }
            if done {
                append_message(
                    &tx,
                    store.publish_events,
                    id,
                    caller,
                    field(args, "body")?,
                    sender,
                )?;
            }
            json!({"id":id})
        }
        "reply" => {
            append_message(
                &tx,
                store.publish_events,
                thread()?,
                caller,
                field(args, "body")?,
                sender,
            )?;
            json!({"id":thread()?})
        }
        _ => return Err("Unknown tool".into()),
    };
    tx.commit()?;
    Ok(result)
}
