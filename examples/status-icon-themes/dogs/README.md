# Dogs

A floppy-eared puppy status icon theme bundled with daruda as `builtin/dogs`.
Select **Dogs** in Settings → Workspace → Sidebar → Status icon theme; no
installation is needed.

To customize a separate `user/dogs` copy, choose **Open theme
folder**, copy this entire directory into it, then select **Dogs · user/dogs**.
Keep `theme.json` and every SVG file together. See the [theme format](../README.md)
for profile paths and layer support.

| State | Pose | Loop |
|---|---|---|
| Disconnected | Empty kennel | Static |
| Connecting | Play-bow stretch with a gently moving tail | 4 seconds |
| Working | Left-facing trot with alternating diagonal paws | 2 seconds |
| Executing tool | Alternating front-paw digging strokes | 2 seconds |
| Needs attention | One front paw raised, a head tilt and blink | 8 seconds |
| Idle | Chin resting on the front paws | Static |
| Completed | Upright smile with two matching relaxed eyelids | Static |
| Errored | Wide-eyed surprise | Static |
| Failed | Low crouch with drooping ears | Static |

Four states animate on daruda's shared 250ms clock. Connecting and Needs
attention hold each frame for 500ms; repeated attention frames provide quiet
pauses between gestures. The trot alternates diagonal paw pairs, with a small
body rise and a delayed ear movement. Stretching keeps the front paws planted.

Rounded eyes, a broad nose and a separate muzzle give
the puppy its canine expression. Fixed white facial layers remain opaque in
both appearances; the body uses state colors with a deeper Working blue in dark
appearance. Every mask shares a 16 × 16 view box for 16px/12px rendering.

The character sheet was created with AI image generation and converted to
self-contained SVG paths.
