use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum HelperRequest {
    Ping,
    CurrentInterfaceType {
        interface: String,
    },
    SetMonitorMode {
        interface: String,
        monitor_name: Option<String>,
    },
    SetChannel {
        interface: String,
        channel: u16,
        ht_mode: String,
    },
    SetInterfaceType {
        interface: String,
        if_type: String,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperResponse {
    pub ok: bool,
    pub result: Option<String>,
    pub error: Option<String>,
}

impl HelperResponse {
    pub fn ok(result: Option<String>) -> Self {
        Self {
            ok: true,
            result,
            error: None,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(message.into()),
        }
    }
}
