# Status icon themes

Settings → Workspace → Sidebar → Status icon theme selects the artwork used by
left-dock lane and session indicators, including the compact rail. Dots,
Icons, Cats, Dogs and Rabbits are bundled; Dots is the default. Animal artwork
is embedded in release and debug executables on every platform, so it needs no
separate installation. Their stable IDs are `builtin/cats`, `builtin/dogs` and
`builtin/rabbits`. Custom themes inherit Dots or Icons and override individual
states.
Selections apply immediately and survive restart; activity, permission, elapsed
time and completion decisions remain owned by the sessions.

## Examples

- [Soft Ice](soft-ice/theme.json) inherits Icons and overrides tool execution
  with two animated frames.
- [Cats](cats/README.md) replaces all nine states with animated kitten artwork.
- [Dogs](dogs/README.md) and [Rabbits](rabbits/README.md) provide nine static poses.
  All three use opaque facial layers, an empty home for Disconnected, and a
  raised front paw for Needs attention.

The animal directories are also the source of the bundled artwork. Copy one to
the user theme folder only to customize it; that copy has a separate `user/` ID.
Existing `user/cats`, `user/dogs` and `user/rabbits` selections keep using their
installed files. Select the bundled theme to use the artwork shipped with the
app. Updates never overwrite user packages or change the saved selection.

## Create a user theme

1. Choose **Open theme folder** beside the selector. This opens the active
   profile's `status-icon-themes` directory, beside `config.toml`. With an explicit
   `DARUDA_DATA_DIR`, it is `<DARUDA_DATA_DIR>/status-icon-themes`.
2. Copy [the Soft Ice example](soft-ice/theme.json)
   and its two SVG files into a child directory, for example `soft-ice/`.
3. Edit `theme.json` and the SVG files. Saved changes are discovered automatically,
   including while Settings is open. Choose `Soft Ice · user/soft-ice`.

The JSON `id`, rather than the folder or display name, identifies a theme. Keep
it stable when publishing updates. Each package has this layout:

```text
status-icon-themes/
  soft-ice/
    theme.json
    tool-0.svg
    tool-1.svg
```

```json
{
  "version": 1,
  "id": "soft-ice",
  "name": "Soft Ice",
  "extends": "icons",
  "states": {
    "executing_tool": {
      "frames": ["tool-0.svg", "tool-1.svg"],
      "ticks_per_frame": 2,
      "colors": { "dark": "#81cfe6", "light": "#256780" }
    }
  }
}
```

`extends` is `dots` (default) or `icons`. Missing states, frames and colors
inherit the selected base. A colors-only override preserves the base animation.
A single frame makes that state static. Colors must provide both dark and light
`#rrggbb` values. Each SVG is a monochrome mask tinted with the selected color.
Use opacity inside SVG for dimmer elements. A frame can combine masks in order
with fixed-color details, such as white eyes that remain white in dark mode:

```json
{
  "layers": [
    { "svg": "working-0.svg" },
    { "svg": "working-0-white.svg", "color": "#f7f8f8" }
  ]
}
```

Place this object in the state's `frames` array instead of a filename. A layer
without `color` inherits the state's dark/light tint; a fixed `#rrggbb` color
applies in both appearances. All layers share the 16 × 16 view box and advance
together on the existing frame clock. SVG fill colors do not select layer colors.
Layered frames require an app build with layer support; older builds accept only
filename frames.

| State key | Meaning | Multiple frames allowed |
|---|---|---|
| `disconnected` | Agent chat before connection; gray plug in Icons | No |
| `connecting` | Connection preparation or handshake | Yes |
| `working` | Busy session, including live background subagents | Yes |
| `executing_tool` | Tool execution detail while busy | Yes |
| `needs_attention` | Waiting for user confirmation | Yes |
| `idle` | Connected and waiting | No |
| `completed` | Unseen successful result | No |
| `errored` | Unseen execution error | No |
| `failed` | Failed session | No |

The application shares one 250ms animation clock. `ticks_per_frame` defaults to
1 and accepts 1–40; 2 means each frame holds for 500ms. A sequence loops. Themes
cannot start timers, change status priority or keep a completed session busy.
Static frames do not request animation pulses. The normal artwork boxes remain
16px and 12px; verify both, in dark and light appearances.

SVG files must be UTF-8, self-contained shapes (`svg`, `g`, `path`, `rect`,
`circle`, `ellipse`, `line`, `polyline`, `polygon`, `title`, `desc`). Prefer
`viewBox="0 0 16 16"` and outlined paths. Scripts, text, styles, images, external
references and `url(...)` references are rejected. Frame paths must be relative
`.svg` paths within the package, including after symlink resolution. The loader
validates SVG before publishing a theme and caches loaded bytes; rendering does
not read files. File edits change the renderer cache key.

Limits: 128 KiB per manifest, 64 KiB of SVG data per frame (combined across
1–4 layers), 32 frames per state and 128 installed themes. IDs contain
1–80 ASCII letters, digits, hyphens or underscores. Names
contain 1–80 characters. Unknown manifest fields and state keys are rejected.

## Distribute a plugin theme

An installed Claude plugin can provide the same packages under its install root:

```text
my-plugin/
  .claude-plugin/plugin.json
  daruda/
    status-icon-themes/
      soft-ice/
        theme.json
        tool-0.svg
        tool-1.svg
```

Daruda discovers **user-scope installations** from the existing installed-plugin
manifest. No skill or executable hook is required for a theme. Marketplace
previews, project-scope and local-scope installations do not become global UI
providers. Existing plugin installation/removal watching updates the catalog;
Daruda never edits the plugin's installed files.

The saved selection is `plugin/<plugin-id>@<marketplace>/<theme-id>`. User themes
use `user/<theme-id>`, so they cannot replace a bundled theme or collide with a
plugin's ID. Duplicate IDs in one namespace are reported; the first directory
in sorted order is retained. To customize an installed theme, copy its package
to the user folder and select the user copy.

## Missing or invalid themes

A removed or invalid selected theme falls back to Dots. Settings retains and
shows the unavailable ID so reinstalling or fixing the package restores the
selection automatically. Invalid packages are omitted without hiding valid
neighbors; details are logged under `status_icon_theme`. The saved preference
is never silently rewritten. Old `dots` / `icons` configuration values continue
to load unchanged.
