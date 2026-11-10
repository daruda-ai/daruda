use super::*;
use crate::status_icon_themes::model::{LayerColor, State};

#[test]
fn animal_themes_embed_all_states_and_opaque_facial_layers() {
    for path in Assets::iter() {
        assert!(
            matches!(
                Assets::get(&path).unwrap().data,
                std::borrow::Cow::Borrowed(_)
            ),
            "{path} must be embedded even in debug packages"
        );
    }
    let (catalog, errors) = load();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(catalog.themes.len(), 3);
    for name in ["cats", "dogs", "rabbits"] {
        let theme = &catalog.themes[&format!("builtin/{name}")];
        assert_eq!(theme.states.len(), 9);
        let working = &theme.states[&State::Working];
        assert_eq!(working.colors, Some((0x3d7fc7, 0x326797)));
        for (state, artwork) in &theme.states {
            assert!(!artwork.frames.is_empty(), "{name}/{state:?}");
            for frame in &artwork.frames {
                assert!(!frame.layers.is_empty());
                if *state != State::Disconnected {
                    assert!(
                        frame
                            .layers
                            .iter()
                            .any(|layer| layer.color == LayerColor::Fixed(0xf7f8f8)),
                        "{name}/{state:?} must retain facial whites"
                    );
                }
            }
        }
        for state in [
            State::Connecting,
            State::Working,
            State::ExecutingTool,
            State::NeedsAttention,
        ] {
            let poses: std::collections::HashSet<_> = theme.states[&state]
                .frames
                .iter()
                .map(|frame| &frame.layers[0].key)
                .collect();
            assert!(poses.len() > 1, "{name}/{state:?} must visibly animate");
        }
    }
}
