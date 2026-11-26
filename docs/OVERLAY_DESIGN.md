# Approved overlay design

This document records the approved OpenVoiceFlow overlay direction. Future UI
changes should preserve this layout unless Imran explicitly requests a new
design.

## Layout

- Keep the existing compact waveform and push-to-talk control bar.
- Show conversation as separate, rounded dark bubbles directly above the bar.
- Label the user's bubble `You` and the assistant bubble `OpenVoiceFlow`.
- Keep the full message text visible; the native window must grow to fit the
  bubbles before the content is revealed.
- Use a restrained yellow accent for speaker labels and the small status dot.
- Do not add a yellow outline or glow around the bubbles or control bar.

## Motion

- Keep active messages visible while work is happening.
- After completion, leave the chat readable briefly, then move it upward and
  fade it away.
- Respect reduced-motion preferences.

## Implementation guardrails

- Preserve `core:window:allow-set-size` and
  `core:window:allow-set-position` in
  `apps/desktop/src-tauri/capabilities/default.json`. The overlay's dynamic
  sizing and positioning require both permissions.
- Keep chat messages before the control bar in the overlay layout.
- Keep the compact idle window; expand it only when the visible content needs
  room, then return it to the compact size when messages clear.
- Do not restore the old single `Emo is working` status card as the primary
  task presentation.
