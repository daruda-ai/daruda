# Rabbits

A long-eared rabbit status icon theme bundled with daruda as `builtin/rabbits`.
Select **Rabbits** in Settings → Workspace → Sidebar → Status icon theme; no
installation is needed.

To customize a separate `user/rabbits` copy, choose **Open theme
folder**, copy this entire directory into it, then select **Rabbits · user/rabbits**.
Keep `theme.json` and every SVG file together. See the [theme format](../README.md)
for profile paths and layer support.

| State | Pose | Loop |
|---|---|---|
| Disconnected | Empty burrow | Static |
| Connecting | Long waking stretch with a gentle ear tilt | 4 seconds |
| Working | Left-facing hop with hind-paw push-off and landing | 2 seconds |
| Executing tool | Alternating front-paw digging strokes | 2 seconds |
| Needs attention | Raised front paw, a head tilt, ear movement and blink | 8 seconds |
| Idle | Resting low with ears laid back | Static |
| Completed | Upright pose with tall ears and quiet smiling eyelids | Static |
| Errored | Wide-eyed surprise with swept-back ears | Static |
| Failed | Low crouch with folded ears | Static |

Four states animate on daruda's shared 250ms clock. Connecting and Needs
attention hold each frame for 500ms; repeated attention frames provide quiet
pauses between gestures. The hop loads and extends the hindquarters together,
with a soft body rise and following ear movement. Stretching keeps the front
paws planted.

Small side-set eyes and short eyelids distinguish
the rabbit from the kitten and puppy. Fixed white facial layers remain opaque
in both appearances; the body uses state colors with a deeper Working blue in
dark appearance. Every mask shares a 16 × 16 view box for 16px/12px rendering.

The character sheet was created with AI image generation and converted to
self-contained SVG paths.
