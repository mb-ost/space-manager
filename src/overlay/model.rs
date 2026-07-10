//! Pure indicator/spaces model (AF-8).
//!
//! `build_spaces` turns the managed-window list + current index into the list of
//! buttons the overlay bar renders. This replaces the old `"1-2-[3]-4"` string
//! diffing with a typed, unit-tested model.

use crate::types::ManagedWindow;

/// One button in the overlay bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceButton {
    /// The label to render (custom icon if set, otherwise the 1-based index).
    pub label: String,
    /// Whether this button represents the currently visible space.
    pub is_current: bool,
}

/// Build the button list and the clamped current index.
///
/// If `current` is out of range it is clamped to the last window (or 0 when
/// empty). Never panics.
pub fn build_spaces(windows: &[ManagedWindow], current: usize) -> (Vec<SpaceButton>, usize) {
    if windows.is_empty() {
        return (Vec::new(), 0);
    }

    let clamped = current.min(windows.len() - 1);
    let buttons = windows
        .iter()
        .enumerate()
        .map(|(i, w)| SpaceButton {
            label: w
                .custom_icon
                .clone()
                .unwrap_or_else(|| (i + 1).to_string()),
            is_current: i == clamped,
        })
        .collect();

    (buttons, clamped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ManagedWindow;

    fn win(icon: Option<&str>) -> ManagedWindow {
        let mut w = ManagedWindow::new("cmd".to_string());
        w.custom_icon = icon.map(|s| s.to_string());
        w
    }

    #[test]
    fn test_build_spaces_marks_current() {
        let windows: Vec<_> = (0..5).map(|_| win(None)).collect();
        let (buttons, current) = build_spaces(&windows, 2);
        assert_eq!(current, 2);
        assert!(buttons[2].is_current);
        for (i, b) in buttons.iter().enumerate() {
            if i != 2 {
                assert!(!b.is_current);
            }
        }
    }

    #[test]
    fn test_build_spaces_custom_icon_used() {
        let windows = vec![win(Some("🌐"))];
        let (buttons, _) = build_spaces(&windows, 0);
        assert_eq!(buttons[0].label, "🌐");
    }

    #[test]
    fn test_build_spaces_default_label_is_index() {
        let windows = vec![win(None), win(None), win(None)];
        let (buttons, _) = build_spaces(&windows, 0);
        assert_eq!(buttons[0].label, "1");
        assert_eq!(buttons[1].label, "2");
        assert_eq!(buttons[2].label, "3");
    }

    #[test]
    fn test_build_spaces_empty() {
        let windows: Vec<ManagedWindow> = Vec::new();
        let (buttons, current) = build_spaces(&windows, 0);
        assert!(buttons.is_empty());
        assert_eq!(current, 0);
    }

    #[test]
    fn test_build_spaces_current_out_of_range() {
        let windows = vec![win(None), win(None), win(None)];
        let (buttons, current) = build_spaces(&windows, 9);
        assert_eq!(current, 2);
        assert_eq!(buttons.len(), 3);
        // Only the clamped index is current; nothing past the end.
        assert!(buttons[2].is_current);
        assert!(!buttons[0].is_current);
        assert!(!buttons[1].is_current);
    }
}
