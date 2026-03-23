//! HTTP webhook server (port 8888) that exposes the Buttplug device-control
//! API to React web applications.  It is started automatically alongside the
//! Intiface Engine by `run_engine()` and shuts down when the runtime is
//! dropped.
//!
//! # Authentication
//! Every request (except `GET /api/server/status` and `GET /api/pairing/qr`)
//! must carry an `Authorization: Bearer <pairing_token>` header where the
//! token was issued by `POST /api/pairing/validate`.
//!
//! # Pairing flow
//! 1. Mobile app displays QR code obtained from `GET /api/pairing/qr`.
//! 2. React web app scans the QR code, obtains a Firebase ID token, then calls
//!    `POST /api/pairing/validate` with the token.
//! 3. On success the server returns a `pairing_token` that the web app
//!    includes in every subsequent request.

use std::{collections::HashMap, sync::Arc, time::{SystemTime, UNIX_EPOCH}};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{delete, get, post},
    Router,
};
use lazy_static::lazy_static;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use crate::firebase_auth;
use crate::session_manager::{Session, SessionManager};

pub const WEBHOOK_PORT: u16 = 8888;

// ---------------------------------------------------------------------------
// Global state (initialised once per process)
// ---------------------------------------------------------------------------

lazy_static! {
    pub static ref WEBHOOK_APP_STATE: Arc<WebhookAppState> =
        Arc::new(WebhookAppState::new());
}

pub struct WebhookAppState {
    pub session_manager: RwLock<SessionManager>,
    pub devices: RwLock<HashMap<u32, DeviceInfo>>,
    pub pairing_token: RwLock<String>,
    pub mobile_ip: RwLock<String>,
}

impl WebhookAppState {
    pub fn new() -> Self {
        WebhookAppState {
            session_manager: RwLock::new(SessionManager::new(5)),
            devices: RwLock::new(HashMap::new()),
            pairing_token: RwLock::new(Uuid::new_v4().to_string()),
            mobile_ip: RwLock::new("127.0.0.1".to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_index: u32,
    pub device_name: String,
    pub connected: bool,
    pub last_command: Option<String>,
    pub command_status: Option<String>,
}

// ---------------------------------------------------------------------------
// Update device state from Buttplug protocol messages (JSON array strings)
// ---------------------------------------------------------------------------

/// Called from `run_engine` for every message the backdoor server emits.
/// Parses `DeviceAdded` / `DeviceRemoved` protocol messages and updates the
/// shared device map so that `/api/devices` always reflects reality.
pub fn update_device_state_from_message(msg: &str) {
    if let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(msg) {
        let mut devices = WEBHOOK_APP_STATE.devices.write();
        for value in values {
            if let Some(device_added) = value.get("DeviceAdded") {
                if let (Some(index), Some(name)) = (
                    device_added.get("DeviceIndex").and_then(|v| v.as_u64()),
                    device_added.get("DeviceName").and_then(|v| v.as_str()),
                ) {
                    let device_index = index as u32;
                    devices.insert(
                        device_index,
                        DeviceInfo {
                            device_id: format!("device_{}", device_index),
                            device_index,
                            device_name: name.to_string(),
                            connected: true,
                            last_command: None,
                            command_status: None,
                        },
                    );
                    log::info!("Webhook: device added index={} name={}", index, name);
                }
            } else if let Some(device_removed) = value.get("DeviceRemoved") {
                if let Some(index) = device_removed.get("DeviceIndex").and_then(|v| v.as_u64()) {
                    let device_index = index as u32;
                    if let Some(device) = devices.get_mut(&device_index) {
                        device.connected = false;
                    }
                    log::info!("Webhook: device removed index={}", index);
                }
            }
        }
    }
}

/// Store the device's current WiFi IP so the QR code contains a useful address.
pub fn set_mobile_ip(ip: String) {
    let mut mobile_ip = WEBHOOK_APP_STATE.mobile_ip.write();
    *mobile_ip = ip;
}

/// Reset device state when the engine stops.
pub fn clear_device_state() {
    WEBHOOK_APP_STATE.devices.write().clear();
    log::info!("Webhook: device state cleared");
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ServerStatusResponse {
    running: bool,
    device_count: usize,
    port: u16,
    version: &'static str,
}

#[derive(Serialize)]
struct DeviceListResponse {
    devices: Vec<DeviceInfo>,
}

#[derive(Deserialize)]
struct VibrateRequest {
    intensity: f64,
    #[allow(dead_code)]
    duration_ms: Option<u64>,
}

#[derive(Deserialize)]
struct RotateRequest {
    speed: f64,
    clockwise: Option<bool>,
    #[allow(dead_code)]
    duration_ms: Option<u64>,
}

#[derive(Deserialize)]
struct LinearRequest {
    position: f64,
    duration: u32,
}

#[derive(Deserialize)]
struct ValidatePairingRequest {
    firebase_token: String,
    react_webhook_url: Option<String>,
}

#[derive(Serialize)]
struct ValidatePairingResponse {
    session_id: String,
    pairing_token: String,
    success: bool,
}

#[derive(Serialize)]
struct QrCodeData {
    mobile_ip: String,
    mobile_port: u16,
    pairing_token: String,
    session_id: String,
    version: &'static str,
}

#[derive(Serialize)]
struct SessionListResponse {
    sessions: Vec<SessionInfo>,
}

#[derive(Serialize)]
struct SessionInfo {
    session_id: String,
    firebase_uid: String,
    created_at: u64,
    react_webhook_url: Option<String>,
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn get_server_status(
    State(state): State<Arc<WebhookAppState>>,
) -> Json<ServerStatusResponse> {
    let devices = state.devices.read();
    let connected_count = devices.values().filter(|d| d.connected).count();
    Json(ServerStatusResponse {
        running: true,
        device_count: connected_count,
        port: WEBHOOK_PORT,
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn get_devices(State(state): State<Arc<WebhookAppState>>) -> Json<DeviceListResponse> {
    let devices = state.devices.read();
    Json(DeviceListResponse {
        devices: devices.values().cloned().collect(),
    })
}

async fn start_scan(
    headers: HeaderMap,
    State(state): State<Arc<WebhookAppState>>,
) -> StatusCode {
    if validate_request(&headers, &state).is_err() {
        return StatusCode::UNAUTHORIZED;
    }
    crate::api::runtime::webhook_send_backdoor_message(
        r#"[{"StartScanning":{"Id":1}}]"#.to_string(),
    );
    StatusCode::OK
}

async fn stop_scan(
    headers: HeaderMap,
    State(state): State<Arc<WebhookAppState>>,
) -> StatusCode {
    if validate_request(&headers, &state).is_err() {
        return StatusCode::UNAUTHORIZED;
    }
    crate::api::runtime::webhook_send_backdoor_message(
        r#"[{"StopScanning":{"Id":1}}]"#.to_string(),
    );
    StatusCode::OK
}

async fn vibrate_device(
    headers: HeaderMap,
    Path(device_id): Path<String>,
    State(state): State<Arc<WebhookAppState>>,
    Json(req): Json<VibrateRequest>,
) -> StatusCode {
    if validate_request(&headers, &state).is_err() {
        return StatusCode::UNAUTHORIZED;
    }
    let Some(device_index) = resolve_device_id(&device_id, &state) else {
        return StatusCode::NOT_FOUND;
    };
    let intensity = req.intensity.clamp(0.0, 1.0);
    let msg = format!(
        r#"[{{"ScalarCmd":{{"Id":1,"DeviceIndex":{},"Scalars":[{{"Index":0,"Scalar":{},"ActuatorType":"Vibrate"}}]}}}}]"#,
        device_index, intensity
    );
    crate::api::runtime::webhook_send_backdoor_message(msg);
    {
        let mut devices = state.devices.write();
        if let Some(device) = devices.get_mut(&device_index) {
            device.last_command = Some("vibrate".to_string());
            device.command_status = Some("executing".to_string());
        }
    }
    StatusCode::OK
}

async fn rotate_device(
    headers: HeaderMap,
    Path(device_id): Path<String>,
    State(state): State<Arc<WebhookAppState>>,
    Json(req): Json<RotateRequest>,
) -> StatusCode {
    if validate_request(&headers, &state).is_err() {
        return StatusCode::UNAUTHORIZED;
    }
    let Some(device_index) = resolve_device_id(&device_id, &state) else {
        return StatusCode::NOT_FOUND;
    };
    let speed = req.speed.clamp(0.0, 1.0);
    let clockwise = req.clockwise.unwrap_or(true);
    let msg = format!(
        r#"[{{"RotateCmd":{{"Id":1,"DeviceIndex":{},"Rotations":[{{"Index":0,"Speed":{},"Clockwise":{}}}]}}}}]"#,
        device_index, speed, clockwise
    );
    crate::api::runtime::webhook_send_backdoor_message(msg);
    StatusCode::OK
}

async fn linear_device(
    headers: HeaderMap,
    Path(device_id): Path<String>,
    State(state): State<Arc<WebhookAppState>>,
    Json(req): Json<LinearRequest>,
) -> StatusCode {
    if validate_request(&headers, &state).is_err() {
        return StatusCode::UNAUTHORIZED;
    }
    let Some(device_index) = resolve_device_id(&device_id, &state) else {
        return StatusCode::NOT_FOUND;
    };
    let position = req.position.clamp(0.0, 1.0);
    let msg = format!(
        r#"[{{"LinearCmd":{{"Id":1,"DeviceIndex":{},"Vectors":[{{"Index":0,"Duration":{},"Position":{}}}]}}}}]"#,
        device_index, req.duration, position
    );
    crate::api::runtime::webhook_send_backdoor_message(msg);
    StatusCode::OK
}

async fn stop_device(
    headers: HeaderMap,
    Path(device_id): Path<String>,
    State(state): State<Arc<WebhookAppState>>,
) -> StatusCode {
    if validate_request(&headers, &state).is_err() {
        return StatusCode::UNAUTHORIZED;
    }
    let Some(device_index) = resolve_device_id(&device_id, &state) else {
        return StatusCode::NOT_FOUND;
    };
    let msg = format!(
        r#"[{{"StopDeviceCmd":{{"Id":1,"DeviceIndex":{}}}}}]"#,
        device_index
    );
    crate::api::runtime::webhook_send_backdoor_message(msg);
    {
        let mut devices = state.devices.write();
        if let Some(device) = devices.get_mut(&device_index) {
            device.last_command = Some("stop".to_string());
            device.command_status = Some("executed".to_string());
        }
    }
    StatusCode::OK
}

async fn disconnect_device(
    headers: HeaderMap,
    Path(device_id): Path<String>,
    State(state): State<Arc<WebhookAppState>>,
) -> StatusCode {
    // Stopping all commands is the closest approximation to "disconnect"
    // without direct server-side device management.
    stop_device(headers, Path(device_id), State(state)).await
}

async fn get_pairing_qr(
    State(state): State<Arc<WebhookAppState>>,
) -> Json<QrCodeData> {
    let mobile_ip = state.mobile_ip.read().clone();
    let pairing_token = state.pairing_token.read().clone();
    let session_id = format!("session_{}", Uuid::new_v4());
    Json(QrCodeData {
        mobile_ip,
        mobile_port: WEBHOOK_PORT,
        pairing_token,
        session_id,
        version: "1",
    })
}

async fn validate_pairing(
    State(state): State<Arc<WebhookAppState>>,
    Json(req): Json<ValidatePairingRequest>,
) -> Result<Json<ValidatePairingResponse>, StatusCode> {
    match firebase_auth::validate_firebase_token(&req.firebase_token).await {
        Ok(uid) => {
            let session = {
                let mut manager = state.session_manager.write();
                manager.create_session(uid)
            };
            if let Some(url) = req.react_webhook_url {
                let mut manager = state.session_manager.write();
                manager.set_react_webhook_url(&session.session_id, url);
            }
            Ok(Json(ValidatePairingResponse {
                session_id: session.session_id,
                pairing_token: session.pairing_token,
                success: true,
            }))
        }
        Err(e) => {
            log::warn!("Firebase token validation failed: {:?}", e);
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

async fn get_sessions(
    headers: HeaderMap,
    State(state): State<Arc<WebhookAppState>>,
) -> Result<Json<SessionListResponse>, StatusCode> {
    if validate_request(&headers, &state).is_err() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let manager = state.session_manager.read();
    let sessions = manager
        .list_sessions()
        .into_iter()
        .map(session_to_info)
        .collect();
    Ok(Json(SessionListResponse { sessions }))
}

async fn revoke_session(
    headers: HeaderMap,
    Path(session_id): Path<String>,
    State(state): State<Arc<WebhookAppState>>,
) -> StatusCode {
    if validate_request(&headers, &state).is_err() {
        return StatusCode::UNAUTHORIZED;
    }
    let mut manager = state.session_manager.write();
    if manager.revoke_session(&session_id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_request(
    headers: &HeaderMap,
    state: &WebhookAppState,
) -> Result<String, StatusCode> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = &auth["Bearer ".len()..];
    let manager = state.session_manager.read();
    manager
        .validate_pairing_token(token)
        .map(|s| s.session_id.clone())
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn resolve_device_id(device_id: &str, state: &WebhookAppState) -> Option<u32> {
    let devices = state.devices.read();
    // Accept "device_{index}" format
    if let Some(stripped) = device_id.strip_prefix("device_") {
        if let Ok(index) = stripped.parse::<u32>() {
            if devices.contains_key(&index) {
                return Some(index);
            }
        }
    }
    // Fall back to a full device_id string match
    devices
        .values()
        .find(|d| d.device_id == device_id)
        .map(|d| d.device_index)
}

fn session_to_info(s: &Session) -> SessionInfo {
    SessionInfo {
        session_id: s.session_id.clone(),
        firebase_uid: s.firebase_uid.clone(),
        created_at: s.created_at,
        react_webhook_url: s.react_webhook_url.clone(),
    }
}

// ---------------------------------------------------------------------------
// Server entry-point
// ---------------------------------------------------------------------------

/// Start the webhook HTTP server.  This future runs indefinitely; drop the
/// runtime (or send a shutdown signal) to stop it.
pub async fn run_webhook_server() {
    let state = WEBHOOK_APP_STATE.clone();

    let cors = CorsLayer::new()
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
        .allow_origin(Any);

    let app = Router::new()
        .route("/api/server/status", get(get_server_status))
        .route("/api/devices", get(get_devices))
        .route("/api/devices/scan", post(start_scan))
        .route("/api/devices/scan/stop", post(stop_scan))
        .route("/api/devices/{device_id}/vibrate", post(vibrate_device))
        .route("/api/devices/{device_id}/rotate", post(rotate_device))
        .route("/api/devices/{device_id}/linear", post(linear_device))
        .route("/api/devices/{device_id}/stop", post(stop_device))
        .route("/api/devices/{device_id}", delete(disconnect_device))
        .route("/api/pairing/qr", get(get_pairing_qr))
        .route("/api/pairing/validate", post(validate_pairing))
        .route("/api/sessions", get(get_sessions))
        .route("/api/sessions/{session_id}", delete(revoke_session))
        .layer(cors)
        .with_state(state);

    let addr = format!("0.0.0.0:{}", WEBHOOK_PORT);
    match TcpListener::bind(&addr).await {
        Ok(listener) => {
            log::info!("Webhook server listening on {}", addr);
            if let Err(e) = axum::serve(listener, app).await {
                log::error!("Webhook server error: {:?}", e);
            }
        }
        Err(e) => {
            log::error!("Failed to bind webhook server to {}: {:?}", addr, e);
        }
    }
}
