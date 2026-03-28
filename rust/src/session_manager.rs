use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub firebase_uid: String,
    pub pairing_token: String,
    pub react_webhook_url: Option<String>,
    pub created_at: u64,
}

pub struct SessionManager {
    sessions: HashMap<String, Session>,
    max_sessions: usize,
}

impl SessionManager {
    pub fn new(max_sessions: usize) -> Self {
        SessionManager {
            sessions: HashMap::new(),
            max_sessions,
        }
    }

    pub fn create_session(&mut self, firebase_uid: String) -> Session {
        // Remove oldest session if at max capacity
        if self.sessions.len() >= self.max_sessions {
            if let Some(oldest_id) = self
                .sessions
                .iter()
                .min_by_key(|(_, s)| s.created_at)
                .map(|(id, _)| id.clone())
            {
                self.sessions.remove(&oldest_id);
            }
        }

        let session_id = Uuid::new_v4().to_string();
        let pairing_token = Uuid::new_v4().to_string();
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let session = Session {
            session_id: session_id.clone(),
            firebase_uid,
            pairing_token,
            react_webhook_url: None,
            created_at,
        };

        self.sessions.insert(session_id, session.clone());
        session
    }

    pub fn validate_pairing_token(&self, token: &str) -> Option<&Session> {
        self.sessions.values().find(|s| s.pairing_token == token)
    }

    pub fn revoke_session(&mut self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }

    pub fn list_sessions(&self) -> Vec<&Session> {
        let mut sessions: Vec<&Session> = self.sessions.values().collect();
        sessions.sort_by_key(|s| s.created_at);
        sessions
    }

    pub fn set_react_webhook_url(&mut self, session_id: &str, url: String) -> bool {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.react_webhook_url = Some(url);
            true
        } else {
            false
        }
    }
}
