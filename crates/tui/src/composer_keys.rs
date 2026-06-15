use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerEditAction {
    InsertChar(char),
    InsertNewline,
    DeleteBefore,
    DeleteAfter,
    DeleteWordBefore,
    MoveLeft,
    MoveRight,
    MoveWordLeft,
    MoveWordRight,
    MoveStart,
    MoveHome,
    MoveEnd,
    MoveUp,
    MoveDown,
}

impl ComposerEditAction {
    pub(crate) fn is_modifier_action(self) -> bool {
        matches!(
            self,
            Self::InsertNewline
                | Self::DeleteWordBefore
                | Self::MoveWordLeft
                | Self::MoveWordRight
                | Self::MoveStart
                | Self::MoveEnd
        )
    }
}

/// Shared readline-style composer editing keymap used by the normal Pod TUI
/// and the workspace panel. Callers still own higher-level routing such as
/// completion popups, Enter submission, Tab target switching, Esc focus, and
/// row/list navigation.
pub(crate) fn composer_edit_action(key: KeyEvent) -> Option<ComposerEditAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        KeyCode::Enter if alt && !ctrl => Some(ComposerEditAction::InsertNewline),
        KeyCode::Char('a') if ctrl && !alt => Some(ComposerEditAction::MoveStart),
        KeyCode::Char('e') if ctrl && !alt => Some(ComposerEditAction::MoveEnd),
        KeyCode::Left if ctrl || alt => Some(ComposerEditAction::MoveWordLeft),
        KeyCode::Right if ctrl || alt => Some(ComposerEditAction::MoveWordRight),
        KeyCode::Backspace if ctrl || alt => Some(ComposerEditAction::DeleteWordBefore),
        KeyCode::Char('w') if ctrl && !alt => Some(ComposerEditAction::DeleteWordBefore),
        KeyCode::Backspace if !ctrl && !alt => Some(ComposerEditAction::DeleteBefore),
        KeyCode::Delete if !ctrl && !alt => Some(ComposerEditAction::DeleteAfter),
        KeyCode::Left if !ctrl && !alt => Some(ComposerEditAction::MoveLeft),
        KeyCode::Right if !ctrl && !alt => Some(ComposerEditAction::MoveRight),
        KeyCode::Up if !ctrl && !alt => Some(ComposerEditAction::MoveUp),
        KeyCode::Down if !ctrl && !alt => Some(ComposerEditAction::MoveDown),
        KeyCode::Home if !alt => Some(ComposerEditAction::MoveHome),
        KeyCode::End if !alt => Some(ComposerEditAction::MoveEnd),
        KeyCode::Char(c) if !ctrl && !alt => Some(ComposerEditAction::InsertChar(c)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn modified(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn maps_word_motion_and_word_delete_keys() {
        assert_eq!(
            composer_edit_action(modified(KeyCode::Left, KeyModifiers::CONTROL)),
            Some(ComposerEditAction::MoveWordLeft)
        );
        assert_eq!(
            composer_edit_action(modified(KeyCode::Right, KeyModifiers::ALT)),
            Some(ComposerEditAction::MoveWordRight)
        );
        assert_eq!(
            composer_edit_action(modified(KeyCode::Backspace, KeyModifiers::CONTROL)),
            Some(ComposerEditAction::DeleteWordBefore)
        );
        assert_eq!(
            composer_edit_action(modified(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            Some(ComposerEditAction::DeleteWordBefore)
        );
    }

    #[test]
    fn maps_alt_enter_to_newline() {
        assert_eq!(
            composer_edit_action(modified(KeyCode::Enter, KeyModifiers::ALT)),
            Some(ComposerEditAction::InsertNewline)
        );
        assert_eq!(
            composer_edit_action(modified(
                KeyCode::Enter,
                KeyModifiers::ALT | KeyModifiers::CONTROL,
            )),
            None
        );
    }

    #[test]
    fn leaves_enter_tab_esc_and_control_letters_for_callers() {
        assert_eq!(composer_edit_action(key(KeyCode::Enter)), None);
        assert_eq!(composer_edit_action(key(KeyCode::Tab)), None);
        assert_eq!(composer_edit_action(key(KeyCode::Esc)), None);
        assert_eq!(
            composer_edit_action(modified(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn plain_letters_remain_text_input() {
        assert_eq!(
            composer_edit_action(key(KeyCode::Char('j'))),
            Some(ComposerEditAction::InsertChar('j'))
        );
        assert_eq!(
            composer_edit_action(key(KeyCode::Char('o'))),
            Some(ComposerEditAction::InsertChar('o'))
        );
    }
}
