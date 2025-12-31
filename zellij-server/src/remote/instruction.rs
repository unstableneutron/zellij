use crate::ClientId;
use zellij_remote_core::FrameStore;
use zellij_utils::pane_size::Size;

/// Instructions sent TO the remote thread
#[derive(Debug, Clone)]
pub enum RemoteInstruction {
    /// Frame data ready to be sent to remote clients
    FrameReady {
        client_id: ClientId,
        frame_store: FrameStore,
    },
    /// Client resized their viewport
    ClientResize {
        client_id: ClientId,
        size: Size,
    },
    /// Remote client connected
    ClientConnected {
        client_id: ClientId,
        size: Size,
    },
    /// Remote client disconnected
    ClientDisconnected {
        client_id: ClientId,
    },
    /// Session is shutting down
    Shutdown,
}

/// Instructions sent FROM the remote thread to inject input
#[derive(Debug, Clone)]
pub enum RemoteInputInstruction {
    /// Remote client sent keyboard input
    Key {
        client_id: ClientId,
        key: Vec<u8>,
    },
    /// Remote client sent mouse event
    Mouse {
        client_id: ClientId,
        row: usize,
        col: usize,
        button: u8,
    },
}
