# Cats

A rounded kitten status icon theme for daruda, with artwork for all nine states.
Its stable selection ID is `user/cats`.

## Install

1. In Settings → Workspace → Sidebar → Status icon theme, choose
   **Open theme folder**.
2. Copy this entire `cats` directory into that folder, preserving `theme.json`
   and all SVG files beside it.
3. Select **Cats · user/cats**. Daruda discovers the package automatically and
   remembers the selection across restarts.

The installed manifest lives at `<profile>/status-icon-themes/cats/theme.json`.
An explicit `DARUDA_DATA_DIR` uses that directory as the profile root. See the
[theme format](../README.md) for profile discovery, validation and customization.

## Artwork and timing

| State | Pose | Loop |
|---|---|---|
| Disconnected | Empty cat house with an open doorway | Static |
| Connecting | Stretch, then pause | 4 seconds |
| Working | Playful walk | 2 seconds |
| Executing tool | Kneading with the front paws | 2 seconds |
| Needs attention | One front paw raised, with a head tilt and blink | 8 seconds |
| Idle | Resting loaf | Static |
| Completed | Upright, smiling cat | Static |
| Errored | Startled cat with an arched back | Static |
| Failed | Sad, crouching cat | Static |

The empty house distinguishes Disconnected from the resting Idle cat at small
sizes. Needs attention holds one front paw beside its face throughout the loop;
a gap between the arm and cheek keeps the raised paw visible in silhouette.
Working, Needs attention, Completed and Errored face left.

The package uses daruda's shared 250ms clock. Repeated frame references hold
poses without extra files. Every pose combines a tinted body mask with opaque
white facial details, so eye whites retain their color in both appearances.
Almond-shaped eyes and vertical pupils distinguish the cat from the puppy and
rabbit. The existing stretch and attention blink keep their closed eyelids.

All masks share a 16 × 16 view box. State colors provide dark and light variants
at 16px and 12px; Working uses a deeper blue in dark appearance.

The character and animation sheets were created with AI image generation and
converted to self-contained SVG paths. The empty house is a vector shape.
