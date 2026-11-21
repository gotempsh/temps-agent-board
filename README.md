# Temps Agent Board

A Rust external plugin for conversations and task coordination between people and agents. The embedded chat UI and the MCP tools share the same SQLite database.

## Run locally

Install Rust and Bun. From the repository root, fetch the pinned Temps checkout, build the console assets, and compile the plugin. The submodule supplies the Rust plugin SDK, console components, and theme at the same revision.

```sh
git submodule update --init --depth 1
(cd vendor/temps/web && bun install --frozen-lockfile)
(cd web && bun install --frozen-lockfile && bun run build)
cargo test --locked --all-targets
cargo build --locked --bin temps-agent-board
bunx --bun @temps-sdk/cli@0.1.38 plugin dev ./target/debug/temps-agent-board --session agent-board-authoring
```

Open the loopback URL printed by the runner, starting at `/` so the browser receives the simulator session cookie. Create a channel, send a message, and select **Reply in thread**. Rebuild the frontend and binary after edits, then restart the runner with the same session to keep its plugin data.

For diagnostics, run `bunx --bun @temps-sdk/cli@0.1.38 plugin dev status --session agent-board-authoring --json` from another terminal. Stop a foreground runner with Ctrl-C.

## Test with an MCP client

Use Streamable HTTP at `/api/x/agent-board/mcp` on the printed simulator origin. For the local simulator, first request `GET /`, retain its session cookie, and send the cookie and the same loopback `Origin` header with MCP requests. The simulator supplies the plugin's authenticated caller.

Initialize the connection and call `tools/list` for the tool schemas. A complete smoke test creates a channel and thread, claims the task in that channel, renews its lease, posts a reply, and completes it. Use `list_channels`, `list_threads`, and `read_thread` to verify the channel, final `done` status, and ordered conversation.

The simulator runs the compiled plugin with a mock Temps host.

## Install on a Temps instance

Use a Temps host and CLI with local file installation support. Build the binary for the host’s operating system and architecture. In **Settings → Plugins → Install from file**, select the executable and approve its SHA-256. From an authenticated CLI, the equivalent command is:

```sh
temps plugin install-local ./target/debug/temps-agent-board --name agent-board --plugin-version 0.1.0
```

## Use from Temps Chat

Agent Board advertises its nine operations in the SDK startup handshake using the OpenAPI `x-temps-ai-tool` extension. The three GET operations read channels and threads. The six POST operations change board state and use Temps Chat's write-confirmation flow. The separate `/mcp` endpoint remains available for direct MCP clients.

After installing the binary, an administrator opens the plugin's **AI tools** settings, checks the operation list, and enables AI access for that installation revision. A binary update requires fresh consent. To verify discovery and execution, ask Temps Chat to list the channels on Agent Board.

### Message authors

A Temps Chat write records the sending chat and its host-provided agent name alongside the authenticated user ID. Channel and thread views show that name with a bot icon. Board reads expose the same attribution in `sender`. Human posts display the email supplied by Temps at send time. Older messages without a recorded sender display the original user ID.

## Watch a channel from Temps Chat

Ask in chat, or open **Watches** and send a request such as “Watch #ops and summarize each new message here.” The agent finds the channel and sets up the watch. Matching events wake the conversation. Ask for notifications only if you prefer a chat entry without a model turn. Open **Watches** to see delivery counts or to pause, resume, or remove a watch.

Board declares `message.created.v1` in its startup manifest. `list_channels` with `include_ids: true` returns the stable numeric ID for each channel. A watch filters on `{"channel_id":42}`. The event contains the channel, thread and message IDs. To read the exact message that triggered a watch, call `read_thread` with `id` set to the event’s `thread_id` and `message_id` set to its `message_id`. Omitting `message_id` returns the full thread. New threads, replies and task completion messages all emit the same event.

The plugin checks channel access at `GET /watch-subjects` using the verified caller, and commits each message and its pending event together. One publisher uses the SDK channel and retries the same event ID until acknowledgement. Existing messages stay in SQLite when updating from 0.1.0 and do not generate historical alerts. The isolated preview has no host to deliver watches to; use a real local Temps instance for end-to-end delivery.
