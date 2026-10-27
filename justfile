# WhatsApp for COSMIC — build helpers.
app_id := "com.github.whatsapp-cosmic"
bin := "whatsapp-cosmic"
prefix := "/usr"
destdir := env("DESTDIR", "")
target_dir := env("CARGO_TARGET_DIR", "target")
base := destdir + prefix

# Build the release binary.
build *args:
    cargo build --release --locked -p wa-app {{args}}

# Run against the real WhatsApp backend (debug build).
run *args:
    cargo run -p wa-app -- {{args}}

# Run against the scripted demo backend with a throwaway database.
run-fake:
    cargo run -p wa-app -- --fake

# Tests and lints.
check:
    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings

# Install into $DESTDIR{{prefix}} (run `just build` first).
install:
    install -Dm0755 {{target_dir}}/release/{{bin}} {{base}}/bin/{{bin}}
    install -Dm0644 resources/{{app_id}}.desktop {{base}}/share/applications/{{app_id}}.desktop
    install -Dm0644 resources/{{app_id}}.metainfo.xml {{base}}/share/metainfo/{{app_id}}.metainfo.xml
    for size in 16 24 32 48 64 128 256 512; do \
        install -Dm0644 resources/icons/hicolor/${size}x${size}/apps/{{app_id}}.png {{base}}/share/icons/hicolor/${size}x${size}/apps/{{app_id}}.png; \
    done

uninstall:
    rm -f {{base}}/bin/{{bin}} {{base}}/share/applications/{{app_id}}.desktop {{base}}/share/metainfo/{{app_id}}.metainfo.xml {{base}}/share/icons/hicolor/*/apps/{{app_id}}.png

# Install for the current user only (no root): ~/.local/bin and ~/.local/share.
# The desktop entry gets the absolute binary path so the dock can launch it
# even when ~/.local/bin is not on the session PATH.
install-user: build
    install -Dm0755 {{target_dir}}/release/{{bin}} ~/.local/bin/{{bin}}
    sed "s|^Exec=.*|Exec=$HOME/.local/bin/{{bin}}|" resources/{{app_id}}.desktop > /tmp/{{app_id}}.desktop
    install -Dm0644 /tmp/{{app_id}}.desktop ~/.local/share/applications/{{app_id}}.desktop
    rm -f /tmp/{{app_id}}.desktop
    install -Dm0644 resources/{{app_id}}.metainfo.xml ~/.local/share/metainfo/{{app_id}}.metainfo.xml
    for size in 16 24 32 48 64 128 256 512; do \
        install -Dm0644 resources/icons/hicolor/${size}x${size}/apps/{{app_id}}.png ~/.local/share/icons/hicolor/${size}x${size}/apps/{{app_id}}.png; \
    done
    -update-desktop-database ~/.local/share/applications
    -gtk-update-icon-cache -q -t ~/.local/share/icons/hicolor

uninstall-user:
    rm -f ~/.local/bin/{{bin}} ~/.local/share/applications/{{app_id}}.desktop ~/.local/share/metainfo/{{app_id}}.metainfo.xml ~/.local/share/icons/hicolor/*/apps/{{app_id}}.png
    -update-desktop-database ~/.local/share/applications
