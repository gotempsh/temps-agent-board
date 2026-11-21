use crate::board::{BoardStore, Error};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::future::Future;
use std::path::Path;
use std::time::Duration;

pub const MESSAGE_CREATED: &str = "message.created.v1";

pub fn declaration() -> temps_plugin_sdk::prelude::temps_core::external_plugin::watch::PublishedEvent
{
    serde_json::from_value(json!({
        "type": MESSAGE_CREATED, "title": "New channel message",
        "attributes_schema": {"type":"object", "properties":{
            "channel_id":{"type":"integer","title":"Channel ID"},
            "thread_id":{"type":"integer","title":"Thread ID"},
            "message_id":{"type":"integer","title":"Message ID"}
        }, "required":["channel_id","thread_id","message_id"]},
        "filterable":["channel_id"]
    }))
    .expect("valid built-in event declaration")
}

#[derive(Debug, PartialEq)]
pub struct PendingMessage {
    pub event_id: String,
    pub occurred_at_ms: i64,
    pub attributes: Value,
    pub origin_chat_id: Option<String>,
}

fn next_event(path: &Path) -> Result<Option<PendingMessage>, Error> {
    let db = Connection::open(path)?;
    let event = db
        .query_row(
            "SELECT e.event_id, c.id, m.thread, m.id, e.origin_chat_id, e.occurred_at_ms
         FROM pending_message_events e
         JOIN messages m ON m.id=e.message_id
         JOIN threads t ON t.id=m.thread
         JOIN channels c ON c.name=t.channel
         ORDER BY e.message_id LIMIT 1",
            [],
            |row| {
                Ok(PendingMessage {
                    event_id: row.get(0)?,
                    attributes: json!({
                        "channel_id": row.get::<_, i64>(1)?,
                        "thread_id": row.get::<_, i64>(2)?,
                        "message_id": row.get::<_, i64>(3)?,
                    }),
                    origin_chat_id: row.get(4)?,
                    occurred_at_ms: row.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(event)
}

fn acknowledge(path: &Path, event_id: &str) -> Result<(), Error> {
    Connection::open(path)?.execute(
        "DELETE FROM pending_message_events WHERE event_id=?",
        [event_id],
    )?;
    Ok(())
}

pub fn start(store: BoardStore, client: temps_plugin_sdk::TempsClient) {
    tokio::spawn(publish_pending(store, move |message| {
        let client = client.clone();
        async move {
            let signal = temps_plugin_sdk::PublishWatchEvent {
                source: MESSAGE_CREATED.to_owned(),
                subject: message.attributes["channel_id"].to_string(),
                event_id: message.event_id,
                occurred_at_ms: message.occurred_at_ms,
                attributes: message.attributes,
                origin_chat_id: message.origin_chat_id,
            };
            let ack = client.publish_watch_event(signal).await?;
            if !ack.accepted {
                return Err("host did not acknowledge watch event".into());
            }
            Ok(())
        }
    }));
}

/// One publisher per plugin process. Scan once on startup to replay pending
/// events, then sleep until a write commits. A lost acknowledgement replays
/// the same ID so the host can deduplicate the delivery.
pub async fn publish_pending<F, Fut>(store: BoardStore, mut publish: F)
where
    F: FnMut(PendingMessage) -> Fut,
    Fut: Future<Output = Result<(), Error>>,
{
    let mut backoff = Duration::from_secs(1);
    loop {
        let path = store.path().to_path_buf();
        let attempt: Result<bool, Error> = async {
            let Some(event) = tokio::task::spawn_blocking(move || next_event(&path)).await?? else {
                return Ok(false);
            };
            let id = event.event_id.clone();
            publish(event).await?;
            let path = store.path().to_path_buf();
            tokio::task::spawn_blocking(move || acknowledge(&path, &id)).await??;
            Ok(true)
        }
        .await;
        match attempt {
            Ok(more) => {
                backoff = Duration::from_secs(1);
                if !more {
                    store.pending.notified().await;
                }
            }
            Err(error) => {
                eprintln!(
                    "Board event delivery will retry in {}s: {error}",
                    backoff.as_secs()
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::BoardStore;
    use temps_plugin_sdk::prelude::{AuthenticatedCaller, PluginRole};

    #[tokio::test]
    async fn lost_ack_replays_identical_event_and_idle_worker_wakes_for_next_message() {
        use tokio::{sync::mpsc, time::timeout};
        let temp = tempfile::tempdir().unwrap();
        let store = BoardStore::open(temp.path().join("board.sqlite"), true).unwrap();
        let caller = AuthenticatedCaller::without_platform_delegation(
            1,
            "test",
            PluginRole::User,
            ["projects:write"],
        );
        let (sent, mut received) = mpsc::unbounded_channel();
        let mut first = true;
        let worker = tokio::spawn(publish_pending(store.clone(), move |event| {
            sent.send(event).unwrap();
            let fail = std::mem::take(&mut first);
            async move {
                if fail {
                    Err("lost ACK".into())
                } else {
                    Ok(())
                }
            }
        }));
        let mut ids = Vec::new();
        for channel in ["ops", "other"] {
            let thread = store
                .call(
                    &caller,
                    "create_thread",
                    &json!({"channel":channel,"title":"test","body":"private text"}),
                    None,
                )
                .unwrap();
            let event = timeout(Duration::from_secs(5), received.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(event.attributes["thread_id"], thread["id"]);
            assert_eq!(
                declaration().subject(&event.attributes, false),
                Some(event.attributes["channel_id"].to_string())
            );
            assert!(!event.attributes.to_string().contains("private text"));
            ids.push(event.attributes["channel_id"].clone());
            if channel == "ops" {
                let retry = timeout(Duration::from_secs(5), received.recv())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(event, retry);
            }
        }
        assert_ne!(ids[0], ids[1]);
        worker.abort();
    }
    #[test]
    fn legacy_board_migrates_without_emitting_historical_messages() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("board.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE channels (name TEXT PRIMARY KEY NOT NULL);
            INSERT INTO channels VALUES ('ops');",
        )
        .unwrap();
        drop(db);
        let store = BoardStore::open(path.clone(), true).unwrap();
        let caller = AuthenticatedCaller::without_platform_delegation(
            1,
            "local",
            PluginRole::User,
            ["projects:write", "projects:read"],
        );

        let agent = crate::board::AgentSender {
            chat_id: "chat-42".into(),
            name: Some("Coordinator".into()),
        };
        let names = store
            .call(&caller, "list_channels", &json!({}), Some(&agent))
            .unwrap();
        assert_eq!(names, json!(["general", "ops"]));
        let with_ids = store
            .call(
                &caller,
                "list_channels",
                &json!({"include_ids":true}),
                Some(&agent),
            )
            .unwrap();
        let ops = with_ids
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "ops")
            .unwrap();
        assert!(next_event(&path).unwrap().is_none());
        assert!(store
            .call(
                &caller,
                "create_thread",
                &json!({"channel":"ops","title":"test","body":""}),
                Some(&agent)
            )
            .is_err());
        assert!(next_event(&path).unwrap().is_none());
        let id = store
            .call(
                &caller,
                "create_thread",
                &json!({"channel":"ops","title":"test","body":"hello"}),
                Some(&agent),
            )
            .unwrap()["id"]
            .clone();
        store
            .call(
                &caller,
                "reply",
                &json!({"id":id,"body":"a reply"}),
                Some(&agent),
            )
            .unwrap();
        store
            .call(
                &caller,
                "claim_task",
                &json!({"channel":"ops"}),
                Some(&agent),
            )
            .unwrap();
        store
            .call(
                &caller,
                "complete_task",
                &json!({"id":id,"body":"done"}),
                Some(&agent),
            )
            .unwrap();
        let first = next_event(&path).unwrap().unwrap();
        assert_eq!(
            first.attributes,
            json!({"channel_id":ops["id"], "thread_id":id,"message_id":1})
        );
        assert_eq!(first.origin_chat_id.as_deref(), Some("chat-42"));
        let matching = store
            .call(
                &caller,
                "read_thread",
                &json!({"id":id, "message_id":2}),
                None,
            )
            .unwrap();
        assert_eq!(matching.as_array().unwrap().len(), 1);
        assert_eq!(matching[0]["body"], "a reply");
        assert_eq!(matching[0]["id"], 2);
        assert_eq!(matching[0]["sender"]["name"], "Coordinator");
        let different_thread = store
            .call(
                &caller,
                "read_thread",
                &json!({"id":999, "message_id":2}),
                None,
            )
            .unwrap();
        assert_eq!(different_thread, json!([]));
        drop(store);
        let _reopened = BoardStore::open(path.clone(), true).unwrap();
        assert_eq!(next_event(&path).unwrap().unwrap(), first);
        acknowledge(&path, &first.event_id).unwrap();
        for message_id in [2, 3] {
            let next = next_event(&path).unwrap().unwrap();
            assert_ne!(next.event_id, first.event_id);
            assert_eq!(next.attributes["message_id"], message_id);
            acknowledge(&path, &next.event_id).unwrap();
        }
        assert!(next_event(&path).unwrap().is_none());
    }
}
