#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
icon_tmp_dir=$(mktemp -d)
trap 'rm -rf "$icon_tmp_dir"' EXIT
android_res=src-tauri/gen/android/app/src/main/res

# Generate in a temporary folder so unused iOS assets stay out of the repo.
cargo tauri icon assets/branding/icon-manifest.json --output "$icon_tmp_dir/icons"
cp "$icon_tmp_dir/icons/"*.png "$icon_tmp_dir/icons/"*.icns "$icon_tmp_dir/icons/"*.ico src-tauri/icons/
cp -R "$icon_tmp_dir/icons/android/." "$android_res/"

# Keep the mascot inside Android's safe area under every launcher mask.
cat > "$android_res/mipmap-anydpi-v26/ic_launcher.xml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background" />
    <foreground>
        <inset android:drawable="@mipmap/ic_launcher_foreground"
            android:insetLeft="21%" android:insetTop="21%"
            android:insetRight="21%" android:insetBottom="21%" />
    </foreground>
</adaptive-icon>
XML
cp "$android_res/mipmap-anydpi-v26/ic_launcher.xml" "$android_res/mipmap-anydpi-v26/ic_launcher_round.xml"

cargo tauri icon assets/branding/screen-pal.png --output "$icon_tmp_dir/logo" --png 256
cp "$icon_tmp_dir/logo/256x256.png" assets/branding/logo.png
