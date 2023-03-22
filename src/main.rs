use std::str::FromStr;
use std::{net::SocketAddr, sync::Arc};

use axum::extract::Query;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use dashmap::DashMap;
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const WORLD_SIZE: usize = 280;

// Our shared state
struct AppState {
    worlds: [WorldState; WORLD_SIZE],
}

struct WorldState {
    points: Arc<DashMap<String, u32>>,
    tx: broadcast::Sender<WorldMessage>,
}

#[derive(Deserialize)]
struct Params {
    world: usize,
    rsn: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
enum WorldMessage {
    Sync(DashMap<String, u32>),
    Update(WorldUpdate),
    Remove(WorldRemove),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct WorldUpdate {
    rsn: String,
    points: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct WorldRemove {
    rsn: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wintertodt_server=trace".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Set up application state for use with with_state().

    let app_state = Arc::new(AppState {
        worlds: [(); WORLD_SIZE].map(|_| WorldState {
            points: Arc::new(DashMap::new()),
            tx: broadcast::channel(2000).0,
        }),
    });

    // let var : [WorldState; 600]

    let app = Router::new()
        .route("/", get(index))
        .route("/websocket", get(websocket_handler))
        .with_state(app_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::debug!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .expect("Error while running server");
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<Params>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let world_state = state.worlds.get(params.world);

    if world_state.is_none() {
        return Err("World not found");
    }

    let world_points = &world_state.unwrap().points;
    {
        if world_points.get(params.rsn.as_str()).is_some() {
            return Err("RSN already present");
        }
    }
    Ok(ws.on_upgrade(|socket| websocket(socket, state, params)))
}

// This function deals with a single websocket connection, i.e., a single
// connected client / user, for which we will spawn two independent tasks (for
// receiving / sending chat messages).
async fn websocket(stream: WebSocket, state: Arc<AppState>, params: Params) {
    // By splitting, we can send and receive at the same time.
    let (mut sender, mut receiver) = stream.split();

    let world_state = &state.worlds[params.world];

    let full_current_state = {
        serde_json::to_string(&WorldMessage::Sync((*world_state.points).clone()))
            .expect("Failed to serialize sync")
    };
    sender
        .send(Message::Text(full_current_state))
        .await
        .expect("Failed to send initial");

    // We subscribe *before* sending the "joined" message, so that we will also
    // display it to our client.
    let mut rx = world_state.tx.subscribe();

    // Now send the "joined" message to all subscribers.
    tracing::info!("'{}' joined in world '{}'.", &params.rsn, &params.world);

    // Spawn the first task that will receive broadcast messages and send text
    // messages over the websocket to our client.
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            // In any websocket error, break loop.
            let serialized_world_update =
                serde_json::to_string(&msg).expect("Failed to serialize world message");
            if sender
                .send(Message::Text(serialized_world_update))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Clone things we want to pass (move) to the receiving task.
    let tx = world_state.tx.clone();
    let rsn = params.rsn.clone();
    let world_points = state.worlds[params.world].points.clone();

    // Spawn a task that takes messages fro m the websocket, prepends the user
    // name, and sends them to all broadcast subscribers.
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = receiver.next().await {
            if let Ok(points) = u32::from_str(text.as_str()) {
                world_points.insert(rsn.clone(), points);
                let _ = tx.send(WorldMessage::Update(WorldUpdate {
                    rsn: rsn.clone(),
                    points,
                }));
            }
        }
    });

    // If any one of the tasks run to completion, we abort the other.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }

    world_state.points.remove(&params.rsn);
    let send_result = world_state.tx.send(WorldMessage::Remove(WorldRemove {
        rsn: params.rsn.clone(),
    }));
    if let Err(e) = send_result {
        tracing::warn!("Tried to send when no one left {:?}", e);
    }
    tracing::info!("'{}' left in world '{}'.", &params.rsn, &params.world);
    // let _ = state.tx.send(msg);
    //
    // // Remove username from map so new clients can take it again.
    // state.user_set.lock().expect().remove(&username);
}

// Include utf-8 file at **compile** time.
async fn index() -> Html<&'static str> {
    Html(include_str!("../chat.html"))
}
