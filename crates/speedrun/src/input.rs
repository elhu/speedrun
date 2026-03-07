use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Every user-visible action the player can perform.
///
/// This enum is the single source of truth for keybinding dispatch.
/// New actions start as no-ops in `app.rs::handle_action` and are
/// wired up in later Phase 3 epics.
#[derive(Debug, PartialEq)]
pub enum Action {
    Quit,
    TogglePlayback,
    SeekForward,
    SeekBackward,
    SeekForward30s,
    SeekBackward30s,
    StepForward,
    StepBackward,
    SpeedUp,
    SpeedDown,
    NextMarker,
    PrevMarker,
    JumpToPercent(u8),
    JumpToStart,
    JumpToEnd,
    ToggleControls,
    ToggleHelp,
    StartSearch,
    NextMatch,
    PrevMatch,
    Escape,
    AddMarker,
    AddLabeledMarker,
}

/// Map a crossterm key event to a player action.
///
/// Returns `None` for unmapped keys. The caller is responsible for
/// filtering out non-Press events before calling this function.
pub fn map_key_event(key: KeyEvent) -> Option<Action> {
    match key.code {
        // Quit
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Esc => Some(Action::Escape),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),

        // Search
        KeyCode::Char('/') => Some(Action::StartSearch),
        KeyCode::Char('n') => Some(Action::NextMatch),
        KeyCode::Char('N') => Some(Action::PrevMatch),

        // Playback toggle
        KeyCode::Char(' ') => Some(Action::TogglePlayback),

        // Seeking — Shift+arrow for 30s, plain arrow for 5s
        KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(Action::SeekForward30s)
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(Action::SeekBackward30s)
        }
        KeyCode::Right => Some(Action::SeekForward),
        KeyCode::Left => Some(Action::SeekBackward),

        // Step frame-by-frame
        KeyCode::Char('.') => Some(Action::StepForward),
        KeyCode::Char(',') => Some(Action::StepBackward),

        // Speed control
        KeyCode::Char('+') | KeyCode::Char('=') => Some(Action::SpeedUp),
        KeyCode::Char('-') => Some(Action::SpeedDown),

        // Marker navigation
        KeyCode::Char(']') => Some(Action::NextMarker),
        KeyCode::Char('[') => Some(Action::PrevMarker),

        // Marker authoring
        KeyCode::Char('m') => Some(Action::AddMarker),
        KeyCode::Char('M') => Some(Action::AddLabeledMarker),

        // Percent jump (digit keys 0-9)
        KeyCode::Char(c @ '0'..='9') => {
            let digit = c as u8 - b'0';
            Some(Action::JumpToPercent(digit))
        }

        // Jump to start/end
        KeyCode::Home | KeyCode::Char('g') => Some(Action::JumpToStart),
        KeyCode::End | KeyCode::Char('G') => Some(Action::JumpToEnd),

        // UI toggles
        KeyCode::Tab => Some(Action::ToggleControls),
        KeyCode::Char('?') => Some(Action::ToggleHelp),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ───────────────────────────────────────────────────────────────

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn key_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    // ── Quit ─────────────────────────────────────────────────────────────────

    #[test]
    fn q_quits() {
        assert_eq!(map_key_event(key(KeyCode::Char('q'))), Some(Action::Quit));
    }

    #[test]
    fn esc_maps_to_escape() {
        assert_eq!(map_key_event(key(KeyCode::Esc)), Some(Action::Escape));
    }

    #[test]
    fn ctrl_c_quits() {
        assert_eq!(
            map_key_event(key_ctrl(KeyCode::Char('c'))),
            Some(Action::Quit)
        );
    }

    // ── Playback ─────────────────────────────────────────────────────────────

    #[test]
    fn space_toggles_playback() {
        assert_eq!(
            map_key_event(key(KeyCode::Char(' '))),
            Some(Action::TogglePlayback)
        );
    }

    // ── Seeking ──────────────────────────────────────────────────────────────

    #[test]
    fn right_seeks_forward() {
        assert_eq!(
            map_key_event(key(KeyCode::Right)),
            Some(Action::SeekForward)
        );
    }

    #[test]
    fn left_seeks_backward() {
        assert_eq!(
            map_key_event(key(KeyCode::Left)),
            Some(Action::SeekBackward)
        );
    }

    #[test]
    fn shift_right_seeks_forward_30s() {
        assert_eq!(
            map_key_event(key_shift(KeyCode::Right)),
            Some(Action::SeekForward30s)
        );
    }

    #[test]
    fn shift_left_seeks_backward_30s() {
        assert_eq!(
            map_key_event(key_shift(KeyCode::Left)),
            Some(Action::SeekBackward30s)
        );
    }

    // ── Stepping ─────────────────────────────────────────────────────────────

    #[test]
    fn dot_steps_forward() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('.'))),
            Some(Action::StepForward)
        );
    }

    #[test]
    fn comma_steps_backward() {
        assert_eq!(
            map_key_event(key(KeyCode::Char(','))),
            Some(Action::StepBackward)
        );
    }

    // ── Speed control ────────────────────────────────────────────────────────

    #[test]
    fn plus_speeds_up() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('+'))),
            Some(Action::SpeedUp)
        );
    }

    #[test]
    fn equals_speeds_up() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('='))),
            Some(Action::SpeedUp)
        );
    }

    #[test]
    fn minus_speeds_down() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('-'))),
            Some(Action::SpeedDown)
        );
    }

    // ── Marker navigation ────────────────────────────────────────────────────

    #[test]
    fn right_bracket_next_marker() {
        assert_eq!(
            map_key_event(key(KeyCode::Char(']'))),
            Some(Action::NextMarker)
        );
    }

    #[test]
    fn left_bracket_prev_marker() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('['))),
            Some(Action::PrevMarker)
        );
    }

    // ── Percent jump ─────────────────────────────────────────────────────────

    #[test]
    fn digit_keys_jump_to_percent() {
        for digit in 0..=9u8 {
            let c = (b'0' + digit) as char;
            assert_eq!(
                map_key_event(key(KeyCode::Char(c))),
                Some(Action::JumpToPercent(digit)),
                "digit {digit} should map to JumpToPercent({digit})"
            );
        }
    }

    // ── Jump to start/end ────────────────────────────────────────────────────

    #[test]
    fn home_jumps_to_start() {
        assert_eq!(map_key_event(key(KeyCode::Home)), Some(Action::JumpToStart));
    }

    #[test]
    fn g_jumps_to_start() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('g'))),
            Some(Action::JumpToStart)
        );
    }

    #[test]
    fn end_jumps_to_end() {
        assert_eq!(map_key_event(key(KeyCode::End)), Some(Action::JumpToEnd));
    }

    #[test]
    fn shift_g_jumps_to_end() {
        assert_eq!(
            map_key_event(key_shift(KeyCode::Char('G'))),
            Some(Action::JumpToEnd)
        );
    }

    // ── UI toggles ───────────────────────────────────────────────────────────

    #[test]
    fn tab_toggles_controls() {
        assert_eq!(
            map_key_event(key(KeyCode::Tab)),
            Some(Action::ToggleControls)
        );
    }

    #[test]
    fn question_mark_toggles_help() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('?'))),
            Some(Action::ToggleHelp)
        );
    }

    // ── Search key mappings ─────────────────────────────────────────────────

    #[test]
    fn slash_maps_to_start_search() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('/'))),
            Some(Action::StartSearch)
        );
    }

    #[test]
    fn n_maps_to_next_match() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('n'))),
            Some(Action::NextMatch)
        );
    }

    #[test]
    fn shift_n_maps_to_prev_match() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('N'))),
            Some(Action::PrevMatch)
        );
    }

    // ── Marker authoring ──────────────────────────────────────────────────────

    #[test]
    fn m_maps_to_add_marker() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('m'))),
            Some(Action::AddMarker)
        );
    }

    #[test]
    fn test_shift_m_maps_to_add_labeled_marker() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('M'))),
            Some(Action::AddLabeledMarker)
        );
    }

    // ── Unmapped keys ────────────────────────────────────────────────────────

    #[test]
    fn unmapped_keys_return_none() {
        assert_eq!(map_key_event(key(KeyCode::Char('x'))), None);
        assert_eq!(map_key_event(key(KeyCode::F(1))), None);
        assert_eq!(map_key_event(key(KeyCode::Char('a'))), None);
        assert_eq!(map_key_event(key(KeyCode::Insert)), None);
        assert_eq!(map_key_event(key(KeyCode::Delete)), None);
    }
}
