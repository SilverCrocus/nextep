# Nextep branding

The Screen Pal mascot holds a television with a visible bezel, glass screen,
power light, and two feet.

- `app-icon.png`: original square artwork with a charcoal background.
- `screen-pal.png`: transparent mascot used for Android's foreground layer.
- `logo.png`: generated 256px version used on the sign-in screen.
- `icon-manifest.json`: source files and background colour for Tauri's icon generator.
- `prompts.txt`: prompts used with the built-in image generation tool.

Run `scripts/generate-icons.sh` from the repository root to regenerate the
desktop icons, Android launcher resources, and sign-in logo. The script adds
spacing around the Android foreground so the ears and TV feet fit inside the
launcher's safe area.
