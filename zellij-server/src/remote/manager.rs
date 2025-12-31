use std::collections::HashMap;

use crate::ClientId;
use zellij_remote_core::{RemoteSession, RenderUpdate, StyleTable};
use zellij_utils::pane_size::Size;

/// Manages remote client connections and state
pub struct RemoteManager {
    /// The remote session that tracks all state
    session: RemoteSession,
    /// Shared style table for efficient encoding
    style_table: StyleTable,
    /// Maps Zellij ClientId to remote internal client ID
    client_mapping: HashMap<ClientId, u64>,
    /// Next remote client ID to assign
    next_remote_id: u64,
    /// Current screen dimensions
    cols: usize,
    rows: usize,
}

impl RemoteManager {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            session: RemoteSession::new(cols, rows),
            style_table: StyleTable::new(),
            client_mapping: HashMap::new(),
            next_remote_id: 1,
            cols,
            rows,
        }
    }

    /// Register a new remote client, returns the remote client ID
    ///
    /// If the zellij_id is already registered, the old remote client is removed first.
    pub fn add_client(&mut self, zellij_id: ClientId, size: Size) -> u64 {
        if let Some(old_remote_id) = self.client_mapping.remove(&zellij_id) {
            self.session.remove_client(old_remote_id);
            log::info!(
                "Removed existing remote client: zellij_id={}, old_remote_id={}",
                zellij_id,
                old_remote_id
            );
        }

        let remote_id = self.next_remote_id;
        self.next_remote_id += 1;
        self.client_mapping.insert(zellij_id, remote_id);

        let window_size = Self::compute_window_size(&size);
        self.session.add_client(remote_id, window_size);
        log::info!(
            "Remote client registered: zellij_id={}, remote_id={}, size={:?}",
            zellij_id,
            remote_id,
            size
        );
        remote_id
    }

    /// Compute the render window size from terminal dimensions
    ///
    /// The window_size determines how many frames ahead the client can buffer.
    /// We use a small fixed window for now; this can be tuned based on RTT.
    fn compute_window_size(_size: &Size) -> u32 {
        4
    }

    /// Remove a remote client
    pub fn remove_client(&mut self, zellij_id: ClientId) {
        if let Some(remote_id) = self.client_mapping.remove(&zellij_id) {
            self.session.remove_client(remote_id);
            log::info!(
                "Remote client removed: zellij_id={}, remote_id={}",
                zellij_id,
                remote_id
            );
        }
    }

    /// Get remote ID for a Zellij client
    pub fn get_remote_id(&self, zellij_id: ClientId) -> Option<u64> {
        self.client_mapping.get(&zellij_id).copied()
    }

    /// Check if a Zellij client is remote
    pub fn is_remote_client(&self, zellij_id: ClientId) -> bool {
        self.client_mapping.contains_key(&zellij_id)
    }

    /// Get mutable access to session
    pub fn session_mut(&mut self) -> &mut RemoteSession {
        &mut self.session
    }

    /// Get reference to session
    pub fn session(&self) -> &RemoteSession {
        &self.session
    }

    /// Get mutable access to style table
    pub fn style_table_mut(&mut self) -> &mut StyleTable {
        &mut self.style_table
    }

    /// Get reference to style table
    pub fn style_table(&self) -> &StyleTable {
        &self.style_table
    }

    /// Get render update for a specific client
    pub fn get_render_update(&mut self, zellij_id: ClientId) -> Option<RenderUpdate> {
        let remote_id = self.get_remote_id(zellij_id)?;
        self.session.get_render_update(remote_id)
    }

    /// Get number of connected remote clients
    pub fn client_count(&self) -> usize {
        self.client_mapping.len()
    }

    /// Get current screen dimensions
    pub fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Update screen dimensions
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.session.frame_store.resize(cols, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_size() -> Size {
        Size { rows: 24, cols: 80 }
    }

    #[test]
    fn test_add_remove_client() {
        let mut manager = RemoteManager::new(80, 24);

        let remote_id = manager.add_client(1, test_size());
        assert_eq!(remote_id, 1);
        assert!(manager.is_remote_client(1));
        assert_eq!(manager.client_count(), 1);

        manager.remove_client(1);
        assert!(!manager.is_remote_client(1));
        assert_eq!(manager.client_count(), 0);
    }

    #[test]
    fn test_multiple_clients() {
        let mut manager = RemoteManager::new(80, 24);

        let id1 = manager.add_client(1, test_size());
        let id2 = manager.add_client(2, test_size());

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(manager.client_count(), 2);

        assert_eq!(manager.get_remote_id(1), Some(1));
        assert_eq!(manager.get_remote_id(2), Some(2));
        assert_eq!(manager.get_remote_id(3), None);
    }

    #[test]
    fn test_duplicate_add_client_replaces_old() {
        let mut manager = RemoteManager::new(80, 24);

        let id1 = manager.add_client(1, test_size());
        assert_eq!(id1, 1);
        assert_eq!(manager.client_count(), 1);

        let id2 = manager.add_client(1, test_size());
        assert_eq!(id2, 2);
        assert_eq!(manager.client_count(), 1);

        assert_eq!(manager.get_remote_id(1), Some(2));
    }

    #[test]
    fn test_resize_updates_frame_store() {
        let mut manager = RemoteManager::new(80, 24);
        assert_eq!(manager.dimensions(), (80, 24));

        manager.resize(120, 40);
        assert_eq!(manager.dimensions(), (120, 40));
    }
}
