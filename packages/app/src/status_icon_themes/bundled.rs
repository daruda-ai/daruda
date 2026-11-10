//! Animal packages embedded in every executable, independent of profile files.
use super::{load as loader, model::Catalog};
use std::{path::Path, sync::OnceLock};

#[derive(rust_embed::Embed)]
#[folder = "../../examples/status-icon-themes/"]
#[include = "cats/*"]
#[include = "dogs/*"]
#[include = "rabbits/*"]
#[exclude = "**/*.md"]
struct Assets;

pub(super) fn load() -> (Catalog, Vec<loader::LoadError>) {
    static LOADED: OnceLock<(Catalog, Vec<loader::LoadError>)> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            let mut catalog = Catalog::default();
            let mut errors = Vec::new();
            for path in Assets::iter().filter(|p| p.ends_with("/theme.json")) {
                let path = Path::new(path.as_ref());
                let root = path.parent().expect("bundled theme directory");
                let manifest = Assets::get(&path.to_string_lossy()).expect("embedded manifest");
                let theme = loader::package_bytes(path, "builtin", &manifest.data, |relative| {
                    let asset = root.join(relative).to_string_lossy().replace('\\', "/");
                    Assets::get(&asset)
                        .map(|file| file.data.into())
                        .ok_or_else(|| loader::error(Path::new(&asset), "missing bundled SVG"))
                });
                match theme {
                    Ok(theme) => {
                        catalog.themes.insert(theme.id.clone(), theme);
                    }
                    Err(error) => errors.push(error),
                }
            }
            (catalog, errors)
        })
        .clone()
}

#[cfg(test)]
mod tests;
