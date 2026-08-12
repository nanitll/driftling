# Verified checklist: shipping a polished open-source Linux desktop app in 2026 (Driftling)

## 1. Flathub

### App ID rules
- Reverse-DNS `{tld}.{vendor}.{product}`; ≥3 components, ≤5 components, ≤255 chars; components only `[A-Za-z0-9_]`, dash only in last component. — https://docs.flathub.org/docs/for-app-authors/requirements
- GitHub-hosted projects without their own domain must use `io.github.<user>.<Project>` with **at least 4 components** (`com.github.*` is reserved for GitHub-the-company). Since you own no verified domain-based ID for Driftling yet, either use `io.github.<youruser>.Driftling` or use your own domain (e.g. `ru.darkpact.Driftling`-style, but note domain must be provable for verification). — https://docs.flathub.org/docs/for-app-authors/requirements, https://docs.flathub.org/docs/for-app-authors/verification
- Verification for `io.github.*` = log in to Flathub with the GitHub account owning the repo (or org admin). — https://docs.flathub.org/docs/for-app-authors/verification

### Submission requirements
- Manifest (JSON/YAML), must build from source, all deps as manifest sources, pinned to tarball/tag **with commit id** (never branch tip); SDK must be hosted on Flathub; must pass `flatpak-builder-lint`. — https://docs.flathub.org/docs/for-app-authors/requirements, https://github.com/flathub/flathub/wiki/App-Submission
- **New 2025+ policy: AI-generated code/content and AI-driven submission PRs are banned** ("Applications containing AI-generated or AI-assisted code … are not allowed. Submission pull requests must not be generated, opened, or automated using AI tools"). Plan the submission PR accordingly (human-authored). LLM usage in linter-exception PRs "results in permanent denial". — https://github.com/flathub/flathub/wiki/App-Requirements, https://docs.flathub.org/docs/for-app-authors/linter

### Metainfo (`/app/share/metainfo/%{id}.metainfo.xml`) — required to pass validation
Source: https://docs.flathub.org/docs/for-app-authors/metainfo-guidelines
- `<id>` == Flatpak app ID exactly; `<name>`, `<summary>`; `<description>` with ≥1 non-empty `p`/`ol`/`ul`.
- `<developer id="...">` with `<name>` child — **required**.
- `<metadata_license>` (CC0-1.0/FSFAP) + `<project_license>` (SPDX).
- `<url type="homepage">` minimum; recommended: bugtracker, donation, vcs-browser, translate.
- `<content_rating type="oars-1.1">` — generate at the OARS site; a pet app is trivially "all ages".
- `<releases>` **required** ("Applications must supply a releases tag … to pass validation"), ordered, no future dates — i.e. release notes per version are effectively mandatory.
- `<launchable type="desktop-id">…​.desktop</launchable>` required for GUI apps; ≥1 screenshot required, linked by permanent URL (commit/tag, not branch).
- Validation: `flatpak run --command=flatpak-builder-lint org.flatpak.Builder appstream file.metainfo.xml`; **warnings are fatal too**.

### Icon / desktop file
- SVG preferred or ≥256×256 PNG; desktop file mandatory. — https://docs.flathub.org/docs/for-app-authors/requirements
- Quality guidelines (gates the storefront "quality" pass / featuring): icon square, margins per grid template, contrast on light+dark, no baked shadows; screenshots ≤1000×700 (2000×1400 HiDPI), window-only with native decoration+shadow+rounded corners, no wallpaper, taken on Linux, default fonts/accent, 1-sentence captions without trailing period; name ≤20 chars (ideally ≤15), summary ≤35 (ideally 10–25); brand colors in light+dark variants. — https://docs.flathub.org/docs/for-app-authors/metainfo-guidelines/quality-guidelines

### Permissions review quirks relevant to a Wayland pet/overlay app
Source (exact linter error names): https://docs.flathub.org/docs/for-app-authors/linter
- `--socket=wayland` alone triggers `finish-args-only-wayland` — exception "granted only if application explicitly lacks X11 support". **Driftling is layer-shell/Wayland-only, so this exception is exactly your case; state it in the PR.** If you ever add X11, the required combo is `wayland + fallback-x11 + --share=ipc`.
- Autostart: `--filesystem=xdg-config/autostart` is **denied** ("not granted as Autostart portal exists") — you must use the XDG **Background portal** (`org.freedesktop.portal.Background.RequestBackground`), which needs *no* static permission (portal talk-names are auto-granted; adding `--talk-name=org.freedesktop.portal.*` is itself a linter error `finish-args-portal-talk-name`). Rust binding: `ashpd` crate (https://crates.io/crates/ashpd). Portal docs: https://docs.flatpak.org/en/latest/sandbox-permissions.html
- Arbitrary D-Bus (`--socket=session-bus`) never granted; specific `--talk-name`s need justification and go through the exceptions file (`flathub-infra/flatpak-builder-lint` `exceptions.json`, PR-based, check live via `https://flathub.org/api/v2/exceptions/<app-id>`). — https://docs.flathub.org/docs/for-app-authors/linter, https://github.com/flathub-infra/flatpak-builder-lint/blob/master/flatpak_builder_lint/staticfiles/exceptions.json
- "Static permissions must be kept to an absolute minimum; rely on XDG portals". — https://docs.flathub.org/docs/for-app-authors/requirements

### Pet-app precedent
- **Shijima-Qt** (`com.pixelomer.ShijimaQt`) — a shimeji runner — *did* pass Flathub review and is live, but is now flagged unmaintained (upstream archived); it had an experimental wayland-layer-shell branch. So overlay/pet apps are admissible; there is a vacant niche. — https://flathub.org/apps/com.pixelomer.ShijimaQt, https://github.com/pixelomer/Shijima-Qt
- wl_shimeji itself: no Flathub listing found (only Shijima variants). Caveat: absence verified only via search, not exhaustively.

## 2. Debian/.deb + AppImage

### .deb outside the archive
- Build with `cargo-deb` (standard for Rust). — https://www.joshmcguigan.com/blog/host-debian-repo/
- Two sane tiers: (a) attach .deb to **GitHub Releases** (fine for early adopters, no `apt upgrade` path), (b) run a tiny **signed apt repo** — an apt repo is just a static file tree, hostable on any static host/object storage; `reprepro` is the pragmatic middle ground (GPG signing, multiple dists), `aptly` if you need multiple kept versions/REST API. — https://wiki.debian.org/DebianRepository/Setup, https://www.joshmcguigan.com/blog/host-debian-repo/, https://chabik.com/aptly-for-own-debian-repository/, https://oneuptime.com/blog/post/2026-03-02-setup-private-apt-repository-reprepro-ubuntu/view
- Hybrid hack exists (GitHub releases *as* apt repo) but requires GPG signing anyway. — https://github.com/rpatterson/github-apt-repos

### AppImage
- Embed **update information** in the AppImage + publish the `.zsync` file next to the release; AppImageUpdate/`appimageupdatetool` then does delta updates (zsync2). Never auto-update without explicit user consent (official guidance). — https://docs.appimage.org/packaging-guide/optional/updates.html, https://appimage-builder.readthedocs.io/en/latest/advanced/updates.html, https://github.com/AppImageCommunity/AppImageUpdate/wiki
- Desktop integration (menu entry, icon, exec bit) is delegated to the optional `appimaged` daemon on the user side — ship correct .desktop + icon inside the AppImage and don't try to self-integrate. — https://docs.appimage.org/packaging-guide/optional/updates.html, https://en.wikipedia.org/wiki/AppImage

### Release automation for Rust workspaces: cargo-dist ("dist")
- Rebranded to **dist** (Oct 2024) but repo still `axodotdev/cargo-dist`; actively released — **v0.32.0, 2026-05-22**. — https://blog.axo.dev/2024/10/new-name, https://github.com/axodotdev/cargo-dist/releases
- Generates: GitHub Actions release CI, per-target tarballs (linux x64/arm64 gnu+musl, win, mac), shell + PowerShell installers, Homebrew tap, npm installer, **SHA256 checksums**, and **GitHub Artifact Attestation JSON bundles**; cross-compiles via cargo-zigbuild/cargo-xwin. Pairs with `cargo-release` for version/tag flow. — https://github.com/axodotdev/cargo-dist/releases, https://opensource.axo.dev/cargo-dist/book/workspaces/cargo-release-guide.html
- Caveat: it does **not** produce .deb/Flatpak/AppImage — keep those as separate CI jobs. (Confirmed by its artifact list above.)

## 3. Release hygiene

- **SemVer 2.0.0** for versioning (for an app, the API is "user-visible behavior + config/save format"): https://semver.org/spec/v2.0.0.html ; **Keep a Changelog 1.1.0** format, `CHANGELOG.md` mirrored into AppStream `<releases>`: https://keepachangelog.com/en/1.1.0/
- Signing, what's actually common in 2026:
  - Big ecosystems use **Sigstore/cosign keyless** (Kubernetes signs all release artifacts this way; npm/Homebrew/GitHub attestations use the same bundle format, verifiable with cosign ≥2.4). — https://kubernetes.io/docs/tasks/administer-cluster/verify-signed-artifacts/, https://blog.sigstore.dev/cosign-verify-bundles/, https://github.com/sigstore/cosign
  - For a small project the lowest-friction respectable setup: SHA256SUMS + **GitHub Artifact Attestations** (cargo-dist emits these for free), optionally **minisign** for an offline-verifiable signature (cosign is explicitly "inspired by minisign"; minisign remains the small-project favorite for plain files). — https://github.com/axodotdev/cargo-dist/releases, https://github.com/sigstore/cosign
- Reproducible-ish Rust builds — the realistic checklist mature small projects follow:
  - commit `Cargo.lock`, build with `--locked`;
  - `SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)`;
  - `RUSTFLAGS="--remap-path-prefix $PWD=/build"` to strip absolute paths;
  - known residual issues tracked upstream (full bit-reproducibility across hosts is still not guaranteed). — https://reproducible-builds.org/docs/rust/, https://rust-secure-code.github.io/rust-supply-chain-security/build.html, https://github.com/rust-lang/rust/issues/129080

## 4. Desktop polish: icons, grouping, single instance, tray

### app_id ↔ desktop file (the generic-icon issue you hit)
- On Wayland the compositor matches windows to apps by **`app_id` == basename of the .desktop file** (without `.desktop`); on X11 the same job is done by `WM_CLASS`, and `StartupWMClass=` in the .desktop file tells the shell which WM_CLASS belongs to it. Mismatch ⇒ generic icon in KDE taskbar/alt-tab. — https://nicolasfella.de/posts/fixing-wayland-taskbar-icons/, https://thoughts.greyh.at/posts/startup-wm-class/, https://github.com/bitwarden/clients/issues/17760
- **Concrete rule for Driftling**: ship `org.example.Driftling.desktop` (matching your final app ID) and set the Wayland app_id to exactly that string in *every* toplevel — for winit/eframe windows use `WindowAttributesExtWayland::with_name("org.example.Driftling", "")` ("The general name sets an application ID, which should match the .desktop file"); for your layer-shell surfaces set the same string as the layer-surface namespace/app_id where applicable. Also add `StartupWMClass=org.example.Driftling` for the future X11/XWayland path. — https://docs.rs/winit/latest/winit/platform/wayland/, https://thoughts.greyh.at/posts/startup-wm-class/
- Flatpak note: desktop file, icon and metainfo must all be renamed to the app ID (`--rename-desktop-file`, `--rename-icon` exist for this). — https://docs.flatpak.org/en/latest/conventions.html

### Single instance
- Convention: claim a well-known **D-Bus name** (same reverse-DNS as app ID) at startup; if taken, exit (optionally forward a "show settings" call to the running instance); implement `--replace` via `DBUS_NAME_FLAG_ALLOW_REPLACEMENT`/`REPLACE_EXISTING` and exit on `NameLost`. This is what GApplication does under the hood and works identically inside Flatpak. Rust: `zbus::Connection::request_name`. — https://dbus.freedesktop.org/doc/dbus-tutorial.html, https://dbus.freedesktop.org/doc/api/html/group__DBusBus.html

### Tray in 2026
- SNI (StatusNotifierItem) is still the protocol. KDE Plasma: native. GNOME: **still no built-in tray** — users need an extension (classic `ubuntu/gnome-shell-extension-appindicator`, or the newer "Status Tray" which ships its own `org.kde.StatusNotifierWatcher`). Design so the tray is optional, not the only control surface. — https://extensions.gnome.org/extension/615/appindicator-support/, https://github.com/keithvassallomt/status-tray
- Rust: **`ksni`** is the go-to pure-Rust SNI implementation (tokio/async-io/blocking feature flags, no GTK linkage) — right fit for a non-GTK egui/layer-shell app; `tray-icon` (tauri) is the cross-platform alternative but pulls GTK/libappindicator on Linux. — https://github.com/iovxw/ksni, https://lib.rs/crates/ksni, https://crates.io/crates/tray-icon

## 5. i18n for Rust desktop apps

- The 2026 mainstream stack for non-GTK Rust desktop apps is **Fluent via `i18n-embed`** (+ `i18n-embed-fl` for compile-time-checked `fl!()` macro, `rust-embed` for bundling `.ftl` assets, `DesktopLanguageRequester`/`sys-locale` for OS locale detection, orchestrated by `cargo-i18n`). Layout: `i18n/{lang}/{domain}.ftl`. — https://lib.rs/crates/i18n-embed, https://lib.rs/crates/i18n-embed-fl, https://crates.io/crates/cargo-i18n, https://docs.rs/i18n-embed/latest/i18n_embed/
- `i18n-embed` also supports gettext, but its own docs call gettext "technically inferior to fluent … however the developer/translator ecosystem around gettext is more mature" — choose gettext only if you want Weblate/Poedit-style translator UX or GNOME ecosystem alignment; Flathub metainfo has a `<translation>` tag for gettext `.mo`/Qt `.qm` prefixes if you go that way. — https://lib.rs/crates/i18n-embed, https://docs.flathub.org/docs/for-app-authors/metainfo-guidelines
- egui has no official i18n layer; in practice egui/eframe apps use the i18n-embed+Fluent stack above (it's toolkit-agnostic — plain `String`s into `ui.label`). Caveat: "what egui apps use" is an ecosystem observation, not a single citable survey; the citable fact is that i18n-embed is the desktop-oriented i18n crate with OS-locale integration. — https://docs.rs/i18n-embed/latest/i18n_embed/

## Driftling-specific action list (derived)
1. Pick final app ID now (`io.github.<user>.driftling` needs 4 components and GitHub-account verification; own-domain ID enables verified badge via DNS) — renaming later breaks Flathub identity.
2. Set Wayland app_id + desktop file + icon name to that ID today (fixes KDE generic icon before it fossilizes in screenshots).
3. Wire autostart through `ashpd` Background portal from the start — it's the only Flathub-approvable path and also works outside Flatpak.
4. Manifest: `--socket=wayland` only + justify "no X11 support" (layer-shell); zero D-Bus talk-names beyond portals; ksni's SNI registration works via the session bus name `org.kde.StatusNotifierItem-…` — verify under `flatpak-builder-lint` early (talk-name for `org.kde.StatusNotifierWatcher` is a commonly granted one, but confirm against the live exceptions API).
5. Release train: cargo-dist for tarballs/installers/checksums/attestations + separate CI jobs for cargo-deb (attach to GH Releases first, apt repo later) and AppImage with embedded zsync update info; Flathub repo updates via the flathubbot PR flow after initial acceptance.
6. Metainfo from day one: OARS all-ages, `<releases>` synced from Keep-a-Changelog, 1000×700 window-only screenshots on default Plasma/GNOME, brand colors light+dark, name ≤15 chars ("Driftling" = 9, fits).
7. Note the Flathub AI policy (section 1) when preparing the submission PR.

Sources: [Flathub requirements](https://docs.flathub.org/docs/for-app-authors/requirements), [Flathub metainfo guidelines](https://docs.flathub.org/docs/for-app-authors/metainfo-guidelines), [Flathub quality guidelines](https://docs.flathub.org/docs/for-app-authors/metainfo-guidelines/quality-guidelines), [Flathub linter](https://docs.flathub.org/docs/for-app-authors/linter), [Flathub verification](https://docs.flathub.org/docs/for-app-authors/verification), [flathub wiki App Requirements](https://github.com/flathub/flathub/wiki/App-Requirements), [flatpak-builder-lint exceptions](https://github.com/flathub-infra/flatpak-builder-lint/blob/master/flatpak_builder_lint/staticfiles/exceptions.json), [Flatpak conventions](https://docs.flatpak.org/en/latest/conventions.html), [Flatpak sandbox permissions](https://docs.flatpak.org/en/latest/sandbox-permissions.html), [Shijima-Qt on Flathub](https://flathub.org/apps/com.pixelomer.ShijimaQt), [Shijima-Qt repo](https://github.com/pixelomer/Shijima-Qt), [Debian repo setup](https://wiki.debian.org/DebianRepository/Setup), [hosting a Debian repo](https://www.joshmcguigan.com/blog/host-debian-repo/), [aptly guide](https://chabik.com/aptly-for-own-debian-repository/), [reprepro guide](https://oneuptime.com/blog/post/2026-03-02-setup-private-apt-repository-reprepro-ubuntu/view), [github-apt-repos](https://github.com/rpatterson/github-apt-repos), [AppImage updates](https://docs.appimage.org/packaging-guide/optional/updates.html), [appimage-builder zsync](https://appimage-builder.readthedocs.io/en/latest/advanced/updates.html), [AppImageUpdate wiki](https://github.com/AppImageCommunity/AppImageUpdate/wiki), [AppImage (Wikipedia)](https://en.wikipedia.org/wiki/AppImage), [dist rename blog](https://blog.axo.dev/2024/10/new-name), [cargo-dist releases](https://github.com/axodotdev/cargo-dist/releases), [cargo-release guide](https://opensource.axo.dev/cargo-dist/book/workspaces/cargo-release-guide.html), [semver](https://semver.org/spec/v2.0.0.html), [keep a changelog](https://keepachangelog.com/en/1.1.0/), [Kubernetes signed artifacts](https://kubernetes.io/docs/tasks/administer-cluster/verify-signed-artifacts/), [sigstore bundle verify](https://blog.sigstore.dev/cosign-verify-bundles/), [cosign](https://github.com/sigstore/cosign), [reproducible-builds Rust](https://reproducible-builds.org/docs/rust/), [Rust supply chain guide](https://rust-secure-code.github.io/rust-supply-chain-security/build.html), [rustc repro tracking issue](https://github.com/rust-lang/rust/issues/129080), [Nicolas Fella on Wayland icons](https://nicolasfella.de/posts/fixing-wayland-taskbar-icons/), [StartupWMClass demystified](https://thoughts.greyh.at/posts/startup-wm-class/), [Bitwarden app_id issue](https://github.com/bitwarden/clients/issues/17760), [winit Wayland platform](https://docs.rs/winit/latest/winit/platform/wayland/), [D-Bus tutorial](https://dbus.freedesktop.org/doc/dbus-tutorial.html), [D-Bus bus API](https://dbus.freedesktop.org/doc/api/html/group__DBusBus.html), [AppIndicator GNOME extension](https://extensions.gnome.org/extension/615/appindicator-support/), [Status Tray extension](https://github.com/keithvassallomt/status-tray), [ksni](https://github.com/iovxw/ksni), [tray-icon](https://crates.io/crates/tray-icon), [i18n-embed](https://lib.rs/crates/i18n-embed), [i18n-embed-fl](https://lib.rs/crates/i18n-embed-fl), [cargo-i18n](https://crates.io/crates/cargo-i18n), [i18n-embed docs](https://docs.rs/i18n-embed/latest/i18n_embed/), [ashpd](https://crates.io/crates/ashpd)