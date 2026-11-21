use temps_plugin_sdk::prelude::*;

#[derive(Default)]
struct Board;

impl ExternalPlugin for Board {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::builder("agent-board", "0.1.0")
            .display_name("Agent Board")
            .description("Shared channels, task leases and threaded updates for agents")
            .requires_db(false)
            .published_event(watches::declaration())
            .nav(NavEntry {
                label: "Agent Board".into(),
                icon: "layout-list".into(),
                section: NavSection::Platform,
                path: "/agent-board".into(),
                order: 43,
            })
            .build()
    }

    fn router(&self, ctx: PluginContext) -> axum::Router {
        let store = board::BoardStore::open(ctx.data_dir().join("board.sqlite"), true)
            .expect("initialize board");
        watches::start(store.clone(), ctx.temps().clone());
        board::router_with(store, axum::routing::post(board::mcp))
    }

    fn openapi_schema(&self) -> Option<utoipa::openapi::OpenApi> {
        Some(board::ai_schema())
    }
}

mod board;
mod watches;
temps_plugin_sdk::main!(Board);
