use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

const API_CHAT_EVENT: &str = "api-chat://request";
const API_CHAT_CANCEL_EVENT: &str = "api-chat://cancel";
pub const API_CHAT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub struct ApiChatBridgeState {
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<ApiChatBridgeEvent>>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiChatBridgeRequest {
    pub request_id: String,
    pub project_id: String,
    pub project_path: String,
    pub project_name: String,
    pub message: String,
    pub use_web_search: bool,
    pub use_any_txt_search: bool,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReference {
    pub title: String,
    pub path: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ApiChatBridgeEvent {
    Start,
    Context {
        references: Vec<ChatReference>,
        #[serde(default)]
        warnings: Vec<String>,
    },
    Token {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Done {
        response: String,
        references: Vec<ChatReference>,
        #[serde(default)]
        warnings: Vec<String>,
    },
    Error {
        error: String,
    },
}

pub fn dispatch_api_chat_request(
    app: &AppHandle,
    request: ApiChatBridgeRequest,
) -> Result<mpsc::Receiver<ApiChatBridgeEvent>, String> {
    let request_id = request.request_id.clone();
    let (tx, rx) = mpsc::channel();
    let state = app.state::<ApiChatBridgeState>();
    state
        .pending
        .lock()
        .map_err(|_| "API chat bridge state is unavailable".to_string())?
        .insert(request_id.clone(), tx);

    if let Err(err) = app.emit(API_CHAT_EVENT, &request) {
        let _ = state
            .pending
            .lock()
            .map(|mut pending| pending.remove(&request_id));
        return Err(format!("Failed to dispatch chat request to the WebView: {err}"));
    }

    Ok(rx)
}

pub fn drop_pending_api_chat_request(app: &AppHandle, request_id: &str) {
    let state = app.state::<ApiChatBridgeState>();
    if let Ok(mut pending) = state.pending.lock() {
        pending.remove(request_id);
    };
}

pub fn cancel_pending_api_chat_request(app: &AppHandle, request_id: &str) {
    drop_pending_api_chat_request(app, request_id);
    let _ = app.emit(API_CHAT_CANCEL_EVENT, request_id.to_string());
}

#[tauri::command]
pub fn api_chat_bridge_push_event(
    state: State<'_, ApiChatBridgeState>,
    request_id: String,
    event: ApiChatBridgeEvent,
) -> Result<(), String> {
    let sender = state
        .pending
        .lock()
        .map_err(|_| "API chat bridge state is unavailable".to_string())?
        .get(&request_id)
        .cloned()
        .ok_or_else(|| format!("No pending API chat request found for {request_id}"))?;

    sender
        .send(event.clone())
        .map_err(|_| format!("API chat request channel closed for {request_id}"))?;

    if matches!(event, ApiChatBridgeEvent::Done { .. } | ApiChatBridgeEvent::Error { .. }) {
        if let Ok(mut pending) = state.pending.lock() {
            pending.remove(&request_id);
        }
    }

    Ok(())
}
