//! Host-provided artwork choices, including unavailable saved selections.
use crate::{SettingsView, strings as s};
use daruda_config::StatusIconTheme;
use daruda_ui::select::SelectOption;
use gpui::{Context, SharedString, Window};

pub(super) fn options(
    themes: &[(String, String)],
    selected: &StatusIconTheme,
) -> Vec<SelectOption> {
    let mut choices = vec![
        SelectOption::new("dots", s::settings::status_icon_theme_dots()),
        SelectOption::new("icons", s::settings::status_icon_theme_icons()),
    ];
    choices.extend(themes.iter().map(|(id, name)| {
        let label = match id.as_str() {
            "builtin/cats" => s::settings::status_icon_theme_cats(),
            "builtin/dogs" => s::settings::status_icon_theme_dogs(),
            "builtin/rabbits" => s::settings::status_icon_theme_rabbits(),
            _ => format!("{name} · {id}"),
        };
        SelectOption::new(id.clone(), label)
    }));
    if let StatusIconTheme::Custom(id) = selected
        && !themes.iter().any(|(choice, _)| choice == id)
    {
        choices.push(SelectOption::new(
            id.clone(),
            format!("{} · {id}", s::settings::status_icon_theme_unavailable()),
        ));
    }
    choices
}
impl SettingsView {
    pub fn set_status_icon_themes(
        &mut self,
        themes: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.status_icon_themes = themes;
        self.refresh_status_icon_theme_select(window, cx);
        cx.notify();
    }
    /// Both the host's catalog and the live selection shape the option list,
    /// so every refresh rebuilds it from the store rather than moving the value.
    pub(super) fn refresh_status_icon_theme_select(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let live = crate::store::SettingsStore::global(cx).user_arc();
        let selected = &live.left_dock.status_icon_theme;
        let choices = options(&self.status_icon_themes, selected);
        let value = SharedString::from(selected.as_str().to_owned());
        self.status_icon_theme_select.update(cx, |select, cx| {
            select.set_items(choices, window, cx);
            select.set_selected_value(&value, window, cx);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_choices_use_localized_labels_and_keep_user_copies_distinct() {
        let themes = vec![
            ("builtin/cats".into(), "Cats".into()),
            ("builtin/dogs".into(), "Dogs".into()),
            ("builtin/rabbits".into(), "Rabbits".into()),
            ("user/cats".into(), "My cats".into()),
        ];
        let choices = options(&themes, &StatusIconTheme::Custom("builtin/cats".into()));
        assert_eq!(choices.len(), 6);
        for (id, label) in [
            ("builtin/cats", s::settings::status_icon_theme_cats()),
            ("builtin/dogs", s::settings::status_icon_theme_dogs()),
            ("builtin/rabbits", s::settings::status_icon_theme_rabbits()),
            ("user/cats", "My cats · user/cats".into()),
        ] {
            let choice = choices.iter().find(|o| o.value.as_ref() == id).unwrap();
            assert_eq!(choice.label.as_ref(), label, "{id}");
        }
    }

    #[test]
    fn unavailable_selection_stays_selectable_without_hiding_builtins() {
        let selected = StatusIconTheme::Custom("user/gone".into());
        let unavailable = options(&[], &selected);
        let entry = unavailable
            .iter()
            .find(|o| o.value.as_ref() == "user/gone")
            .unwrap();
        assert!(
            entry
                .label
                .contains(&s::settings::status_icon_theme_unavailable())
        );
        assert_eq!(
            options(&[("user/gone".into(), "Returned".into())], &selected).len(),
            3
        );
    }
}
