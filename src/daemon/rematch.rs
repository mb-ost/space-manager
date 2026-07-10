//! Pure, deterministic window re-match heuristic (R9, AF-1/AF-8).
//!
//! During `resync()` we must re-bind managed windows to live Hyprland clients
//! after their addresses have gone stale (monitor wake, Hyprland reload). This
//! is the single most correctness-critical new piece of logic, so it is a pure
//! function with no I/O and an exhaustive positive/negative test table.
//!
//! Matching rules (strong key first, never guess):
//! 1. **PID** is the strong key. A managed window whose last-known pid is still
//!    present on a live client re-matches to that client. PID wins over class.
//! 2. **Title tiebreak.** For still-unmatched managed windows, among remaining
//!    unmatched clients of the same class, prefer an exact stored-title match.
//! 3. **Oldest-unmatched-first.** Remaining same-class clients are assigned to
//!    managed windows in ascending `id` order (ids are creation-timestamp based,
//!    so this is oldest-first), each taking the next available same-class client.
//! 4. **Never guess.** Any managed window still unmatched is marked closed, never
//!    speculatively bound. A client is assigned to at most one managed window.

use crate::hypr::ClientInfo;
use crate::types::ManagedWindow;

/// Our stable per-window id (never changes across a window's lifetime).
pub type ManagedWindowId = String;

/// The outcome of re-matching managed windows to live clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchOutcome {
    /// `(managed id, client address, client pid)` for confidently matched windows.
    pub matches: Vec<(ManagedWindowId, String, Option<u32>)>,
    /// Managed windows with no confident match (marked closed).
    pub closed: Vec<ManagedWindowId>,
}

/// Re-match managed windows to live clients. Pure and deterministic.
pub fn match_windows(managed: &[ManagedWindow], clients: &[ClientInfo]) -> MatchOutcome {
    let mut matches: Vec<(ManagedWindowId, String, Option<u32>)> = Vec::new();

    // Managed window indices still needing a match.
    let mut pending: Vec<usize> = (0..managed.len()).collect();
    // Client indices not yet assigned to a managed window.
    let mut client_used = vec![false; clients.len()];

    let record_match =
        |matches: &mut Vec<(ManagedWindowId, String, Option<u32>)>, m: &ManagedWindow, c: &ClientInfo| {
            matches.push((m.id.clone(), c.address.clone(), c.pid));
        };

    // --- Step 1: PID strong key ---
    pending.retain(|&mi| {
        let m = &managed[mi];
        if let Some(pid) = m.pid {
            if let Some(ci) = clients
                .iter()
                .enumerate()
                .position(|(ci, c)| !client_used[ci] && c.pid == Some(pid))
            {
                client_used[ci] = true;
                record_match(&mut matches, m, &clients[ci]);
                return false; // matched -> drop from pending
            }
        }
        true
    });

    // --- Step 2: exact title tiebreak within same class ---
    pending.retain(|&mi| {
        let m = &managed[mi];
        if m.class.is_empty() {
            return true;
        }
        if let Some(ci) = clients.iter().enumerate().position(|(ci, c)| {
            !client_used[ci] && c.class == m.class && !m.title.is_empty() && c.title == m.title
        }) {
            client_used[ci] = true;
            record_match(&mut matches, m, &clients[ci]);
            return false;
        }
        true
    });

    // --- Step 3: oldest-unmatched-first by ascending id, same class ---
    // Sort remaining managed windows by id (creation order) for stable assignment.
    pending.sort_by(|&a, &b| managed[a].id.cmp(&managed[b].id));
    pending.retain(|&mi| {
        let m = &managed[mi];
        if m.class.is_empty() {
            return true;
        }
        if let Some(ci) = clients
            .iter()
            .enumerate()
            .position(|(ci, c)| !client_used[ci] && c.class == m.class)
        {
            client_used[ci] = true;
            record_match(&mut matches, m, &clients[ci]);
            return false;
        }
        true
    });

    // --- Step 4: never guess -> mark remaining closed ---
    let closed = pending.into_iter().map(|mi| managed[mi].id.clone()).collect();

    MatchOutcome { matches, closed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypr::ClientInfo;
    use crate::types::ManagedWindow;

    fn client(addr: &str, class: &str, title: &str, pid: Option<u32>) -> ClientInfo {
        ClientInfo {
            address: addr.to_string(),
            class: class.to_string(),
            title: title.to_string(),
            pid,
            at: (0, 0),
            size: (100, 100),
            workspace_id: 1,
            workspace_name: "1".to_string(),
            monitor: Some(0),
        }
    }

    fn managed(id: &str, class: &str, title: &str, pid: Option<u32>) -> ManagedWindow {
        let mut w = ManagedWindow::new("cmd".to_string());
        w.id = id.to_string();
        w.class = class.to_string();
        w.title = title.to_string();
        w.pid = pid;
        w.address = String::new();
        w
    }

    fn matched_addr(outcome: &MatchOutcome, id: &str) -> Option<String> {
        outcome
            .matches
            .iter()
            .find(|(mid, _, _)| mid == id)
            .map(|(_, addr, _)| addr.clone())
    }

    #[test]
    fn test_match_windows_by_pid_alive() {
        let managed = vec![managed("win_1", "brave", "A", Some(100))];
        let clients = vec![client("0xaaa", "brave", "A", Some(100))];
        let out = match_windows(&managed, &clients);
        assert_eq!(matched_addr(&out, "win_1"), Some("0xaaa".to_string()));
        assert!(out.closed.is_empty());
    }

    #[test]
    fn test_match_windows_pid_wins_over_class() {
        // Client class differs from stored class, but pid matches -> still binds.
        let managed = vec![managed("win_1", "brave", "A", Some(100))];
        let clients = vec![client("0xaaa", "totally-different", "Z", Some(100))];
        let out = match_windows(&managed, &clients);
        assert_eq!(matched_addr(&out, "win_1"), Some("0xaaa".to_string()));
    }

    #[test]
    fn test_match_windows_title_tiebreak() {
        // Two same-class managed windows, no pids; one client title matches.
        let managed = vec![
            managed("win_1", "brave", "Work", None),
            managed("win_2", "brave", "Personal", None),
        ];
        let clients = vec![
            client("0xaaa", "brave", "Personal", None),
            client("0xbbb", "brave", "Work", None),
        ];
        let out = match_windows(&managed, &clients);
        assert_eq!(matched_addr(&out, "win_1"), Some("0xbbb".to_string()));
        assert_eq!(matched_addr(&out, "win_2"), Some("0xaaa".to_string()));
    }

    #[test]
    fn test_match_windows_two_same_class_disambiguated_by_pid() {
        let managed = vec![
            managed("win_1", "brave", "A", Some(100)),
            managed("win_2", "brave", "A", Some(200)),
        ];
        let clients = vec![
            client("0xaaa", "brave", "A", Some(200)),
            client("0xbbb", "brave", "A", Some(100)),
        ];
        let out = match_windows(&managed, &clients);
        assert_eq!(matched_addr(&out, "win_1"), Some("0xbbb".to_string()));
        assert_eq!(matched_addr(&out, "win_2"), Some("0xaaa".to_string()));
    }

    #[test]
    fn test_match_windows_ambiguous_oldest_first() {
        // No pid/title signal; assignment by ascending id (oldest first), stable.
        let managed = vec![
            managed("win_2", "brave", "", None),
            managed("win_1", "brave", "", None),
        ];
        let clients = vec![
            client("0xaaa", "brave", "", None),
            client("0xbbb", "brave", "", None),
        ];
        let out = match_windows(&managed, &clients);
        // win_1 (oldest by id) takes the first available client (0xaaa).
        assert_eq!(matched_addr(&out, "win_1"), Some("0xaaa".to_string()));
        assert_eq!(matched_addr(&out, "win_2"), Some("0xbbb".to_string()));
    }

    #[test]
    fn test_match_windows_pid_dead_marked_closed() {
        // pid gone, and no same-class client to fall back on.
        let managed = vec![managed("win_1", "brave", "A", Some(999))];
        let clients = vec![client("0xaaa", "firefox", "Z", Some(100))];
        let out = match_windows(&managed, &clients);
        assert!(matched_addr(&out, "win_1").is_none());
        assert_eq!(out.closed, vec!["win_1".to_string()]);
    }

    #[test]
    fn test_match_windows_no_client_marked_closed() {
        let managed = vec![managed("win_1", "brave", "A", None)];
        let clients: Vec<ClientInfo> = Vec::new();
        let out = match_windows(&managed, &clients);
        assert!(out.matches.is_empty());
        assert_eq!(out.closed, vec!["win_1".to_string()]);
    }

    #[test]
    fn test_match_windows_client_assigned_once() {
        // Two managed windows, one client -> only one binds, other closed.
        let managed = vec![
            managed("win_1", "brave", "", None),
            managed("win_2", "brave", "", None),
        ];
        let clients = vec![client("0xaaa", "brave", "", None)];
        let out = match_windows(&managed, &clients);
        assert_eq!(out.matches.len(), 1);
        assert_eq!(out.closed.len(), 1);
        // The single client is assigned to exactly one managed window.
        let assigned: Vec<_> = out.matches.iter().map(|(_, a, _)| a.clone()).collect();
        assert_eq!(assigned, vec!["0xaaa".to_string()]);
    }

    #[test]
    fn test_match_windows_empty_inputs() {
        let out = match_windows(&[], &[]);
        assert!(out.matches.is_empty());
        assert!(out.closed.is_empty());

        let managed = vec![managed("win_1", "brave", "A", Some(1))];
        let out = match_windows(&managed, &[]);
        assert!(out.matches.is_empty());
        assert_eq!(out.closed, vec!["win_1".to_string()]);
    }
}
